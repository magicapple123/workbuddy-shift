//! 轮换决策：候选构建、目标决策与延迟/提示。

use super::*;

/// 从积分查询结果提取候选（失败时生成 invalid 候选，供日志/防抖动参照）。
pub(super) fn to_candidate(account: &Value, credit: &Value) -> Candidate {
    let account_id = account
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let display_name = account_display_name_or(account);
    let ok = credit.get("ok").and_then(|v| v.as_bool()) == Some(true);
    let expired = credit.get("expired").and_then(|v| v.as_bool()) == Some(true);
    let total_remaining = credit
        .get("totalRemaining")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let soonest_expire_at = credit.get("soonestExpireAt").and_then(|v| v.as_i64());
    let error = credit
        .get("error")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Candidate {
        valid: ok && !expired && total_remaining > 0.0,
        account_id,
        display_name,
        soonest_expire_at,
        total_remaining,
        error,
    }
}

pub(super) fn account_display_name_or(account: &Value) -> String {
    account::account_display_name(account)
}

/// 测试辅助：按档位切分账号。生产轮换不再过滤，因为同一套 CLI 跨档位共用。
#[cfg(test)]
pub(super) fn candidates_for_variant(accounts: &[Value], variant: WbVariant) -> Vec<Value> {
    accounts
        .iter()
        .filter(|account| WbVariant::from_account(account) == variant)
        .cloned()
        .collect()
}

/// 决策结果。
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// 不切，附原因。
    Skip(String),
    /// 切到目标账号 id。
    Switch(String),
}

/// 纯策略函数：在候选里选目标并判断是否切换（可单测）。
///
/// - `current_account_id`：当前 CLI 账号（可能不在候选中）。
/// - `last_switch_at_ms`：上次切换时间，None 视为从未切换。
/// - `min_urgency_ms`：紧迫度阈值——目标到期剩余超过该值则不切（都还早，无需切）。
/// - `has_live_session`：是否存在心跳新鲜的 CLI 会话；true 则本次不切（存活门控）。
/// - `min_remaining`：目标最小剩余积分，低于则不切（默认 0 关闭）。
///
/// 参数都是纯策略输入，调用方（轮换周期与用例）需要逐项显式传入，故刻意放行
/// `too_many_arguments`；继续往这个签名里加参数前，先考虑是否该引入参数结构体。
#[allow(clippy::too_many_arguments)]
pub fn decide_target(
    candidates: &[Candidate],
    current_account_id: Option<&str>,
    last_switch_at_ms: Option<i64>,
    cooldown_ms: i64,
    min_gap_ms: i64,
    min_urgency_ms: i64,
    has_live_session: bool,
    min_remaining: f64,
) -> Decision {
    // 1) 有效候选（可被选为目标）
    let mut valid: Vec<&Candidate> = candidates.iter().filter(|c| c.valid).collect();
    if valid.is_empty() {
        return Decision::Skip("没有可用账号（查询失败/已过期/无剩余积分）".to_string());
    }
    // 2) 目标 = 紧迫度最高（到期最早）
    valid.sort_by_key(|c| c.urgency_key());
    let target = valid[0];
    // 3) 紧迫度检查：目标到期还早（> 阈值）→ 无需切换
    if let Some(target_ts) = target.soonest_expire_at {
        let remaining_ms = target_ts - now_ms();
        if remaining_ms > min_urgency_ms {
            return Decision::Skip(format!(
                "所有账号到期都还早（最紧迫的还剩 {} 天），无需切换",
                remaining_ms / (24 * 3_600_000)
            ));
        }
    }
    // 4) 已是目标 → 不切
    if current_account_id == Some(target.account_id.as_str()) {
        return Decision::Skip("当前账号已是最紧迫账号".to_string());
    }
    // 5) 冷却期
    if let Some(ts) = last_switch_at_ms {
        if ts + cooldown_ms > now_ms() {
            return Decision::Skip("处于切换冷却期".to_string());
        }
    }
    // 6) 存活门控：有 CLI 会话在跑 → 不切（活进程持旧 key，切了不生效还会破坏不变式）
    if has_live_session {
        return Decision::Skip(SKIP_LIVE_SESSION_REASON.to_string());
    }
    // 7) 价值过滤：目标剩余积分太少，切过去不值得
    if min_remaining > 0.0 && target.total_remaining < min_remaining {
        return Decision::Skip(format!(
            "目标账号剩余积分不足（{}，阈值 {}），不值得切换",
            target.total_remaining, min_remaining
        ));
    }
    // 8) 防抖动：目标比当前早到期，但差异 < 阈值 → 不切
    if let Some(current) = candidates
        .iter()
        .find(|c| c.account_id == current_account_id.unwrap_or(""))
    {
        if let (Some(cur_ts), Some(target_ts)) =
            (current.soonest_expire_at, target.soonest_expire_at)
        {
            if cur_ts > 0 && target_ts < cur_ts && cur_ts - target_ts < min_gap_ms {
                return Decision::Skip(format!(
                    "目标到期仅早 {} 小时，未达切换阈值（{} 小时）",
                    (cur_ts - target_ts) / 3_600_000,
                    min_gap_ms / 3_600_000
                ));
            }
        }
    }
    Decision::Switch(target.account_id.clone())
}

/// 目标账号剩余到期天数（向上取整，至少 1）：提示文案里的「N 天后到期」。
pub(super) fn remaining_days_until(expire_at: i64, now: i64) -> i64 {
    let remaining = (expire_at - now).max(0);
    ((remaining + DAY_MS - 1) / DAY_MS).max(1)
}

/// 轮换推迟提示的通知内容（**唯一**构造点：标题与正文都在这里，措辞改动只应发生在此处）。
///
/// 口径说明：会话可能是"开着但闲置"，所以只说"检测到有会话在运行"，
/// 不能写"CLI 正在使用中"。
pub(super) fn rotate_deferred_notify(target_name: &str, remaining_days: Option<i64>) -> Value {
    let expiry = match remaining_days {
        Some(days) => format!("的积分 {days} 天后到期"),
        None => "的积分即将到期".to_string(),
    };
    json!({
        "title": ROTATE_NOTIFY_TITLE,
        "body": format!(
            "「{target_name}」{expiry}，但检测到有 CodeBuddy CLI 会话在运行；重启 CLI 后新账号才会生效。"
        ),
    })
}

/// 「要提示谁」的判定：只有**真实原因就是存活门控**、并且把门控强制放开后决策链给出
/// `Switch` 时，才返回该目标账号；任何其它 Skip（紧迫度/已是目标/冷却/价值/防抖动）
/// 都说明"不是仅被会话拦下"，不值得打扰用户。
///
/// `decide_without_gate` 只有在前半段成立时才会被调用（重跑决策链不是免费操作）。
pub(super) fn deferred_prompt_target(
    real_reason: &str,
    decide_without_gate: impl FnOnce() -> Decision,
) -> Option<String> {
    if real_reason != SKIP_LIVE_SESSION_REASON {
        return None;
    }
    match decide_without_gate() {
        Decision::Switch(target_id) => Some(target_id),
        Decision::Skip(_) => None,
    }
}
