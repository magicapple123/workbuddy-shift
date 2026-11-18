//! 状态投影：缓存滚动/合并、状态记录、展示与到期对账。

use super::config::*;
use super::cycle::*;
use super::*;

pub(super) fn parse_travel_state(raw: Option<&str>) -> Option<&'static str> {
    let normalized = raw.map(|s| s.trim().to_ascii_lowercase())?;
    KNOWN_STATES
        .iter()
        .copied()
        .find(|state| *state == normalized)
}

pub(super) fn result_claimed(entry: &Value) -> bool {
    entry.get("claimed").and_then(Value::as_bool) == Some(true)
}

pub(super) fn result_in_flight(entry: &Value) -> bool {
    entry.get("ok").and_then(Value::as_bool) == Some(true) && !result_claimed(entry)
}

pub(super) fn cache_results(cache: &Value) -> Option<&Map<String, Value>> {
    cache.get("results").and_then(Value::as_object)
}

/// 跨日时丢掉已结束/失败记录，但保留仍在 traveling/arrived 的未领奖励。
pub(super) fn roll_cache_to_today(cache: &mut Value, today: &str) {
    if cache.get("date").and_then(Value::as_str) == Some(today) {
        return;
    }
    let kept = cache_results(cache)
        .map(|results| {
            results
                .iter()
                .filter(|(_, entry)| result_in_flight(entry))
                .map(|(id, entry)| (id.clone(), entry.clone()))
                .collect::<Map<String, Value>>()
        })
        .unwrap_or_default();
    cache["date"] = json!(today);
    cache["completed"] = json!(false);
    cache["results"] = Value::Object(kept);
}

pub(super) fn has_retryable_results(results: &Map<String, Value>) -> bool {
    results
        .values()
        .any(|entry| is_retryable_skip(entry.get("skip").and_then(Value::as_str)))
}

pub(super) fn truncate_message(msg: &str) -> String {
    msg.chars().take(80).collect()
}

pub(super) fn nonempty_str(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}

pub(super) fn nonzero_credit(value: &Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::Number(n) if n.as_i64() == Some(0) || n.as_f64() == Some(0.0) => None,
        other if !other.is_null() => Some(other.clone()),
        _ => None,
    }
}

pub(super) fn apply_status_record(result: &mut Value, status: &Value) {
    if let Some(name) = nonempty_str(status.get("locationName").unwrap_or(&Value::Null)) {
        result["locationName"] = json!(name);
    }
    if let Some(credit) = nonzero_credit(status.get("rewardCredit").unwrap_or(&Value::Null)) {
        result["rewardCredit"] = credit;
    }
    let arrive_at = status.get("arriveAt").and_then(Value::as_i64).unwrap_or(0);
    if arrive_at > 0 {
        result["arriveAt"] = json!(arrive_at);
    }
}

pub(super) fn arrive_at_secs(arrive_at: i64) -> i64 {
    if arrive_at > 1_000_000_000_000 {
        arrive_at / 1000
    } else {
        arrive_at
    }
}

/// 旅行中且已过到达时间（或缓存没有到达时间）时，需要再问一次官方 status。
pub(super) fn in_flight_due(entry: &Value, now: i64) -> bool {
    if !result_in_flight(entry) {
        return false;
    }
    let arrive_at = entry.get("arriveAt").and_then(Value::as_i64).unwrap_or(0);
    let arrive_secs = arrive_at_secs(arrive_at);
    arrive_secs <= 0 || arrive_secs <= now
}

/// 官网 idle + daily_limit_reached：当日已完成，保留地点/积分只改完结标记。
pub(super) fn apply_daily_limit_reached(result: &mut Value, status: &Value) {
    result["ok"] = json!(true);
    result["already"] = json!(true);
    result["skip"] = json!("daily-limit");
    result["claimed"] = json!(true);
    result["state"] = json!("idle");
    result["message"] = json!("今日已派");
    if result.get("claimedAt").and_then(Value::as_i64).unwrap_or(0) == 0 {
        result["claimedAt"] = json!(now_ms());
    }
    apply_status_record(result, status);
}

/// 合并领取状态：旧记录已领取则保留；积分按非空优先。
/// 官方状态显示新的 traveling/arrived 时，视为新行程，不把上一趟的 claimed 盖回去。
pub(super) fn merge_claim_state(prior: &Value, new: &Value) -> Value {
    let mut merged = new.clone();
    if result_in_flight(&merged) {
        if nonempty_str(merged.get("locationName").unwrap_or(&Value::Null)).is_none() {
            if let Some(name) = nonempty_str(prior.get("locationName").unwrap_or(&Value::Null)) {
                merged["locationName"] = json!(name);
            }
        }
        // 上一趟已领取的积分不能带到新行程上当「预计奖励」。
        if result_in_flight(prior)
            && nonzero_credit(merged.get("rewardCredit").unwrap_or(&Value::Null)).is_none()
        {
            if let Some(credit) = nonzero_credit(prior.get("rewardCredit").unwrap_or(&Value::Null))
            {
                merged["rewardCredit"] = credit;
            }
        }
        if result_in_flight(prior)
            && merged.get("arriveAt").and_then(Value::as_i64).unwrap_or(0) <= 0
        {
            if let Some(arrive_at) = prior
                .get("arriveAt")
                .and_then(Value::as_i64)
                .filter(|v| *v > 0)
            {
                merged["arriveAt"] = json!(arrive_at);
            }
        }
        return merged;
    }
    if !result_claimed(prior) {
        return merged;
    }
    let new_has_credit =
        nonzero_credit(merged.get("rewardCredit").unwrap_or(&Value::Null)).is_some();
    merged["claimed"] = json!(true);
    if !new_has_credit {
        merged["rewardCredit"] = prior.get("rewardCredit").cloned().unwrap_or(Value::Null);
        merged["claimedAt"] = prior.get("claimedAt").cloned().unwrap_or(json!(0));
    }
    if nonempty_str(merged.get("locationName").unwrap_or(&Value::Null)).is_none() {
        if let Some(name) = nonempty_str(prior.get("locationName").unwrap_or(&Value::Null)) {
            merged["locationName"] = json!(name);
        }
    }
    merged["state"] = json!("idle");
    merged
}

