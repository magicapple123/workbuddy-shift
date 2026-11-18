//! 派猫猫旅行：状态机、接口封装、每日缓存与自动派发/领取奖励。
//!
//! 对照 WorkDaddy `daemon.js` + `growth-travel.js` 的派猫猫旅行实现。
//! 状态机以服务端 `data.state` 为准：idle ->(depart)-> traveling ->(到点)-> arrived ->(claim)-> idle。
//! 官网 `idle` + `daily_limit_reached` 对应「累了，明天再来吧」，展示为已结束，不能再按 traveling 倒计时。

use chrono::Local;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::modules::account::{
    self, account_display_name, build_auth_headers, load_accounts, variant_of,
};
use crate::modules::config::{
    http_request, load_checkin_config, load_travel_cache, load_travel_config, now_ms, now_secs,
    save_travel_cache, with_travel_cache_lock, RunFlagGuard, TRAVEL_API_PREFIX,
    WORKBUDDY_API_ENDPOINT,
};
use crate::modules::refresh::{ensure_fresh_token, refresh_account_token};

static TRAVEL_RUNNING: AtomicBool = AtomicBool::new(false);
static TRAVEL_CLAIM_RUNNING: AtomicBool = AtomicBool::new(false);

/// 派发周期：启动即派发，之后每 30 分钟补一轮（并重试 no-buddy / 瞬时错误）。
pub const TRAVEL_RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// 领取奖励检查周期：启动立刻查一轮，之后每隔 15 分钟检查并领取。
pub const TRAVEL_CLAIM_INTERVAL: Duration = Duration::from_secs(15 * 60);

const KNOWN_STATES: [&str; 3] = ["idle", "traveling", "arrived"];

/// 判定为「可重试」的跳过原因：这些情况下当日不算完成，后续轮次继续重试。
///
/// 针对参考项目 Bug：账号初始无 Buddy 时缓存了 no-buddy 且标记 completed，
/// 之后账号有了 Buddy 也不会再重试，导致一直显示「无 Buddy」。
fn is_retryable_skip(skip: Option<&str>) -> bool {
    matches!(
        skip,
        Some("no-buddy")
            | Some("error")
            | Some("config-error")
            | Some("no-location")
            | Some("location-unavailable")
            | Some("status-error")
    )
}

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn account_key(account: &Value) -> String {
    account
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(String::from)
        .unwrap_or_else(|| account_display_name(account))
}

mod config;
mod cycle;
mod status;

pub use config::unsupported_variant_skip;
pub use cycle::{depart_travel_for_account, run_travel_claim_cycle, run_travel_cycle};
pub use status::{reconcile_due_travel, travel_display};

#[cfg(test)]
mod tests {
    use super::*;

    use super::config::*;
    use super::cycle::*;
    use super::status::*;
    include!("tests/cycle.rs");
}
