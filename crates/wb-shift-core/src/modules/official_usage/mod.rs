//! WorkBuddy 官方请求用量投影。
//!
//! 这里负责请求最近 31 个自然日、连续扫描明细、校验和归一化，并只向上层
//! 暴露统计字段与有限的请求摘要。投影会写入本地缓存，避免每次打开统计页
//! 都打官方用量接口；上游可能携带的 prompt/input 等字段永远不会被复制。
//!
//! 取数方式受服务端限制：单次查询最多返回 [`OFFICIAL_USAGE_PAGE_SIZE`] 条，
//! 超过时**保留区间内最早的一批**（`total` 也一起被截断成同一数字），翻页
//! 拿不到更新的记录。因此这里不靠 `total` 判断是否取完，而是「取满即把起点
//! 推进到本轮最新一条的时间戳」反复扫描，直到某一轮取不满为止。

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Map, Value};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::modules::account::{account_display_name, get_str, variant_of};
use crate::modules::config::{atomic_write, now_ms, official_usage_cache_file, store_dir};
use crate::modules::credits::authenticated_post;
use crate::modules::variant::WbVariant;

pub const OFFICIAL_USAGE_URL: &str =
    "https://www.workbuddy.cn/billing/meter/get-user-request-usage";
/// 国际版同构接口（实测存在），形态与 `credits` 的国际版 billing 路径一致，无 `/v2` 前缀。
pub const AI_OFFICIAL_USAGE_URL: &str =
    "https://www.workbuddy.ai/billing/meter/get-user-request-usage";

/// 按账号档位选择官方用量接口 URL，这是唯一的档位分派点。
/// 绝不跨档位请求：国际版 token 不打国内域，国内版 token 也不打国际域
/// （会污染统计并触发网关一致性校验失败）。
fn official_usage_url_for(account: &Value) -> &'static str {
    match variant_of(account) {
        WbVariant::Cn => OFFICIAL_USAGE_URL,
        WbVariant::Ai => AI_OFFICIAL_USAGE_URL,
    }
}
pub const OFFICIAL_USAGE_PAGE_SIZE: usize = 3_000;

/// 同时采集的账号数上限。
///
/// 每个账号内部是串行翻页，账号之间互不依赖；串行时 12 个账号要排队等完全部分页。
/// 限到 6 是为了不把官方接口打到限频（单账号仍会自己退避重试）。
// 12 个账号并发采集：账号间互相独立，单账号内部仍串行翻页。
// 限频可回退到 6（见 AUDIT §3 T4）；仅在官方接口大面积 429 时需要。
const OFFICIAL_USAGE_FETCH_CONCURRENCY: usize = 12;
/// 每个账号落进 `requests` 明细的条数上限。
///
/// 官方接口已全量扫回（见 [`OFFICIAL_USAGE_PAGE_SIZE`] 的分页扫描），这里只是
/// 「明细对外暴露多少条」的闸门：定得太低，用户排查高消耗请求时看不到更早的
/// 记录；不设上限，重度账号会把明细撑到几万条，缓存文件与前端渲染一起变慢。
/// 1000 条配合前端分页与按消耗排序，足够覆盖排查场景，同时保持有界。
pub const OFFICIAL_USAGE_DETAIL_LIMIT: usize = 1_000;
/// 连续扫描的轮数上限：每轮最多 [`OFFICIAL_USAGE_PAGE_SIZE`] 条，100 轮足够覆盖
/// 单账号单窗口的任何真实量级，同时兜住「服务端异常回同样一页」的死循环。
const OFFICIAL_USAGE_MAX_ROUNDS: usize = 100;
static OFFICIAL_USAGE_MEMORY: Mutex<Option<Value>> = Mutex::new(None);

fn official_usage_fetch_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn take_object_fields(value: &Value, keys: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(object) = value.as_object() else {
        return out;
    };
    for key in keys {
        if let Some(field) = object.get(*key) {
            out.insert((*key).to_string(), field.clone());
        }
    }
    out
}

fn sanitize_models(value: Option<&Value>) -> Value {
    Value::Array(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                Value::Object(take_object_fields(
                    item,
                    &["model", "requestCount", "credit"],
                ))
            })
            .collect(),
    )
}

fn sanitize_daily(value: Option<&Value>) -> Value {
    Value::Array(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let mut point = take_object_fields(item, &["date", "usage"]);
                point.insert("models".into(), sanitize_models(item.get("models")));
                Value::Object(point)
            })
            .collect(),
    )
}

