//! 稳定身份派生：消息/请求 id 与摘要的确定性派生、占位符归一与 len-prefixed 编码。

use super::*;

/// 长度编码后拼接：避免「两段内容边界不同」产生同一个原始摘要。
pub(super) fn push_len_prefixed(buffer: &mut Vec<u8>, bytes: &[u8]) {
    buffer.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    buffer.extend_from_slice(bytes);
}

/// 索引里一条消息条目的 id（空白视为缺失）。
pub(super) fn message_id(message: &Value) -> Option<String> {
    message
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// 规范化占位符表：按索引顺序给消息 / 请求 id 编号，会话自身 id 用固定标记。
///
/// 同一逻辑会话的源与副本 id 不同但**顺序一致**，因此两侧得到同一套占位符，逐条摘要可比。
pub(super) fn placeholder_map(index: &Value, conversation_id: &str) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(messages) = index.get("messages").and_then(Value::as_array) {
        for (index, message) in messages.iter().enumerate() {
            if let Some(id) = message_id(message) {
                map.entry(id).or_insert_with(|| format!("m{index}"));
            }
        }
    }
    if let Some(requests) = index.get("requests").and_then(Value::as_array) {
        for (index, request) in requests.iter().enumerate() {
            let Some(id) = request
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            map.entry(id.to_string())
                .or_insert_with(|| format!("r{index}"));
        }
    }
    let own_id = conversation_id.trim();
    if !own_id.is_empty() {
        map.insert(own_id.to_string(), SESSION_ID_MARKER.to_string());
    }
    map
}

/// 归一化一条消息文件：顶层 `id` 与 `extra` 内的 id 引用换成占位符，其余字段原样保留。
///
/// 与复制路径的写入范围严格一致（`id` + `extra`）：复制不改 `message` 正文，
/// 归一化同样不改——两侧口径一致，摘要才可能相等。
pub(super) fn normalize_message(value: &Value, placeholders: &BTreeMap<String, String>) -> Value {
    let mut out = value.clone();
    if let Some(object) = out.as_object_mut() {
        if let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_string) {
            if let Some(marker) = placeholders.get(&id) {
                object.insert("id".to_string(), json!(marker));
            }
        }
        if let Some(extra) = object.get("extra").cloned() {
            object.insert(
                "extra".to_string(),
                replace_ids_in_extra(&extra, placeholders),
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 确定性 id 派生（design §6.1：幂等的地基）
// ---------------------------------------------------------------------------

/// 派生副本消息 id：`sha256("vscode-session-link:v1" + 目标 uid + 目标会话 id + 序号 + 源记录摘要)`
/// 取前 32 位小写 hex。
///
/// 同一目标会话、同一序号、同一源内容 → 同一个 id，因此重复执行只会覆盖同一批文件；
/// 序号进公式是为了让「内容完全相同的重复消息」不互相撞 id。
pub fn derive_message_id(
    target_uid: &str,
    target_conversation_id: &str,
    seq_index: usize,
    source_digest: &str,
    salt: u32,
) -> String {
    derive_id(&[
        target_uid,
        target_conversation_id,
        &seq_index.to_string(),
        source_digest,
        &salt.to_string(),
    ])
}

/// 派生副本请求 id：与消息 id 同公式，用请求序号与请求摘要，并加 `req` 段区分。
pub fn derive_request_id(
    target_uid: &str,
    target_conversation_id: &str,
    request_index: usize,
    source_digest: &str,
    salt: u32,
) -> String {
    derive_id(&[
        target_uid,
        target_conversation_id,
        "req",
        &request_index.to_string(),
        source_digest,
        &salt.to_string(),
    ])
}

/// 派生实现：各段长度编码后拼接再取 SHA-256 前 32 位（段内容含分隔符也不会歧义）。
pub(super) fn derive_id(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DERIVE_PREFIX.as_bytes());
    hasher.update([0u8]);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    to_hex(&hasher.finalize())[..32].to_string()
}

/// 请求条目的规范化摘要（派生请求 id 用；与消息摘要无关）。
///
/// 只做占位符替换，不参与记录序列摘要——请求不是记录，改动请求不改变判定。
pub fn request_digest(request: &Value, placeholders: &BTreeMap<String, String>) -> String {
    line_digest_of(&replace_ids_in_extra(request, placeholders).to_string())
}

/// 按索引顺序取出请求的规范化摘要序列，供派生请求 id 使用。
pub fn request_digests(index: &Value, conversation_id: &str) -> Vec<String> {
    let placeholders = placeholder_map(index, conversation_id);
    index
        .get("requests")
        .and_then(Value::as_array)
        .map(|requests| {
            requests
                .iter()
                .map(|request| request_digest(request, &placeholders))
                .collect()
        })
        .unwrap_or_default()
}
