    #[test]
    fn parses_cockpit_resource_shape_and_marks_expiry() {
        let now = 1_800_000_000_000_i64;
        let resource = resource_summary(
            &json!({
                "PackageCode": "TCACA_code_007_nzdH5h4Nl0",
                "PackageName": "活动赠送包",
                "CycleCapacitySizePrecise": "100.5",
                "CycleCapacityRemainPrecise": "75.25",
                "DeductionEndTime": now + 2 * 24 * 3600 * 1000,
                "Status": 0,
            }),
            now,
        );

        assert_eq!(resource["packageName"], "活动赠送包");
        assert_eq!(resource["total"], 100.5);
        assert_eq!(resource["remaining"], 75.25);
        assert_eq!(resource["used"], 25.25);
        assert_eq!(resource["expiringSoon"], true);
        assert_eq!(resource["expired"], false);
    }

    #[test]
    fn parses_second_millisecond_and_datetime_timestamps() {
        assert_eq!(
            parse_timestamp_ms(Some(&json!(1_800_000_000))),
            Some(1_800_000_000_000)
        );
        assert_eq!(
            parse_timestamp_ms(Some(&json!(1_800_000_000_000_i64))),
            Some(1_800_000_000_000)
        );
        assert!(parse_timestamp_ms(Some(&json!("2099-01-02 03:04:05"))).is_some());
    }

    fn local_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
        Local.with_ymd_and_hms(year, month, day, hour, minute, second)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn start_at_uses_priority_and_falls_back_from_invalid_values() {
        let now = 1_800_000_000_000_i64;
        let cycle = local_ms(2026, 11, 1, 0, 0, 0);
        let resource = resource_summary(&json!({
            "CycleStartTime": "2026-11-01 00:00:00",
            "DeductionStartTime": now - 3_600_000,
            "CreateTime": now - 7_200_000,
        }), now);
        assert_eq!(resource["startAt"].as_i64(), Some(cycle));

        let resource = resource_summary(&json!({
            "CycleStartTime": "invalid", "cycleStartTime": "",
            "DeductionStartTime": now - 3_600_000, "CreateTime": now - 7_200_000,
        }), now);
        assert_eq!(resource["startAt"].as_i64(), Some(now - 3_600_000));
        let resource = resource_summary(&json!({
            "cycleStartTime": "2026-11-02 03:04:05", "createTime": now,
        }), now);
        assert_eq!(resource["startAt"].as_i64(), Some(local_ms(2026, 11, 2, 3, 4, 5)));
        let resource = resource_summary(&json!({
            "CycleStartTime": null, "DeductionStartTime": "invalid", "createTime": now,
        }), now);
        assert_eq!(resource["startAt"].as_i64(), Some(now));
    }

    #[test]
    fn start_at_parses_seconds_milliseconds_and_date_only_midnight() {
        let now = 1_800_000_000_000_i64;
        assert_eq!(resource_summary(&json!({"CreateTime": 1_800_000_000}), now)["startAt"], json!(now));
        assert_eq!(resource_summary(&json!({"createTime": now}), now)["startAt"], json!(now));
        assert_eq!(
            resource_summary(&json!({"CycleStartTime": "2026-11-01"}), now)["startAt"],
            json!(local_ms(2026, 11, 1, 0, 0, 0))
        );
    }

    #[test]
    fn start_at_is_null_without_a_parseable_source() {
        let now = 1_800_000_000_000_i64;
        assert_eq!(resource_summary(&json!({"PackageCode": "quota"}), now)["startAt"], Value::Null);
        assert_eq!(resource_summary(&json!({
            "CycleStartTime": "", "DeductionStartTime": null, "CreateTime": "invalid",
        }), now)["startAt"], Value::Null);
    }

    #[test]
    fn expiry_date_only_still_uses_end_of_day() {
        assert_eq!(
            parse_timestamp_ms(Some(&json!("2026-11-01"))),
            Some(local_ms(2026, 11, 1, 23, 59, 59))
        );
    }

    #[test]
    fn extracts_nested_accounts() {
        let response = json!({
            "code": 0,
            "data": {"Response": {"Data": {"Accounts": [{"PackageName": "基础包"}]}}}
        });
        let accounts = resource_accounts(&response);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["PackageName"], "基础包");
    }

    #[test]
    fn extracts_new_top_level_accounts_and_packages() {
        let response = json!({
            "code": 0,
            "data": {
                "Accounts": [{"PackageCode": "paid"}],
                "Packages": [{"PackageCode": "summary"}]
            }
        });
        assert_eq!(resource_accounts(&response).len(), 1);
        assert_eq!(resource_accounts(&response)[0]["PackageCode"], "paid");
        assert_eq!(resource_packages(&response).len(), 1);
        assert_eq!(resource_packages(&response)[0]["PackageCode"], "summary");
        assert!(has_resource_accounts(&response));
        assert!(has_resource_packages(&response));
    }

    #[test]
    fn parses_summary_capacity_fields_and_explicit_used_value() {
        let resource = resource_summary(
            &json!({
                "PackageCode": "summary",
                "CycleTotalCapacity": "4485",
                "CycleUsedCapacity": "2156.70999737",
                "CycleRemainCapacity": "2328.29000263",
                "CapacityUnit": "credits"
            }),
            1_800_000_000_000,
        );
        assert_eq!(resource["total"], 4485.0);
        assert_eq!(resource["used"], 2156.70999737);
        assert_eq!(resource["remaining"], 2328.29000263);
        assert_eq!(resource["expireAt"], Value::Null);
    }

    #[test]
    fn keeps_detail_batches_and_only_fills_missing_summary_packages() {
        let summary = vec![
            resource_summary(
                &json!({
                    "PackageCode": "activity",
                    "CycleTotalCapacity": 100,
                    "CycleRemainCapacity": 80
                }),
                1_800_000_000_000,
            ),
            resource_summary(
                &json!({
                    "PackageCode": "free",
                    "CycleTotalCapacity": 500,
                    "CycleRemainCapacity": 300
                }),
                1_800_000_000_000,
            ),
        ];
        let details = vec![
            resource_summary(
                &json!({
                    "PackageCode": "activity",
                    "CycleCapacitySizePrecise": "60",
                    "CycleCapacityRemainPrecise": "40",
                    "DeductionEndTime": 1_800_000_100_000_i64
                }),
                1_800_000_000_000,
            ),
            resource_summary(
                &json!({
                    "PackageCode": "activity",
                    "CycleCapacitySizePrecise": "40",
                    "CycleCapacityRemainPrecise": "40",
                    "DeductionEndTime": 1_800_000_200_000_i64
                }),
                1_800_000_000_000,
            ),
        ];
        let merged = merge_resources(summary, details);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["remaining"], 40.0);
        assert_eq!(merged[1]["remaining"], 40.0);
        assert_eq!(merged[2]["packageCode"], "free");
        assert_eq!(merged[2]["remaining"], 300.0);
    }

    #[test]
    fn accepts_empty_detail_accounts_as_a_valid_success() {
        let response = json!({"code": 0, "data": {"Accounts": []}});
        assert!(is_success(&response));
        assert!(has_resource_accounts(&response));
        assert!(resource_accounts(&response).is_empty());
    }

    #[test]
    fn partial_new_success_returns_available_resources() {
        let resources = normalized_new_resources(
            &json!({"code": 500, "message": "summary failed"}),
            &json!({"code": 0, "data": {"Accounts": []}}),
            &json!({
                "code": 0,
                "data": {"data": {"Accounts": [{
                    "PackageCode": "free",
                    "CycleCapacitySizePrecise": "100",
                    "CycleCapacityRemainPrecise": "75"
                }]}}
            }),
            1_800_000_000_000,
        )
        .expect("合法空 paid 和可用 free 明细应视为部分成功");

        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["packageCode"], "free");
        assert_eq!(resources[0]["remaining"], 75.0);
    }

    #[test]
    fn valid_empty_new_arrays_do_not_trigger_legacy_fallback() {
        let resources = normalized_new_resources(
            &json!({"code": 0, "data": {"Packages": []}}),
            &json!({"code": 0, "data": {"Accounts": []}}),
            &json!({"code": 0, "data": {"Accounts": []}}),
            1_800_000_000_000,
        );
        assert_eq!(resources, Some(Vec::new()));
    }

    #[test]
    fn all_invalid_new_responses_require_legacy_fallback() {
        let resources = normalized_new_resources(
            &json!({"code": 500, "message": "summary failed"}),
            &json!({"code": 500, "message": "paid failed"}),
            &json!({"code": 0, "data": {}}),
            1_800_000_000_000,
        );
        assert_eq!(resources, None);
    }

    #[test]
    fn new_request_bodies_match_workbuddy_filters() {
        let paid = paid_packages_body();
        assert_eq!(paid["PageNumber"], 1);
        assert_eq!(paid["PageSize"], 200);
        assert_eq!(paid["Status"], json!([0, 3]));
        assert_eq!(paid["NeedRenewInfo"], true);
        assert!(paid["PackageCodes"]
            .as_array()
            .is_some_and(|codes| codes.iter().any(|code| code == "TCACA_code_038_OhvqZtiPKr")));

        let free = free_packages_body();
        assert_eq!(free["PageNumber"], 1);
        assert_eq!(free["PageSize"], 200);
        assert_eq!(free["Status"], json!([0, 3]));
        assert!(free["SlicePeriodStartTime"].as_str().is_some());
        assert!(free["SlicePeriodEndTime"].as_str().is_some());
        assert!(free["PackageCodes"]
            .as_array()
            .is_some_and(|codes| codes.iter().any(|code| code == "TCACA_code_007_nzdH5h4Nl0")));
        assert!(paid.get("NeedInUsage").is_none());
        assert!(free.get("NeedInUsage").is_none());
    }

    #[test]
    fn selects_endpoint_from_known_account_domain_and_keeps_headers_aligned() {
        let codebuddy = json!({
            "domain": "www.codebuddy.cn",
            "access_token": "redacted",
            "uid": "u1"
        });
        let workbuddy = json!({
            "domain": "www.workbuddy.cn",
            "access_token": "redacted",
            "uid": "u2"
        });
        let unknown = json!({"domain": "attacker.example", "access_token": "redacted"});
        let ai = json!({
            "domain": "www.workbuddy.ai",
            "variant": "ai",
            "access_token": "redacted",
            "uid": "u3"
        });

        assert_eq!(
            format!("{}{}", api_base_for(&codebuddy), RESOURCE_SUMMARY_PATH),
            "https://www.codebuddy.cn/billing/meter/get-user-resource-summary"
        );
        assert_eq!(
            format!("{}{}", api_base_for(&workbuddy), RESOURCE_SUMMARY_PATH),
            "https://www.workbuddy.cn/billing/meter/get-user-resource-summary"
        );
        assert_eq!(
            format!("{}{}", api_base_for(&unknown), RESOURCE_SUMMARY_PATH),
            "https://www.codebuddy.cn/billing/meter/get-user-resource-summary"
        );
        // 国际版固定国际版域，绝不落到国内域。
        assert_eq!(api_base_for(&ai), WbVariant::Ai.api_endpoint());
        assert_eq!(
            format!("{}{}", api_base_for(&ai), RESOURCE_SUMMARY_PATH),
            format!(
                "{}/billing/meter/get-user-resource-summary",
                WbVariant::Ai.api_endpoint()
            )
        );

        let headers = resource_auth_headers(&codebuddy, api_base_for(&codebuddy));
        assert_eq!(
            headers.get("X-Client-Platform").map(String::as_str),
            Some("web")
        );
        assert_eq!(
            headers.get("Accept").map(String::as_str),
            Some("application/json, text/plain, */*")
        );
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer redacted")
        );
        assert_eq!(headers.get("X-User-Id").map(String::as_str), Some("u1"));
        assert_eq!(
            headers.get("X-Domain").map(String::as_str),
            Some("www.codebuddy.cn")
        );
        assert_eq!(
            headers.get("Origin").map(String::as_str),
            Some("https://www.codebuddy.cn")
        );
        assert_eq!(
            headers.get("Referer").map(String::as_str),
            Some("https://www.codebuddy.cn/profile/plans-usage")
        );

        // 国际版账号：Origin/Referer 跟随国际版域，X-Domain 仍用账号自身 domain。
        let ai_headers = resource_auth_headers(&ai, api_base_for(&ai));
        assert_eq!(
            ai_headers.get("Origin").map(String::as_str),
            Some(WbVariant::Ai.api_endpoint())
        );
        assert_eq!(
            ai_headers.get("Referer").map(String::as_str),
            Some(format!("{}/profile/plans-usage", WbVariant::Ai.api_endpoint()).as_str())
        );
        assert_eq!(
            ai_headers.get("X-Domain").map(String::as_str),
            Some("www.workbuddy.ai")
        );

        let workbuddy_headers = resource_auth_headers(&workbuddy, api_base_for(&workbuddy));
        assert_eq!(
            workbuddy_headers.get("Origin").map(String::as_str),
            Some("https://www.workbuddy.cn")
        );
        assert_eq!(
            workbuddy_headers.get("Referer").map(String::as_str),
            Some("https://www.workbuddy.cn/profile/plans-usage")
        );
        assert_eq!(
            workbuddy_headers.get("X-Domain").map(String::as_str),
            Some("www.workbuddy.cn")
        );

        let unknown_headers = resource_auth_headers(&unknown, api_base_for(&unknown));
        assert_eq!(
            unknown_headers.get("Origin").map(String::as_str),
            Some("https://www.codebuddy.cn")
        );
        assert_eq!(
            unknown_headers.get("Referer").map(String::as_str),
            Some("https://www.codebuddy.cn/profile/plans-usage")
        );

        // 官方用量 URL 固定 workbuddy.cn，Origin 必须跟请求 host，X-Domain 仍用账号域。
        let usage_url = "https://www.workbuddy.cn/billing/meter/get-user-request-usage";
        assert_eq!(request_origin(usage_url), WORKBUDDY_WEB_ENDPOINT);
        let usage_headers = resource_auth_headers(&codebuddy, request_origin(usage_url));
        assert_eq!(
            usage_headers.get("Origin").map(String::as_str),
            Some("https://www.workbuddy.cn")
        );
        assert_eq!(
            usage_headers.get("X-Domain").map(String::as_str),
            Some("www.codebuddy.cn")
        );
        assert_eq!(
            request_origin("https://www.codebuddy.cn/v2/billing/meter/get-user-resource"),
            WORKBUDDY_API_ENDPOINT
        );
    }

    /// Origin 必须能识别第三个域；未知域仍回落国内版（既有契约）。
    #[test]
    fn request_origin_covers_ai_domain_and_keeps_fallback() {
        assert_eq!(
            request_origin(&format!(
                "{}/billing/meter/daily-checkin",
                WbVariant::Ai.api_endpoint()
            )),
            WbVariant::Ai.api_endpoint()
        );
        assert_eq!(
            request_origin("https://www.workbuddy.cn/x"),
            WORKBUDDY_WEB_ENDPOINT
        );
        assert_eq!(
            request_origin("https://www.codebuddy.cn/x"),
            WORKBUDDY_API_ENDPOINT
        );
        assert_eq!(request_origin("attacker://x"), WORKBUDDY_API_ENDPOINT);
        assert_eq!(
            request_origin("https://www.workbuddy.ai.evil.com/x"),
            WORKBUDDY_API_ENDPOINT
        );
    }

    /// 路径候选：国际版先 /billing/meter/... 再 /v2/billing/meter/...；国内版单候选。
    #[test]
    fn billing_path_candidates_by_variant() {
        let ai = json!({"variant": "ai", "access_token": "t"});
        let cn = json!({"access_token": "t"});

        assert_eq!(
            variant_of(&ai).billing_paths(RESOURCE_SUMMARY_PATH),
            vec![
                "/billing/meter/get-user-resource-summary",
                "/v2/billing/meter/get-user-resource-summary"
            ]
        );
        assert_eq!(
            variant_of(&cn).billing_paths(RESOURCE_SUMMARY_PATH),
            vec!["/billing/meter/get-user-resource-summary"]
        );

        let legacy = format!("{CHECKIN_API_PREFIX}{USER_RESOURCE_SUFFIX}");
        assert_eq!(
            variant_of(&ai).billing_paths(&legacy),
            vec![
                "/billing/meter/get-user-resource",
                "/v2/billing/meter/get-user-resource"
            ]
        );
        assert_eq!(
            variant_of(&cn).billing_paths(&legacy),
            vec!["/v2/billing/meter/get-user-resource"]
        );
    }

    /// 只有 404 允许回落；401/10085/传输错误必须原样返回。
    #[test]
    fn only_route_missing_allows_fallback() {
        assert!(is_route_missing(&json!({"code": 404})));
        assert!(!is_route_missing(
            &json!({"code": 401, "message": "unauthorized"})
        ));
        assert!(!is_route_missing(&json!({
            "code": 10085,
            "msg": "请求不合法，如有疑问请联系客服"
        })));
        assert!(!is_route_missing(
            &json!({"code": -1, "message": "error sending request"})
        ));
        assert!(!is_route_missing(
            &json!({"code": 0, "data": {"Accounts": []}})
        ));

        // 也复核「不该触发刷新」的三类：401 之外的都不刷新。
        assert!(!is_unauthorized(&json!({
            "code": 10085,
            "msg": "请求不合法，如有疑问请联系客服"
        })));
        assert!(!is_unauthorized(
            &json!({"code": 404, "message": "not found"})
        ));
    }
