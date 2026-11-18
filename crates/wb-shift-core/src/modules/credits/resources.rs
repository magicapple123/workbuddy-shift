//! 资源解析与投影：响应字段取值、资源合并、到期时间与积分结果投影。

use super::*;

pub(super) fn first_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}

pub(super) use crate::modules::config::parse_number;

pub(super) fn first_number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| parse_number(value.get(*key)))
}

pub(super) fn parse_timestamp_ms(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = parse_number(Some(value)) {
        let millis = if number.abs() < 10_000_000_000.0 {
            number * 1000.0
        } else {
            number
        };
        return Some(millis.round() as i64);
    }

    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }

    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(parsed.timestamp_millis());
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return Local
            .from_local_datetime(&parsed)
            .single()
            .map(|date| date.timestamp_millis());
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
        return Local
            .from_local_datetime(&parsed)
            .single()
            .map(|date| date.timestamp_millis());
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(23, 59, 59))
        .and_then(|date| Local.from_local_datetime(&date).single())
        .map(|date| date.timestamp_millis())
}

pub(super) fn value_at_path<'a>(mut current: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

pub(super) fn resource_accounts(response: &Value) -> Vec<&Value> {
    let paths: &[&[&str]] = &[
        &["data", "Accounts"],
        &["data", "data", "Accounts"],
        &["data", "Response", "Data", "Accounts"],
        &["data", "data", "Response", "Data", "Accounts"],
        &["data", "accounts"],
        &["data", "data", "accounts"],
    ];

    for path in paths {
        if let Some(items) = value_at_path(response, path).and_then(Value::as_array) {
            return items.iter().collect();
        }
    }
    Vec::new()
}

pub(super) fn resource_packages(response: &Value) -> Vec<&Value> {
    let paths: &[&[&str]] = &[
        &["data", "Packages"],
        &["data", "data", "Packages"],
        &["data", "Response", "Data", "Packages"],
        &["data", "data", "Response", "Data", "Packages"],
        &["data", "packages"],
        &["data", "data", "packages"],
    ];

    for path in paths {
        if let Some(items) = value_at_path(response, path).and_then(Value::as_array) {
            return items.iter().collect();
        }
    }
    Vec::new()
}

pub(super) fn has_resource_accounts(response: &Value) -> bool {
    let paths: &[&[&str]] = &[
        &["data", "Accounts"],
        &["data", "data", "Accounts"],
        &["data", "Response", "Data", "Accounts"],
        &["data", "data", "Response", "Data", "Accounts"],
        &["data", "accounts"],
        &["data", "data", "accounts"],
    ];
    paths.iter().any(|path| {
        value_at_path(response, path)
            .and_then(Value::as_array)
            .is_some()
    })
}

pub(super) fn has_resource_packages(response: &Value) -> bool {
    let paths: &[&[&str]] = &[
        &["data", "Packages"],
        &["data", "data", "Packages"],
        &["data", "Response", "Data", "Packages"],
        &["data", "data", "Response", "Data", "Packages"],
        &["data", "packages"],
        &["data", "data", "packages"],
    ];
    paths.iter().any(|path| {
        value_at_path(response, path)
            .and_then(Value::as_array)
            .is_some()
    })
}

/// 到期时间：优先 DeductionEndTime/ExpiredTime；仅当 CycleEndTime 比其早超过
/// `EXPIRY_CYCLE_OVERRIDE_DAYS` 时改用周期结束时间（视前者为长期占位）。
/// 最终值距 now 超过 `FAR_FUTURE_EXPIRY_DAYS` 则视为长期有效。
pub(super) fn resolve_expire_at(raw: &Value, now: i64) -> Option<i64> {
    let deduction_end = parse_timestamp_ms(first_value(
        raw,
        &[
            "DeductionEndTime",
            "deductionEndTime",
            "ExpiredTime",
            "expiredTime",
        ],
    ));
    let cycle_end = parse_timestamp_ms(first_value(raw, &["CycleEndTime", "cycleEndTime"]));
    let override_ms = EXPIRY_CYCLE_OVERRIDE_DAYS * 24 * 3600 * 1000;
    let expire_at = match (deduction_end, cycle_end) {
        (Some(deduction), Some(cycle)) if deduction.saturating_sub(cycle) > override_ms => {
            Some(cycle)
        }
        (Some(deduction), _) => Some(deduction),
        (None, cycle) => cycle,
    };
    let far_future_ms = FAR_FUTURE_EXPIRY_DAYS * 24 * 3600 * 1000;
    expire_at.filter(|value| value.saturating_sub(now) <= far_future_ms)
}

