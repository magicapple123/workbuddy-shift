use super::scan::WINDOW_DAYS;
use serde_json::{json, Value};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

/// 账号归因后的条目：`(账号, 模型)` 聚合的输入；hook 通路（`rate_limit_events.rs`）也产出它。
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    pub(crate) account_id: String,
    pub(crate) model: Option<String>,
    pub(crate) reset_at: i64,
    pub(crate) first_seen_at: i64,
    pub(crate) hit_count: u32,
    /// 来源标签：`workbuddy-cn` / `workbuddy-ai` / `codebuddy-cli` / `codebuddy-ide` /
    /// `codebuddy-cn-ide` / `vscode-ext`（第四轮 13-B：卡片要能说清「这是哪个客户端」）。
    ///
    /// `vscode-ext` 由 VS Code 插件宿主的 agent 日志产出（`ScanSource::AgentLog`，见
    /// `ledger::cache::scan_agent_logs`）：它同时是前端 `SOURCE_LABELS`、`has_vscode` 与
    /// borrowed 兜底的判据，因此插件来源必须用这个既有字符串。
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
/// 第二轮把 `model: None` 的条目按 **`(账号, resetAt)`** 归并到同一账号下有模型的条目：
/// 插件宿主日志被轮转/截断时缺少 `[ModelProvider]` 行，同一次 429 会被扫成
/// `{model: null, source: "vscode-ext"}` + `{model: "…", source: "codebuddy-ide"}` 两条，
/// 用户看到两张卡片却只有一次限流。归并键只认「同账号 + 同重置时刻」：
/// **重置时刻不同就是两次不同的限流**，绝不合并（否则会把两次限流压成一条，用户漏掉一次）。
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

    let by_account = merge_unknown_models(latest);
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

impl Resolved {
    /// 同一次限流的两个观测合并成一条：命中次数累加、首次出现取更早。
    ///
    /// 来源若任一侧是 `vscode-ext` 就保留它 —— 触发限流的是插件本身（另侧只是把模型名
    /// 补了回来），而 `has_vscode` 与 borrowed 兜底都认这个标签。
    fn merge(&mut self, other: Resolved) {
        self.hit_count = self.hit_count.saturating_add(other.hit_count);
        self.first_seen_at = self.first_seen_at.min(other.first_seen_at);
        if self.model.is_none() {
            self.model = other.model;
        }
        if other.source == "vscode-ext" && self.source != "vscode-ext" {
            self.source = other.source;
        }
    }
}

/// 把 `model: None` 的条目归并到同账号、**同 `resetAt`** 的有模型条目上。
///
/// 只在「可以确定是同一次限流」时归并：
/// - 键是 `(账号, resetAt)`：重置时刻不同就是两次限流，一律不合并（漏报比重复更糟）；
/// - 同账号同 `resetAt` 有多条有模型条目时（两个模型恰好同时解锁），无模型那条只并入
///   **第一条**（模型名在 `BTreeMap` 里有序，结果确定），其余条目原样保留 —— 宁可少并，
///   也不把无模型的那条复制成多份；
/// - 无模型条目之间本来就无法用模型区分，先按 `(账号, resetAt)` 收一次再归并。
fn merge_unknown_models(
    latest: BTreeMap<(String, Option<String>), Resolved>,
) -> BTreeMap<String, Vec<Resolved>> {
    let mut entries: Vec<Resolved> = Vec::new();
    // (账号, resetAt) → `entries` 里的下标；同键多条有模型条目时只登记第一条。
    let mut by_event: BTreeMap<(String, i64), usize> = BTreeMap::new();
    let mut unknown: BTreeMap<(String, i64), Resolved> = BTreeMap::new();
    for item in latest.into_values() {
        if item.model.is_some() {
            by_event
                .entry((item.account_id.clone(), item.reset_at))
                .or_insert(entries.len());
            entries.push(item);
            continue;
        }
        match unknown.entry((item.account_id.clone(), item.reset_at)) {
            Entry::Occupied(mut entry) => entry.get_mut().merge(item),
            Entry::Vacant(entry) => {
                entry.insert(item);
            }
        }
    }
    for ((account_id, reset_at), item) in unknown {
        match by_event.get(&(account_id, reset_at)) {
            Some(&index) => entries[index].merge(item),
            None => entries.push(item),
        }
    }
    let mut by_account: BTreeMap<String, Vec<Resolved>> = BTreeMap::new();
    for item in entries {
        by_account
            .entry(item.account_id.clone())
            .or_default()
            .push(item);
    }
    by_account
}
