    /// 回归 issue #94：信封凭据的旅行请求应在入口短路并返回可读错误，
    /// 不发出空 Bearer。
    #[tokio::test]
    async fn envelope_credentials_short_circuit_before_request() {
        let account = json!({
            "id": "envelope-only",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let resp = travel_request("/whatever", "POST", Some(json!({})), &account).await;
        assert_eq!(resp["code"], -2);
        let msg = resp["message"].as_str().expect("message 应为字符串");
        assert!(msg.contains("信封"), "错误文案应可读：{msg}");
    }

    #[test]
    fn retryable_skips_are_not_terminal() {
        assert!(is_retryable_skip(Some("no-buddy")));
        assert!(is_retryable_skip(Some("error")));
        assert!(is_retryable_skip(Some("config-error")));
        assert!(is_retryable_skip(Some("no-location")));
        assert!(is_retryable_skip(Some("location-unavailable")));
        assert!(is_retryable_skip(Some("status-error")));
        assert!(!is_retryable_skip(Some("daily-limit")));
        assert!(!is_retryable_skip(None));
    }

    #[test]
    fn display_defaults_to_untraveled_when_no_cache() {
        let value = travel_display("no-such-account");
        assert_eq!(value["label"], "untraveled");
    }

    /// 国际版账号：depart 直接短路，不发请求、不写缓存。
    #[tokio::test]
    async fn ai_account_depart_is_skipped_without_request() {
        let ai = json!({"id": "ai-1", "uid": "u-1", "variant": "ai", "access_token": "t"});
        let result = depart_travel_for_account(&ai).await;
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["reason"], "unsupported_variant");
        assert_eq!(result["ok"], Value::Null);

        // 国内版账号（缺档位字段）不会走短路。
        let cn = json!({"id": "cn-1", "uid": "u-2", "access_token": ""});
        assert!(variant_of(&cn).supports_travel());
    }

    /// 档位过滤：国际版账号不进入旅行周期候选。
    #[test]
    fn travel_capable_accounts_exclude_unsupported_variants() {
        let accounts = vec![
            json!({"id": "cn-1"}),
            json!({"id": "ai-1", "variant": "ai"}),
            json!({"id": "ai-2", "domain": "www.workbuddy.ai"}),
        ];
        let kept = travel_capable_accounts(accounts);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["id"], "cn-1");
    }

    #[test]
    fn unsupported_skip_shape_is_stable() {
        assert_eq!(
            unsupported_variant_skip(),
            json!({"status": "skipped", "reason": "unsupported_variant"})
        );
    }

    #[test]
    fn truncate_message_caps_length() {
        assert_eq!(truncate_message("a"), "a");
        let long: String = "x".repeat(200);
        assert_eq!(truncate_message(&long).chars().count(), 80);
    }

    #[test]
    fn parse_travel_state_accepts_known_case_insensitive() {
        assert_eq!(parse_travel_state(Some("idle")), Some("idle"));
        assert_eq!(parse_travel_state(Some("Arrived")), Some("arrived"));
        assert_eq!(parse_travel_state(Some(" TRAVELING ")), Some("traveling"));
        assert_eq!(parse_travel_state(None), None);
        assert_eq!(parse_travel_state(Some("")), None);
        assert_eq!(parse_travel_state(Some("unknown")), None);
    }

    #[test]
    fn classify_depart_does_not_treat_http_429_as_daily_limit() {
        assert_eq!(
            classify_depart_error(429, "too many requests"),
            DepartClass::Other
        );
        assert_eq!(
            classify_depart_error(429, "daily limit reached"),
            DepartClass::DailyLimit
        );
        assert_eq!(
            classify_depart_error(0, "daily_limit_reached"),
            DepartClass::DailyLimit
        );
        assert_eq!(
            classify_depart_error(0, "already traveling"),
            DepartClass::AlreadyTraveling
        );
    }

    #[test]
    fn merge_keeps_prior_credit_when_new_claim_lacks_it() {
        let prior = json!({
            "claimed": true, "rewardCredit": 6, "claimedAt": 100, "state": "idle",
        });
        let new = json!({
            "claimed": true, "rewardCredit": null, "claimedAt": 200,
            "message": "已领取（网页端）",
        });
        let merged = merge_claim_state(&prior, &new);
        assert_eq!(merged["claimed"], true);
        assert_eq!(merged["rewardCredit"], 6);
        assert_eq!(merged["claimedAt"], 100);
        assert_eq!(merged["state"], "idle");
    }

    #[test]
    fn merge_does_not_cover_new_trip_with_old_claim() {
        let prior = json!({
            "claimed": true, "rewardCredit": 8, "claimedAt": 100, "state": "idle",
        });
        let new = json!({
            "ok": true, "already": true, "claimed": false, "rewardCredit": null,
            "claimedAt": 0, "state": "traveling", "message": "已在旅行中",
        });
        let merged = merge_claim_state(&prior, &new);
        assert_eq!(merged["claimed"], false);
        assert_eq!(merged["state"], "traveling");
        assert_eq!(merged["already"], true);
        assert_eq!(merged["rewardCredit"], Value::Null);
    }

    #[test]
    fn merge_keeps_claimed_without_credit() {
        let prior = json!({
            "claimed": true, "rewardCredit": null, "claimedAt": 100, "state": "idle",
        });
        let new = json!({
            "ok": false, "claimed": false, "rewardCredit": null, "state": "idle",
        });
        let merged = merge_claim_state(&prior, &new);
        assert_eq!(merged["claimed"], true);
        assert_eq!(merged["rewardCredit"], Value::Null);
        assert_eq!(merged["state"], "idle");
    }

    #[test]
    fn official_status_decides_whether_to_depart() {
        assert_eq!(decide_travel_action("idle", false), TravelAction::Depart);
        assert_eq!(
            decide_travel_action("idle", true),
            TravelAction::SkipDailyLimit
        );
        assert_eq!(
            decide_travel_action("traveling", true),
            TravelAction::WaitTraveling
        );
        assert_eq!(decide_travel_action("arrived", true), TravelAction::Claim);
    }

    #[test]
    fn merge_keeps_new_credit_over_prior_null() {
        let prior = json!({
            "claimed": true, "rewardCredit": null, "claimedAt": 100,
        });
        let new = json!({
            "claimed": true, "rewardCredit": 7, "claimedAt": 200,
        });
        let merged = merge_claim_state(&prior, &new);
        assert_eq!(merged["rewardCredit"], 7);
        assert_eq!(merged["claimedAt"], 200);
    }

    #[test]
    fn merge_noop_when_prior_unclaimed() {
        let prior = json!({ "claimed": false, "rewardCredit": null });
        let new = json!({ "claimed": false, "rewardCredit": null });
        let merged = merge_claim_state(&prior, &new);
        assert_eq!(merged["claimed"], false);
    }

    #[test]
    fn roll_cache_keeps_in_flight_and_drops_claimed() {
        let mut cache = json!({
            "date": "2026-09-08",
            "completed": true,
            "results": {
                "a": { "ok": true, "claimed": false, "state": "traveling" },
                "b": { "ok": true, "claimed": true, "rewardCredit": 4, "state": "idle" },
                "c": { "ok": false, "skip": "no-buddy" },
            }
        });
        roll_cache_to_today(&mut cache, "2026-09-09");
        assert_eq!(cache["date"], "2026-09-09");
        assert_eq!(cache["completed"], false);
        assert_eq!(cache["results"]["a"]["state"], "traveling");
        assert!(cache["results"].get("b").is_none());
        assert!(cache["results"].get("c").is_none());
    }

    #[test]
    fn in_flight_due_when_arrive_at_passed_or_missing() {
        let traveling = json!({ "ok": true, "claimed": false, "arriveAt": 100 });
        assert!(in_flight_due(&traveling, 100));
        assert!(in_flight_due(&traveling, 101));
        assert!(!in_flight_due(&traveling, 99));
        assert!(in_flight_due(&json!({ "ok": true, "claimed": false }), 1));
        assert!(!in_flight_due(
            &json!({ "ok": true, "claimed": true, "arriveAt": 1 }),
            100
        ));
        let millis = json!({
            "ok": true,
            "claimed": false,
            "arriveAt": 1_788_964_568_000_i64,
        });
        assert!(in_flight_due(&millis, 1_788_964_568));
        assert!(!in_flight_due(&millis, 1_788_964_567));
    }

    #[test]
    fn daily_limit_marks_finished_and_keeps_trip_details() {
        let mut result = json!({
            "ok": true,
            "claimed": false,
            "state": "traveling",
            "message": "已在旅行中",
            "locationName": "咖啡馆",
            "rewardCredit": 9,
            "arriveAt": 1788964568_i64,
        });
        apply_daily_limit_reached(
            &mut result,
            &json!({
                "locationName": "",
                "rewardCredit": 0,
                "arriveAt": 0,
            }),
        );
        assert_eq!(result["claimed"], true);
        assert_eq!(result["skip"], "daily-limit");
        assert_eq!(result["state"], "idle");
        assert_eq!(result["message"], "今日已派");
        assert_eq!(result["locationName"], "咖啡馆");
        assert_eq!(result["rewardCredit"], 9);
        assert_eq!(display_label(true, &result), "finished");
        assert_eq!(
            display_label(
                true,
                &json!({
                    "ok": true,
                    "claimed": false,
                    "state": "traveling",
                    "locationName": "咖啡馆",
                    "rewardCredit": 9,
                })
            ),
            "traveling"
        );
    }
