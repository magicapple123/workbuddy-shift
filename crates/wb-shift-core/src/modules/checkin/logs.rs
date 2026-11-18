//! 签到日志：日期工具、日志补档位与读取、当日结果查询。

use super::*;

pub fn date_str(ts_ms: Option<i64>) -> String {
    let dt = Local::now();
    if let Some(ms) = ts_ms {
        let secs = ms / 1000;
        chrono::DateTime::from_timestamp(secs, 0)
            .map(|d| d.with_timezone(&Local).format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| dt.format("%Y-%m-%d").to_string())
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// 给签到日志行补齐档位（宿主按档位过滤用）。
///
/// 新写入的日志自带 `variant`；历史行按当前账号库回填，账号已删除或缺失时按
/// 国内版解释（缺省即 cn，见 design D2）。纯函数，便于单测。
pub fn checkin_logs_with_variant(logs: &[Value], accounts: &[Value]) -> Vec<Value> {
    let mut known: HashMap<String, &'static str> = HashMap::new();
    for account in accounts {
        if let Some(id) = account.get("id").and_then(Value::as_str) {
            known.insert(id.to_string(), variant_of(account).as_str());
        }
    }
    logs.iter()
        .map(|entry| {
            let mut row = entry.clone();
            if row.get("variant").and_then(Value::as_str).is_none() {
                let fallback = row
                    .get("accountId")
                    .and_then(Value::as_str)
                    .and_then(|id| known.get(id).copied())
                    .unwrap_or_else(|| WbVariant::parse(None).as_str());
                row["variant"] = json!(fallback);
            }
            row
        })
        .collect()
}

/// 读取签到日志并补齐档位字段。
pub fn load_checkin_logs_with_variant() -> Vec<Value> {
    checkin_logs_with_variant(&load_checkin_logs(), &load_accounts())
}

pub(super) fn latest_today_result<'a>(
    logs: &'a [Value],
    account_id: &str,
    today: &str,
) -> Option<&'a str> {
    logs.iter()
        .rev()
        .find(|entry| {
            entry.get("accountId").and_then(Value::as_str) == Some(account_id)
                && date_str(entry.get("ts").and_then(Value::as_i64)) == today
        })
        .and_then(|entry| entry.get("result").and_then(Value::as_str))
}