pub(super) fn resource_summary(raw: &Value, now: i64) -> Value {
    let slice = first_value(raw, &["SlicePeriodUsageDetails", "slicePeriodUsageDetails"])
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let total_keys = [
        "CycleCapacitySizePrecise",
        "CycleCapacitySize",
        "CycleTotalCapacity",
        "CapacitySizePrecise",
        "CapacitySize",
        "SlicePeriodCapacitySizePrecise",
        "SlicePeriodCapacitySize",
    ];
    let remaining_keys = [
        "CycleCapacityRemainPrecise",
        "CycleCapacityRemain",
        "CycleRemainCapacity",
        "CapacityRemainPrecise",
        "CapacityRemain",
        "SlicePeriodCapacityRemainPrecise",
        "SlicePeriodCapacityRemain",
    ];
    let used_keys = [
        "CycleCapacityUsedPrecise",
        "CycleCapacityUsed",
        "CycleUsedCapacity",
        "CapacityUsedPrecise",
        "CapacityUsed",
        "SlicePeriodCapacityUsedPrecise",
        "SlicePeriodCapacityUsed",
    ];
    let raw_total = first_number(raw, &total_keys)
        .or_else(|| slice.and_then(|value| first_number(value, &total_keys)));
    let raw_remaining = first_number(raw, &remaining_keys)
        .or_else(|| slice.and_then(|value| first_number(value, &remaining_keys)));
    let raw_used = first_number(raw, &used_keys)
        .or_else(|| slice.and_then(|value| first_number(value, &used_keys)));
    let total = raw_total
        .or_else(|| {
            raw_remaining
                .zip(raw_used)
                .map(|(remaining, used)| remaining + used)
        })
        .or(raw_remaining)
        .or(raw_used)
        .unwrap_or(0.0)
        .max(0.0);
    let remaining = raw_remaining
        .unwrap_or_else(|| (total - raw_used.unwrap_or(0.0)).max(0.0))
        .max(0.0);
    let used = raw_used
        .unwrap_or_else(|| (total - remaining).max(0.0))
        .max(0.0);
    let expire_at = resolve_expire_at(raw, now);
    let expired = expire_at.map(|value| value <= now).unwrap_or(false);
    let expiring_soon = expire_at
        .map(|value| value > now && value - now <= EXPIRING_SOON_DAYS * 24 * 3600 * 1000)
        .unwrap_or(false);
    let status = first_value(raw, &["Status", "status"])
        .and_then(|value| parse_number(Some(value)))
        .map(|value| value as i64);

    json!({
        "packageCode": first_value(raw, &["PackageCode", "packageCode"]),
        "packageName": first_value(raw, &["PackageName", "packageName"]),
        "total": total,
        "remaining": remaining,
        "used": used,
        "status": status,
        "expireAt": expire_at,
        "expired": expired,
        "expiringSoon": expiring_soon,
    })
}