pub(super) fn persist_travel_cache(overlay: &Value) {
    with_travel_cache_lock(|| {
        let mut disk = load_travel_cache();
        let today = today_str();
        let overlay_date = overlay.get("date").and_then(Value::as_str);
        if overlay_date == Some(today.as_str()) {
            roll_cache_to_today(&mut disk, &today);
            disk["date"] = json!(today);
        }
        let overlay_results = overlay
            .get("results")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut results = cache_results(&disk).cloned().unwrap_or_default();
        for (id, entry) in overlay_results {
            let merged = if let Some(prior) = results.get(&id) {
                merge_claim_state(prior, &entry)
            } else {
                entry
            };
            results.insert(id, merged);
        }
        if overlay_date == Some(today.as_str()) {
            disk["date"] = json!(today);
        } else if disk.get("date").and_then(Value::as_str).is_none() {
            if let Some(date) = overlay_date {
                disk["date"] = json!(date);
            }
        }
        disk["completed"] = json!(!has_retryable_results(&results));
        disk["results"] = Value::Object(results);
        let _ = save_travel_cache(&disk);
    });
}

pub(super) fn mark_claimed(
    result: &mut Value,
    reward_credit: Option<Value>,
    message: Option<&str>,
) {
    result["claimed"] = json!(true);
    if let Some(credit) = reward_credit {
        if !credit.is_null() {
            result["rewardCredit"] = credit;
        }
    }
    result["claimedAt"] = json!(now_ms());
    result["state"] = json!("idle");
    if let Some(message) = message {
        result["message"] = json!(message);
    }
}

pub(super) fn display_record(label: &str, result: &Value) -> Value {
    let arrive_at = result.get("arriveAt").and_then(Value::as_i64).unwrap_or(0);
    json!({
        "label": label,
        "rewardCredit": nonzero_credit(result.get("rewardCredit").unwrap_or(&Value::Null))
            .unwrap_or(Value::Null),
        "locationName": nonempty_str(result.get("locationName").unwrap_or(&Value::Null))
            .map(str::to_string),
        "arriveAt": if arrive_at > 0 { json!(arrive_at) } else { Value::Null },
    })
}

pub(super) fn display_label(same_day: bool, result: &Value) -> &'static str {
    if result_in_flight(result) {
        "traveling"
    } else if same_day && result.get("skip").and_then(Value::as_str) == Some("no-buddy") {
        "no-buddy"
    } else if same_day && result_claimed(result) {
        "finished"
    } else {
        "untraveled"
    }
}

/// 到点仍卡在 traveling 的缓存，按官方 status 再对一次。
/// 官网 idle + daily_limit_reached 会落成已结束，避免卡片一直显示「即将到达」。
pub async fn reconcile_due_travel(account_id: Option<&str>) {
    let cache = load_travel_cache();
    let Some(results) = cache_results(&cache) else {
        return;
    };
    let now = now_secs();
    let due: Vec<String> = results
        .iter()
        .filter(|(id, entry)| {
            if let Some(filter) = account_id {
                if filter != id.as_str() {
                    return false;
                }
            }
            in_flight_due(entry, now)
        })
        .map(|(id, _)| id.clone())
        .collect();
    if due.is_empty() {
        return;
    }

    let accounts = travel_capable_accounts(load_accounts());
    let mut overlay_results = Map::new();
    for id in due {
        let Some(account) = accounts
            .iter()
            .find(|a| account_key(a).as_str() == id.as_str())
        else {
            continue;
        };
        let prior = results.get(&id).cloned().unwrap_or_else(|| json!({}));
        let updated = sync_account_for_dispatch(account, Some(&prior)).await;
        let updated = merge_claim_state(&prior, &updated);
        overlay_results.insert(id, updated);
    }
    if overlay_results.is_empty() {
        return;
    }
    persist_travel_cache(&json!({
        "date": cache.get("date").cloned().unwrap_or(Value::Null),
        "results": Value::Object(overlay_results),
    }));
}

/// 某账号旅行状态的展示值：`{ label, rewardCredit, locationName }`。
///
/// label 取值：`untraveled`（未旅行）、`no-buddy`、`traveling`（旅行中）、
/// `finished`（已结束，含官网「累了，明天再来吧」）。跨日未领的 traveling/arrived 仍显示旅行中。
/// 档位不支持成长中心（国际版）时返回 skipped 短路结果，同时保留展示字段以避免
/// 旧调用方解析失败。
pub fn travel_display(account_id: &str) -> Value {
    if account::find_account(account_id).is_some_and(|acc| !variant_of(&acc).supports_travel()) {
        let mut skipped = unsupported_variant_skip();
        skipped["label"] = json!("unsupported");
        skipped["rewardCredit"] = Value::Null;
        skipped["locationName"] = Value::Null;
        return skipped;
    }
    let today = today_str();
    let cache = load_travel_cache();
    let Some(r) = cache_results(&cache).and_then(|results| results.get(account_id)) else {
        return display_record("untraveled", &json!({}));
    };
    let same_day = cache.get("date").and_then(Value::as_str) == Some(today.as_str());
    let label = display_label(same_day, r);
    if label == "untraveled" {
        display_record("untraveled", &json!({}))
    } else {
        display_record(label, r)
    }
}
