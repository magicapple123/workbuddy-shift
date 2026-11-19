//! 本地积分观察快照与统计投影。
//!
//! WorkBuddy 只返回当前资源余额，没有可复用的历史账单序列。因此这里把
//! 成功查询到的余额保存为本地观察值，再用相邻快照的正向下降量推导“观察到
//! 的消耗”。首次快照和余额增加只建立新的基线，不产生负数消耗。

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::modules::account::{account_display_name, load_accounts, variant_of};
use crate::modules::config::{
    atomic_write, credit_usage_snapshots_file, load_checkin_logs, now_ms, store_dir,
    CHECKIN_LOG_KEEP_DAYS,
};
use crate::modules::locks;
use crate::modules::official_usage;
use crate::modules::variant::WbVariant;

pub const CREDIT_SNAPSHOT_RETENTION_DAYS: i64 = 90;
pub const CREDIT_SNAPSHOT_MAX_RECORDS: usize = 5_000;
pub const CREDIT_SNAPSHOT_DEDUPE_WINDOW_MS: i64 = 5 * 60 * 1000;
const CREDIT_STATS_MAX_EVENTS: usize = 200;

static SNAPSHOT_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
struct Snapshot {
    ts: i64,
    account_id: String,
    account_name: String,
    total: f64,
    remaining: f64,
    /// 快照所属档位；旧数据无该字段时按国内版解释（零迁移）。
    variant: WbVariant,
}

#[derive(Clone, Debug)]
struct UsageEvent {
    ts: i64,
    date: String,
    account_id: String,
    /// 该次消耗所属档位（取自产生下降区间的快照）。
    variant: WbVariant,
    amount: f64,
}

#[derive(Clone, Debug)]
struct CheckinEvent {
    ts: i64,
    date: String,
    account_id: Option<String>,
    account_name: String,
    result: String,
    error: Option<String>,
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(value)) => value.as_f64(),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn snapshot_from_value(value: &Value) -> Option<Snapshot> {
    let account_id = non_empty_string(value.get("accountId"))?;
    let ts = value.get("ts").and_then(Value::as_i64)?;
    let total = number(value.get("total"))?.max(0.0);
    let remaining = number(value.get("remaining"))?.max(0.0);
    Some(Snapshot {
        ts,
        account_id,
        account_name: non_empty_string(value.get("accountName"))
            .unwrap_or_else(|| "unknown".to_string()),
        total,
        remaining,
        // 旧快照没有 `variant` 字段：按国内版解释（历史数据零迁移）。
        variant: WbVariant::parse(value.get("variant").and_then(Value::as_str)),
    })
}

fn snapshot_value(
    ts: i64,
    account_id: &str,
    account_name: &str,
    total: f64,
    remaining: f64,
    variant: WbVariant,
) -> Value {
    json!({
        "ts": ts,
        "accountId": account_id,
        "accountName": account_name,
        "total": total.max(0.0),
        "remaining": remaining.max(0.0),
        "variant": variant.as_str(),
    })
}

/// 读取本地观察快照；文件缺失或损坏时返回空列表。
pub fn load_snapshots() -> Vec<Value> {
    let path = credit_usage_snapshots_file();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) {
            return items;
        }
    }
    vec![]
}

fn should_suppress_duplicate(
    snapshots: &[Value],
    account_id: &str,
    total: f64,
    remaining: f64,
    at_ms: i64,
) -> bool {
    let Some(latest) = snapshots
        .iter()
        .filter_map(snapshot_from_value)
        .filter(|snapshot| snapshot.account_id == account_id)
        .max_by_key(|snapshot| snapshot.ts)
    else {
        return false;
    };

    latest.ts <= at_ms
        && at_ms - latest.ts <= CREDIT_SNAPSHOT_DEDUPE_WINDOW_MS
        && (latest.total - total).abs() < f64::EPSILON
        && (latest.remaining - remaining).abs() < f64::EPSILON
}

fn normalize_snapshots(snapshots: &[Value], at_ms: i64) -> Vec<Value> {
    let cutoff = at_ms.saturating_sub(CREDIT_SNAPSHOT_RETENTION_DAYS * 24 * 3600 * 1000);
    let mut kept: Vec<Value> = snapshots
        .iter()
        .filter_map(|value| {
            let snapshot = snapshot_from_value(value)?;
            (snapshot.ts >= cutoff && snapshot.ts <= at_ms).then_some(value.clone())
        })
        .collect();
    if kept.len() > CREDIT_SNAPSHOT_MAX_RECORDS {
        kept.drain(..kept.len() - CREDIT_SNAPSHOT_MAX_RECORDS);
    }
    kept
}

/// 写入一个成功的资源观察值。
///
/// 同一账号同一资源值在短时间内只保留一条；资源值发生变化时立即保留，
/// 这样余额下降可以归因到新快照。返回值表示本次是否实际写入。
pub fn record_snapshot(
    account_id: &str,
    account_name: &str,
    total: f64,
    remaining: f64,
    variant: WbVariant,
) -> bool {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return false;
    }

    let at_ms = now_ms();
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续写快照。
    let _guard = locks::recover(&SNAPSHOT_WRITE_LOCK);
    let snapshots = load_snapshots();
    if should_suppress_duplicate(&snapshots, account_id, total, remaining, at_ms) {
        return false;
    }

    let mut kept = normalize_snapshots(&snapshots, at_ms);
    kept.push(snapshot_value(
        at_ms,
        account_id,
        account_name.trim(),
        total,
        remaining,
        variant,
    ));
    if kept.len() > CREDIT_SNAPSHOT_MAX_RECORDS {
        kept.drain(..kept.len() - CREDIT_SNAPSHOT_MAX_RECORDS);
    }

    if let Err(error) = std::fs::create_dir_all(store_dir()).and_then(|_| {
        let content = serde_json::to_string_pretty(&kept).unwrap_or_default();
        atomic_write(&credit_usage_snapshots_file(), &content)
    }) {
        eprintln!("[积分统计] 保存积分快照失败: {error}");
        return false;
    }
    true
}

mod collect;
mod project;

pub use project::get_statistics;

#[cfg(test)]
mod tests {
    use super::*;

    use super::collect::*;
    use super::project::*;
    include!("tests/project.rs");
}