pub(super) fn merge_resources(
    summary_resources: Vec<Value>,
    detail_resources: Vec<Value>,
) -> Vec<Value> {
    let detail_codes: HashSet<String> = detail_resources
        .iter()
        .filter_map(|resource| {
            first_value(resource, &["packageCode", "PackageCode"])
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut resources = detail_resources;
    resources.extend(summary_resources.into_iter().filter(|resource| {
        first_value(resource, &["packageCode", "PackageCode"])
            .and_then(Value::as_str)
            .map(|code| !detail_codes.contains(code))
            .unwrap_or(true)
    }));
    resources
}

pub(super) fn normalized_new_resources(
    summary_response: &Value,
    paid_response: &Value,
    free_response: &Value,
    now: i64,
) -> Option<Vec<Value>> {
    let summary_ok = is_success(summary_response) && has_resource_packages(summary_response);
    let paid_ok = is_success(paid_response) && has_resource_accounts(paid_response);
    let free_ok = is_success(free_response) && has_resource_accounts(free_response);
    if !(summary_ok || paid_ok || free_ok) {
        return None;
    }

    let summary_resources = if summary_ok {
        resource_packages(summary_response)
            .into_iter()
            .map(|resource| resource_summary(resource, now))
            .collect()
    } else {
        Vec::new()
    };
    let mut detail_resources = Vec::new();
    if paid_ok {
        detail_resources.extend(
            resource_accounts(paid_response)
                .into_iter()
                .map(|resource| resource_summary(resource, now)),
        );
    }
    if free_ok {
        detail_resources.extend(
            resource_accounts(free_response)
                .into_iter()
                .map(|resource| resource_summary(resource, now)),
        );
    }
    Some(merge_resources(summary_resources, detail_resources))
}

pub(super) fn credit_result(account: &Value, resources: Vec<Value>, now: i64) -> Value {
    let total_remaining: f64 = resources
        .iter()
        .filter_map(|resource| resource.get("remaining").and_then(|value| value.as_f64()))
        .sum();
    let total_capacity: f64 = resources
        .iter()
        .filter_map(|resource| resource.get("total").and_then(|value| value.as_f64()))
        .sum();
    let soonest_expire_at = resources
        .iter()
        .filter(|resource| {
            resource
                .get("remaining")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0)
                > 0.0
        })
        .filter_map(|resource| resource.get("expireAt").and_then(|value| value.as_i64()))
        .min();
    let expiring_soon = resources.iter().any(|resource| {
        resource
            .get("expiringSoon")
            .and_then(|value| value.as_bool())
            == Some(true)
            && resource
                .get("remaining")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0)
                > 0.0
    });
    let expired = resources.iter().any(|resource| {
        resource.get("expired").and_then(|value| value.as_bool()) == Some(true)
            && resource
                .get("remaining")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0)
                > 0.0
    });
    let expiring_soon_remaining: f64 = resources
        .iter()
        .filter(|resource| {
            resource
                .get("expiringSoon")
                .and_then(|value| value.as_bool())
                == Some(true)
        })
        .filter_map(|resource| resource.get("remaining").and_then(|value| value.as_f64()))
        .sum();
    let expired_remaining: f64 = resources
        .iter()
        .filter(|resource| resource.get("expired").and_then(|value| value.as_bool()) == Some(true))
        .filter_map(|resource| resource.get("remaining").and_then(|value| value.as_f64()))
        .sum();
    let account_id = account.get("id").cloned().unwrap_or(Value::Null);
    let account_name = account_display_name(account);
    if let Some(account_id) = account_id.as_str() {
        let _ = credit_usage::record_snapshot(
            account_id,
            &account_name,
            total_capacity,
            total_remaining,
            variant_of(account),
        );
    }

    json!({
        "ok": true,
        "accountId": account_id,
        "accountName": account_name,
        "updatedAt": now,
        "totalCapacity": total_capacity,
        "totalRemaining": total_remaining,
        "expiringSoonRemaining": expiring_soon_remaining,
        "expiredRemaining": expired_remaining,
        "soonestExpireAt": soonest_expire_at,
        "expiringSoon": expiring_soon,
        "expired": expired,
        "resources": resources,
    })
}

// ---------------------------------------------------------------------------
// 企业版分流（issue #167）：企业配额不在个人版资源接口里，官方按 `enterpriseId`
// 分流到 `get-enterprise-user-usage`。个人版路径不改动；企业结果输出同一
// `CreditExpiry` 结构（多一个 `unlimited` 标记）。
// ---------------------------------------------------------------------------

