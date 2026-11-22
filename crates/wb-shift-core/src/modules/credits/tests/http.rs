    #[test]
    fn transport_error_is_code_minus_one_with_message() {
        assert!(is_transport_error(&json!({
            "code": -1,
            "message": "error sending request for url (https://www.workbuddy.cn/billing/meter/get-user-resource-summary)"
        })));
        assert!(!is_transport_error(&json!({"code": -1, "message": ""})));
        assert!(!is_transport_error(&json!({"code": -1, "message": "   "})));
        assert!(!is_transport_error(&json!({"code": -1})));
        assert!(!is_transport_error(&json!({
            "code": 10085,
            "msg": "请求不合法，如有疑问请联系客服"
        })));
        assert!(!is_transport_error(
            &json!({"code": 401, "message": "unauthorized"})
        ));
        assert!(!is_transport_error(&json!({"code": 0, "data": {}})));
        assert!(!is_unauthorized(&json!({
            "code": 10085,
            "msg": "请求不合法，如有疑问请联系客服"
        })));
    }

    #[test]
    fn accepts_object_response_without_code() {
        assert!(is_success(&json!({"data": {"Response": {"Data": {}}}})));
        assert!(is_success(
            &json!({"data": {"Response": {"Data": {"Accounts": []}}}})
        ));
        assert!(is_success(&json!({"code": "0", "data": {}})));
        assert!(!is_success(&json!({"message": "failed"})));
        assert!(!is_success(&json!({"data": {}, "ok": false})));
        assert!(!is_success(&Value::Null));
        assert!(!is_success(&json!({"code": 500, "message": "failed"})));
    }

    #[test]
    fn sums_only_resources_that_are_expiring_soon() {
        let now = 1_800_000_000_000_i64;
        let resources = [
            resource_summary(
                &json!({
                    "CycleCapacityRemainPrecise": 80,
                    "DeductionEndTime": now + 2 * 24 * 3600 * 1000,
                }),
                now,
            ),
            resource_summary(
                &json!({
                    "CycleCapacityRemainPrecise": 20,
                    "DeductionEndTime": now + 20 * 24 * 3600 * 1000,
                }),
                now,
            ),
        ];
        let expiring: f64 = resources
            .iter()
            .filter(|resource| resource["expiringSoon"] == true)
            .map(|resource| resource["remaining"].as_f64().unwrap())
            .sum();
        assert_eq!(expiring, 80.0);
    }

    #[test]
    fn prefers_cycle_end_when_deduction_end_is_a_far_placeholder() {
        // 035：DeductionEndTime=2049 占位，CycleEndTime=月底 → 取月底
        let now = 1_800_000_000_000_i64;
        let cycle_end = now + 14 * 24 * 3600 * 1000;
        let resource = resource_summary(
            &json!({
                "PackageCode": "TCACA_code_035_ArVxJcGDsm",
                "CycleTotalCapacity": 100,
                "CycleRemainCapacity": 80,
                "DeductionEndTime": 2_049_792_688_000_i64,
                "CycleEndTime": cycle_end,
            }),
            now,
        );
        assert_eq!(resource["expireAt"].as_i64(), Some(cycle_end));
        assert_eq!(resource["expired"], false);
        assert_eq!(resource["expiringSoon"], false);
    }

    #[test]
    fn uses_deduction_end_when_only_real_expiry_is_present() {
        // 006：只有 DeductionEndTime=14 天后 → 取 DeductionEndTime
        let now = 1_800_000_000_000_i64;
        let deduction_end = now + 14 * 24 * 3600 * 1000;
        let resource = resource_summary(
            &json!({
                "PackageCode": "TCACA_code_006_DbXS0lrypC",
                "CycleTotalCapacity": 250,
                "CycleRemainCapacity": 250,
                "DeductionEndTime": deduction_end,
            }),
            now,
        );
        assert_eq!(resource["expireAt"].as_i64(), Some(deduction_end));
    }

    #[test]
    fn keeps_subscription_deduction_end_when_cycle_is_within_override_window() {
        // 订阅型：两者相差 14 天 → 取 DeductionEndTime（回归保护）
        let now = 1_800_000_000_000_i64;
        let cycle_end = now + 16 * 24 * 3600 * 1000;
        let deduction_end = cycle_end + 14 * 24 * 3600 * 1000;
        let resource = resource_summary(
            &json!({
                "PackageCode": "TCACA_code_002_AkiJS3ZHF5",
                "CycleCapacitySizePrecise": "500",
                "CycleCapacityRemainPrecise": "400",
                "DeductionEndTime": deduction_end,
                "CycleEndTime": cycle_end,
            }),
            now,
        );
        assert_eq!(resource["expireAt"].as_i64(), Some(deduction_end));
    }

    #[test]
    fn drops_far_future_placeholder_when_no_cycle_end() {
        // 远期单值：只有 DeductionEndTime=2049 → expireAt 为 null
        let now = 1_800_000_000_000_i64;
        let resource = resource_summary(
            &json!({
                "PackageCode": "placeholder",
                "CycleTotalCapacity": 100,
                "CycleRemainCapacity": 100,
                "DeductionEndTime": 2_049_792_688_000_i64,
            }),
            now,
        );
        assert_eq!(resource["expireAt"], Value::Null);
        assert_eq!(resource["expired"], false);
        assert_eq!(resource["expiringSoon"], false);
    }

    #[test]
    fn package_code_tables_include_intl_codes() {
        let paid: HashSet<&str> = PAID_PACKAGE_CODES.iter().copied().collect();
        let free: HashSet<&str> = FREE_PACKAGE_CODES.iter().copied().collect();
        for code in ["TCACA_code_003_FAnt7lcmRT", "TCACA_code_036_lupO5WgNdG"] {
            assert!(paid.contains(code), "missing paid code {code}");
        }
        for code in [
            "TCACA_code_001_PqouKr6QWV",
            "TCACA_code_006_DbXS0lrypC",
            "TCACA_code_035_ArVxJcGDsm",
            "TCACA_code_037_WxOD3MpI2o",
            "TCACA_code_039_KRcQj7wUat",
            "TCACA_code_040_mi9rCYg46x",
        ] {
            assert!(free.contains(code), "missing free code {code}");
        }
    }

    #[test]
    fn parses_string_cycle_end_from_official_detail_payload() {
        // 官方明细响应里 CycleEndTime 是 "YYYY-MM-DD HH:mm:ss" 字符串（035 的真实形态），
        // 与 2049 的 DeductionEndTime 并存时取字符串周期时间。
        let now = Local
            .with_ymd_and_hms(2026, 9, 16, 12, 0, 0)
            .unwrap()
            .timestamp_millis();
        let expected_cycle_end = Local
            .with_ymd_and_hms(2026, 9, 30, 23, 59, 59)
            .unwrap()
            .timestamp_millis();
        let resource = resource_summary(
            &json!({
                "PackageCode": "TCACA_code_035_ArVxJcGDsm",
                "PackageName": "Free Plan Subscription",
                "CapacitySize": 100,
                "CapacityRemain": 100,
                "CycleCapacitySize": 100,
                "CycleCapacityRemain": 100,
                "CycleEndTime": "2026-09-30 23:59:59",
                "DeductionEndTime": 2_049_792_688_000_i64,
                "ExpiredTime": "",
            }),
            now,
        );
        assert_eq!(resource["expireAt"].as_i64(), Some(expected_cycle_end));
        assert_eq!(resource["expired"], false);
        assert_eq!(resource["expiringSoon"], false);
    }

    #[test]
    fn three_endpoint_query_only_for_domestic_variant() {
        // 国际版没有 summary/paid/free 三路（官网只用 get-user-resource），
        // 发三路等于 3×2 个 404，故只对国内版启用。
        assert!(uses_three_endpoint_query(WbVariant::Cn));
        assert!(!uses_three_endpoint_query(WbVariant::Ai));
    }

    // -----------------------------------------------------------------------
    // 企业版分流（issue #167）
    // -----------------------------------------------------------------------

    #[test]
    fn parses_enterprise_usage_wrappers_and_field_cases() {
        // {data:{…}} 形态 + camelCase + 本地时间字符串
        let expected = Local
            .with_ymd_and_hms(2026, 11, 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis();
        let usage = parse_enterprise_usage(&json!({
            "code": 0,
            "data": {"credit": 500, "limitNum": 2000, "cycleResetTime": "2026-11-01 00:00:00"}
        }))
        .expect("正常企业响应应解析成功");
        assert_eq!(usage.limit_num, 2000.0);
        assert_eq!(usage.used, 500.0);
        assert_eq!(usage.cycle_reset_at, Some(expected));

        // {data:{data:{…}}} 形态 + snake_case；重置时间缺失置 None
        let usage = parse_enterprise_usage(&json!({
            "data": {"data": {"used_num": "12.5", "limit_num": "100"}}
        }))
        .expect("嵌套包装与 snake_case 应兼容");
        assert_eq!(usage.limit_num, 100.0);
        assert_eq!(usage.used, 12.5);
        assert_eq!(usage.cycle_reset_at, None, "缺重置时间不阻塞展示");

        // 字段直接位于响应顶层
        let usage =
            parse_enterprise_usage(&json!({"limitNum": 50, "credit": 10})).expect("顶层字段应兼容");
        assert_eq!(usage.limit_num, 50.0);
        assert_eq!(usage.used, 10.0);

        // CycleResetTime（PascalCase）与非法字符串：解析失败置 None，不阻塞展示
        let usage = parse_enterprise_usage(&json!({
            "data": {
                "credit": 1,
                "limitNum": 10,
                "CycleResetTime": "2026-11-01 00:00:00"
            }
        }))
        .expect("PascalCase 重置时间字段应兼容");
        assert_eq!(usage.cycle_reset_at, Some(expected));

        let usage = parse_enterprise_usage(&json!({
            "data": {"credit": 1, "limitNum": 10, "cycleResetTime": "not-a-date"}
        }))
        .expect("重置时间解析失败不阻塞展示");
        assert_eq!(usage.cycle_reset_at, None);
        assert_eq!(usage.limit_num, 10.0);
    }

    #[test]
    fn parses_enterprise_usage_unlimited_marker() {
        let usage = parse_enterprise_usage(&json!({
            "code": 0,
            "data": {"credit": 0, "limitNum": -1}
        }))
        .expect("limitNum=-1 应解析为不限量");
        assert_eq!(usage.limit_num, -1.0);
    }

    #[test]
    fn parse_enterprise_usage_rejects_malformed_payloads() {
        assert!(parse_enterprise_usage(&Value::Null).is_none());
        assert!(parse_enterprise_usage(&json!({"code": 0, "data": {}})).is_none());
        assert!(parse_enterprise_usage(&json!({"data": {"limitNum": "abc"}})).is_none());
        assert!(
            parse_enterprise_usage(&json!({"data": {"limitNum": "inf"}})).is_none(),
            "非有限数字应判解析失败"
        );
        assert!(parse_enterprise_usage(&json!({"data": {"limitNum": null}})).is_none());
        // 字符串数字仍可解析
        assert_eq!(
            parse_enterprise_usage(&json!({"data": {"limitNum": "2000"}})).map(|u| u.limit_num),
            Some(2000.0)
        );
    }

    #[test]
    fn enterprise_limited_result_maps_to_single_quota_resource() {
        let now = 1_800_000_000_000_i64;
        let cycle_reset_at = now + 30 * 24 * 3600 * 1000;
        // 不带 id：跳过积分快照落盘（单测不写用户数据目录）。
        let account = json!({"uid": "u-1", "enterpriseId": "e-1"});
        let result = enterprise_credit_result(
            &account,
            &json!({
                "code": 0,
                "data": {"credit": 500, "limitNum": 2000, "cycleResetTime": cycle_reset_at}
            }),
            now,
        );

        assert_eq!(result["ok"], true);
        assert_eq!(result["unlimited"], false);
        assert_eq!(result["totalCapacity"], 2000.0);
        assert_eq!(result["totalRemaining"], 1500.0);
        assert_eq!(result["soonestExpireAt"].as_i64(), Some(cycle_reset_at));
        assert_eq!(result["expiringSoon"], false);
        assert_eq!(result["expired"], false);
        let resources = result["resources"].as_array().expect("resources 应为数组");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["packageName"], "企业配额");
        assert_eq!(resources[0]["total"], 2000.0);
        assert_eq!(resources[0]["remaining"], 1500.0);
        assert_eq!(resources[0]["used"], 500.0);
        assert_eq!(resources[0]["expireAt"].as_i64(), Some(cycle_reset_at));
    }

    #[test]
    fn enterprise_unlimited_result_uses_null_balance_and_empty_resources() {
        let account = json!({"uid": "u-1", "enterpriseId": "e-1"});
        let result = enterprise_credit_result(
            &account,
            &json!({"code": 0, "data": {"credit": 0, "limitNum": -1}}),
            1_800_000_000_000,
        );

        assert_eq!(result["ok"], true);
        assert_eq!(result["unlimited"], true);
        assert!(result["totalCapacity"].is_null(), "不限量不把 null 当 0");
        assert!(result["totalRemaining"].is_null());
        assert_eq!(result["soonestExpireAt"], Value::Null);
        assert_eq!(result["resources"], json!([]));
    }

    #[test]
    fn enterprise_exhausted_quota_clamps_remaining_to_zero() {
        let result = enterprise_credit_result(
            &json!({"uid": "u-1"}),
            &json!({"code": 0, "data": {"credit": 2500, "limitNum": 2000}}),
            1_800_000_000_000,
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["unlimited"], false);
        assert_eq!(result["totalRemaining"], 0.0);
        assert_eq!(result["resources"][0]["remaining"], 0.0);
        assert_eq!(result["resources"][0]["packageName"], "企业配额");
    }

    #[test]
    fn enterprise_zero_limit_is_limited_quota_not_unlimited() {
        // limitNum=0 是有限配额用尽，不是解析失败，也不是不限量。
        let usage = parse_enterprise_usage(&json!({"data": {"credit": 0, "limitNum": 0}}))
            .expect("limitNum=0 应解析为有限配额");
        assert_eq!(usage.limit_num, 0.0);

        let result = enterprise_credit_result(
            &json!({"uid": "u-1"}),
            &json!({"code": 0, "data": {"credit": 0, "limitNum": 0}}),
            1_800_000_000_000,
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["unlimited"], false);
        assert_eq!(result["totalCapacity"], 0.0);
        assert_eq!(result["totalRemaining"], 0.0);
        assert_eq!(
            result["resources"].as_array().map(|rows| rows.len()),
            Some(1)
        );
        assert_eq!(result["resources"][0]["packageName"], "企业配额");
        assert_eq!(result["resources"][0]["remaining"], 0.0);
    }

    #[test]
    fn enterprise_error_and_malformed_responses_return_readable_errors() {
        let account = json!({"id": "acc-1", "enterpriseId": "e-1"});
        let upstream = enterprise_credit_result(
            &account,
            &json!({"code": 500, "message": "企业配额服务异常"}),
            1_800_000_000_000,
        );
        assert_eq!(upstream["ok"], false);
        assert_eq!(upstream["accountId"], "acc-1");
        assert_eq!(upstream["error"], "企业配额服务异常");

        // 成功但 payload 不可解析：给出可读错误，而不是空态或 0。
        let malformed =
            enterprise_credit_result(&account, &json!({"code": 0, "data": {}}), 1_800_000_000_000);
        assert_eq!(malformed["ok"], false);
        let message = malformed["error"].as_str().expect("error 应为字符串");
        assert!(message.contains("企业配额"), "错误文案应可读：{message}");
    }

    #[test]
    fn resolves_enterprise_id_from_console_accounts_payload() {
        let hit = json!({
            "code": 0,
            "data": {"accounts": [{"uid": "u-1", "enterpriseId": "e-9"}, {"enterpriseId": "e-2"}]}
        });
        assert_eq!(enterprise_id_from_accounts(&hit), Some("e-9".to_string()));

        // 大写容器名与 snake_case 字段兜底
        assert_eq!(
            enterprise_id_from_accounts(
                &json!({"code": 0, "data": {"Accounts": [{"enterprise_id": "e-3"}]}})
            ),
            Some("e-3".to_string())
        );
        // 空列表 / 无企业 ID / 失败响应都返回 None（补拉失败走可读错误）
        assert_eq!(
            enterprise_id_from_accounts(&json!({"code": 0, "data": {"accounts": []}})),
            None
        );
        assert_eq!(
            enterprise_id_from_accounts(
                &json!({"code": 0, "data": {"accounts": [{"uid": "u-1"}]}})
            ),
            None
        );
        assert_eq!(
            enterprise_id_from_accounts(&json!({"code": 401, "message": "unauthorized"})),
            None
        );
    }

    /// 用户可见文案的硬约束（issue #189）：不含 `<html` / `<!DOCTYPE` / 任何标签。
    fn assert_no_raw_html(text: &str) {
        let lower = text.to_ascii_lowercase();
        assert!(!lower.contains("<html"), "积分错误不得含 <html：{text}");
        assert!(
            !lower.contains("<!doctype"),
            "积分错误不得含 <!DOCTYPE：{text}"
        );
        assert!(!text.contains('<'), "积分错误不得含标签：{text}");
        assert!(!text.contains('>'), "积分错误不得含标签：{text}");
    }

    /// 真实取数路径回归：网关返回整页 HTML 时，账号卡片的积分位显示人话而不是网页。
    ///
    /// 链路：`http_request` 把非 JSON 响应分类成结构化错误 → `legacy_credit_result`
    /// 取 `message` 作为 `error` → `credit-section.tsx` 直接渲染该字段。
    #[test]
    fn credit_error_from_html_page_is_readable_not_raw_body() {
        let account = json!({"id": "acc-credit-html", "uid": "u-html", "nickname": "示例"});
        let now = 1_700_000_000_000i64;

        // 401 + 官方登录页：告诉用户去重新登录。
        let login_page = crate::modules::config::failure_response(
            401,
            "<!DOCTYPE html><html><head><title>登录</title></head><body>扫码登录</body></html>",
        );
        let result = legacy_credit_result(&account, &login_page, now);
        assert_eq!(result["ok"], false);
        let error = result["error"].as_str().unwrap_or_default();
        assert!(error.contains("登录已失效"), "{error}");
        assert_no_raw_html(error);

        // 502 + 网关错误页：指向代理 / 网关，不误导用户去重登。
        let gateway_page = crate::modules::config::failure_response(
            502,
            "<html><head><title>502 Bad Gateway</title></head><body>openresty</body></html>",
        );
        let result = legacy_credit_result(&account, &gateway_page, now);
        let error = result["error"].as_str().unwrap_or_default();
        assert!(error.contains("代理或网关"), "{error}");
        assert_no_raw_html(error);

        // 空响应：只说「格式异常 + 状态码」，正文一个字都不带。
        let empty = crate::modules::config::failure_response(503, "");
        let result = legacy_credit_result(&account, &empty, now);
        let error = result["error"].as_str().unwrap_or_default();
        assert!(error.contains("返回格式异常"), "{error}");
        assert!(error.contains("503"), "{error}");
        assert_no_raw_html(error);
    }

    /// 官方给的人话（JSON `message`）必须逐字保留：分类器不许改写正常错误。
    #[test]
    fn credit_error_keeps_upstream_human_message() {
        let account = json!({"id": "acc-credit-json", "uid": "u-json"});
        let response = crate::modules::config::sanitize_error_payload(
            400,
            json!({"code": 400, "message": "积分包已过期，请重新购买"}),
        );
        let result = legacy_credit_result(&account, &response, 1_700_000_000_000i64);
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"], "积分包已过期，请重新购买");
    }
