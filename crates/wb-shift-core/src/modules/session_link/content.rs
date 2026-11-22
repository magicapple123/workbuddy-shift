//! 会话正文的摘要、完整性校验与规范化规则。
//!
//! 摘要有两套**口径**，刻意分开（改任一套都不影响另一套）：
//! - 存储口径：`line_digests` / `total_digest`，供写后校验、备份清单与配对基线使用，
//!   必须逐字反映磁盘上的字节；
//! - 比较口径：`comparable_line_digests`，只在同步判定里使用，把「复制/同步时对末条
//!   assistant 消息做的归一留痕」改写成确定性形式后再取摘要（见
//!   [`comparable_line_digest`]）。归一本身是写入的既定行为，副本不该因此被判定成
//!   「用户改动」；但存储口径不能跟着变，否则基线文件与写后校验会一起失真。

use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 归一化时替代「本副本 sessionId」的固定标记。其余 sessionId 一律保留原文。
pub const SESSION_ID_MARKER: &str = "__wb_shift_session_id__";

/// 正文归一化规则版本；**规则变化必须递增**（design §3.1）。
///
/// 配对基线依赖此值判断是否可继续比较：版本不同一律视为不可验证，不得跨版本比较摘要。
pub const NORMALIZATION_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// 内容版本（design §3.1）
// ---------------------------------------------------------------------------

/// 归一化后的有序内容身份：逐行摘要 + 总摘要 + 记录数。
///
/// 行摘要对「长度编码 + 归一化行字节」取 SHA-256，记录分隔无歧义；行顺序、重复次数、
/// 键序都参与摘要，因此重排、去重、重写都会产生不同的内容身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedContent {
    pub record_count: usize,
    pub line_digests: Vec<String>,
    /// 判定用的行摘要：只把带「末条归一签名」的 assistant 行改写成确定性形式，
    /// 其余行与 `line_digests` 逐字相同；没有签名时两者完全相等。
    pub comparable_line_digests: Vec<String>,
    pub total_digest: String,
}

/// 一次成功读取的正文快照。
///
/// `text` 与摘要来自同一次读取：复制时直接写这一份，避免二次读取产生的 TOCTOU；
/// `full_digest` 覆盖原始字节（含空白），供之后的预览版本校验使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSnapshot {
    pub text: String,
    pub full_digest: String,
    pub normalized: NormalizedContent,
}

/// 正文读取结果。`Unavailable` 表示内容不可验证（空/非法/读取期间变化），
/// 调用方不得据此快进或覆盖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentState {
    Missing,
    Ready(ContentSnapshot),
    Unavailable(String),
}

/// 原始字节摘要（含空白与换行）。
pub fn full_digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// 由有序行摘要推导总摘要（前缀可自洽重算，便于校验基线未被篡改）。
pub fn total_digest_of(line_digests: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"workbuddy-shift-lines-v1\0");
    hasher.update((line_digests.len() as u64).to_be_bytes());
    for digest in line_digests {
        hasher.update(digest.as_bytes());
        hasher.update([0u8]);
    }
    to_hex(&hasher.finalize())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 单行摘要在长度编码后计算：记录分隔无歧义（design §3.1）。
