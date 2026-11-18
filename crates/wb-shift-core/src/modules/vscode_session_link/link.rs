//! 关联读写：会话内容/索引读取、位置翻译与会话索引翻译。

use super::identity::*;
use super::*;

/// 读取会话内容状态：按 `index.json.messages[]` 顺序逐条摘要。
///
/// - `Missing`：会话目录下没有 `index.json`（会话不存在）；
/// - `Unavailable`：索引无法解析 / 缺少消息列表 / 内容为空 / 消息文件缺失或损坏——
///   一律不猜，判定侧会得到 `Unknown`；
/// - `Ready`：`normalized` 为有序记录摘要（判定用），`full_digest` 覆盖索引与全部消息文件的
///   原始字节（预览凭据的版本绑定用：任何字节改动都会让它失配）。
///
/// `text` 存放会话索引原文：VS Code 侧没有单一正文字段，写入以文件为单位（见
/// [`translate_session_index`] / [`translate_message`]），该字段不参与写入。
pub fn read_session_content(conv_dir: &Path, conversation_id: &str) -> ContentState {
    let index_path = conv_dir.join("index.json");
    let index_bytes = match std::fs::read(&index_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ContentState::Missing,
        Err(error) => return ContentState::Unavailable(format!("会话索引无法读取：{error}")),
    };
    let index: Value = match serde_json::from_slice(&index_bytes) {
        Ok(value) => value,
        Err(_) => return ContentState::Unavailable("会话索引无法解析，内容可能不完整".to_string()),
    };
    let Some(messages) = index.get("messages").and_then(Value::as_array) else {
        return ContentState::Unavailable("会话索引缺少消息列表，内容可能不完整".to_string());
    };
    if messages.is_empty() {
        return ContentState::Unavailable("会话内容为空，无法确认".to_string());
    }

    let placeholders = placeholder_map(&index, conversation_id);
    let mut line_digests: Vec<String> = Vec::with_capacity(messages.len());
    let mut raw = Vec::new();
    push_len_prefixed(&mut raw, &index_bytes);

    for message in messages {
        let Some(id) = message_id(message) else {
            return ContentState::Unavailable(
                "会话索引中的消息缺少 id，内容可能不完整".to_string(),
            );
        };
        let file = conv_dir.join("messages").join(format!("{id}.json"));
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ContentState::Unavailable(format!(
                    "消息 {id} 的文件不存在，内容可能不完整"
                ));
            }
            Err(error) => {
                return ContentState::Unavailable(format!("消息 {id} 的文件无法读取：{error}"));
            }
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return ContentState::Unavailable(format!("消息 {id} 的文件不是文本，无法确认"));
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return ContentState::Unavailable(format!("消息 {id} 的文件无法解析，内容可能不完整"));
        };
        line_digests.push(line_digest_of(
            &normalize_message(&value, &placeholders).to_string(),
        ));
        push_len_prefixed(&mut raw, &bytes);
    }

    let normalized = NormalizedContent {
        record_count: line_digests.len(),
        total_digest: session_link::total_digest_of(&line_digests),
        line_digests,
    };
    ContentState::Ready(ContentSnapshot {
        text: String::from_utf8_lossy(&index_bytes).to_string(),
        full_digest: full_digest_of(&raw),
        normalized,
    })
}

// ---------------------------------------------------------------------------
// 位置对齐与引用翻译（design §6.1）
// ---------------------------------------------------------------------------

/// 位置对齐的翻译表：源记录序列的前 `target_ids.len()` 条与目标记录一一对应。
///
/// 判定保证「目标等于基线、源是基线的严格有序追加」，因此前缀逐位置对应是成立的；
/// 目标多出的部分不会翻译（不会凭空造出目标 id）。
pub fn positional_translation(
    source_ids: &[String],
    target_ids: &[String],
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (source_id, target_id) in source_ids.iter().zip(target_ids.iter()) {
        map.insert(source_id.clone(), target_id.clone());
    }
    map
}

/// 按翻译表重写会话索引里的一条消息条目（只换 id，其余字段原样）。
pub fn translate_message_entry(entry: &Value, translation: &BTreeMap<String, String>) -> Value {
    let mut out = entry.clone();
    if let Some(object) = out.as_object_mut() {
        if let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_string) {
            if let Some(new_id) = translation.get(&id) {
                object.insert("id".to_string(), json!(new_id));
            }
        }
    }
    out
}

/// 按翻译表重写会话索引里的一条请求条目：`id` 与 `messages[]` 引用。
pub fn translate_request_entry(request: &Value, translation: &BTreeMap<String, String>) -> Value {
    let mut out = request.clone();
    let Some(object) = out.as_object_mut() else {
        return out;
    };
    if let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_string) {
        if let Some(new_id) = translation.get(&id) {
            object.insert("id".to_string(), json!(new_id));
        }
    }
    if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            if let Some(id) = message.as_str().map(str::to_string) {
                if let Some(new_id) = translation.get(&id) {
                    *message = json!(new_id);
                }
            }
        }
    }
    out
}

/// 按翻译表重写整份会话索引（`messages[].id` + `requests[].id` + `requests[].messages[]`）。
pub fn translate_session_index(index: &Value, translation: &BTreeMap<String, String>) -> Value {
    let mut out = index.clone();
    if let Some(messages) = out.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            *message = translate_message_entry(message, translation);
        }
    }
    if let Some(requests) = out.get_mut("requests").and_then(Value::as_array_mut) {
        for request in requests.iter_mut() {
            *request = translate_request_entry(request, translation);
        }
    }
    out
}

/// 按翻译表重写一条消息文件：顶层 `id` 与 `extra` 内的 id 引用（与复制路径同范围）。
pub fn translate_message(value: &Value, translation: &BTreeMap<String, String>) -> Value {
    let mut out = value.clone();
    if let Some(object) = out.as_object_mut() {
        if let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_string) {
            if let Some(new_id) = translation.get(&id) {
                object.insert("id".to_string(), json!(new_id));
            }
        }
        if let Some(extra) = object.get("extra").cloned() {
            object.insert(
                "extra".to_string(),
                replace_ids_in_extra(&extra, translation),
            );
        }
    }
    out
}

/// 会话索引里的消息 id 序列（按索引顺序，空白 id 跳过）。
pub fn message_ids_of(index: &Value) -> Vec<String> {
    index
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().filter_map(message_id).collect())
        .unwrap_or_default()
}

/// 读会话索引（文件缺失或损坏返回 `None`）：翻译与写入前的定位用。
pub fn read_session_index(conv_dir: &Path) -> Option<Value> {
    read_json(&conv_dir.join("index.json"))
}
