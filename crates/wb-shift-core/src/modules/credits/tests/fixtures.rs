    /// 回归 issue #94：信封凭据在 authenticated_post 入口短路，不发空 Bearer，
    /// 也不会进入刷新重试链路。
    #[tokio::test]
    async fn envelope_credentials_short_circuit_before_request() {
        let account = json!({
            "id": "envelope-only",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let resp = authenticated_post(&account, "https://example.invalid/api", json!({})).await;
        assert_eq!(resp["code"], -2);
        let msg = resp["message"].as_str().expect("message 应为字符串");
        assert!(msg.contains("信封"), "错误文案应可读：{msg}");
    }

    /// 回归 issue #94 遗留：积分查询入口（新三路接口与旧接口回退的唯一入口）
    /// 也要对加密信封凭据短路，不能空跑请求后回显网关 401 文案。
    #[tokio::test]
    async fn envelope_credentials_short_circuit_in_credit_query() {
        let account = json!({
            "id": "envelope-only",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let resp = get_credit_expiry(&account).await;
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["accountId"], "envelope-only");
        let msg = resp["error"].as_str().expect("error 应为字符串");
        assert!(msg.contains("信封"), "错误文案应可读：{msg}");
    }
