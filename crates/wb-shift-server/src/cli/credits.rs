//! `credits` 子命令：逐账号积分（实时 / 本地快照）。
//!
//! 实时路径用 `JoinSet` 限流到 [`MAX_CREDITS_CONCURRENCY`] 个并发，整体受
//! `--timeout` 预算约束（默认 8s）。超时或单账号失败**不 fail-fast**：
//! 该账号写 `{ ok: false, error }`，顶层置 `partial: true`，其余账号照常返回。
//! 这样 AI 拿到的是"部分可用"的账号池，而不是一句失败。

use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

use wb_shift_core::modules::credit_usage;
use wb_shift_core::modules::credits as core_credits;

use super::*;

/// 一次 `credits` 查询的选项。
pub(crate) struct CreditsOptions {
    /// `--cached`：只读本地快照，不联网。
    pub(crate) cached: bool,
    /// 实时查询的整体预算（秒）。
    pub(crate) timeout_secs: u64,
}

impl CreditsOptions {
    pub(crate) fn from_options(options: &Options) -> Result<Self, String> {
        Ok(Self {
            cached: options.has("cached"),
            timeout_secs: parse_timeout(options.value("timeout"))?,
        })
    }
}

/// `credits [--json] [--cached] [--timeout 秒]`。
pub(crate) async fn credits_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json", "cached"]) {
        return code;
    }
    let credits_options = match CreditsOptions::from_options(options) {
        Ok(options) => options,
        Err(error) => return emit_error(&error, EXIT_USAGE),
    };
    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let (entries, partial) = entries(&accounts, &credits_options).await;
    emit(
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "partial": partial,
            "accounts": entries,
        }),
        EXIT_OK,
    )
}

/// 逐账号积分条目 + 是否会话不完整（`partial`）。
///
/// 返回的数组与 `accounts` **一一对应、顺序一致**，调用方可以按下标配对。
pub(crate) async fn entries(accounts: &[Value], options: &CreditsOptions) -> (Vec<Value>, bool) {
    if options.cached {
        let snapshots = credit_usage::load_snapshots();
        progress(&format!(
            "[credits] 读取本地快照：{} 条记录（不联网）",
            snapshots.len()
        ));
        return cached_entries(accounts, &snapshots);
    }
    live_entries(accounts, options.timeout_secs).await
}

/// 实时路径：并发 4、总预算 `timeout_secs`，逐账号失败不影响其它账号。
async fn live_entries(accounts: &[Value], timeout_secs: u64) -> (Vec<Value>, bool) {
    let total = accounts.len();
    let observed_at = config::now_ms();
    if total == 0 {
        return (Vec::new(), false);
    }
    progress(&format!(
        "[credits] 实时查询 {total} 个账号（并发 {}，预算 {timeout_secs}s）",
        MAX_CREDITS_CONCURRENCY
    ));

    let mut slots: Vec<Option<Value>> = vec![None; total];
    let mut completed = 0usize;
    let budget = Duration::from_secs(timeout_secs.max(1));

    let collected = timeout(budget, async {
        let mut set: JoinSet<(usize, Value)> = JoinSet::new();
        let mut next = 0usize;
        loop {
            while set.len() < MAX_CREDITS_CONCURRENCY && next < total {
                let index = next;
                let account = accounts[index].clone();
                set.spawn(async move { (index, core_credits::get_credit_expiry(&account).await) });
                next += 1;
            }
            if set.is_empty() {
                break;
            }
            match set.join_next().await {
                Some(Ok((index, raw))) => {
                    if let Some(slot) = slots.get_mut(index) {
                        *slot = Some(live_entry(accounts.get(index), &raw, observed_at));
                    }
                    completed += 1;
                    progress(&format!("[credits] {completed}/{total}"));
                }
                Some(Err(error)) => {
                    // 单个账号的任务异常不终止整批：留空的槽位在下面统一补成失败条目。
                    progress(&format!("[credits] 查询任务异常：{error}"));
                }
                None => break,
            }
        }
    })
    .await;

    let timed_out = collected.is_err();
    if timed_out {
        progress(&format!(
            "[credits] 总预算 {timeout_secs}s 用尽，未完成的账号按失败返回"
        ));
    }

    let mut partial = timed_out;
    let entries: Vec<Value> = accounts
        .iter()
        .enumerate()
        .map(|(index, account)| {
            if let Some(entry) = slots.get_mut(index).and_then(|slot| slot.take()) {
                return entry;
            }
            partial = true;
            json!({
                "id": account::get_str(account, "id").unwrap_or_default(),
                "source": "live",
                "ok": false,
                "error": if timed_out { "查询超时（总预算用尽）" } else { "查询未完成" },
                "remaining": Value::Null,
                "total": Value::Null,
                "expiringSoonRemaining": Value::Null,
                "soonestExpireAt": Value::Null,
                "observedAt": observed_at,
            })
        })
        .collect();

    (entries, partial)
}

