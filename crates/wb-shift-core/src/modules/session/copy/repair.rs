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
/// 失败时保留 `会话内容无法验证（…），未复制` 外壳（现有断言与前端提示依赖它）；
/// 修复层带来的**新**信息（修复后仍无法校验 / 记录数不一致 / 另一行的行诊断）用中文
/// 分号追加在后面，同一句行诊断不重复拼接（见 [`repair_failure`]）。
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
        .map_err(|error| repair_failure(&unrepaired, reason, &error))?;
    let Some(repair) = repair else {
        // 严格校验刚判为不可验证、这里却给出「无需修复」：两次判断不一致，保守放弃。
        return Err(unrepaired);
    };
    let session_link::JsonlRepair {
        text,
        dropped_tail_line,
        split_lines,
        kept_tail_line_records,
        nul_stripped,
    } = repair;
    let normalized = session_link::normalize_jsonl(&text, cid)
        .map_err(|error| format!("{unrepaired}；修复后的内容仍无法校验（{error}）"))?;
    let note = describe_repair(
        dropped_tail_line,
        kept_tail_line_records,
        &split_lines,
        &nul_stripped,
    );
    Ok((
        ContentSnapshot {
            full_digest: full_digest_of(text.as_bytes()),
            text,
            normalized,
        },
        note,
    ))
}

/// 修复失败时的错误文案拼装：**同一句诊断只说一遍，不同句必须都说**。
///
/// `reason` 来自严格校验，本身已经是「第 N 行的格式无法识别（列 / 字节数 / 是否末行）」
/// 这句完整诊断；修复层对同一形态给出的诊断与之逐字相同（同一个
/// `describe_parse_failure`），早先直接 `：{error}` 拼上去，最终 toast 里同一句出现两遍
/// （issue #1 用户贴的报错就是这个形状）。
///
/// 但「逐字相同」只是其中一种情况：首个坏行**可修**、真正修不出来的行在它后面时，修复层
/// 给的是**另一行**的诊断。这时若把修复层诊断无条件丢掉，用户会被告知去修一个本来可修
/// 的行，真正的阻断点从文案里消失（QA 实测：第 1 行粘连 + 第 2 行 `hello`，文案只剩
/// 第 1 行）。所以判据必须是「两句是不是同一句」，而不是「有没有修复层诊断」。
///
/// 「修复动作本身失败」（修复后仍无法校验 / 记录数对不上）是另一类：它的文案里往往
/// 嵌着 `reason` 原文，`contains` 会把它误判成重复，因此必须先按前缀单独放行——这类
/// 错误说明源里有结构性损坏、修复后仍过不了防线，必须让用户看到。
fn repair_failure(unrepaired: &str, reason: &str, error: &str) -> String {
    const NEW_INFO_PREFIXES: [&str; 2] = ["修复后的内容仍无法校验", "修复后的记录数不一致"];
    // 修复动作本身失败：文案虽可能嵌着 `reason`，但不是重复，必须保留。
    if NEW_INFO_PREFIXES
        .iter()
        .any(|prefix| error.starts_with(prefix))
    {
        return format!("{unrepaired}；{error}");
    }
    // 同一句诊断只说一遍；不同句（真正的阻断点在后）必须追加，否则用户会定位到错的行。
    if error == reason || error.contains(reason) || reason.contains(error) {
        unrepaired.to_string()
    } else {
        format!("{unrepaired}；{error}")
    }
}

/// 把修复动作翻译成一句人话（会带到复制报告里，最终显示给用户）。
///
/// 四档必须能区分、且不谎报：整行丢弃（丢了没写完的一条）、末行「保留完整值 + 丢残尾」
/// （既说清丢了什么，也说清同行的完整记录还在）、粘连拆行（内容未丢失）、
/// 剥离裸 NUL 填充（内容未丢失，可能一行里有好几条）。
fn describe_repair(
    dropped_tail_line: Option<usize>,
    kept_tail_line_records: Option<usize>,
    split_lines: &[(usize, usize)],
    nul_stripped: &[(usize, usize)],
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
    for (line, bytes) in nul_stripped {
        notes.push(format!(
            "源会话第 {line} 行有 {bytes} 字节空填充（NUL），已剥离（记录内容未丢失）"
        ));
    }
    (!notes.is_empty()).then(|| notes.join("；"))
}