fn sanitize_requests(value: Option<&Value>) -> Value {
    Value::Array(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                Value::Object(take_object_fields(
                    item,
                    &[
                        "accountId",
                        "accountName",
                        "requestId",
                        "credit",
                        "model",
                        "client",
                        "requestTime",
                    ],
                ))
            })
            .collect(),
    )
}

fn sanitize_accounts(value: Option<&Value>) -> Value {
    Value::Array(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let mut account = take_object_fields(
                    item,
                    &[
                        "accountId",
                        "accountName",
                        "ok",
                        "requestCount",
                        "detailTruncated",
                        "usageToday",
                        "usage7Days",
                        "usageThisMonth",
                        "error",
                        "reportedTotal",
                        "fetchedCount",
                    ],
                );
                account.insert("models".into(), sanitize_models(item.get("models")));
                account.insert("daily".into(), sanitize_daily(item.get("daily")));
                Value::Object(account)
            })
            .collect(),
    )
}

fn sanitize_cached_payload(value: &Value) -> Option<Value> {
    let status = value.get("status")?.as_str()?;
    if !matches!(status, "complete" | "partial" | "unavailable") {
        return None;
    }
    let mut payload = take_object_fields(
        value,
        &[
            "status",
            "rangeStart",
            "rangeEnd",
            "summary",
            "detailLimitPerAccount",
            "collectedAt",
            "errors",
        ],
    );
    payload.insert("daily".into(), sanitize_daily(value.get("daily")));
    payload.insert("models".into(), sanitize_models(value.get("models")));
    payload.insert("accounts".into(), sanitize_accounts(value.get("accounts")));
    payload.insert("requests".into(), sanitize_requests(value.get("requests")));
    Some(Value::Object(payload))
}

fn parse_official_usage_cache(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    let payload = value.get("payload").unwrap_or(&value);
    sanitize_cached_payload(payload)
}

fn load_official_usage_cache_from(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_official_usage_cache(&text)
}

fn save_official_usage_cache_to(path: &Path, payload: &Value) {
    let body = json!({ "payload": payload });
    let content = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(store_dir);
    let _ = std::fs::create_dir_all(parent).and_then(|_| atomic_write(path, &content));
}

fn remembered_official_usage() -> Option<Value> {
    if let Ok(guard) = OFFICIAL_USAGE_MEMORY.lock() {
        if let Some(cached) = guard.as_ref() {
            return Some(cached.clone());
        }
    }
    let loaded = load_official_usage_cache_from(&official_usage_cache_file())?;
    if let Ok(mut guard) = OFFICIAL_USAGE_MEMORY.lock() {
        *guard = Some(loaded.clone());
    }
    Some(loaded)
}

fn remember_official_usage(payload: &Value) {
    if let Ok(mut guard) = OFFICIAL_USAGE_MEMORY.lock() {
        *guard = Some(payload.clone());
    }
    save_official_usage_cache_to(&official_usage_cache_file(), payload);
}

/// 只做官方用量采集（渐进式刷新用）：单飞 + 复用同一份结果。
///
/// 并发调用只有第一个真正发请求：后来者等锁期间若已有 `collectedAt >= 自己进入时刻`
/// 的结果，直接复用（`reused: true`），不会重复打 12 个账号 × N 页。
pub async fn refresh_official_usage(accounts: &[Value]) -> Value {
    let started = now_ms();
    let _guard = official_usage_fetch_lock().lock().await;
    if let Some(cached) = remembered_official_usage() {
        let collected_at = cached
            .get("collectedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if collected_at >= started {
            return json!({
                "officialUsage": cached,
                "elapsedMs": now_ms() - started,
                "reused": true,
            });
        }
    }
    let collected_at = now_ms();
    let usage = collect_official_usage(accounts, collected_at).await;
    remember_official_usage(&usage);
    json!({
        "officialUsage": usage,
        "elapsedMs": now_ms() - started,
        "reused": false,
    })
}

/// 统计页默认读缓存；`refresh = true` 时才重新请求官方用量接口。
pub async fn official_usage_for_statistics(accounts: &[Value], at_ms: i64, refresh: bool) -> Value {
    if !refresh {
        if let Some(cached) = remembered_official_usage() {
            return cached;
        }
    }
    let _guard = official_usage_fetch_lock().lock().await;
    if !refresh {
        if let Some(cached) = remembered_official_usage() {
            return cached;
        }
    }
    let usage = collect_official_usage(accounts, at_ms).await;
    remember_official_usage(&usage);
    usage
}

mod aggregate;
mod fetch;
mod parse;

pub use fetch::collect_official_usage;

#[cfg(test)]
mod tests {
    use super::*;

    use super::aggregate::*;
    use super::parse::*;
    include!("tests/aggregate.rs");
}
