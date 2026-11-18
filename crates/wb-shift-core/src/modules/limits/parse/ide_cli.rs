//! CodeBuddy IDE 日志行解析：IDE 归因、模型 id 与会话/会话 id 提取。

use super::*;

/// IDE 格式的事件 id 与模型归因。
///
/// 事件 id 是 `conversationId`（三级提取）；模型走 `conversationId → modelId`（会话级，
/// 实测限额行前 814 ms 命中）→ 本行 `modelId=` → 本行 JSON `"model"`（报错行内，
/// 是本次请求自己的模型）→ 文件级 `[handleAuthError] modelId=` → 未知（不猜）。
pub(crate) fn ide_attribution(
    line: &str,
    conversation_models: &HashMap<String, String>,
    auth_error_model: Option<&str>,
) -> (Option<String>, Option<String>) {
    let session_id = ide_conversation_id(line).map(str::to_string);
    let model = session_id
        .as_deref()
        .and_then(|conversation| conversation_models.get(conversation))
        .cloned()
        .or_else(|| ide_model_id(line).map(str::to_string))
        .or_else(|| ide_json_model(line))
        .or_else(|| auth_error_model.map(str::to_string));
    (session_id, model)
}

/// 取 IDE 行的 `modelId=` 字段（哨兵值见 `MODEL_UNKNOWN`）。
pub(crate) fn ide_model_id(line: &str) -> Option<&str> {
    field_model(line, IDE_MODEL_FIELD).filter(|value| known_model(value))
}

/// IDE 会话 id 形态：32 位 hex（`conversationId`）或 36 位带连字符 uuid（兼容旧形态）。
pub(crate) fn is_ide_session_id(value: &str) -> bool {
    is_hex_id(value, 32) || is_hex_id(value, 36)
}

/// IDE 的会话 id（不含 JSON 形式）：`conversationId=<conv32>` → `[AcpAgent:<conv32>]`
/// → `[AcpConnection:<conv32>]`。
///
/// 用于「顺序扫描时把 `modelId=` 记到哪个会话」——`[handleAuthError]` 这类行没有会话 id，
/// 只作为文件级兜底。
pub(crate) fn ide_session_id(line: &str) -> Option<&str> {
    field_after(line, "conversationId=")
        .filter(|value| is_ide_session_id(value))
        .or_else(|| bracket_tag_after(line, IDE_ACP_AGENT_TAG))
        .or_else(|| bracket_tag_after(line, IDE_ACP_CONNECTION_TAG))
}

/// IDE 的事件 id：会话 id，再回退报错行里的 JSON 形式 `"conversationId":"<conv32>"`。
pub(crate) fn ide_conversation_id(line: &str) -> Option<&str> {
    ide_session_id(line)
        .or_else(|| json_field(line, "conversationId").filter(|value| is_ide_session_id(value)))
}

/// 报错行内的 JSON 模型字段 `"model":"<value>"`（哨兵值同 `MODEL_UNKNOWN`）。
pub(crate) fn ide_json_model(line: &str) -> Option<String> {
    let value = json_field(line, "model")?;
    known_model(value).then(|| value.to_string())
}