/// 企业配额用量（企业接口响应解析结果）。
pub(super) struct EnterpriseUsage {
    /// 总量；`-1` 表示不限量。
    pub(super) limit_num: f64,
    /// 已用量。
    pub(super) used: f64,
    /// 周期重置时间（毫秒时间戳）；解析失败为 `None`，不阻塞展示。
    pub(super) cycle_reset_at: Option<i64>,
}

/// 解析企业配额响应（纯函数，便于单测）。
///
/// 兼容三种包装形态 `{data:{data:{…}}}` / `{data:{…}}` / 字段直接位于响应顶层；
/// 字段双命名兼容（官方两处实现分别用 camelCase 与 snake_case，网关真实形态未确证）。
/// 总量缺失或非有限数字视为解析失败（`None`），由上层给出可读错误。
pub(super) fn parse_enterprise_usage(response: &Value) -> Option<EnterpriseUsage> {
    let data = response.get("data").unwrap_or(&Value::Null);
    let nested = data.get("data").unwrap_or(&Value::Null);
    for payload in [nested, data, response] {
        if !payload.is_object() {
            continue;
        }
        let Some(limit_num) = first_number(payload, &["limitNum", "limit_num"]) else {
            continue;
        };
        if !limit_num.is_finite() {
            continue;
        }
        let used = first_number(payload, &["credit", "used_num"])
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
            .max(0.0);
        let cycle_reset_at =
            parse_timestamp_ms(first_value(payload, &["cycleResetTime", "CycleResetTime"]));
        return Some(EnterpriseUsage {
            limit_num,
            used,
            cycle_reset_at,
        });
    }
    None
}

/// 积分查询失败结果（`ok:false` + 可读错误），与企业判定失败同一形态。
pub(super) fn credit_error_result(account: &Value, error: String) -> Value {
    json!({
        "ok": false,
        "accountId": account.get("id").cloned().unwrap_or(Value::Null),
        "accountName": account_display_name(account),
        "error": error,
    })
}

/// 企业配额响应 → `CreditExpiry`（与个人版同形，多一个 `unlimited` 标记）。
pub(super) fn enterprise_credit_result(account: &Value, response: &Value, now: i64) -> Value {
    if !is_success(response) {
        return credit_error_result(account, response_error(response));
    }
    let Some(usage) = parse_enterprise_usage(response) else {
        return credit_error_result(account, "企业配额数据无法解析".to_string());
    };
    if usage.limit_num == -1.0 {
        // 不限量：数字字段置 null、资源列表为空；前端按 `unlimited` 分支展示，
        // 不写积分快照（0/0 快照会把统计页误显示成余额为 0）。
        return json!({
            "ok": true,
            "accountId": account.get("id").cloned().unwrap_or(Value::Null),
            "accountName": account_display_name(account),
            "updatedAt": now,
            "unlimited": true,
            "totalCapacity": Value::Null,
            "totalRemaining": Value::Null,
            "expiringSoonRemaining": 0,
            "expiredRemaining": 0,
            "soonestExpireAt": Value::Null,
            "expiringSoon": false,
            "expired": false,
            "resources": [],
        });
    }
    // 企业配额以单条资源（名称「企业配额」）呈现，expireAt 即周期重置时间。
    let remaining = (usage.limit_num - usage.used).max(0.0);
    let resource = json!({
        "packageCode": Value::Null,
        "packageName": "企业配额",
        "total": usage.limit_num,
        "remaining": remaining,
        "used": usage.used,
        "status": Value::Null,
        "expireAt": usage.cycle_reset_at,
        "expired": false,
        "expiringSoon": false,
    });
    let mut result = credit_result(account, vec![resource], now);
    if let Some(object) = result.as_object_mut() {
        object.insert("unlimited".to_string(), Value::Bool(false));
    }
    result
}

/// 从 `/console/accounts` 响应提取第一个账号的企业 ID（纯函数，便于单测）。
pub(super) fn enterprise_id_from_accounts(response: &Value) -> Option<String> {
    if !is_success(response) {
        return None;
    }
    let accounts = value_at_path(response, &["data", "accounts"])
        .or_else(|| value_at_path(response, &["data", "Accounts"]))
        .and_then(Value::as_array)?;
    enterprise_id_of(accounts.first()?)
}
