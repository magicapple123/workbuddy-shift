//! 取数：账号分页抓取与并发汇总入口。

use super::aggregate::*;
use super::parse::*;
use super::*;

async fn fetch_account_usage(
    account: &Value,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Result<AccountFetch, String> {
    let url = official_usage_url_for(account);
    let end_time = format!("{range_end} 23:59:59");
    let mut start_time = format!("{range_start} 00:00:00");
    let mut rounds = 0usize;
    let mut fetched_raw = 0;
    let mut rows = Vec::new();
    let mut seen_request_ids = HashSet::new();
    let mut previous_newest: Option<i64> = None;

    loop {
        let response = authenticated_post(
            account,
            url,
            json!({
                "startTime": start_time,
                "endTime": end_time,
                "pageNum": 1,
                "pageSize": OFFICIAL_USAGE_PAGE_SIZE,
            }),
        )
        .await;
        let page = parse_page(&response)?;
        fetched_raw += page.raw_len;
        let newest_ts = page.rows.iter().map(|row| row.request_ts).max();
        merge_rows(
            &mut rows,
            &mut seen_request_ids,
            page.rows,
            range_start,
            range_end,
        );

        match decide_sweep(page.raw_len, newest_ts, previous_newest) {
            SweepDecision::Complete => break,
            SweepDecision::Stalled => {
                eprintln!(
                    "[官方用量] 取数起点无法继续推进，已保留 {} 条记录",
                    rows.len()
                );
                break;
            }
            SweepDecision::Continue(newest_ts) => {
                rounds += 1;
                if rounds >= OFFICIAL_USAGE_MAX_ROUNDS {
                    return Err("官方用量分页超过安全上限".to_string());
                }
                start_time =
                    start_time_at(newest_ts).ok_or_else(|| "官方用量分页时间无效".to_string())?;
                previous_newest = Some(newest_ts);
            }
        }
    }

    Ok(AccountFetch {
        // 服务端的 total 不可信，这里回报窗口内实际取回并去重后的条数。
        reported_total: rows.len(),
        fetched_raw,
        rows,
    })
}

/// 查询全部当前账号并生成官方请求用量投影。
pub async fn collect_official_usage(accounts: &[Value], at_ms: i64) -> Value {
    let today = local_date_at(at_ms);
    let range_start = today - Duration::days(30);
    let range_end = today;
    let range_start_text = range_start.format("%Y-%m-%d").to_string();
    let range_end_text = range_end.format("%Y-%m-%d").to_string();

    let mut successful_accounts = 0usize;
    let mut account_rows = Vec::new();
    let mut errors = Vec::new();
    let mut daily_totals: HashMap<NaiveDate, f64> = HashMap::new();
    let mut daily_models: HashMap<NaiveDate, HashMap<String, (usize, f64)>> = HashMap::new();
    let mut account_daily_totals: HashMap<String, HashMap<NaiveDate, f64>> = HashMap::new();
    type AccountDailyModels = HashMap<String, HashMap<NaiveDate, HashMap<String, (usize, f64)>>>;
    let mut account_daily_models: AccountDailyModels = HashMap::new();
    let mut total_today = 0.0;
    let mut total_week = 0.0;
    let mut total_month = 0.0;
    let mut recent_requests: Vec<(i64, Value)> = Vec::new();
    let mut model_totals: HashMap<String, (usize, f64)> = HashMap::new();

    // 先并发把各账号的明细取回来，再按账号原始顺序聚合：
    // 聚合里用到的「累计到当前账号」的派生值（如单账号每日序列）依赖处理顺序，
    // 因此只并行网络取数，不并行聚合，保证输出与串行实现逐字一致。
    let mut fetched: Vec<(usize, Result<AccountFetch, String>)> =
        Vec::with_capacity(accounts.len());
    if !accounts.is_empty() {
        let permits = Arc::new(Semaphore::new(
            OFFICIAL_USAGE_FETCH_CONCURRENCY.min(accounts.len()),
        ));
        let mut tasks = JoinSet::new();
        for (index, account) in accounts.iter().enumerate() {
            let account = account.clone();
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                let _permit = permits.acquire_owned().await.expect("semaphore stays open");
                (
                    index,
                    fetch_account_usage(&account, range_start, range_end).await,
                )
            });
        }
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(item) => fetched.push(item),
                Err(error) => eprintln!("[官方用量] 采集任务异常退出：{error}"),
            }
        }
        fetched.sort_by_key(|(index, _)| *index);
    }

    for (index, result) in fetched {
        let Some(account) = accounts.get(index) else {
            continue;
        };
        let account_id = get_str(account, "id").unwrap_or_else(|| format!("unknown-{index}"));
        let account_name = account_display_name(account);
        match result {
            Ok(result) => {
                successful_accounts += 1;
                let (usage_today, usage_week, usage_month, daily) =
                    aggregate_rows(&result.rows, today);
                total_today += usage_today;
                total_week += usage_week;
                total_month += usage_month;
                for row in &result.rows {
                    add_model_usage(&mut model_totals, row);
                    // 每日按模型聚合（全量，不受 100 条明细限制）
                    add_model_usage(daily_models.entry(row.date).or_default(), row);
                    // 单账号每日按模型聚合（同样不受明细条数限制）
                    add_model_usage(
                        account_daily_models
                            .entry(account_id.clone())
                            .or_default()
                            .entry(row.date)
                            .or_default(),
                        row,
                    );
                }
                for (date, amount) in daily {
                    *daily_totals.entry(date).or_insert(0.0) += amount;
                    *account_daily_totals
                        .entry(account_id.clone())
                        .or_default()
                        .entry(date)
                        .or_insert(0.0) += amount;
                }

                let mut sorted_rows = result.rows.clone();
                sorted_rows.sort_by_key(|row| Reverse(row.request_ts));
                for row in sorted_rows.iter().take(OFFICIAL_USAGE_DETAIL_LIMIT) {
                    recent_requests.push((
                        row.request_ts,
                        request_value(&account_id, &account_name, row),
                    ));
                }
                account_rows.push(json!({
                    "accountId": account_id,
                    "accountName": account_name,
                    "ok": true,
                    "requestCount": result.reported_total,
                    "detailTruncated": result.reported_total > OFFICIAL_USAGE_DETAIL_LIMIT,
                    "usageToday": usage_today,
                    "usage7Days": usage_week,
                    "usageThisMonth": usage_month,
                    "error": Value::Null,
                    "reportedTotal": result.reported_total,
                    "fetchedCount": result.fetched_raw,
                    "models": aggregate_models(&result.rows),
                    "daily": daily_series(
                        account_daily_totals
                            .get(&account_id)
                            .unwrap_or(&HashMap::new()),
                        account_daily_models
                            .get(&account_id)
                            .unwrap_or(&HashMap::new()),
                        range_start,
                        range_end,
                    ),
                }));
            }
            Err(error) => {
                // 失败必须可诊断（issue #225）：账号、接口 URL 与错误链一起落到
                // `~/.workbuddy-shift/error.log`，与更新器/限额 hook 共用同一出口。
                // 页面只会显示「官方用量暂不可用」，具体原因在这里留证。
                crate::modules::error_log::record(
                    "backend",
                    "官方用量取数失败",
                    &format!(
                        "account={account_name} accountId={account_id} variant={} url={} error={error}",
                        variant_of(account).as_str(),
                        official_usage_url_for(account),
                    ),
                );
                errors.push(json!({
                    "accountId": account_id,
                    "accountName": account_name,
                    "error": error,
                }));
                account_rows.push(json!({
                    "accountId": account_id,
                    "accountName": account_name,
                    "ok": false,
                    "requestCount": 0,
                    "detailTruncated": false,
                    "usageToday": Value::Null,
                    "usage7Days": Value::Null,
                    "usageThisMonth": Value::Null,
                    "error": errors.last().and_then(|item| item.get("error")).cloned().unwrap_or(Value::Null),
                    "reportedTotal": Value::Null,
                    "fetchedCount": 0,
                    "models": [],
                    "daily": [],
                }));
            }
        }
    }

    recent_requests.sort_by_key(|(ts, _)| Reverse(*ts));
    let requests: Vec<Value> = recent_requests
        .into_iter()
        .map(|(_, value)| value)
        .collect();

    let daily = daily_series(&daily_totals, &daily_models, range_start, range_end);

    let status = if accounts.is_empty() {
        "unavailable"
    } else if successful_accounts == accounts.len() {
        "complete"
    } else if successful_accounts > 0 {
        "partial"
    } else {
        "unavailable"
    };

    json!({
        "status": status,
        "rangeStart": range_start_text,
        "rangeEnd": range_end_text,
        "collectedAt": at_ms,
        "summary": {
            "usageToday": total_today,
            "usage7Days": total_week,
            "usageThisMonth": total_month,
        },
        "daily": daily,
        "accounts": account_rows,
        "requests": requests,
        "models": model_usage_values(model_totals),
        "detailLimitPerAccount": OFFICIAL_USAGE_DETAIL_LIMIT,
        "errors": errors,
    })
}
