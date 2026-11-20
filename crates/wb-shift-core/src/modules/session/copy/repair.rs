//! 复制路径的源正文兜底：严格校验失败时在内存里做一次可解释的容错修复。
//!
//! 单独一层的原因：这段逻辑只在「严格校验判为 `Unavailable`」时才会走，与编排/写入
//! 无关；放进 `plan_execute` 会让那个文件越过 AGENTS.md 第 4 节的文件规模阈值。
//! 红线仍是两条：**源文件只读**（只 `read`，修复后的文本写的是副本）、**严格校验
//! 语义不变**（[`session_link::read_content_snapshot`] 仍然把坏行判成 `Unavailable`）。

use super::*;

/// 源正文严格校验失败时的兜底：在内存里做一次可解释的容错修复并重算快照。
///
/// 红线：**源文件只读**（这里只 `read`，不写回；修复后的文本写的是副本），**严格校验
/// 语义不变**（[`session_link::read_content_snapshot`] 仍然把坏行判成 `Unavailable`），
/// 容错只是复制路径上额外的一步。
///
/// 返回的快照用**修复后文本**计算 `full_digest` 与 `normalized`：`copy_body` 会再跑一次
/// `normalize_jsonl`、`write_copy_body` 会写后回读比对，基准与待写文本必须同源，否则
/// 必然出现「内容保存后校验不一致」的假失败。
///
/// 失败时保留 `会话内容无法验证（…），未复制` 外壳（现有断言与前端提示依赖它），
/// 修复诊断追加在后面。
pub(in crate::modules::session) fn repair_source_snapshot(
    path: &Path,
    cid: &str,
    reason: &str,
) -> Result<(ContentSnapshot, Option<String>), String> {
    let unrepaired = format!("会话内容无法验证（{reason}），未复制");
    // 兜底读取必须自带「读取期间是否变化」的判定（与 `read_content_snapshot` 同口径的
    // len + modified 前后比对）：少了这一步，坏行路径就成了整条复制链上唯一没有 TOCTOU
    // 保护的读点，可能把客户端正在写、只写了一半的文件当成完整副本复制走。
    let before = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Err(unrepaired),
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Err(unrepaired),
    };
    let changed = match std::fs::metadata(path) {
        Ok(after) => after.len() != before.len() || after.modified().ok() != before.modified().ok(),
        Err(_) => true,
    };
    if changed {
        return Err(
            "会话内容无法验证（内容在读取过程中发生变化，无法确认完整），未复制".to_string(),
        );
    }
    let raw = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Err(unrepaired),
    };
    let repair = session_link::repair_jsonl_for_copy(&raw, cid)
        .map_err(|error| format!("{unrepaired}：{error}"))?;
    let Some(repair) = repair else {
        // 严格校验刚判为不可验证、这里却给出「无需修复」：两次判断不一致，保守放弃。
        return Err(unrepaired);
    };
    let session_link::JsonlRepair {
        text,
        dropped_tail_line,
        split_lines,
        kept_tail_line_records,
    } = repair;
    let normalized = session_link::normalize_jsonl(&text, cid)
        .map_err(|error| format!("{unrepaired}：{error}"))?;
    let note = describe_repair(dropped_tail_line, kept_tail_line_records, &split_lines);
    Ok((
        ContentSnapshot {
            full_digest: full_digest_of(text.as_bytes()),
            text,
            normalized,
        },
        note,
    ))
}

/// 把修复动作翻译成一句人话（会带到复制报告里，最终显示给用户）。
///
/// 三档必须能区分、且不谎报：整行丢弃（丢了没写完的一条）、末行「保留完整值 + 丢残尾」
/// （既说清丢了什么，也说清同行的完整记录还在）、粘连拆行（内容未丢失）。
fn describe_repair(
    dropped_tail_line: Option<usize>,
    kept_tail_line_records: Option<usize>,
    split_lines: &[(usize, usize)],
) -> Option<String> {
    let mut notes: Vec<String> = Vec::new();
    if let Some(line) = dropped_tail_line {
        notes.push(match kept_tail_line_records {
            // 该行里有完整记录被保留：必须同时说清「丢的是半截」与「保住了几条」，
            // 否则用户会以为整行都丢了（丢两条只报一条属于谎报损失）。
            Some(kept) => format!(
                "源会话第 {line} 行末尾有 1 条不完整记录，已跳过（同行的 {kept} 条完整记录已保留）"
            ),
            None => format!("源会话末尾有 1 条不完整记录（第 {line} 行），已跳过"),
        });
    }
    for (line, count) in split_lines {
        notes.push(format!(
            "源会话第 {line} 行有 {count} 条记录粘连，已拆分为独立记录（内容未丢失）"
        ));
    }
    (!notes.is_empty()).then(|| notes.join("；"))
}
