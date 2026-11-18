//! 聚合：模型/每日/明细行的统计投影。

use super::parse::*;
use super::*;

pub(super) fn aggregate_rows(
    rows: &[RequestRow],
    today: NaiveDate,
) -> (f64, f64, f64, HashMap<NaiveDate, f64>) {
    let mut today_usage = 0.0;
    let mut week_usage = 0.0;
    let mut month_usage = 0.0;
    let mut daily = HashMap::new();
    for row in rows {
        let distance = (today - row.date).num_days();
        if distance == 0 {
            today_usage += row.credit;
        }
        if (0..7).contains(&distance) {
            week_usage += row.credit;
        }
        if row.date.year() == today.year() && row.date.month() == today.month() {
            month_usage += row.credit;
        }
        *daily.entry(row.date).or_insert(0.0) += row.credit;
    }
    (today_usage, week_usage, month_usage, daily)
}

pub(super) fn model_name(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() || model == "—" {
        "未知模型".to_string()
    } else {
        model.to_string()
    }
}

pub(super) fn add_model_usage(usage: &mut HashMap<String, (usize, f64)>, row: &RequestRow) {
    let entry = usage.entry(model_name(&row.model)).or_insert((0, 0.0));
    entry.0 += 1;
    entry.1 += row.credit;
}

pub(super) fn model_usage_values(usage: HashMap<String, (usize, f64)>) -> Vec<Value> {
    let mut models: Vec<(String, (usize, f64))> = usage.into_iter().collect();
    models.sort_by(
        |(left_name, (left_count, left_credit)), (right_name, (right_count, right_credit))| {
            right_credit
                .partial_cmp(left_credit)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right_count.cmp(left_count))
                .then_with(|| left_name.cmp(right_name))
        },
    );
    models
        .into_iter()
        .map(|(model, (request_count, credit))| {
            json!({
                "model": model,
                "requestCount": request_count,
                "credit": credit,
            })
        })
        .collect()
}

pub(super) fn aggregate_models(rows: &[RequestRow]) -> Vec<Value> {
    let mut usage = HashMap::new();
    for row in rows {
        add_model_usage(&mut usage, row);
    }
    model_usage_values(usage)
}

/// 生成 range_start..=range_end 的逐日序列（无数据的天补 0，模型聚合缺省为空）。
pub(super) fn daily_series(
    totals: &HashMap<NaiveDate, f64>,
    models: &HashMap<NaiveDate, HashMap<String, (usize, f64)>>,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Vec<Value> {
    let mut daily = Vec::new();
    let mut date = range_start;
    while date <= range_end {
        daily.push(json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "usage": totals.get(&date).copied().unwrap_or(0.0),
            "models": models
                .get(&date)
                .map(|models| model_usage_values(models.clone()))
                .unwrap_or_default(),
        }));
        date += Duration::days(1);
    }
    daily
}

pub(super) fn request_value(account_id: &str, account_name: &str, row: &RequestRow) -> Value {
    json!({
        "accountId": account_id,
        "accountName": account_name,
        "requestId": row.request_id,
        "credit": row.credit,
        "model": row.model,
        "client": row.client,
        "requestTime": row.request_time,
    })
}