///
/// VS Code 侧把「一条消息」当作一行来复用同一套摘要口径（`vscode_session_link`）。
pub(crate) fn line_digest_of(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut hasher = Sha256::new();
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// 逐行校验 JSONL 完整性并按本副本 sessionId 归一。
///
/// - 空正文、空行、非法 JSON 一律返回 Err（design：未知一律 Unknown，不快进）；
/// - 保留记录顺序与重复次数，不按记录 id 去重，不忽略未知字段；
/// - 仅把本副本自己的 sessionId 文本替换为固定标记，其它 sessionId 原样保留
///   （沿用既有复制的引用替换语义）；键序与格式差异因此会成为保守差异。
pub fn normalize_jsonl(text: &str, own_session_id: &str) -> Result<NormalizedContent, String> {
    let own_session_id = own_session_id.trim();
    if own_session_id.is_empty() {
        return Err("缺少会话 id，无法读取内容".to_string());
    }
    if text.trim().is_empty() {
        return Err("内容为空".to_string());
    }

    let mut lines: Vec<&str> = text.split('\n').collect();
    // 末尾换行（含多个）不算记录；中间空行仍视为异常。
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    let mut line_digests = Vec::with_capacity(lines.len());
    let mut comparable_line_digests = Vec::with_capacity(lines.len());
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            return Err(format!("第 {} 行为空，内容可能不完整", index + 1));
        }
        // 只做语法校验，不构建 `Value` 树：同一个解析器、同样的接受集合，
        // 但省掉整行对象的分配与建树（长会话上约占归一化耗时的四成）。
        serde_json::from_str::<serde::de::IgnoredAny>(line).map_err(|error| {
            describe_parse_failure(line, index + 1, index + 1 == lines.len(), &error)
        })?;
        // 绝大多数行不含自己的会话 id：只在真的出现时才分配替换副本。
        let marked = if line.contains(own_session_id) {
            Cow::Owned(line.replace(own_session_id, SESSION_ID_MARKER))
        } else {
            Cow::Borrowed(line)
        };
        let digest = line_digest_of(&marked);
        // 比较口径：只有带「末条归一签名」的行才重算，其余行直接复用存储口径的摘要。
        comparable_line_digests.push(comparable_line_digest(line, own_session_id, &digest));
        line_digests.push(digest);
    }
    let total_digest = total_digest_of(&line_digests);
    Ok(NormalizedContent {
        record_count: line_digests.len(),
        line_digests,
        comparable_line_digests,
        total_digest,
    })
}

/// 判定用的行摘要：把「末条归一签名」改写成确定性形式后再取摘要。
///
/// 命中改写时**只**做三件事：`status` 统一成 `completed`、删掉 `_meta.incompleteNormalized`
/// 留痕、`_meta` 因此变空时整条删掉——时间戳不参与比较，因此不同时间做的归一得到同一个
/// 摘要。不合条件（不是 assistant 消息、没有归一痕迹、解析失败）一律回退到 `raw_digest`：
/// 新的比较口径绝不能把原本可读的内容变成不可读，也不能放宽其它任何差异。
fn comparable_line_digest(line: &str, own_session_id: &str, raw_digest: &str) -> String {
    match comparable_normalized_line(line, own_session_id) {
        Some(rewritten) => line_digest_of(&rewritten),
        None => raw_digest.to_string(),
    }
}

/// 归一痕迹的便宜预筛关键字（`INCOMPLETE_META_KEY` 也含它，两个都查只是为了显式）。
const INCOMPLETE_KEYWORD: &str = "incomplete";

/// 把带「末条归一签名」的 assistant 消息行改写成确定性形式；不必要的行返回 `None`。
///
/// 条件（全部满足才改写）：`type == "message"`、`role == "assistant"`，且 `status` 是
/// `"incomplete"` 或对象里存在 `_meta.incompleteNormalized`。
fn comparable_normalized_line(line: &str, own_session_id: &str) -> Option<String> {
    // 先做子串预筛：正文动辄上千行，只有可能带痕迹的行才值得付解析与建树的代价。
    if !line.contains(INCOMPLETE_KEYWORD) && !line.contains(INCOMPLETE_META_KEY) {
        return None;
    }
    let mut record: serde_json::Value = serde_json::from_str(line).ok()?;
    {
        let object = record.as_object_mut()?;
        if object.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            return None;
        }
        if object.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            return None;
        }
        let status = object.get("status").and_then(serde_json::Value::as_str);
        let has_marker = object
            .get("_meta")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|meta| meta.contains_key(INCOMPLETE_META_KEY));
        if status != Some("incomplete") && !has_marker {
            return None;
        }
        // 只有这两个取值被归一统一成 `completed`；其它取值（含缺失）一律原样保留。
        let rewrite_status = matches!(status, Some("incomplete") | Some("completed"));
        if rewrite_status {
            object.insert(
                "status".to_string(),
                serde_json::Value::String("completed".to_string()),
            );
        }
        if let Some(meta) = object
            .get_mut("_meta")
            .and_then(serde_json::Value::as_object_mut)
        {
            meta.remove(INCOMPLETE_META_KEY);
        }
        // 空 `_meta` 与「没有 `_meta`」必须等价：否则两侧只差一个空壳就被判成内容不同。
        let empty_meta = object
            .get("_meta")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|meta| meta.is_empty());
        if empty_meta {
            object.remove("_meta");
        }
    }
    // `serde_json::Value` 的对象是 BTreeMap（未开启 `preserve_order`），键序确定，
    // 因此同一份语义内容无论原来的书写顺序如何都得到同一条序列化结果。
    let serialized = serde_json::to_string(&record).ok()?;
    // 与存储口径一致：只替换本副本自己的会话 id，其它 sessionId 原样保留。
    Some(if serialized.contains(own_session_id) {
        serialized.replace(own_session_id, SESSION_ID_MARKER)
    } else {
        serialized
    })
}

