    /// 回归 issue #94：信封凭据的签到请求应在入口短路并返回可读错误，
    /// 不发出空 Bearer（此前会被网关 401 后把 HTML 原样回显）。
    #[tokio::test]
    async fn envelope_credentials_short_circuit_before_request() {
        let account = json!({
            "id": "envelope-only",
            "variant": "cn",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let resp = checkin_request_once("/whatever", &account, WbVariant::Cn).await;
        assert_eq!(resp["code"], -2);
        let msg = resp["message"].as_str().expect("message 应为字符串");
        assert!(msg.contains("信封"), "错误文案应可读：{msg}");
    }

    #[tokio::test]
    async fn excluded_account_never_starts_passive_operation() {
        let account = json!({"id": "excluded", "variant": "cn"});
        let cfg = json!({"excluded_account_ids": ["excluded"]});
        let result = with_auto_checkin_preference(&account, &cfg, async {
            panic!("excluded account must not query status, refresh credentials or submit checkin")
        })
        .await;
        assert_eq!(result["result"], "skipped");
        assert_eq!(result["reason"], "auto_checkin_disabled");
        assert_eq!(result["ok"], false);
        assert!(result.get("todayCheckedIn").is_none());
    }

    #[tokio::test]
    async fn reenabled_account_still_runs() {
        let account = json!({"id": "excluded", "variant": "cn"});
        for cfg in [json!({"excluded_account_ids": []}), json!({})] {
            let mut calls = 0;
            let result = with_auto_checkin_preference(&account, &cfg, async {
                calls += 1;
                json!({"result": "success"})
            })
            .await;
            assert_eq!(calls, 1);
            assert_eq!(result["result"], "success");
        }
    }

    #[tokio::test]
    async fn checkin_all_reports_skips_without_running_excluded_accounts() {
        // 全部账号都在名单里：批量路径不发起任何请求，逐账号返回 skipped 原因。
        let accounts = vec![
            json!({"id": "a", "variant": "cn"}),
            json!({"id": "c", "variant": "cn"}),
        ];
        let cfg = json!({"excluded_account_ids": ["a", "c", "unknown"]});
        let rows = checkin_all_rows(accounts, &cfg).await;
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row["result"], "skipped");
            assert_eq!(row["reason"], "auto_checkin_disabled");
        }
        assert_eq!(rows[0]["accountId"], "a");
        assert_eq!(rows[1]["accountId"], "c");
    }
