//! 采集事件：本地日期序列、签到/快照事件解析与账号名补充。

use super::*;

pub(super) fn local_date(ts: i64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|date| date.date_naive())
}

pub(super) fn local_date_string(ts: i64) -> Option<String> {
    local_date(ts).map(|date| date.format("%Y-%m-%d").to_string())
}

/// 生成逐日观察序列（daily_start..=today，无数据的天补 0）；无快照起点时返回空数组。
pub(super) fn local_daily_series(
    daily: &HashMap<String, f64>,
    daily_start: Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<Value> {
    let Some(start) = daily_start else {
        return Vec::new();
    };
    let mut points = Vec::new();
    let mut date = start;
    while date <= today {
        let key = date.format("%Y-%m-%d").to_string();
        points.push(json!({
            "date": key,
            "usage": daily.get(&key).copied().unwrap_or(0.0),
        }));
        date += ChronoDuration::days(1);
    }
    points
}

pub(super) fn parse_checkin_event(value: &Value) -> Option<CheckinEvent> {
    let ts = value
        .get("ts")
        .and_then(Value::as_i64)
        .or_else(|| crate::modules::config::norm_ts(value.get("ts")))?;
    Some(CheckinEvent {
        ts,
        date: local_date_string(ts)?,
        account_id: non_empty_string(value.get("accountId")),
        account_name: non_empty_string(value.get("email")).unwrap_or_else(|| "unknown".to_string()),
        result: non_empty_string(value.get("result")).unwrap_or_else(|| "error".to_string()),
        error: non_empty_string(value.get("error")),
    })
}

pub(super) fn parse_snapshots(values: &[Value], at_ms: i64) -> Vec<Snapshot> {
    let cutoff = at_ms.saturating_sub(CREDIT_SNAPSHOT_RETENTION_DAYS * 24 * 3600 * 1000);
    let mut snapshots: Vec<Snapshot> = values
        .iter()
        .filter_map(snapshot_from_value)
        .filter(|snapshot| snapshot.ts >= cutoff && snapshot.ts <= at_ms)
        .collect();
    snapshots.sort_by_key(|snapshot| snapshot.ts);
    snapshots
}

pub(super) fn usage_in_windows(date: &str, today: NaiveDate) -> (bool, bool, bool) {
    let Some(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok() else {
        return (false, false, false);
    };
    let distance = (today - date).num_days();
    (
        distance == 0,
        (0..7).contains(&distance),
        date.year() == today.year() && date.month() == today.month(),
    )
}

pub(super) fn add_account_name(
    account_ids: &mut Vec<String>,
    account_names: &mut HashMap<String, String>,
    account_variants: &mut HashMap<String, WbVariant>,
    account_id: String,
    account_name: String,
    variant: WbVariant,
) {
    if !account_ids.contains(&account_id) {
        account_ids.push(account_id.clone());
    }
    let should_replace = account_names
        .get(&account_id)
        .map(|name| name == "unknown" && account_name != "unknown")
        .unwrap_or(true);
    if should_replace {
        account_names.insert(account_id.clone(), account_name);
    }
    // 档位一旦确定为国际版就不再被旧快照的默认值覆盖。
    account_variants
        .entry(account_id)
        .and_modify(|existing| {
            if *existing == WbVariant::Cn && variant == WbVariant::Ai {
                *existing = variant;
            }
        })
        .or_insert(variant);
}

pub(super) fn checkin_identity(event: &CheckinEvent) -> Option<String> {
    if let Some(account_id) = event.account_id.as_ref() {
        return Some(format!("account:{account_id}"));
    }
    (event.account_name != "unknown").then(|| format!("legacy:{}", event.account_name))
}