/// 单行解析失败的诊断文案：保留解析器的列号与错误类别，便于定位真实坏行。
///
/// 生产上遇到的两类真实形态（进程被杀留下末行半截、退出时没写换行导致两行粘连）
/// 只看「第 N 行格式无法识别」无法区分，用户也无法判断是不是自己的会话坏了；
/// 因此把「列号、解析器给出的错误类别、该行字节数、是否文件末行」一并带上。
/// 文案骨架（`第 N 行的格式无法识别（…），内容可能不完整`）保持不变：前端提示与
/// 既有排障脚本按这个骨架匹配。
///
/// `is_last_line` 按**归一化口径**判断（末尾空行已被剔除），因此它表达的是
/// 「最后一个非空行」——末行截断与中间行损坏的处置方式完全不同（见
/// [`super::content_repair`]），诊断里必须能区分。
pub(super) fn describe_parse_failure(
    line: &str,
    line_no: usize,
    is_last_line: bool,
    error: &serde_json::Error,
) -> String {
    let tail = if is_last_line {
        "文件末行"
    } else {
        "不是末行"
    };
    format!(
        "第 {line_no} 行的格式无法识别（第 {} 列，{}；该行 {} 字节；{tail}），内容可能不完整",
        error.column(),
        serde_error_detail(error),
        line.len()
    )
}

/// 解析器错误里去掉「 at line L column C」后缀后的正文，即错误类别本身。
///
/// 用 `Display` 而不是 `classify()`：`classify()` 只给出 `syntax` / `eof` 这种粗分类，
/// 而这里需要「EOF while parsing an object」这类能直接指出坏在哪一层的描述
/// （`Display` 的结构是 `<类别> at line <L> column <C>`，行号列号已单独上报）。
fn serde_error_detail(error: &serde_json::Error) -> String {
    let text = error.to_string();
    match text.rfind(" at line ") {
        Some(index) if index > 0 => text[..index].to_string(),
        _ => text,
    }
}

/// 读取正文快照并做完整性校验；读取期间文件被改动时判定为不可验证。
pub fn read_content_snapshot(path: &Path, own_session_id: &str) -> ContentState {
    note_real_read();
    let before = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return ContentState::Missing,
        Err(error) => return ContentState::Unavailable(format!("内容无法读取：{error}")),
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return ContentState::Missing,
        Err(error) => return ContentState::Unavailable(format!("内容读取失败：{error}")),
    };
    let after = std::fs::metadata(path);
    let changed = match after {
        Ok(meta) => meta.len() != before.len() || meta.modified().ok() != before.modified().ok(),
        Err(_) => true,
    };
    if changed {
        return ContentState::Unavailable("内容在读取过程中发生变化，无法确认完整".to_string());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return ContentState::Unavailable("内容格式异常，无法读取".to_string()),
    };
    let full_digest = full_digest_of(text.as_bytes());
    match normalize_jsonl(&text, own_session_id) {
        Ok(normalized) => ContentState::Ready(ContentSnapshot {
            text,
            full_digest,
            normalized,
        }),
        Err(reason) => ContentState::Unavailable(reason),
    }
}

// ---------------------------------------------------------------------------
// 「最后一条 assistant 消息停在 incomplete」的归一（纯文本变换，不做读写与校验改动）
// ---------------------------------------------------------------------------

