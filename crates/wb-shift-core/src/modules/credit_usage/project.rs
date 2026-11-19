//! 统计投影：build_statistics 与 get_statistics 入口。

use super::collect::*;
use super::*;

pub(super) fn build_statistics(
    snapshot_values: &[Value],
    checkin_values: &[Value],
    accounts: &[Value],
    at_ms: i64,
) -> Value {
    let snapshots = parse_snapshots(snapshot_values, at_ms);
    let today = local_date(at_ms).unwrap_or_else(|| Local::now().date_naive());
    let checkin_cutoff = at_ms.saturating_sub(CHECKIN_LOG_KEEP_DAYS * 24 * 3600 * 1000);
    let checkins: Vec<CheckinEvent> = checkin_values
        .iter()
        .filter_map(parse_checkin_event)
        .filter(|event| event.ts >= checkin_cutoff && event.ts <= at_ms)
        .collect();

    let mut account_ids = Vec::new();
    let mut current_account_ids = Vec::new();
    let mut account_names = HashMap::new();
    // 账号行带档位便于区分（国际版/国内版账号可能同名）。
    let mut account_variants: HashMap<String, WbVariant> = HashMap::new();
    for account in accounts {
        if let Some(id) = non_empty_string(account.get("id")) {
            if !current_account_ids.contains(&id) {
                current_account_ids.push(id.clone());
            }
            add_account_name(
                &mut account_ids,
                &mut account_names,
                &mut account_variants,
                id,
                account_display_name(account),
                variant_of(account),
            );
        }
    }
    for snapshot in &snapshots {
        add_account_name(
            &mut account_ids,
            &mut account_names,
            &mut account_variants,
            snapshot.account_id.clone(),
            snapshot.account_name.clone(),
            snapshot.variant,
        );
    }
    for event in &checkins {
        if let Some(account_id) = &event.account_id {
            let variant = account_variants
                .get(account_id)
                .copied()
                .unwrap_or(WbVariant::Cn);
            add_account_name(
                &mut account_ids,
                &mut account_names,
                &mut account_variants,
                account_id.clone(),
                event.account_name.clone(),
                variant,
            );
        }
    }

    let mut by_account: HashMap<String, Vec<Snapshot>> = HashMap::new();
    for snapshot in snapshots {
        by_account
            .entry(snapshot.account_id.clone())
            .or_default()
            .push(snapshot);
    }
    let coverage_start_at = by_account
        .values()
        .flat_map(|snapshots| snapshots.iter().map(|snapshot| snapshot.ts))
        .min();
    let coverage_end_at = by_account
        .values()
        .flat_map(|snapshots| snapshots.iter().map(|snapshot| snapshot.ts))
        .max();
    // 窗口覆盖判定：本地快照只有从窗口第一天起就存在，窗口内的消耗才是完整的。
    // 只看今天快照时 7 天/本月必须标「未覆盖」，否则页面会把今日值当成 7 天/本月
    // （issue #225 截图里 28.7 / 28.7 / 28.7 的直接成因）。
    let (coverage_days, covers_7_days, covers_this_month) =
        coverage_windows(coverage_start_at.and_then(local_date), today);

    let mut usage_events = Vec::new();
    let mut usage_totals: HashMap<String, (f64, f64, f64)> = HashMap::new();
    let mut daily_usage: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut latest_snapshots = HashMap::new();
    for (account_id, mut snapshots) in by_account {
        snapshots.sort_by_key(|snapshot: &Snapshot| snapshot.ts);
        if let Some(latest) = snapshots.last() {
            latest_snapshots.insert(account_id.clone(), latest.clone());
        }
        for pair in snapshots.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            let amount = previous.remaining - current.remaining;
            if amount <= f64::EPSILON {
                continue;
            }
            let Some(date) = local_date_string(current.ts) else {
                continue;
            };
            let entry = usage_totals.entry(account_id.clone()).or_default();
            let (today_usage, week_usage, month_usage) = usage_in_windows(&date, today);
            if today_usage {
                entry.0 += amount;
            }
            if week_usage {
                entry.1 += amount;
            }
            if month_usage {
                entry.2 += amount;
            }
            *daily_usage
                .entry(account_id.clone())
                .or_default()
                .entry(date.clone())
                .or_default() += amount;
            usage_events.push(UsageEvent {
                ts: current.ts,
                date,
                account_id: account_id.clone(),
                variant: current.variant,
                amount,
            });
        }
    }

    let mut checkin_today_latest: HashMap<String, (i64, String)> = HashMap::new();
    let mut today_success = 0;
    let mut today_already = 0;
    let mut today_failed = 0;
    for event in &checkins {
        if event.date != today.format("%Y-%m-%d").to_string() {
            continue;
        }
        match event.result.as_str() {
            "success" => today_success += 1,
            "already" => today_already += 1,
            _ => today_failed += 1,
        }
        if let Some(identity) = checkin_identity(event) {
            if checkin_today_latest
                .get(&identity)
                .map(|(ts, _)| *ts <= event.ts)
                .unwrap_or(true)
            {
                checkin_today_latest.insert(identity, (event.ts, event.result.clone()));
            }
        }
    }

    let today_checked_in_accounts = checkin_today_latest
        .values()
        .filter(|(_, result)| result == "success" || result == "already")
        .count();
    let today_key = today.format("%Y-%m-%d").to_string();

    let mut current_remaining = 0.0;
    let mut current_capacity = 0.0;
    for account_id in &current_account_ids {
        if let Some(snapshot) = latest_snapshots.get(account_id) {
            current_remaining += snapshot.remaining;
            current_capacity += snapshot.total;
        }
    }

    let mut last_checkins: HashMap<String, CheckinEvent> = HashMap::new();
    for event in &checkins {
        let Some(account_id) = &event.account_id else {
            continue;
        };
        if last_checkins
            .get(account_id)
            .map(|current: &CheckinEvent| current.ts <= event.ts)
            .unwrap_or(true)
        {
            last_checkins.insert(account_id.clone(), event.clone());
        }
    }

    // 逐日序列起点：全局最早快照与保留窗口下界的较大者；无快照时为 None（返回空序列）
    let daily_start = coverage_start_at.and_then(local_date).map(|coverage_date| {
        let earliest = today - ChronoDuration::days(CREDIT_SNAPSHOT_RETENTION_DAYS - 1);
        coverage_date.max(earliest)
    });
    let empty_account_daily: HashMap<String, f64> = HashMap::new();

    let account_summaries: Vec<Value> = account_ids
        .iter()
        .map(|account_id| {
            let latest = latest_snapshots.get(account_id);
            let is_current = current_account_ids.contains(account_id);
            let (usage_today, usage_week, usage_month) = usage_totals
                .get(account_id)
                .copied()
                .unwrap_or_default();
            let today_checkin = checkins
                .iter()
                .filter(|event| {
                    event.account_id.as_deref() == Some(account_id.as_str())
                        && event.date == today_key
                })
                .max_by_key(|event| event.ts);
            let last_checkin = last_checkins.get(account_id);
            json!({
                "accountId": account_id,
                "accountName": account_names.get(account_id).cloned().unwrap_or_else(|| "unknown".to_string()),
                "variant": account_variants
                    .get(account_id)
                    .copied()
                    .unwrap_or(WbVariant::Cn)
                    .as_str(),
                "isCurrent": is_current,
                "currentRemaining": is_current.then(|| latest.map(|snapshot| snapshot.remaining)).flatten(),
                "totalCapacity": is_current.then(|| latest.map(|snapshot| snapshot.total)).flatten(),
                "lastSnapshotAt": latest.map(|snapshot| snapshot.ts),
                "usageToday": usage_today,
                "usage7Days": usage_week,
                "usageThisMonth": usage_month,
                "checkedInToday": today_checkin.map(|event| event.result == "success" || event.result == "already"),
                "checkinStatusToday": today_checkin.map(|event| event.result.clone()),
                "lastCheckinAt": last_checkin.map(|event| event.ts),
                "lastCheckinResult": last_checkin.map(|event| event.result.clone()),
                "daily": local_daily_series(
                    daily_usage.get(account_id).unwrap_or(&empty_account_daily),
                    daily_start,
                    today,
                ),
            })
        })
        .collect();

    let mut aggregate_daily: HashMap<String, f64> = HashMap::new();
    for account_daily in daily_usage.values() {
        for (date, amount) in account_daily {
            *aggregate_daily.entry(date.clone()).or_insert(0.0) += amount;
        }
    }
    let daily = local_daily_series(&aggregate_daily, daily_start, today);

    let mut events: Vec<(i64, Value)> = usage_events
        .iter()
        .map(|event| {
            (
                event.ts,
                json!({
                    "kind": "usage",
                    "ts": event.ts,
                    "date": event.date,
                    "accountId": event.account_id,
                    "accountName": account_names.get(&event.account_id).cloned().unwrap_or_else(|| "unknown".to_string()),
                    "variant": event.variant.as_str(),
                    "amount": event.amount,
                }),
            )
        })
        .collect();
    events.extend(checkins.iter().map(|event| {
        (
            event.ts,
            json!({
                "kind": "checkin",
                "ts": event.ts,
                "date": event.date,
                "accountId": event.account_id,
                "accountName": event.account_id.as_ref().and_then(|id| account_names.get(id)).cloned().unwrap_or_else(|| event.account_name.clone()),
                "variant": event
                    .account_id
                    .as_ref()
                    .and_then(|id| account_variants.get(id))
                    .copied()
                    .unwrap_or(WbVariant::Cn)
                    .as_str(),
                "result": event.result,
                "error": event.error,
            }),
        )
    }));
    events.sort_by_key(|event| Reverse(event.0));
    let recent_events: Vec<Value> = events
        .into_iter()
        .take(CREDIT_STATS_MAX_EVENTS)
        .map(|(_, event)| event)
        .collect();

    json!({
        "generatedAt": at_ms,
        "retentionDays": CREDIT_SNAPSHOT_RETENTION_DAYS,
        "coverageStartAt": coverage_start_at,
        "coverageEndAt": coverage_end_at,
        // 本地观察口径的覆盖面：前端据此决定哪些窗口可以显示数字、哪些必须是「—」。
        "coverage": {
            "startAt": coverage_start_at,
            "endAt": coverage_end_at,
            "days": coverage_days,
            "covers7Days": covers_7_days,
            "coversThisMonth": covers_this_month,
        },
        "summary": {
            "currentRemaining": current_remaining,
            "currentCapacity": current_capacity,
            "usageToday": usage_totals.values().map(|value| value.0).sum::<f64>(),
            "usage7Days": usage_totals.values().map(|value| value.1).sum::<f64>(),
            "usageThisMonth": usage_totals.values().map(|value| value.2).sum::<f64>(),
            "todayCheckedInAccounts": today_checked_in_accounts,
            "todaySuccess": today_success,
            "todayAlready": today_already,
            "todayFailed": today_failed,
        },
        "daily": daily,
        "accounts": account_summaries,
        "events": recent_events,
    })
}

