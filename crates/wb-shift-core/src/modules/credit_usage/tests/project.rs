    fn at_local_date(days_ago: i64, hour: u32) -> i64 {
        let today = Local::now().date_naive() - ChronoDuration::days(days_ago);
        Local
            .with_ymd_and_hms(today.year(), today.month(), today.day(), hour, 0, 0)
            .single()
            .expect("valid local test date")
            .timestamp_millis()
    }

    fn snap(ts: i64, remaining: f64) -> Value {
        snapshot_value(
            ts,
            "account-1",
            "one@example.com",
            100.0,
            remaining,
            WbVariant::Cn,
        )
    }

    fn snap_variant(ts: i64, remaining: f64, variant: WbVariant) -> Value {
        snapshot_value(
            ts,
            "account-1",
            "one@example.com",
            100.0,
            remaining,
            variant,
        )
    }

    /// 旧快照没有 `variant` 字段：按国内版解释，且统计行显式带档位。
    #[test]
    fn legacy_snapshots_without_variant_are_read_as_cn() {
        let legacy = json!({
            "ts": at_local_date(0, 12),
            "accountId": "account-1",
            "accountName": "one@example.com",
            "total": 100.0,
            "remaining": 90.0,
        });
        assert!(legacy.get("variant").is_none());
        let parsed = snapshot_from_value(&legacy).expect("旧快照仍可解析");
        assert_eq!(parsed.variant, WbVariant::Cn);

        let now = at_local_date(0, 12);
        let stats = build_statistics(
            &[
                json!({
                    "ts": now - 60_000,
                    "accountId": "account-1",
                    "accountName": "one@example.com",
                    "total": 100.0,
                    "remaining": 100.0,
                }),
                legacy,
            ],
            &[],
            &[],
            now,
        );
        assert_eq!(stats["summary"]["usageToday"], 10.0);
        assert_eq!(stats["accounts"][0]["variant"], "cn");
        assert_eq!(stats["events"][0]["variant"], "cn");
    }

    /// 新快照带档位：账号行与事件都能区分国内版/国际版。
    #[test]
    fn snapshots_and_events_carry_variant() {
        let now = at_local_date(0, 12);
        let stats = build_statistics(
            &[
                snap_variant(now - 60_000, 100.0, WbVariant::Ai),
                snap_variant(now, 80.0, WbVariant::Ai),
            ],
            &[],
            &[json!({"id": "account-1", "variant": "ai", "email": "one@example.com"})],
            now,
        );

        assert_eq!(stats["summary"]["usageToday"], 20.0);
        let account = stats["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["accountId"] == "account-1")
            .expect("account row");
        assert_eq!(account["variant"], "ai");
        assert_eq!(stats["events"][0]["kind"], "usage");
        assert_eq!(stats["events"][0]["variant"], "ai");
    }

    /// 账号库里的档位优先于旧快照的默认国内版解释。
    #[test]
    fn account_variant_wins_over_legacy_snapshot_default() {
        let now = at_local_date(0, 12);
        let stats = build_statistics(
            &[json!({
                "ts": now,
                "accountId": "ai-1",
                "accountName": "ai@example.com",
                "total": 10.0,
                "remaining": 10.0,
            })],
            &[json!({
                "ts": now,
                "accountId": "ai-1",
                "email": "ai@example.com",
                "result": "success",
            })],
            &[json!({"id": "ai-1", "variant": "ai"})],
            now,
        );
        let account = stats["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["accountId"] == "ai-1")
            .expect("account row");
        assert_eq!(account["variant"], "ai");
        assert_eq!(stats["events"][0]["variant"], "ai");
    }

    #[test]
    fn first_observation_does_not_create_usage() {
        let now = at_local_date(0, 12);
        let stats = build_statistics(&[snap(now, 100.0)], &[], &[], now);

        assert_eq!(stats["summary"]["usageToday"], 0.0);
        assert_eq!(stats["daily"][0]["usage"], 0.0);
    }

    #[test]
    fn positive_decreases_are_assigned_to_the_newer_local_day() {
        let now = at_local_date(0, 12);
        let yesterday = at_local_date(1, 12);
        let stats = build_statistics(&[snap(yesterday, 100.0), snap(now, 70.0)], &[], &[], now);

        assert_eq!(stats["summary"]["usageToday"], 30.0);
        assert_eq!(stats["summary"]["usage7Days"], 30.0);
        assert_eq!(
            stats["daily"].as_array().unwrap().last().unwrap()["usage"],
            30.0
        );
        assert_eq!(stats["events"][0]["kind"], "usage");
        assert_eq!(stats["events"][0]["amount"], 30.0);
    }

    #[test]
    fn increases_reset_the_baseline_without_negative_usage() {
        let now = at_local_date(0, 12);
        let earlier = at_local_date(2, 12);
        let yesterday = at_local_date(1, 12);
        let stats = build_statistics(
            &[
                snap(earlier, 100.0),
                snap(yesterday, 125.0),
                snap(now, 115.0),
            ],
            &[],
            &[],
            now,
        );

        assert_eq!(stats["summary"]["usageToday"], 10.0);
        assert_eq!(stats["summary"]["usage7Days"], 10.0);
        assert_eq!(stats["events"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn retention_and_month_windows_use_local_calendar_dates() {
        let now = at_local_date(0, 12);
        let old = at_local_date(CREDIT_SNAPSHOT_RETENTION_DAYS + 1, 12);
        let stats = build_statistics(&[snap(old, 100.0), snap(now, 80.0)], &[], &[], now);

        assert_eq!(stats["summary"]["usage7Days"], 0.0);
        assert_eq!(stats["summary"]["usageThisMonth"], 0.0);
        assert_eq!(stats["coverageStartAt"], now);
    }

    #[test]
    fn checkin_events_are_separate_from_credit_usage() {
        let now = at_local_date(0, 12);
        let logs = vec![json!({
            "ts": now,
            "accountId": "account-1",
            "email": "one@example.com",
            "result": "success",
        })];
        let stats = build_statistics(
            &[snap(now - 60_000, 100.0), snap(now, 90.0)],
            &logs,
            &[],
            now,
        );

        assert_eq!(stats["summary"]["usageToday"], 10.0);
        assert_eq!(stats["summary"]["todayCheckedInAccounts"], 1);
        assert_eq!(stats["events"].as_array().unwrap().len(), 2);
        let event_kinds: Vec<&str> = stats["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].as_str())
            .collect();
        assert!(event_kinds.contains(&"checkin"));
        assert!(event_kinds.contains(&"usage"));
    }

    #[test]
    fn checkins_without_identity_are_kept_as_events_but_not_counted_as_accounts() {
        let now = at_local_date(0, 12);
        let logs = vec![json!({
            "ts": now,
            "result": "success",
        })];
        let stats = build_statistics(&[], &logs, &[], now);

        assert_eq!(stats["summary"]["todayCheckedInAccounts"], 0);
        assert_eq!(stats["events"][0]["kind"], "checkin");
    }

    #[test]
    fn historical_only_accounts_do_not_look_current() {
        let now = at_local_date(0, 12);
        let stats = build_statistics(
            &[snap(now - 60_000, 100.0), snap(now, 80.0)],
            &[],
            &[json!({"id": "current-account"})],
            now,
        );

        assert_eq!(stats["summary"]["currentRemaining"], 0.0);
        let historical = stats["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["accountId"] == "account-1")
            .expect("historical account summary");
        assert_eq!(historical["isCurrent"], false);
        assert!(historical["currentRemaining"].is_null());
        assert_eq!(historical["lastSnapshotAt"], now);
    }

    #[test]
    fn snapshot_normalization_filters_old_values_and_caps_records() {
        let now = at_local_date(0, 12);
        let old = snap(
            now - (CREDIT_SNAPSHOT_RETENTION_DAYS + 1) * 24 * 3600 * 1000,
            100.0,
        );
        let mut values = vec![old];
        values.extend((0..(CREDIT_SNAPSHOT_MAX_RECORDS + 1)).map(|index| {
            snap(
                now - (CREDIT_SNAPSHOT_MAX_RECORDS as i64 - index as i64) * 1_000,
                100.0,
            )
        }));

        let normalized = normalize_snapshots(&values, now);

        assert_eq!(normalized.len(), CREDIT_SNAPSHOT_MAX_RECORDS);
        assert!(normalized.iter().all(|value| {
            snapshot_from_value(value)
                .map(|snapshot| {
                    snapshot.ts >= now - CREDIT_SNAPSHOT_RETENTION_DAYS * 24 * 3600 * 1000
                })
                .unwrap_or(false)
        }));
    }

    #[test]
    fn same_value_is_suppressed_only_inside_the_short_window() {
        let now = at_local_date(0, 12);
        let previous = snap(now - CREDIT_SNAPSHOT_DEDUPE_WINDOW_MS - 1, 100.0);
        assert!(!should_suppress_duplicate(
            std::slice::from_ref(&previous),
            "account-1",
            100.0,
            100.0,
            now,
        ));
        assert!(should_suppress_duplicate(
            &[snap(now - CREDIT_SNAPSHOT_DEDUPE_WINDOW_MS + 1, 100.0)],
            "account-1",
            100.0,
            100.0,
            now,
        ));
    }