/// 把正文里**最后一条 assistant 消息**的 `status` 从 `incomplete` 归一为 `completed`。
///
/// 背景：assistant 消息停在 `status = "incomplete"`（用户中断 / 429 限流 / 500 报错）时，
/// 客户端按**正文**判定「仍在运行」，会把会话永久显示成「思考中」、处理时间一直累加。
/// 复制与同步都在**写正文之前**调用它（写完再改会让行摘要与 `total_digest` 变化，
/// 写后完整性校验必然失败，口径见 `copy/body.rs::write_copy_body`）；`at` 是留痕时间戳，
/// `reason` 写明调用场景（`copy normalize` / `sync normalize`），便于事后追溯来源。
///
/// 命中条件（任一不满足即原样返回 `None`，调用方按原文处理）：
/// - 正文里存在 `type == "message"` 且 `role == "assistant"` 的记录，定位**最后一条**
///   （口径与存量脚本 `fix-incomplete-turns.py` 一致：只看 `role` 会把
///   `{"type":"event","role":"assistant",...}` 之类的非消息记录也当成候选）；
/// - 该记录的 `status` 必须恰好是可识别的 `"incomplete"` 字面量。
///
/// 定位口径是「最后一条 assistant 消息」而不是「最后一条记录」：一轮对话结束后正文
/// 常常还跟着 tool 结果行（`function_call_result` 等），此时客户端仍然会显示「正在执行
/// 工具」，用户看到的还是转圈。这个口径与存量修复脚本
/// `scripts/diagnostics/fix-incomplete-turns.py` 一致，同一批数据两边判断才不会打架。
///
/// 变换只动两处：该行 `status` 的值（`"incomplete"` → `"completed"`）与
/// `_meta.incompleteNormalized` 留痕（插在该行自己的 `}` 前，不是插到文件末尾）。
/// 其余字节一律原样保留——行数、行序、其它行（含其后的 tool 行）全文都不动
/// （摘要按行计算，多余的字节改动都会被当成「内容变了」）。
pub fn normalize_last_incomplete(text: &str, at: &str, reason: &str) -> Option<String> {
    let (start, end) = last_assistant_record_span(text)?;
    let raw = &text[start..end];
    // CRLF 正文很常见：行内容按去 `\r` 的口径解析（与 `normalize_jsonl` 一致），
    // 但这个 `\r` 属于行字节，必须原样补回去，不能悄悄把行尾换行风格改掉。
    let (line, line_ending) = match raw.strip_suffix('\r') {
        Some(line) => (line, "\r"),
        None => (raw, ""),
    };
    let record: serde_json::Value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let object = record.as_object()?;
    // 防御性复查：定位阶段已按 `type == "message"` + `role == "assistant"` 过滤，
    // 这里再确认一次（宁可不动，也不改错行）。
    if object.get("type").and_then(serde_json::Value::as_str) != Some("message") {
        return None;
    }
    if object.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    if object.get("status").and_then(serde_json::Value::as_str) != Some("incomplete") {
        return None;
    }
    // 值必须是可定位的字面量 `"incomplete"`：转义等异常写法一律不动（宁可不改）。
    let (status_start, status_end) = top_level_value_span(line, "status")?;
    if &line[status_start..status_end] != INCOMPLETE_STATUS_LITERAL {
        return None;
    }
    let mut patched = String::with_capacity(line.len() + 64);
    patched.push_str(&line[..status_start]);
    patched.push_str(COMPLETED_STATUS_LITERAL);
    patched.push_str(&line[status_end..]);
    let marker = serde_json::json!({"at": at, "reason": reason});
    let patched = with_incomplete_meta(&patched, marker)?;
    let mut out = String::with_capacity(text.len() + patched.len());
    out.push_str(&text[..start]);
    out.push_str(&patched);
    out.push_str(line_ending);
    out.push_str(&text[end..]);
    Some(out)
}

/// `status` 的两个字面量（含引号），供变换时做精确文本替换。
const INCOMPLETE_STATUS_LITERAL: &str = "\"incomplete\"";
const COMPLETED_STATUS_LITERAL: &str = "\"completed\"";

/// 留痕键名：与存量数据修复脚本 `scripts/diagnostics/fix-incomplete-turns.py` 保持一致。
const INCOMPLETE_META_KEY: &str = "incompleteNormalized";

