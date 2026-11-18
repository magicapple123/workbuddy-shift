//! CodeBuddy CLI 账号自动轮换（防积分过期浪费）。
//!
//! 后台周期检查所有账号的积分到期情况，把 CodeBuddy CLI 切到"最紧迫"的账号
//! （最早到期且仍有剩余积分），防止积分过期浪费。国内/国际是同一套 CLI，
//! 候选不按档位过滤；切号时由 `switch_active_account` 同步区域环境变量。
//! 只写 `~/.codebuddy-rotate/state.json`，不影响 WorkBuddy App。
//!
//! 防抖动约束（核心）：
//! - 冷却期：切换后 cooldown_minutes 内不重复切；
//! - 到期差异阈值：目标比当前账号早到期超过 min_gap_hours 才切，
//!   避免"三个账号都是明天到期"时来回切换。
//!
//! 存活门控与推迟提示（本轮起）：
//! - 只要存在**心跳新鲜**的 CLI 会话（`~/.codebuddy/sessions/*.json`）就跳过本次轮换：
//!   活进程持的是旧 key，切了也不生效，还会破坏「活进程 key == 当前账号」的不变式；
//!   旧版的「活跃保护」（按 transcript mtime 的 30 分钟窗口）已被它取代——那个判据
//!   管不住"开着但闲置"的会话，还会全量扫 `~/.codebuddy/projects`。
//! - 被门控拦下、但"除门控外本来会切换"时提示一次（受每日预算约束，见 `config`）。

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use crate::modules::account;
use crate::modules::codebuddy_cli;
use crate::modules::config::{
    add_rotate_log, load_auto_rotate_config, load_rotate_logs, now_ms, try_consume_rotate_notify,
    RunFlagGuard,
};
use crate::modules::credits;
use crate::modules::variant::WbVariant;

static ROTATE_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_CHECK_AT: AtomicI64 = AtomicI64::new(0);
static LAST_SWITCH_AT: AtomicI64 = AtomicI64::new(0);

/// 一天的毫秒数（新代码里用它代替裸写的 `24 * 3_600_000`）。
const DAY_MS: i64 = 24 * 3_600_000;

/// 存活门控跳过时的固定原因：轮换日志与"是否该提示"的判定都认这一条。
const SKIP_LIVE_SESSION_REASON: &str = "有 CodeBuddy CLI 会话在运行，暂不切换";

/// 推迟提示的通知标题（返回体里的 `notify.title` 与宿主投递用的是同一个值）。
const ROTATE_NOTIFY_TITLE: &str = "workbuddy-shift";

/// 单个账号的积分候选（从 get_credit_expiry 提取，不携带 token）。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub account_id: String,
    pub display_name: String,
    /// 剩余积分中最早到期时间（毫秒）；无剩余积分资源时为 None。
    pub soonest_expire_at: Option<i64>,
    pub total_remaining: f64,
    /// 查询成功、未过期、有剩余积分 → 可被选为目标。
    pub valid: bool,
    pub error: Option<String>,
}

impl Candidate {
    /// 紧迫度排序键：到期越早越紧迫；无到期时间排最后。
    fn urgency_key(&self) -> (i64, i64) {
        match self.soonest_expire_at {
            Some(ts) => (0, ts),
            None => (1, 0),
        }
    }
}

mod cycle;
mod decision;
mod logs;

pub use cycle::run_rotate_cycle;
pub use decision::{decide_target, Decision};
pub use logs::{rotate_logs, rotate_status};

#[cfg(test)]
mod tests {
    use super::*;

    use super::decision::*;
    include!("tests/cycle.rs");
}
