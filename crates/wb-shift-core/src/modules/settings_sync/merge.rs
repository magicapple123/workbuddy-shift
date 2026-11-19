//! 偏好合并：逐键覆盖、denylist、主题归属改写。纯函数，便于单测。
//!
//! 「变更」分两类（第三轮 P0-2 的根因）：
//! - **真实变更**：键值内容不同，或主题归属要从别的 uid 改到目标 uid —— 需要落盘；
//! - **仅改写归属 / revision**：主题的 `resourceKey`、`source` 等业务字段与目标现值
//!   完全一致，目标 `uid` 也已经等于目标账号，差异只剩 `revision` 的提升 —— 这类
//!   变化只进 `cosmetic`，不计入「覆盖 N 项」，也不单独触发写盘，否则每次预览都会
//!   因为 `now` 不同而永远显示「有 1 项变更」，永远不收敛。

use serde_json::{json, Map, Value};

use super::{DENYLIST, THEME_SELECTION_KEY};

/// 一次按键合并的结果。
#[derive(Clone, Debug, Default)]
pub struct MergeOutcome {
    pub merged: Map<String, Value>,
    /// 真实变更（新增键 / 内容不同 / 归属改到目标账号）。
    pub changed: Vec<Value>,
    /// 仅 `revision`（及已被 `changed` 覆盖的归属改写）变化，不计入「覆盖 N 项」。
    pub cosmetic: Vec<Value>,
    /// denylist 跳过的键。
    pub skipped: Vec<Value>,
    /// 合并后的主题值与目标现值不同 —— 即写入会改写主题归属 / `revision`。
    pub theme_rewrite: bool,
}

impl MergeOutcome {
    /// 是否存在需要落盘的真实变更。
    pub fn has_real_changes(&self) -> bool {
        !self.changed.is_empty()
    }
}

/// 逐键覆盖：目标独有键保留，源键覆盖，denylist 跳过。
///
/// `theme_selection` 单独改写 `uid` / `revision`（见模块文档）。只有存在真实变更时
/// 调用方才落盘，因此 cosmetic 分支写入 `merged` 的抬高版 `revision` 只在
/// 「同一文件里有别的真实变更」时才会真正生效（与旧行为一致）。
pub fn merge_preferences(
    source: &Map<String, Value>,
    target: &Map<String, Value>,
    target_uid: &str,
    now: i64,
) -> MergeOutcome {
    let mut merged = target.clone();
    let mut changed = Vec::new();
    let mut cosmetic = Vec::new();
    let mut skipped = Vec::new();

    for (key, value) in source {
        if DENYLIST.contains(&key.as_str()) {
            skipped.push(json!({
                "key": key,
                "reason": "设备本地状态，不随账号同步",
            }));
            continue;
        }
        let next = if key == THEME_SELECTION_KEY {
            rewrite_theme_selection(value, target_uid, now)
        } else {
            value.clone()
        };
        let Some(previous) = merged.get(key).cloned() else {
            merged.insert(key.clone(), next.clone());
            changed.push(json!({
                "key": key,
                "action": "add",
                "from": Value::Null,
                "to": next,
            }));
            continue;
        };
        if previous == next {
            // 逐字相同：既不是真实变更也不是 cosmetic。
            continue;
        }
        if key == THEME_SELECTION_KEY && only_revision_differs(&previous, &next) {
            cosmetic.push(json!({
                "key": key,
                "action": "revision",
                "from": previous,
                "to": next.clone(),
            }));
            merged.insert(key.clone(), next);
            continue;
        }
        merged.insert(key.clone(), next.clone());
        changed.push(json!({
            "key": key,
            "action": "overwrite",
            "from": previous,
            "to": next,
        }));
    }

    let theme_rewrite = merged.get(THEME_SELECTION_KEY) != target.get(THEME_SELECTION_KEY);

    MergeOutcome {
        merged,
        changed,
        cosmetic,
        skipped,
        theme_rewrite,
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

/// 除 `revision` 外逐字段相同？
///
/// 目标 `uid` 与目标账号不一致时，`rewrite_theme_selection` 会改 `uid`，
/// 这里必然返回 `false`（仍算真实变更），符合「归属要改到别的账号」的语义。
fn only_revision_differs(previous: &Value, next: &Value) -> bool {
    let (Some(prev), Some(nxt)) = (previous.as_object(), next.as_object()) else {
        return false;
    };
    if prev.len() != nxt.len() {
        return false;
    }
    nxt.iter()
        .all(|(key, value)| key == "revision" || prev.get(key) == Some(value))
}
