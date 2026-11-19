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
    /// 来源标签：`workbuddy-cn` / `workbuddy-ai` / `codebuddy-cli` / `codebuddy-ide` /
    /// `codebuddy-cn-ide` / `vscode-ext`（第四轮 13-B：卡片要能说清「这是哪个客户端」）。
    pub(crate) source: String,
}

/// 来源标签的中文名（tooltip / 兜底文案用）。
pub(crate) fn source_label(source: &str) -> &'static str {
    match source {
        "workbuddy-cn" => "WorkBuddy",
        "workbuddy-ai" => "WorkBuddy 国际版",
        "codebuddy-cli" => "CodeBuddy CLI",
        "codebuddy-ide" => "CodeBuddy IDE",
        "codebuddy-cn-ide" => "CodeBuddy CN IDE",
        "vscode-ext" => "VS Code 插件",
        _ => "其它客户端",
    }
}

/// 聚合与过滤：per (账号, 模型) 取 `resetAt` 最大的一条（同模型多次限流以最新一次为准），
/// 只保留官方重置时刻尚未到达的条目。
///
/// 过滤基准是**当前时刻**而不是 `scanned_at`：日志扫描结果会缓存 5 分钟，若按扫描时刻过滤，
/// 缓存期间已经解锁的条目会继续出现在台账里；hook 通路也按当前时刻清理，两条通路必须使用
/// 同一口径。`scannedAt` 仍然只表示最近一次真实扫描的时刻（前端据此节流）。
///
/// 无受限模型的账号**不出现在结果里**，前端据此决定是否渲染图标。
pub(super) fn build_payload(
    resolved: Vec<Resolved>,
    scanned_at: i64,
    now: i64,
    cross_client_infer: bool,
) -> Value {
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
            let has_vscode = items.iter().any(|item| item.source == "vscode-ext");
            // 13-B：VS Code 没有自有记录时，用同账号其它客户端里最晚的 resetAt 兜底。
            // 只兜底、不合并：`limited` 保持各来源原样，borrowed 单独一个字段并带来源。
            let borrowed: Option<Value> = if cross_client_infer && !has_vscode {
                items
                    .iter()
                    .filter(|item| item.source != "vscode-ext")
                    .max_by_key(|item| item.reset_at)
                    .map(|item| {
                        json!({
                            "resetAt": item.reset_at,
                            "model": item.model,
                            "source": item.source,
                            "sourceLabel": source_label(&item.source),
                        })
                    })
            } else {
                None
            };
            let limited: Vec<Value> = items
                .into_iter()
                .map(|item| {
                    json!({
                        "model": item.model,
                        "resetAt": item.reset_at,
                        "firstSeenAt": item.first_seen_at,
                        "hitCount": item.hit_count,
                        "source": item.source,
                    })
                })
                .collect();
            json!({
                "accountId": account_id,
                "limited": limited,
                "borrowed": borrowed,
            })
        })
        .collect();
    json!({
        "scannedAt": scanned_at,
        "windowDays": WINDOW_DAYS,
        "accounts": accounts,
    })
}