/// 正文里最后一条 assistant 消息记录的字节区间（半开）。末尾换行（含多个）不算记录。
///
/// 逐行向前保留「最后一条命中」，因此尾部跟随的 tool 行不影响定位。
/// 先做 `"assistant"` 子串预筛再做 JSON 解析：正文动辄上千行，只有命中行才值得解析。
fn last_assistant_record_span(text: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    let mut found: Option<(usize, usize)> = None;
    for segment in text.split_inclusive('\n') {
        let start = offset;
        offset += segment.len();
        let body = segment.strip_suffix('\n').unwrap_or(segment);
        let line = body.strip_suffix('\r').unwrap_or(body);
        if line.trim().is_empty() || !line.contains("assistant") {
            continue;
        }
        if is_assistant_message(line) {
            found = Some((start, start + body.len()));
        }
    }
    found
}

/// 该行是否为一条 assistant **消息**记录。
///
/// 口径与存量修复脚本 `scripts/diagnostics/fix-incomplete-turns.py` 对齐：`type == "message"`
/// **且** `role == "assistant"`。只看 `role` 不够——正文里还有 `{"type":"event",
/// "role":"assistant",...}` 这类非消息记录，一旦它排在真正的末条消息之后，只按 role
/// 定位就会改到错误的行（真实数据暂未触发，但口径必须与脚本一致才好对账）。
fn is_assistant_message(line: &str) -> bool {
    let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    let field_is = |key: &str, expected: &str| {
        record
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == expected)
    };
    field_is("type", "message") && field_is("role", "assistant")
}

/// 给单行 JSON 对象的 `_meta` 补一个留痕属性，只重排 `_meta` 这一段。
///
/// 没有任何 `_meta` 时插在顶层闭合花括号之前（单行对象的最后一个 `}` 就是它）。
fn with_incomplete_meta(line: &str, marker: serde_json::Value) -> Option<String> {
    let mut meta = serde_json::Map::new();
    meta.insert(INCOMPLETE_META_KEY.to_string(), marker);
    let payload = serde_json::Value::Object(meta.clone());
    match top_level_value_span(line, "_meta") {
        Some((start, end)) if line[start..end].starts_with('{') => {
            let mut merged = match serde_json::from_str::<serde_json::Value>(&line[start..end]) {
                Ok(serde_json::Value::Object(map)) => map,
                _ => return None,
            };
            for (key, value) in meta {
                merged.insert(key, value);
            }
            let serialized = serde_json::to_string(&serde_json::Value::Object(merged)).ok()?;
            Some(format!("{}{serialized}{}", &line[..start], &line[end..]))
        }
        // `_meta` 不是对象（异常形态）：整段换成只含留痕的对象，保证留痕一定落得下去。
        Some((start, end)) => {
            let serialized = serde_json::to_string(&payload).ok()?;
            Some(format!("{}{serialized}{}", &line[..start], &line[end..]))
        }
        None => {
            let close = line.rfind('}')?;
            let head = line[..close].trim_end();
            let separator = if head.ends_with('{') { "" } else { "," };
            let serialized = serde_json::to_string(&payload).ok()?;
            let mut out = String::with_capacity(line.len() + 64);
            out.push_str(head);
            out.push_str(separator);
            out.push_str("\"_meta\":");
            out.push_str(&serialized);
            // 闭合花括号及其后的空白原样保留。
            out.push_str(&line[close..]);
            Some(out)
        }
    }
}

