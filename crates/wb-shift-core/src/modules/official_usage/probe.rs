//! 官方用量窗口探测（第四轮 07）：服务端到底接受多少天。
//!
//! 代码里一直写死「近 30 天」，但协议上只带 `startTime` / `endTime`，没有声明上限。
//! 直接改成 365 天有风险（可能被限频或静默截断）。这里用**单账号、一次**探测：
//!
//! - 请求 90 天窗口；
//! - 看返回的最早日期：仍落在最近 31 天内 → 服务端把窗口截断了，上限按 30 天；
//! - 早于 31 天前 → 接受 90 天；
//! - 请求失败或窗口内没有记录 → 保守按 30 天处理（「近 90 天」置灰而不是假装可用）。
//!
//! 结果缓存 24 小时到 `~/.workbuddy-shift/official_usage_probe.json`，避免每次刷新
//! 统计都多打一次接口。

use super::fetch::fetch_account_usage;
use super::parse::local_date_at;
use crate::modules::config::{atomic_write, now_ms, store_dir};
use chrono::Duration;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const PROBE_FILE: &str = "official_usage_probe.json";

/// 探测结果有效期：一天。窗口上限不会频繁变化，没必要每次刷新都探。
const PROBE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

fn probe_path() -> PathBuf {
    store_dir().join(PROBE_FILE)
}

fn read_cached(path: &Path, now: i64) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let probed_at = value.get("probedAt").and_then(Value::as_i64)?;
    let max_days = value.get("maxDays").and_then(Value::as_i64)?;
    if now - probed_at > PROBE_TTL_MS {
        return None;
    }
    Some(json!({
        "maxDays": max_days,
        "probedAt": probed_at,
        "reason": value.get("reason").cloned().unwrap_or(Value::Null),
        "cached": true,
    }))
}

/// 读当前探测结论（不联网）。没探过 / 过期时返回 `maxDays=null`，前端据此置灰。
pub fn cached_window_probe() -> Value {
    let now = now_ms();
    match read_cached(&probe_path(), now) {
        Some(value) => value,
        None => json!({
            "maxDays": Value::Null,
            "probedAt": Value::Null,
            "reason": "尚未探测（刷新统计时会自动探测一次）",
            "cached": false,
        }),
    }
}

/// 探测官方接口可接受的最大窗口；`refresh=false` 时优先用缓存。
pub async fn window_probe(accounts: &[Value], refresh: bool) -> Value {
    let now = now_ms();
    if !refresh {
        if let Some(value) = read_cached(&probe_path(), now) {
            return value;
        }
    }
    let Some(account) = accounts.first() else {
        return json!({
            "maxDays": 30,
            "probedAt": now,
            "reason": "没有可探测的账号",
            "cached": false,
        });
    };
    let today = local_date_at(now);
    let start = today - Duration::days(89);
    let result = match fetch_account_usage(account, start, today).await {
        Ok(fetch) if !fetch.rows.is_empty() => {
            let oldest = fetch.rows.iter().map(|row| row.date).min();
            let clamped = oldest.is_some_and(|date| date > today - Duration::days(31));
            if clamped {
                json!({
                    "maxDays": 30,
                    "probedAt": now,
                    "reason": "请求 90 天窗口时只返回最近 31 天记录，服务端按 30 天截断",
                    "cached": false,
                })
            } else {
                json!({
                    "maxDays": 90,
                    "probedAt": now,
                    "reason": "90 天窗口返回了更早的记录，可按官方口径统计",
                    "cached": false,
                })
            }
        }
        Ok(_) => json!({
            "maxDays": 30,
            "probedAt": now,
            "reason": "90 天窗口内没有记录，无法证伪截断，保守按 30 天",
            "cached": false,
        }),
        Err(error) => json!({
            "maxDays": 30,
            "probedAt": now,
            "reason": format!("90 天窗口探测失败：{error}"),
            "cached": false,
        }),
    };
    let body = json!({
        "maxDays": result["maxDays"],
        "probedAt": now,
        "reason": result["reason"],
    });
    let _ = std::fs::create_dir_all(store_dir())
        .and_then(|_| atomic_write(&probe_path(), &serde_json::to_string(&body).unwrap_or_default()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_expires_after_a_day() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("wb-shift-probe-{unique}.json"));
        std::fs::write(
            &path,
            json!({ "maxDays": 90, "probedAt": 1_000_000, "reason": "ok" }).to_string(),
        )
        .unwrap();
        assert!(read_cached(&path, 1_000_000 + PROBE_TTL_MS - 1).is_some());
        assert!(read_cached(&path, 1_000_000 + PROBE_TTL_MS + 1).is_none());
        std::fs::remove_file(&path).ok();
    }
}
