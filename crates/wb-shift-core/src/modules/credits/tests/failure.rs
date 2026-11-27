    // 结构化失败透传（需求 A：错误提示可行动化）。
    //
    // 这组用例单独成文件是因为 `tests/resources.rs` / `tests/http.rs` 已经接近 500 行硬线
    // （超线要登记豁免，而它们只是测试集合，拆开比登记更清楚）。全部由 `mod tests` 的
    // `include!` 引入，作用域与其它测试文件相同。

    /// 上游失败带档位时必须原样透传：前端据此给「重新登录 / 检查代理 / 复制诊断」。
    #[test]
    fn credit_failure_result_copies_structured_fields() {
        let account = json!({"id": "acc-1", "nickname": "账号一"});
        let response = json!({
            "code": 401,
            "message": "登录已失效，需要重新登录",
            "failureKind": "login_expired",
            "httpStatus": 401,
        });

        let result = credit_failure_result(&account, &response);
        assert_eq!(result["ok"], false);
        assert_eq!(result["accountId"], "acc-1");
        assert_eq!(
            result[crate::modules::config::FAILURE_KIND_FIELD],
            "login_expired"
        );
        assert_eq!(result[crate::modules::config::HTTP_STATUS_FIELD], 401);
        // 文案一个字都不变：与既有构造器逐字相同。
        let expected = credit_error_result(&account, response_error(&response));
        assert_eq!(result["error"], expected["error"]);
        assert_eq!(result["error"], "登录已失效，需要重新登录");
    }

    /// 没有结构化字段时**不写这些键**：纯本地错误 / 老服务端的形状与改造前逐字段相同。
    #[test]
    fn credit_failure_result_omits_fields_when_absent() {
        let account = json!({"id": "acc-1"});
        let response = json!({"code": 10085, "msg": "请求不合法，如有疑问请联系客服"});

        let result = credit_failure_result(&account, &response);
        assert_eq!(result["ok"], false);
        assert!(result.get("failureKind").is_none(), "不得塞 null 占位");
        assert!(result.get("httpStatus").is_none(), "不得塞 null 占位");
        // 键集与既有构造器完全一致（只多出「有值时才有」的结构化字段）。
        let expected = credit_error_result(&account, response_error(&response));
        assert_eq!(result, expected);
    }

    /// 企业配额失败分支（手里有上游响应）同样透传档位。
    #[test]
    fn enterprise_failure_carries_structured_fields() {
        let account = json!({"id": "acc-9", "nickname": "企业号"});
        let response = json!({
            "code": 502,
            "message": "服务端返回了网页而不是数据，可能是代理或网关拦截",
            "failureKind": "gateway_html",
            "httpStatus": 502,
        });

        let result = enterprise_credit_result(&account, &response, 0);
        assert_eq!(result["ok"], false);
        assert_eq!(result["accountId"], "acc-9");
        assert_eq!(
            result[crate::modules::config::FAILURE_KIND_FIELD],
            "gateway_html"
        );
        assert_eq!(result[crate::modules::config::HTTP_STATUS_FIELD], 502);
    }

    /// 个人版旧接口失败分支（`legacy_credit_result`）同样透传档位，文案不变。
    #[test]
    fn legacy_failure_carries_structured_fields() {
        let account = json!({"id": "acc-legacy", "nickname": "旧接口号"});
        let response = json!({
            "code": -1,
            "message": "返回格式异常（HTTP 502）",
            "failureKind": "malformed",
            "httpStatus": 502,
        });

        let result = legacy_credit_result(&account, &response, 0);
        assert_eq!(result["ok"], false);
        assert_eq!(
            result[crate::modules::config::FAILURE_KIND_FIELD],
            "malformed"
        );
        assert_eq!(result[crate::modules::config::HTTP_STATUS_FIELD], 502);
        assert_eq!(result["error"], "返回格式异常（HTTP 502）");
    }

    /// 成功响应不受影响（走 `credit_result`，不带任何失败字段）。
    #[test]
    fn legacy_success_has_no_failure_fields() {
        let account = json!({"id": "acc-ok"});
        let response = json!({
            "code": 0,
            "data": {"Accounts": [{
                "PackageCode": "TCACA_code_007_nzdH5h4Nl0",
                "PackageName": "活动赠送包",
                "CycleCapacitySizePrecise": "10",
                "CycleCapacityRemainPrecise": "10",
                "DeductionEndTime": 1_800_000_000_000_i64,
                "Status": 0,
            }]},
        });

        let result = legacy_credit_result(&account, &response, 1_800_000_000_000);
        assert_eq!(result["ok"], true);
        assert!(result.get("failureKind").is_none());
        assert!(result.get("httpStatus").is_none());
    }