/// 顶层对象里 `key` 对应值的字节区间（半开）。
///
/// 只按「键 → 值」顺序线性扫描顶层：嵌套对象/数组里的同名键不会被误认。
fn top_level_value_span(line: &str, key: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = skip_json_ws(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    index += 1;
    loop {
        index = skip_json_ws(bytes, index);
        if index >= bytes.len() {
            return None;
        }
        match bytes[index] {
            b'}' => return None,
            b',' => {
                index += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let key_end = scan_json_string_end(bytes, index)?;
        let is_target = &line[index + 1..key_end - 1] == key;
        index = skip_json_ws(bytes, key_end);
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        index = skip_json_ws(bytes, index + 1);
        let value_end = scan_json_value_end(bytes, index)?;
        if is_target {
            return Some((index, value_end));
        }
        index = value_end;
    }
}

fn skip_json_ws(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    index
}

/// `start` 处的 JSON 字符串的结束位置（半开，含结束引号）。
fn scan_json_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// `start` 处的 JSON 值的结束位置（半开）：字符串、对象/数组、数字与字面量都覆盖。
fn scan_json_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    if start >= bytes.len() {
        return None;
    }
    match bytes[start] {
        b'"' => scan_json_string_end(bytes, start),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut index = start;
            while index < bytes.len() {
                match bytes[index] {
                    b'"' => index = scan_json_string_end(bytes, index)?,
                    b'{' | b'[' => {
                        depth += 1;
                        index += 1;
                    }
                    b'}' | b']' => {
                        depth = depth.saturating_sub(1);
                        index += 1;
                        if depth == 0 {
                            return Some(index);
                        }
                    }
                    _ => index += 1,
                }
            }
            None
        }
        _ => {
            // 数字 / true / false / null：读到第一个分隔符为止。
            let mut index = start;
            while index < bytes.len() && !matches!(bytes[index], b',' | b'}' | b']' | b' ' | b'\t')
            {
                index += 1;
            }
            Some(index)
        }
    }
}

/// `prefix` 是否为 `full` 的有序前缀（用于基线继承校验）。
pub fn is_ordered_prefix(prefix: &[String], full: &[String]) -> bool {
    prefix.len() <= full.len() && full[..prefix.len()] == *prefix
}

/// 严格有序追加：前缀成立且确实更长（design §3.2 的 fastForward 前提之一）。
pub fn is_strict_ordered_extension(prefix: &[String], full: &[String]) -> bool {
    prefix.len() < full.len() && is_ordered_prefix(prefix, full)
}

// ---------------------------------------------------------------------------
// 预览用的内容指纹与进程内缓存（R5-16）
// ---------------------------------------------------------------------------
//
// 背景：切号弹窗打开即逐组预览，而预览原本逐组全量读取双方正文（某账号 106 个组、
// 约 988 MB）。第二次打开同一账号时内容根本没变，却又要重算一遍，用户看到的就是
// 「点切换、还没开始切就很卡」。
//
// 做法：只缓存**摘要指纹**（每条几百字节），正文读完立即丢弃。全库正文 2.28 GB，
// 缓存快照等于把整库正文留在内存里，**明令禁止**；因此这里的类型都没有 `text` 字段，
// 执行路径（真正要落盘的那几条）拿不到正文，也就无法顺手复用预览缓存。

/// 固定上限的进程内缓存：插入序淘汰（超出上限时丢弃最早的一半），不引入 LRU 依赖。
///
/// 只允许放**小值**（摘要指纹、判定标量）。容量按条目数计算，放进正文会让内存随会话
/// 体量线性增长；这条约束由 [`ContentFingerprint`] 与 `sync::preview` 的判定键共同保证。
pub(crate) struct BoundedCache<K, V> {
    inner: Mutex<BoundedCacheInner<K, V>>,
}

struct BoundedCacheInner<K, V> {
    entries: HashMap<K, V>,
    /// 插入顺序（重复写同一个键不会重复登记）。
    order: VecDeque<K>,
    capacity: usize,
}

impl<K, V> BoundedCache<K, V> {
    /// `capacity` 至少为 1：淘汰一次至少移除一条，否则永远缩不回去。
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(BoundedCacheInner {
                entries: HashMap::new(),
                order: VecDeque::new(),
                capacity: capacity.max(1),
            }),
        }
    }

    /// 锁中毒时继续使用现有数据：缓存里只有整条插入的值，没有半写入状态。
    fn lock(&self) -> MutexGuard<'_, BoundedCacheInner<K, V>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl<K: Eq + Hash + Clone, V: Clone> BoundedCache<K, V> {
    /// 命中返回值副本；未命中返回 `None`。
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        self.lock().entries.get(key).cloned()
    }

    /// 写入并维持上限；超出上限时按插入顺序淘汰最早的一半。
    pub(crate) fn insert(&self, key: K, value: V) {
        let mut inner = self.lock();
        if inner.entries.insert(key.clone(), value).is_none() {
            inner.order.push_back(key);
        }
        let capacity = inner.capacity;
        if inner.entries.len() > capacity {
            let drop_count = (capacity / 2).max(1);
            for _ in 0..drop_count {
                match inner.order.pop_front() {
                    Some(oldest) => {
                        inner.entries.remove(&oldest);
                    }
                    None => break,
                }
            }
        }
    }

    /// 当前条目数（单测断言淘汰行为用）。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

