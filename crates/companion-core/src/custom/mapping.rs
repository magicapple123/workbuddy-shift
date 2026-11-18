//! 模板字段映射：把原始载荷按模板解析成标准事件字段。
//!
//! 只做字段解析与错误归一化；会话生命周期规则仍由根模块的引擎负责。

use super::engine::MappingError;
use crate::custom::limits::EVENT_NAME_MAX;
use crate::custom::pointer;
use crate::custom::template::Template;
use serde_json::{json, Value};

pub(super) struct StandardEventContext<'a> {
    pub(super) template: &'a Template,
    pub(super) source: &'a str,
    pub(super) session_id: &'a str,
    pub(super) ts: i64,
    pub(super) cwd: Option<&'a str>,
    pub(super) title: Option<&'a str>,
}

impl StandardEventContext<'_> {
    pub(super) fn event(&self, round: &str, extra: Value) -> Value {
        let mut value = json!({
            "source": self.source,
            "sessionId": self.session_id,
            "roundId": round,
            "ts": self.ts,
            "sourceLabel": self.template.name,
        });
        if let Some(cwd) = self.cwd.filter(|value| !value.is_empty()) {
            value["cwd"] = json!(cwd);
        }
        if let Some(title) = self.title.filter(|value| !value.is_empty()) {
            value["title"] = json!(title);
        }
        if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            for (key, item) in extra {
                target.insert(key.clone(), item.clone());
            }
        }
        value
    }
}

pub(super) fn required_event(raw: &Value, pointer_text: &str) -> Result<String, MappingError> {
    match pointer::resolve(raw, pointer_text) {
        None | Some(Value::Null) => Err(MappingError::new(
            "missing_event",
            "/mapping/event",
            "载荷中缺少事件名",
        )),
        Some(Value::String(text)) => {
            if text.is_empty() {
                return Err(MappingError::new(
                    "type_mismatch",
                    "/mapping/event",
                    "事件名必须为非空字符串",
                ));
            }
            if text.chars().count() > EVENT_NAME_MAX {
                return Err(MappingError::new(
                    "too_long",
                    "/mapping/event",
                    "字段超出长度上限",
                ));
            }
            Ok(text.clone())
        }
        Some(_) => Err(MappingError::new(
            "type_mismatch",
            "/mapping/event",
            "事件名必须为非空字符串",
        )),
    }
}

fn field_path(field: &str) -> String {
    format!("/mapping/{field}")
}

pub(super) fn id_field(
    raw: &Value,
    pointer_text: Option<&String>,
    max: usize,
    field: &str,
    missing_detail: &str,
) -> Result<Option<String>, MappingError> {
    let Some(pointer_text) = pointer_text else {
        return Ok(None);
    };
    match pointer::resolve(raw, pointer_text) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            if text.is_empty() {
                return Err(MappingError::new(
                    "type_mismatch",
                    field_path(field),
                    missing_detail,
                ));
            }
            if text.chars().count() > max {
                return Err(MappingError::new(
                    "too_long",
                    field_path(field),
                    "字段超出长度上限",
                ));
            }
            Ok(Some(text.clone()))
        }
        Some(_) => Err(MappingError::new(
            "type_mismatch",
            field_path(field),
            missing_detail,
        )),
    }
}

pub(super) fn text_field(
    raw: &Value,
    pointer_text: Option<&String>,
    max: usize,
    field: &str,
) -> Result<Option<String>, MappingError> {
    let Some(pointer_text) = pointer_text else {
        return Ok(None);
    };
    match pointer::resolve(raw, pointer_text) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            if text.chars().count() > max {
                return Err(MappingError::new(
                    "too_long",
                    field_path(field),
                    "字段超出长度上限",
                ));
            }
            Ok(Some(text.clone()))
        }
        Some(_) => Err(MappingError::new(
            "type_mismatch",
            field_path(field),
            "字段必须为字符串",
        )),
    }
}

pub(super) fn timestamp_field(
    raw: &Value,
    pointer_text: Option<&String>,
) -> Result<Option<i64>, MappingError> {
    let Some(pointer_text) = pointer_text else {
        return Ok(None);
    };
    match pointer::resolve(raw, pointer_text) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| {
                MappingError::new(
                    "type_mismatch",
                    field_path("timestamp"),
                    "timestamp 必须为整数 Unix 毫秒",
                )
            }),
        Some(_) => Err(MappingError::new(
            "type_mismatch",
            field_path("timestamp"),
            "timestamp 必须为整数 Unix 毫秒",
        )),
    }
}

/// `ignoreIfPresent`: a pointer counts as present unless it resolves to null or
/// an empty string.
pub(super) fn present_value(raw: &Value, pointer_text: &str) -> bool {
    match pointer::resolve(raw, pointer_text) {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}