/// 返回本地快照、账号列表、签到日志与官方请求用量的统一统计投影。
///
/// `refresh = false` 时官方用量读本地缓存，不打用量接口；
/// 只有统计页「刷新统计」传入 `refresh = true` 才会重新采集。
///
/// `range`（第四轮 07）：`30d`（默认）/ `90d` / `all`。
/// - 30d / 90d：走官方窗口抓取；90d 在服务端不支持时降级回 30d 并置 `rangeFallback`；
/// - all：**不打官方接口**，用本地账本（`credit_ledger`）做全量口径，断网也能看。
pub async fn get_statistics(refresh: bool, range: Option<&str>) -> Value {
    let started = now_ms();
    let at_ms = started;
    let accounts = load_accounts();
    let mut statistics =
        build_statistics(&load_snapshots(), &load_checkin_logs(), &accounts, at_ms);

    let requested = match range.map(str::trim) {
        Some("90d") => "90d",
        Some("all") => "all",
        _ => "30d",
    };
    // 探测结论有 24 小时缓存；refresh 时才会真的打一次 90 天窗口。
    let probe = official_usage::window_probe(&accounts, refresh).await;
    let max_days = probe
        .get("maxDays")
        .and_then(Value::as_i64)
        .unwrap_or(30)
        .max(30);
    let effective = if requested == "90d" && max_days < 90 {
        "30d"
    } else {
        requested
    };
    let official = if effective == "all" {
        // 「全部」本身不依赖官方窗口；但用户点刷新时仍抓一次 30 天窗口来给账本续数据。
        if refresh {
            let _ = official_usage::official_usage_for_statistics_with_window(
                &accounts, at_ms, true, 30,
            )
            .await;
        }
        official_usage::cached_official_usage()
    } else {
        official_usage::official_usage_for_statistics_with_window(
            &accounts,
            at_ms,
            refresh,
            if effective == "90d" { 90 } else { 30 },
        )
        .await
    };
    statistics["officialUsage"] = official;
    statistics["range"] = serde_json::json!(requested);
    statistics["effectiveRange"] = serde_json::json!(effective);
    statistics["rangeFallback"] = serde_json::json!(requested != effective);
    statistics["windowProbe"] = probe;
    // 账本永远返回快照：「全部」用它，30/90 天只作为对照与导出说明。
    let ledger = crate::modules::credit_ledger::snapshot(at_ms);
    statistics["ledgerStatus"] = ledger
        .get("status")
        .cloned()
        .unwrap_or_else(|| serde_json::json!("degraded"));
    statistics["ledgerRows"] = ledger
        .get("rows")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(0));
    statistics["ledgerStartAt"] = ledger.get("startAt").cloned().unwrap_or(Value::Null);
    statistics["ledgerEndAt"] = ledger.get("endAt").cloned().unwrap_or(Value::Null);
    statistics["ledger"] = ledger;
    // 埋点：接口总耗时，前端用来展示「本地 / 积分 / 官方」三段真实用时。
    statistics["elapsedMs"] = serde_json::json!(now_ms() - started);
    statistics
}
