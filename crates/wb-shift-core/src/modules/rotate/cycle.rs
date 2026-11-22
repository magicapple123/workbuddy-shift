//! 轮换周期：run_rotate_cycle 主流程。

use super::decision::*;
use super::*;

/// 一次轮换周期（后台定时任务 / 手动触发共用）。
pub async fn run_rotate_cycle() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&ROTATE_RUNNING) else {
        return json!({"status": "skipped", "reason": "already_running"});
    };

    let cfg = load_auto_rotate_config();
    if cfg.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
        return json!({"status": "disabled"});
    }

    let cooldown_minutes = cfg
        .get("cooldown_minutes")
        .and_then(|v| v.as_i64())
        .unwrap_or(120)
        .max(1);
    let min_gap_hours = cfg
        .get("min_gap_hours")
        .and_then(|v| v.as_i64())
        .unwrap_or(24)
        .max(0);
    let min_urgency_hours = cfg
        .get("min_urgency_hours")
        .and_then(|v| v.as_i64())
        .unwrap_or(72)
        .max(0);
    let min_remaining_credits = cfg
        .get("min_remaining_credits")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .max(0.0);

    // 同一套 CLI：国内/国际账号一起参与轮换，切号时带上对应区域环境。
    let accounts = account::load_accounts();
    let mut candidates: Vec<Candidate> = Vec::with_capacity(accounts.len());
    for acc in &accounts {
        let credit = credits::get_credit_expiry(acc).await;
        candidates.push(to_candidate(acc, &credit));
    }

    // 当前 CLI 账号
    let cli_status = codebuddy_cli::status();
    let current_id = cli_status
        .get("activeAccountId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let now = now_ms();
    let last_switch = LAST_SWITCH_AT.load(Ordering::SeqCst);
    let last_switch_opt = (last_switch > 0).then_some(last_switch);
    // 存活门控：读 `~/.codebuddy/sessions/*.json` 的心跳（单次检查只读这里与 state.json，
    // 不再遍历 `~/.codebuddy/projects` 下的 transcript）。
    let has_live_session = codebuddy_cli::has_live_session(now);
    // 同一决策链跑两次：真实门控值 + 强制放开（只有"除门控外本会切换"时才提示）。
    let decide = |has_live_session: bool| {
        decide_target(
            &candidates,
            current_id.as_deref(),
            last_switch_opt,
            cooldown_minutes * 60_000,
            min_gap_hours * 3_600_000,
            min_urgency_hours * 3_600_000,
            has_live_session,
            min_remaining_credits,
        )
    };
    let decision = decide(has_live_session);

    LAST_CHECK_AT.store(now, Ordering::SeqCst);

    let mut log = json!({
        "ts": now,
        "action": "noop",
        "reason": Value::Null,
        "from": Value::Null,
        "to": Value::Null,
    });
    // 每次检查记录各账号积分快照（供观察后调整 min_remaining_credits）
    log["detail"] = json!(candidates
        .iter()
        .map(|c| json!({
            "name": c.display_name,
            "remaining": c.total_remaining,
            "soonestExpireAt": c.soonest_expire_at,
            "valid": c.valid,
        }))
        .collect::<Vec<_>>());
    if let Some(id) = &current_id {
        log["from"] = json!({"id": id, "name": cli_status.get("activeAccountName").cloned().unwrap_or_default()});
    }

    let mut notify: Option<Value> = None;
    let result = match decision {
        Decision::Skip(reason) => {
            log["action"] = json!("skipped");
            let mut reason = reason;
            // 门控跳过时再问一句：把门控强制放开会不会切？会 → 说明只是被会话拦住，
            // 值得提示一次（否则用户永远不知道积分快过期了）。
            let deferred_target = deferred_prompt_target(&reason, || decide(false));
            if let Some(target_id) = deferred_target {
                if try_consume_rotate_notify(now) {
                    let target = candidates.iter().find(|c| c.account_id == target_id);
                    let name = target
                        .map(|c| c.display_name.clone())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| target_id.clone());
                    let days = target
                        .and_then(|c| c.soonest_expire_at)
                        .map(|expire_at| remaining_days_until(expire_at, now));
                    notify = Some(rotate_deferred_notify(&name, days));
                    reason = format!("{reason}（已提示）");
                } else {
                    reason = format!("{reason}（已达今日提示上限）");
                }
            }
            log["reason"] = json!(reason);
            let mut payload = json!({"status": "skipped", "reason": reason});
            if let Some(notify) = notify {
                payload["notify"] = notify;
            }
            payload
        }
        Decision::Switch(target_id) => {
            let target_name = candidates
                .iter()
                .find(|c| c.account_id == target_id)
                .map(|c| c.display_name.clone())
                .unwrap_or_default();
            let target_variant = accounts
                .iter()
                .find(|account| {
                    account.get("id").and_then(Value::as_str) == Some(target_id.as_str())
                })
                .map(WbVariant::from_account)
                .unwrap_or(WbVariant::Cn);
            log["to"] = json!({
                "id": target_id,
                "name": target_name,
                "variant": target_variant.as_str(),
            });
            match codebuddy_cli::switch_active_account(&target_id) {
                Ok(res) => {
                    LAST_SWITCH_AT.store(now, Ordering::SeqCst);
                    // 「最近切换」时间戳（issue #184）：CLI 已切到目标账号后才记录。
                    // 失败只影响卡片徽标与排序，不得把已成功的轮换改成 error。
                    let _ = account::touch_last_used(&target_id, "rotate");
                    log["action"] = json!("switched");
                    json!({"status": "switched", "to": target_id, "detail": res})
                }
                Err(e) => {
                    log["action"] = json!("error");
                    log["reason"] = json!(e);
                    json!({"status": "error", "error": e})
                }
            }
        }
    };
    add_rotate_log(&log);
    result
}