/// 预览判定用的内容指纹：**只有摘要与计数，绝不含正文**。
///
/// `normalized_digest` 按本副本的 `session_id` 归一，因此 `session_id` 是身份的一部分：
/// 缓存键漏掉它，不同成员的归一化摘要就会串味（R5-16 记的最危险错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFingerprint {
    /// 实读字节数（与 [`ContentStamp`] 里的 `len` 应一致；不一致说明读取期间被追加）。
    pub len: u64,
    /// 文件 mtime（纳秒）；探测不到时记 0，此时指纹不参与缓存。
    pub mtime_ns: i128,
    /// 本副本的会话 id（归一化摘要的输入）。
    pub session_id: String,
    /// 原始字节摘要（含空白），与 [`ContentSnapshot::full_digest`] 同口径。
    pub full_digest: String,
    /// 归一化总摘要，与 [`NormalizedContent::total_digest`] 同口径。
    pub normalized_digest: String,
    /// 记录数（非空行数）。
    pub record_count: usize,
}

/// 正文文件的身份（一次 `metadata`）：缓存键的全部要素，**不读正文**。
///
/// `len` 与 `mtime_ns` 任一变化即视为内容已变；不做内容比对兜底——客户端写入必然改变
/// size，而同一 tick 内等长改写且 mtime 不变的概率可以忽略（design R5-16 §2）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContentStamp {
    /// 找不到正文文件（`projects/` 下没有对应的 `<sessionId>.jsonl`）。
    Missing,
    /// 文件存在且元数据可读。
    Stamped {
        /// 规范化后的绝对路径：同一文件的不同写法必须落在同一个键上。
        path: PathBuf,
        session_id: String,
        len: u64,
        mtime_ns: i128,
    },
    /// 元数据读不出来（权限 / IO 异常）：没有可靠的失效依据，因此不参与缓存。
    Unavailable(String),
}

/// 指纹口径的正文状态：与 [`ContentState`] 一一对应，但**没有 `text` 字段**。
///
/// 执行路径（复制 / 同步写正文）必须拿正文，因此天然无法复用预览缓存——这是编译期
/// 保证，不是注释约定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintedContent {
    Missing,
    Ready(ContentFingerprint),
    Unavailable(String),
}

impl FingerprintedContent {
    /// 可验证时的指纹。
    pub fn ready(&self) -> Option<&ContentFingerprint> {
        match self {
            Self::Ready(fingerprint) => Some(fingerprint),
            _ => None,
        }
    }

    /// 记录数（不可验证时为 0，仅用于展示，口径同原先的 `record_count_of`）。
    pub fn record_count(&self) -> usize {
        self.ready()
            .map_or(0, |fingerprint| fingerprint.record_count)
    }

    /// 由一次真实读取结果与读前的身份探测拼出指纹视图（正文不保留）。
    pub fn from_state(state: &ContentState, session_id: &str, stamp: &ContentStamp) -> Self {
        match state {
            ContentState::Missing => Self::Missing,
            ContentState::Unavailable(reason) => Self::Unavailable(reason.clone()),
            ContentState::Ready(snapshot) => {
                Self::Ready(fingerprint_of(stamp, session_id, snapshot))
            }
        }
    }
}

/// 指纹缓存的条目上限。每条几百字节（路径 + 两个 64 字符摘要 + 计数），
/// 1024 条约 0.4 MB，远低于「RSS 增长 < 30 MB」的验收口径。
pub const FINGERPRINT_CACHE_CAPACITY: usize = 1024;

/// 进程内共享的内容指纹缓存（不落盘：缓存与磁盘不一致会导致误判）。
pub struct FingerprintCache {
    entries: BoundedCache<ContentStamp, ContentFingerprint>,
}

impl FingerprintCache {
    pub fn new() -> Self {
        Self {
            entries: BoundedCache::with_capacity(FINGERPRINT_CACHE_CAPACITY),
        }
    }

    /// 命中返回指纹；未命中、`Missing`、`Unavailable` 一律 `None`。
    pub fn lookup(&self, stamp: &ContentStamp) -> Option<ContentFingerprint> {
        self.entries.get(stamp)
    }

