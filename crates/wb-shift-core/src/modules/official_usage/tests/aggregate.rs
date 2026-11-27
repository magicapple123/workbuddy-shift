    fn local_date(days_ago: i64, hour: u32) -> (NaiveDate, i64) {
        let date = Local::now().date_naive() - Duration::days(days_ago);
        let timestamp = Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, 0, 0)
            .single()
            .expect("valid local test date")
            .timestamp_millis();
        (date, timestamp)
    }

    fn row(date: NaiveDate, ts: i64, credit: f64) -> RequestRow {
        RequestRow {
            request_id: "request-1".to_string(),
            credit,
            model: "model-a".to_string(),
            client: "client-a".to_string(),
            request_time: format!("{date} 12:00:00"),
            request_ts: ts,
            date,
        }
    }

    #[test]
    fn parses_success_page_and_does_not_copy_prompt_fields() {
        let page = parse_page(&json!({
            "code": 0,
            "data": {
                "total": "1",
                "data": [{
                    "requestId": "req-1",
                    "credit": "1.25",
                    "model": "model-a",
                    "client": "cli",
                    "requestTime": "2026-08-24 12:34:56",
                    "input": "do not expose this prompt",
                    "inputTrunc": "also secret"
                }]
            }
        }))
        .expect("valid official page");

        assert_eq!(page.raw_len, 1);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].credit, 1.25);
        let output = request_value("account-1", "one@example.com", &page.rows[0]);
        let text = output.to_string();
        assert!(!text.contains("do not expose"));
        assert!(!text.contains("inputTrunc"));
        assert_eq!(output["requestId"], "req-1");
    }

    #[test]
    fn parse_page_keeps_row_count_without_trusting_capped_total() {
        let page = parse_page(&json!({
            "code": 0,
            "data": {
                "total": "3000",
                "data": [
                    {"requestId": "valid", "credit": 2, "requestTime": "2026-08-24 12:00:00"},
                    {"requestId": "negative", "credit": -1, "requestTime": "2026-08-24 12:00:00"},
                    {"requestId": "bad-time", "credit": 4, "requestTime": "not-a-date"}
                ]
            }
        }))
        .expect("valid official page");

        // 原始条数用于判断「本页是否取满」，其中无法解析的记录不能当作 0 计入。
        assert_eq!(page.raw_len, 3);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].credit, 2.0);
    }

    /// 连续扫描：只有「取满」才继续，且起点必须严格前进，否则会空转。
    #[test]
    fn sweep_decision_only_advances_on_full_pages() {
        assert_eq!(decide_sweep(0, None, None), SweepDecision::Complete);
        assert_eq!(
            decide_sweep(OFFICIAL_USAGE_PAGE_SIZE - 1, Some(100), None),
            SweepDecision::Complete
        );
        assert_eq!(
            decide_sweep(OFFICIAL_USAGE_PAGE_SIZE, Some(100), None),
            SweepDecision::Continue(100)
        );
        assert_eq!(
            decide_sweep(OFFICIAL_USAGE_PAGE_SIZE, Some(200), Some(100)),
            SweepDecision::Continue(200)
        );
        // 取满但时间戳没有前进：记录挤在同一时间点，只能停下
        assert_eq!(
            decide_sweep(OFFICIAL_USAGE_PAGE_SIZE, Some(100), Some(100)),
            SweepDecision::Stalled
        );
        assert_eq!(
            decide_sweep(OFFICIAL_USAGE_PAGE_SIZE, None, Some(100)),
            SweepDecision::Stalled
        );
    }

    /// 相邻两轮的边界记录（同一 requestId + 时间戳）只算一次，窗口外的记录被丢弃。
    #[test]
    fn merged_rounds_dedupe_boundary_rows() {
        let (today, today_ts) = local_date(0, 12);
        let (yesterday, _) = local_date(1, 12);
        let mut rows = Vec::new();
        let mut seen = HashSet::new();

        let boundary = row(today, today_ts, 1.0);
        merge_rows(
            &mut rows,
            &mut seen,
            vec![boundary.clone()],
            yesterday,
            today,
        );
        merge_rows(&mut rows, &mut seen, vec![boundary], yesterday, today);
        assert_eq!(rows.len(), 1);

        let mut out_of_window = row(today - Duration::days(30), today_ts, 5.0);
        out_of_window.request_id = "request-2".to_string();
        merge_rows(&mut rows, &mut seen, vec![out_of_window], yesterday, today);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn aggregation_uses_positive_credits_and_local_today_windows() {
        let (today, today_ts) = local_date(0, 12);
        let (yesterday, yesterday_ts) = local_date(1, 12);
        let (last_month, last_month_ts) = local_date(40, 12);
        let rows = vec![
            row(today, today_ts, 1.5),
            row(yesterday, yesterday_ts, 2.0),
            row(last_month, last_month_ts, 5.0),
        ];

        let (today_usage, week_usage, month_usage, daily) = aggregate_rows(&rows, today);
        assert_eq!(today_usage, 1.5);
        assert_eq!(week_usage, 3.5);
        // 月初时“昨天”可能属于上月（甚至跨年），此时本月仅包含今天这条。
        let expected_month_usage =
            if yesterday.year() == today.year() && yesterday.month() == today.month() {
                3.5
            } else {
                1.5
            };
        assert_eq!(month_usage, expected_month_usage);
        assert_eq!(daily[&today], 1.5);
        assert_eq!(daily[&yesterday], 2.0);
    }

    #[test]
    fn model_aggregation_sorts_by_credit_and_groups_requests() {
        let (today, today_ts) = local_date(0, 12);
        let mut first = row(today, today_ts, 1.0);
        first.model = "model-a".to_string();
        let mut second = row(today, today_ts + 1, 4.0);
        second.model = "model-b".to_string();
        let mut third = row(today, today_ts + 2, 2.0);
        third.model = "model-a".to_string();

        let models = aggregate_models(&[first, second, third]);
        assert_eq!(models[0]["model"], "model-b");
        assert_eq!(models[0]["requestCount"], 1);
        assert_eq!(models[0]["credit"], 4.0);
        assert_eq!(models[1]["model"], "model-a");
        assert_eq!(models[1]["requestCount"], 2);
        assert_eq!(models[1]["credit"], 3.0);
    }

    #[test]
    fn rejects_error_and_missing_data_shapes() {
        assert!(matches!(
            parse_page(&json!({"code": 500, "msg": "bad"})),
            Err(error) if error.message == "官方请求失败（code=500）：bad"
        ));
        assert!(matches!(
            parse_page(&json!({"code": 0, "data": {"total": 0}})),
            Err(error) if error.message == "官方响应格式无效"
        ));
        assert_eq!(
            error_message(&json!({"code": -1, "message": "secret token"})),
            "官方请求失败（网络或服务不可达）"
        );
    }

    #[test]
    fn cache_round_trip_keeps_projection_and_strips_prompt_fields() {
        let payload = json!({
            "status": "complete",
            "rangeStart": "2026-07-26",
            "rangeEnd": "2026-08-25",
            "collectedAt": 1,
            "summary": { "usageToday": 1.5, "usage7Days": 3.0, "usageThisMonth": 3.0 },
            "daily": [{ "date": "2026-08-25", "usage": 1.5, "models": [{ "model": "model-a", "requestCount": 1, "credit": 1.5 }] }],
            "models": [{ "model": "model-a", "requestCount": 1, "credit": 1.5 }],
            "accounts": [{
                "accountId": "account-1",
                "accountName": "one@example.com",
                "ok": true,
                "requestCount": 1,
                "detailTruncated": false,
                "usageToday": 1.5,
                "usage7Days": 1.5,
                "usageThisMonth": 1.5,
                "error": null,
                "reportedTotal": 1,
                "fetchedCount": 1,
                "models": [{ "model": "model-a", "requestCount": 1, "credit": 1.5 }],
                "daily": []
            }],
            "requests": [{
                "accountId": "account-1",
                "accountName": "one@example.com",
                "requestId": "req-1",
                "credit": 1.5,
                "model": "model-a",
                "client": "cli",
                "requestTime": "2026-08-25 12:00:00",
                "input": "do not persist this prompt"
            }],
            "detailLimitPerAccount": 100,
            "errors": [],
            "secret": "drop-me"
        });
        let path = std::env::temp_dir().join(format!(
            "workbuddy-shift-official-usage-cache-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        save_official_usage_cache_to(&path, &payload);
        let loaded = load_official_usage_cache_from(&path).expect("valid cache");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded["status"], "complete");
        assert_eq!(loaded["summary"]["usageToday"], 1.5);
        assert_eq!(loaded["requests"][0]["requestId"], "req-1");
        assert!(loaded.get("secret").is_none());
        let text = loaded.to_string();
        assert!(!text.contains("do not persist"));
        assert!(!text.contains("drop-me"));
    }

    #[test]
    fn cache_parser_rejects_corrupt_and_unknown_status() {
        assert!(parse_official_usage_cache("not-json").is_none());
        assert!(parse_official_usage_cache("{}").is_none());
        assert!(parse_official_usage_cache(r#"{"payload":{"status":"nope"}}"#).is_none());
    }

    /// 档位分派：两档各走自己的域，绝不跨档位。
    #[test]
    fn official_usage_url_follows_account_variant() {
        assert_eq!(
            official_usage_url_for(&json!({"uid": "u-1"})),
            OFFICIAL_USAGE_URL
        );
        assert_eq!(
            official_usage_url_for(&json!({"uid": "u-1", "variant": "cn"})),
            OFFICIAL_USAGE_URL
        );
        assert_eq!(
            official_usage_url_for(&json!({"uid": "u-2", "variant": "ai"})),
            AI_OFFICIAL_USAGE_URL
        );
        assert_eq!(
            official_usage_url_for(&json!({"uid": "u-3", "domain": "www.workbuddy.ai"})),
            AI_OFFICIAL_USAGE_URL
        );
    }

    /// 上游失败带档位时，`UsageFetchError` 必须把 `failureKind` / `httpStatus` 带上来——
    /// 这是 `officialUsage.errors[]` 能给出「下一步动作」的唯一来源。
    #[test]
    fn parse_page_failure_carries_failure_kind() {
        let response = json!({
            "code": 502,
            "message": "服务端返回了网页而不是数据，可能是代理或网关拦截",
            "failureKind": "gateway_html",
            "httpStatus": 502,
        });

        let error = parse_page(&response).expect_err("502 必须判失败");
        // 文案逐字不变（与改造前同一个 `error_message`）。
        assert_eq!(error.message, error_message(&response));
        assert_eq!(
            error.failure_kind.as_deref(),
            Some("gateway_html"),
            "档位必须带上来"
        );
        assert_eq!(error.http_status, Some(502));
        // `Display` 就是文案本身：probe 的 reason 与错误日志沿用旧口径。
        assert_eq!(error.to_string(), error.message);
    }

    /// 业务错误 JSON（没有结构化字段）→ 两个字段都是 `None`，聚合出的 `errors[]` 项不变。
    #[test]
    fn parse_page_business_error_has_no_failure_kind() {
        let response = json!({"code": 10085, "msg": "请求不合法，如有疑问请联系客服"});
        let error = parse_page(&response).expect_err("非 0/200 一律失败");
        assert_eq!(error.message, error_message(&response));
        assert!(error.failure_kind.is_none());
        assert!(error.http_status.is_none());
    }
