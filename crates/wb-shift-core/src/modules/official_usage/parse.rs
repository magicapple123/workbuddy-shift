//! 解析：请求行/分页结构、字段归一、扫描决策与行合并。

use super::*;

#[derive(Clone, Debug)]
pub(super) struct RequestRow {
    pub(super) request_id: String,
    pub(super) credit: f64,
    pub(super) model: String,
    pub(super) client: String,
    pub(super) request_time: String,
    pub(super) request_ts: i64,
    pub(super) date: NaiveDate,
}

#[derive(Clone, Debug)]
pub(super) struct OfficialPage {
    /// 服务端实际返回的条数（含未通过归一化的记录），用于判断本页是否取满。
    pub(super) raw_len: usize,
    pub(super) rows: Vec<RequestRow>,
}

#[derive(Clone, Debug)]
pub(super) struct AccountFetch {
    pub(super) rows: Vec<RequestRow>,
    pub(super) reported_total: usize,
    pub(super) fetched_raw: usize,
}

pub(super) fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

pub(super) use crate::modules::config::parse_number;

pub(super) fn response_code(response: &Value) -> Option<i64> {
    response.get("code").and_then(|value| {
        value.as_i64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
    })
}

pub(super) fn error_message(response: &Value) -> String {
    let upstream_message = response
        .get("message")
        .or_else(|| response.get("msg"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(160).collect::<String>());
    match response_code(response) {
        Some(-1) => "官方请求失败（网络或服务不可达）".to_string(),
        Some(code) => upstream_message
            .map(|message| format!("官方请求失败（code={code}）：{message}"))
            .unwrap_or_else(|| format!("官方请求失败（code={code}）")),
        None => "官方响应格式无效".to_string(),
    }
}

pub(super) fn local_datetime_to_parts(value: NaiveDateTime) -> Option<(NaiveDate, i64)> {
    Local
        .from_local_datetime(&value)
        .single()
        .map(|date| (date.date_naive(), date.timestamp_millis()))
}

pub(super) fn timestamp_to_parts(ts: i64) -> Option<(NaiveDate, i64)> {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|date| (date.date_naive(), ts))
}

pub(super) fn parse_request_time(value: Option<&Value>) -> Option<(NaiveDate, i64, String)> {
    let value = value?;
    if let Some(number) = parse_number(Some(value)) {
        if !number.is_finite() {
            return None;
        }
        let ts = if number.abs() < 10_000_000_000.0 {
            (number * 1000.0).round() as i64
        } else {
            number.round() as i64
        };
        let (date, ts) = timestamp_to_parts(ts)?;
        return Some((date, ts, ts.to_string()));
    }

    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
        let ts = parsed.timestamp_millis();
        let date = Local.timestamp_millis_opt(ts).single()?.date_naive();
        return Some((date, ts, text.to_string()));
    }
    for pattern in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, pattern) {
            let (date, ts) = local_datetime_to_parts(parsed)?;
            return Some((date, ts, text.to_string()));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let parsed = date.and_hms_opt(12, 0, 0)?;
        let (date, ts) = local_datetime_to_parts(parsed)?;
        return Some((date, ts, text.to_string()));
    }
    None
}

pub(super) fn compact_string(value: Option<&Value>, fallback: &str) -> String {
    let text = non_empty(value).unwrap_or_else(|| fallback.to_string());
    text.chars().take(160).collect()
}

pub(super) fn normalize_row(value: &Value) -> Option<RequestRow> {
    let credit = parse_number(value.get("credit"))?;
    if !credit.is_finite() || credit < 0.0 {
        return None;
    }
    let (date, request_ts, request_time) = parse_request_time(
        value
            .get("requestTime")
            .or_else(|| value.get("request_time")),
    )?;
    Some(RequestRow {
        request_id: compact_string(
            value.get("requestId").or_else(|| value.get("request_id")),
            "unknown",
        ),
        credit,
        model: compact_string(value.get("model"), "—"),
        client: compact_string(value.get("client"), "—"),
        request_time,
        request_ts,
        date,
    })
}

pub(super) fn parse_page(response: &Value) -> Result<OfficialPage, String> {
    if response_code(response) != Some(0) && response_code(response) != Some(200) {
        return Err(error_message(response));
    }
    let Some(data) = response.get("data").and_then(Value::as_object) else {
        return Err("官方响应格式无效".to_string());
    };
    let Some(raw_rows) = data.get("data").and_then(Value::as_array) else {
        return Err("官方响应格式无效".to_string());
    };
    // `data.total` 会被服务端一起截断（31 天窗口固定回报 3000），不能当总数用。
    Ok(OfficialPage {
        raw_len: raw_rows.len(),
        rows: raw_rows.iter().filter_map(normalize_row).collect(),
    })
}

/// 本轮之后如何继续扫描。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SweepDecision {
    /// 本轮没取满：窗口内的记录已经取完。
    Complete,
    /// 本轮取满：从该时间戳（含）继续向后取。
    Continue(i64),
    /// 本轮取满但起点无法推进（记录挤在同一时间点）：停止并保留已取到的数据。
    Stalled,
}

pub(super) fn decide_sweep(
    page_len: usize,
    newest_ts: Option<i64>,
    previous_newest: Option<i64>,
) -> SweepDecision {
    if page_len < OFFICIAL_USAGE_PAGE_SIZE {
        return SweepDecision::Complete;
    }
    let Some(newest_ts) = newest_ts else {
        return SweepDecision::Stalled;
    };
    match previous_newest {
        Some(previous) if newest_ts <= previous => SweepDecision::Stalled,
        _ => SweepDecision::Continue(newest_ts),
    }
}

/// 下一轮起点（含边界那条，靠去重去掉重复），格式对齐接口要求。
pub(super) fn start_time_at(ts: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// 合并一轮结果：只保留窗口内的记录，并按 (requestId, requestTime) 去重。
/// 相邻两轮在边界时间戳上必然重复，去重是连续扫描正确性的前提。
pub(super) fn merge_rows(
    rows: &mut Vec<RequestRow>,
    seen_request_ids: &mut HashSet<(String, i64)>,
    incoming: Vec<RequestRow>,
    range_start: NaiveDate,
    range_end: NaiveDate,
) {
    for row in incoming
        .into_iter()
        .filter(|row| row.date >= range_start && row.date <= range_end)
    {
        if seen_request_ids.insert((row.request_id.clone(), row.request_ts)) {
            rows.push(row);
        }
    }
}

pub(super) fn local_date_at(ts: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|date| date.date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}
