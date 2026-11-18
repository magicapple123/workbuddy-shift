//! 偏好合并：逐键覆盖、denylist、主题归属改写。纯函数，便于单测。

use serde_json::{json, Map, Value};

use super::{DENYLIST, THEME_SELECTION_KEY};

/// 一次按键合并的结果。
#[derive(Clone, Debug, Default)]
pub struct MergeOutcome {
    pub merged: Map<String, Value>,
    pub changed: Vec<Value>,
    pub skipped: Vec<Value>,
}

/// 逐键覆盖：目标独有键保留，源键覆盖，denylist 跳过。
///
/// `theme_selection` 单独改写 `uid` / `revision`（见模块文档）。
pub fn merge_preferences(
    source: &Map<String, Value>,
    target: &Map<String, Value>,
    target_uid: &str,
    now: i64,
) -> MergeOutcome {
    let mut merged = target.clone();
    let mut changed = Vec::new();
    let mut skipped = Vec::new();

    for (key, value) in source {
        if DENYLIST.contains(&key.as_str()) {
            skipped.push(json!({
                "key": key,
                "reason": "设备本地状态，不随账号同步",
            }));
            continue;
        }
        let mut next = value.clone();
        if key == THEME_SELECTION_KEY {
            next = rewrite_theme_selection(&next, target_uid, now);
        }
        let previous = merged.get(key).cloned();
        let action = if previous.is_none() {
            "add"
        } else if previous.as_ref() == Some(&next) {
            continue;
        } else {
            "overwrite"
        };
        merged.insert(key.clone(), next.clone());
        changed.push(json!({
            "key": key,
            "action": action,
            "from": previous.unwrap_or(Value::Null),
            "to": next,
        }));
    }

    MergeOutcome {
        merged,
        changed,
        skipped,
    }
}

/// 主题选择：保留资源与来源，把归属改到目标账号，revision 抬到严格更大。
fn rewrite_theme_selection(value: &Value, target_uid: &str, now: i64) -> Value {
    let mut selection = value.as_object().cloned().unwrap_or_default();
    let source_revision = selection
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    selection.insert("uid".to_string(), json!(target_uid));
    selection.insert(
        "revision".to_string(),
        json!(now.max(source_revision.saturating_add(1))),
    );
    Value::Object(selection)
}
