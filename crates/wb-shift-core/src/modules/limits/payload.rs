use super::scan::WINDOW_DAYS;
use serde_json::{json, Value};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

/// 账号归因后的条目：`(账号, 模型)` 聚合的输入；hook 通路（`rate_limit_events.rs`）也产出它。
#[derive(Clone)]
pub(crate) struct Resolved {
    pub(crate) account_id: String,
    pub(crate) model: Option<String>,
    pub(crate) reset_at: i64,
    pub(crate) first_seen_at: i64,
    pub(crate) hit_count: u32,
}

/// 聚合与过滤：per (账号, 模型) 取 `resetAt` 最大的一条（同模型多次限流以最新一次为准），
/// 只保留官方重置时刻尚未到达的条目。
///
/// 过滤基准是**当前时刻**而不是 `scanned_at`：日志扫描结果会缓存 5 分钟，若按扫描时刻过滤，
/// 缓存期间已经解锁的条目会继续出现在台账里；hook 通路也按当前时刻清理，两条通路必须使用
/// 同一口径。`scannedAt` 仍然只表示最近一次真实扫描的时刻（前端据此节流）。
///
/// 无受限模型的账号**不出现在结果里**，前端据此决定是否渲染图标。
pub(super) fn build_payload(resolved: Vec<Resolved>, scanned_at: i64, now: i64) -> Value {
    let mut latest: BTreeMap<(String, Option<String>), Resolved> = BTreeMap::new();
    for item in resolved {
        if item.reset_at <= now {
            continue;
        }
        match latest.entry((item.account_id.clone(), item.model.clone())) {
            Entry::Occupied(mut entry) => {
                if item.reset_at > entry.get().reset_at {
                    entry.insert(item);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(item);
            }
        }
    }

    let mut by_account: BTreeMap<String, Vec<Resolved>> = BTreeMap::new();
    for item in latest.into_values() {
        by_account
            .entry(item.account_id.clone())
            .or_default()
            .push(item);
    }
    let accounts: Vec<Value> = by_account
        .into_iter()
        .map(|(account_id, mut items)| {
            // 按恢复时间升序：最早解锁的排在最前（最有行动价值）。
            items.sort_by_key(|item| item.reset_at);
            let limited: Vec<Value> = items
                .into_iter()
                .map(|item| {
                    json!({
                        "model": item.model,
                        "resetAt": item.reset_at,
                        "firstSeenAt": item.first_seen_at,
                        "hitCount": item.hit_count,
                    })
                })
                .collect();
            json!({ "accountId": account_id, "limited": limited })
        })
        .collect();
    json!({
        "scannedAt": scanned_at,
        "windowDays": WINDOW_DAYS,
        "accounts": accounts,
    })
}