    /// 回填指纹。返回是否真的写进了缓存。
    ///
    /// 落缓存前再核一次身份：读取期间被改写的指纹不能挂在旧键上（挂上去就会在下次
    /// 预览把这具「过期摘要」当成最新结果）；`Missing` / `Unavailable` 一律不缓存——
    /// 文件可能马上被创建或修复（design R5-16 §4）。
    pub fn store(&self, stamp: &ContentStamp, fingerprint: ContentFingerprint) -> bool {
        let ContentStamp::Stamped {
            path,
            session_id,
            len,
            mtime_ns,
        } = stamp
        else {
            return false;
        };
        if fingerprint.session_id != *session_id {
            return false;
        }
        let Ok(current) = std::fs::metadata(path) else {
            return false;
        };
        if current.len() != *len || mtime_nanos(&current) != Some(*mtime_ns) {
            return false;
        }
        self.entries.insert(stamp.clone(), fingerprint);
        true
    }
}

impl Default for FingerprintCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 进程内唯一的指纹缓存。预览是顺序执行，单锁的竞争可以忽略。
pub fn fingerprint_cache() -> &'static FingerprintCache {
    static CACHE: OnceLock<FingerprintCache> = OnceLock::new();
    CACHE.get_or_init(FingerprintCache::new)
}

/// 探测正文文件的身份：一次 `metadata`，不读正文。
pub fn stamp_content(path: &Path, own_session_id: &str) -> ContentStamp {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return ContentStamp::Missing,
        Err(error) => return ContentStamp::Unavailable(format!("内容无法读取：{error}")),
    };
    let Some(mtime_ns) = mtime_nanos(&meta) else {
        // 没有 mtime 就没有失效依据：走非缓存路径（调用方用真实读取兜底）。
        return ContentStamp::Unavailable("无法获取内容的修改时间".to_string());
    };
    ContentStamp::Stamped {
        path: normalize_cache_path(path),
        session_id: own_session_id.trim().to_string(),
        len: meta.len(),
        mtime_ns,
    }
}

/// mtime 的纳秒表示（早于 1970 的时钟用负值表达，仍是可比较、可区分变化的值）。
fn mtime_nanos(meta: &std::fs::Metadata) -> Option<i128> {
    let modified = meta.modified().ok()?;
    Some(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(delta) => delta.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    })
}

/// 缓存键里的路径统一成规范化的绝对路径；规范化失败时退回原路径（键仍可比）。
fn normalize_cache_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 由读前的身份探测与真实读取结果拼出指纹。
fn fingerprint_of(
    stamp: &ContentStamp,
    own_session_id: &str,
    snapshot: &ContentSnapshot,
) -> ContentFingerprint {
    let mtime_ns = match stamp {
        ContentStamp::Stamped { mtime_ns, .. } => *mtime_ns,
        _ => 0,
    };
    ContentFingerprint {
        // `ContentSnapshot::text` 是本次实读的全部字节，长度即文件字节数。
        len: snapshot.text.len() as u64,
        mtime_ns,
        session_id: own_session_id.trim().to_string(),
        full_digest: snapshot.full_digest.clone(),
        normalized_digest: snapshot.normalized.total_digest.clone(),
        record_count: snapshot.normalized.record_count,
    }
}

// ---------------------------------------------------------------------------
// 单测用的真实读取计数（仅测试构建存在）
// ---------------------------------------------------------------------------
//
// R5-16 的验收要求「预览命中缓存时不再触发文件读取」。只比对返回值证明不了这件事
// （陈旧判定与新鲜判定的取值可能恰好相同），所以直接在 `read_content_snapshot`
// 入口计数：用例里取「前后差值」即可断言是否真的读了正文。
//
// 用线程局部而不是全局原子：cargo test 每个用例跑在独立线程里，线程局部计数让
// 前后差值不受并行用例干扰。

#[cfg(test)]
thread_local! {
    /// 当前线程累计的真实正文读取次数。
    static REAL_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 当前线程累计的真实读取次数（仅测试构建存在）。
#[cfg(test)]
pub(crate) fn real_read_count() -> usize {
    REAL_READS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn note_real_read() {
    REAL_READS.with(|count| count.set(count.get() + 1));
}

/// 生产构建：计数不存在，调用点保持零开销。
#[cfg(not(test))]
fn note_real_read() {}