/// 把 core 的积分结果折叠成 CLI 契约字段（不携带 token / 资源明细）。
fn live_entry(account: Option<&Value>, raw: &Value, observed_at: i64) -> Value {
    let id = account
        .and_then(|account| account::get_str(account, "id"))
        .unwrap_or_default();
    if raw.get("ok").and_then(Value::as_bool) == Some(true) {
        return json!({
            "id": id,
            "source": "live",
            "ok": true,
            "remaining": number_or_null(raw.get("totalRemaining")),
            "total": number_or_null(raw.get("totalCapacity")),
            "expiringSoonRemaining": number_or_null(raw.get("expiringSoonRemaining")),
            "soonestExpireAt": raw.get("soonestExpireAt").cloned().unwrap_or(Value::Null),
            "observedAt": raw.get("updatedAt").and_then(Value::as_i64).unwrap_or(observed_at),
        });
    }
    json!({
        "id": id,
        "source": "live",
        "ok": false,
        "error": raw
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("积分查询失败"),
        "remaining": Value::Null,
        "total": Value::Null,
        "expiringSoonRemaining": Value::Null,
        "soonestExpireAt": Value::Null,
        "observedAt": observed_at,
    })
}

/// 归一化负零：core 在空集合上 `sum()` 会得到 `-0.0`（JSON 里合法），但 AI 侧
/// 用严格等于比较 `0` 会意外失败，这里统一折成 `0.0`。
fn number_or_null(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(number)) => match number.as_f64() {
            Some(0.0) => json!(0.0),
            _ => Value::Number(number.clone()),
        },
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// `--cached` 路径：每个账号取本地**最近一条**快照。
///
/// 快照只有 `total/remaining/ts`，没有到期资源，所以
/// `expiringSoonRemaining` / `soonestExpireAt` 一律 `null`（不是 0，避免被读成
/// 「有 0 点积分即将到期」）。
fn cached_entries(accounts: &[Value], snapshots: &[Value]) -> (Vec<Value>, bool) {
    let mut latest: HashMap<String, &Value> = HashMap::new();
    for snapshot in snapshots {
        let Some(account_id) = snapshot.get("accountId").and_then(Value::as_str) else {
            continue;
        };
        let ts = snapshot.get("ts").and_then(Value::as_i64).unwrap_or(0);
        let current = latest
            .get(account_id)
            .and_then(|value| value.get("ts"))
            .and_then(Value::as_i64)
            .unwrap_or(i64::MIN);
        if ts >= current {
            latest.insert(account_id.to_string(), snapshot);
        }
    }

    let mut partial = false;
    let entries: Vec<Value> = accounts
        .iter()
        .map(|account| {
            let id = account::get_str(account, "id").unwrap_or_default();
            let Some(snapshot) = latest.get(&id) else {
                partial = true;
                return json!({
                    "id": id,
                    "source": "cache",
                    "ok": false,
                    "error": "本地没有该账号的积分快照（先跑一次实时的 credits）",
                    "remaining": Value::Null,
                    "total": Value::Null,
                    "expiringSoonRemaining": Value::Null,
                    "soonestExpireAt": Value::Null,
                    "observedAt": Value::Null,
                });
            };
            json!({
                "id": id,
                "source": "cache",
                "ok": true,
                "remaining": number_or_null(snapshot.get("remaining")),
                "total": number_or_null(snapshot.get("total")),
                // 快照没有到期资源：必须显式 null，不能填 0。
                "expiringSoonRemaining": Value::Null,
                "soonestExpireAt": Value::Null,
                "observedAt": snapshot.get("ts").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    (entries, partial)
}
