    #[test]
    fn auto_checkin_excludes_only_matching_ids_and_preserves_legacy_accounts() {
        let accounts = vec![
            json!({"id": "cn-excluded", "email": "shared@example.com"}),
            json!({"id": "cn-allowed", "email": "shared@example.com"}),
            json!({"id": "ai-account", "variant": "ai"}),
            json!({"id": "legacy-account"}),
        ];
        let legacy = auto_checkin_accounts(accounts.clone(), &json!({}));
        assert_eq!(
            legacy,
            vec![
                accounts[0].clone(),
                accounts[1].clone(),
                accounts[3].clone()
            ]
        );

        let cfg = json!({"excluded_account_ids": ["cn-excluded", "deleted-account", "shared@example.com"]});
        let eligible = auto_checkin_accounts(accounts.clone(), &cfg);
        assert_eq!(eligible, vec![accounts[1].clone(), accounts[3].clone()]);
        // 排除设置不影响「是否支持签到」；单账号手动签到不经过名单过滤。
        assert!(variant_of(&accounts[0]).supports_checkin());
        assert!(!allows_auto_checkin(&accounts[0], &cfg));
        assert!(allows_auto_checkin(
            &accounts[0],
            &json!({"excluded_account_ids": []})
        ));
    }

    #[test]
    fn excluded_account_is_not_due_even_after_its_target_time() {
        let (account, target) = due_fixture();
        let cfg = json!({"excluded_account_ids": [account["id"]]});
        let accounts = auto_checkin_accounts(vec![account], &cfg);
        assert!(due_accounts(accounts.clone(), target, Some(TEST_WINDOW), &[]).is_empty());
        assert!(due_accounts(accounts.clone(), target, None, &[]).is_empty());
        // 全部排除时不再为它们的目标时刻安排唤醒。
        assert_eq!(
            plan_next_delay(
                false,
                FAST_RETRY_MAX,
                target - 60_000,
                &accounts,
                Some(TEST_WINDOW)
            ),
            (CHECKIN_RECOVERY_INTERVAL, FAST_RETRY_MAX)
        );
    }

    #[test]
    fn checked_in_status_returns_already_without_submission() {
        assert_eq!(
            decide_from_status(&json!({"ok": true, "todayCheckedIn": true})),
            StatusDecision::Already
        );
    }

    #[test]
    fn failed_status_returns_error_without_submission() {
        assert_eq!(
            decide_from_status(&json!({"ok": false, "error": "offline"})),
            StatusDecision::Error("offline".to_string())
        );
    }

    #[test]
    fn unchecked_status_submits() {
        assert_eq!(
            decide_from_status(&json!({"ok": true, "todayCheckedIn": false})),
            StatusDecision::Submit
        );
    }

    /// 有意扩展：该档位没有状态接口时允许直接提交一次（daily-checkin 自身幂等）。
    #[test]
    fn status_unsupported_variant_submits_once() {
        assert_eq!(
            decide_from_status(&json!({
                "ok": false,
                "statusUnsupported": true,
                "error": "该档位暂无签到状态接口"
            })),
            StatusDecision::Submit
        );
        // 其它失败仍然不进提交流程。
        assert_eq!(
            decide_from_status(&json!({"ok": false, "statusUnsupported": false, "error": "x"})),
            StatusDecision::Error("x".to_string())
        );
    }

    #[test]
    fn inactive_messages_are_recognized() {
        for message in [
            "功能未开启",
            "签到未开放",
            "活动已过期",
            "inactive",
            "Feature not enabled",
            "not available",
        ] {
            assert!(is_inactive_message(message), "{message}");
        }
        assert!(!is_inactive_message("签到成功"));
        assert!(!is_inactive_message("系统繁忙，请稍后重试"));
        assert!(!is_inactive_message(""));
    }

    /// 国际版「未开放 / 已过期」类业务码不得触发 token 刷新与重试（P1-1）。
    ///
    /// `checkin_request_once` 的刷新/重试是唯一会产生副作用的分支，这里直接对
    /// 该分支的前置判定（纯函数）做断言：命中即原样返回，不会走到
    /// `refresh_account_token` + 重发。
    #[test]
    fn ai_inactive_response_skips_token_refresh_and_retry() {
        for resp in [
            json!({"code": 10011, "message": "签到活动已过期"}),
            json!({"code": 1, "msg": "签到未开放"}),
            json!({"code": 1, "message": "Feature not enabled"}),
        ] {
            assert!(
                skips_refresh_before_retry(WbVariant::Ai, &resp),
                "国际版 inactive 响应必须先短路: {resp}"
            );
            // 同一响应确实会被归类为 inactive（两个判定词表一致）。
            let msg = resp
                .get("message")
                .or_else(|| resp.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap();
            assert!(is_inactive_message(msg), "{msg}");
        }

        // 国内版语义逐字不变：「已过期」仍走既有 is_unauthorized 分支（允许刷新重试）。
        let cn_resp = json!({"code": 10011, "message": "签到活动已过期"});
        assert!(!skips_refresh_before_retry(WbVariant::Cn, &cn_resp));
        assert!(is_unauthorized(&cn_resp));

        // 国际版真正的鉴权失败仍然保留「一次刷新 + 一次重试」。
        assert!(!skips_refresh_before_retry(
            WbVariant::Ai,
            &json!({"code": 401, "message": "token 失效"})
        ));
        // 成功响应不短路（幂等「已签到」照常返回）。
        assert!(!skips_refresh_before_retry(
            WbVariant::Ai,
            &json!({"code": 0, "message": "签到成功"})
        ));
    }

    /// 国际版不调用国内版专有的状态接口；国内版保持两次尝试的顺序。
    #[tokio::test]
    async fn ai_account_does_not_call_cn_status_endpoints() {
        let ai = json!({"id": "ai-1", "uid": "u-1", "variant": "ai"});
        let status = get_checkin_status(&ai).await;
        assert_eq!(status["ok"], false);
        assert_eq!(status["statusUnsupported"], true);
        assert!(status.get("raw").is_none());
        // 没有产生任何状态查询响应（未发请求）。
        assert!(status.get("todayCheckedIn").is_none());
    }

    /// 国际版没有签到接口：入口即跳过，绝不发起任何请求。
    ///
    /// 守卫位于 `load_checkin_config` / `ensure_fresh_token` / 状态查询之前，
    /// 因此这里既不会触发 token 刷新，也不会触碰任何签到接口。
    #[tokio::test]
    async fn ai_account_checkin_is_skipped_without_requests() {
        let ai = json!({"id": "ai-skip-checkin", "uid": "u-ai", "variant": "ai"});
        let result = checkin_account(&ai).await;
        assert_eq!(result["result"], "skipped");
        assert_eq!(result["reason"], "unsupported_variant");
        // 不伪装成成功、失败或 inactive。
        assert!(result.get("error").is_none());
        assert!(result.get("inactive").is_none());
    }

    /// 状态成功响应解析保持原有结构（国内版零回归）。
    #[test]
    fn status_from_response_matches_legacy_shape() {
        assert_eq!(status_from_response(&json!({"code": 500})), None);
        let ok = status_from_response(&json!({
            "code": 0,
            "data": {"today_checked_in": true, "extra": 1}
        }))
        .expect("成功响应应解析");
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["todayCheckedIn"], true);
        assert_eq!(ok["raw"]["extra"], 1);

        let snake = status_from_response(&json!({
            "code": 200,
            "data": {"todayCheckedIn": false}
        }))
        .expect("camelCase 也应解析");
        assert_eq!(snake["todayCheckedIn"], false);
    }

    /// 路径候选：国际版先 /billing/meter/... 再回落 /v2/billing/meter/...。
    #[test]
    fn checkin_path_candidates_are_variant_specific() {
        assert_eq!(
            variant_of(&json!({"variant": "ai"}))
                .billing_paths(&format!("{CHECKIN_API_PREFIX}/daily-checkin")),
            vec![
                "/billing/meter/daily-checkin",
                "/v2/billing/meter/daily-checkin"
            ]
        );
        assert_eq!(
            variant_of(&json!({})).billing_paths(&format!("{CHECKIN_API_PREFIX}/daily-checkin")),
            vec!["/v2/billing/meter/daily-checkin"]
        );
    }

    #[test]
    fn same_account_cannot_acquire_two_operation_guards() {
        let account = json!({"id": "checkin-guard-test-account"});
        let first = AccountRunGuard::try_acquire(&account)
            .expect("运行标记可加锁")
            .expect("first operation acquires guard");
        assert!(AccountRunGuard::try_acquire(&account)
            .expect("运行标记可加锁")
            .is_none());
        drop(first);
        assert!(AccountRunGuard::try_acquire(&account)
            .expect("运行标记可加锁")
            .is_some());
    }

    #[tokio::test]
    async fn manual_all_reports_busy_when_cycle_is_running() {
        let _cycle_guard =
            RunFlagGuard::try_acquire(&CHECKIN_RUNNING).expect("test acquires cycle guard");
        let result = run_checkin_all(None, false).await;

        assert_eq!(result["accounts"], json!([]));
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["reason"], "already_running");
    }

    #[test]
    fn is_unauthorized_detects_code() {
        assert!(is_unauthorized(&json!({"code": 401})));
        assert!(is_unauthorized(&json!({"code": 403})));
        assert!(!is_unauthorized(&json!({"code": 0})));
    }

    #[test]
    fn checked_in_today_requires_every_account() {
        let accounts = vec![json!({"id": "a"}), json!({"id": "b"})];
        let logs = vec![
            json!({"accountId": "a", "result": "success", "ts": 1_700_000_000_000_i64}),
            json!({"accountId": "b", "result": "already", "ts": 1_700_000_100_000_i64}),
        ];
        let today = date_str(Some(1_700_000_000_000));
        assert!(accounts_checked_in_today(
            &accounts,
            &logs,
            &today,
            &json!({})
        ));
    }

    #[test]
    fn checked_in_today_false_when_one_failed_last() {
        let accounts = vec![json!({"id": "a"})];
        let logs = vec![
            json!({"accountId": "a", "result": "success", "ts": 1_700_000_000_000_i64}),
            json!({"accountId": "a", "result": "error", "ts": 1_700_000_200_000_i64}),
        ];
        let today = date_str(Some(1_700_000_200_000));
        assert!(!accounts_checked_in_today(
            &accounts,
            &logs,
            &today,
            &json!({})
        ));
    }

    #[test]
    fn checked_in_today_false_when_empty_or_missing() {
        assert!(!accounts_checked_in_today(
            &[],
            &[],
            "2026-08-19",
            &json!({})
        ));
        let accounts = vec![json!({"id": "a"})];
        assert!(!accounts_checked_in_today(
            &accounts,
            &[],
            "2026-08-19",
            &json!({})
        ));
    }

    /// 国际版账号不会有签到日志，不得让托盘永远显示「可签到」。
    #[test]
    fn checked_in_today_ignores_variants_without_checkin() {
        let today = date_str(Some(1_700_000_000_000));
        let logs = vec![json!({
            "accountId": "cn-1",
            "result": "success",
            "ts": 1_700_000_000_000_i64
        })];

        // 仅国际版账号：没有待签到项，不再提示「可签到」。
        let ai_only = vec![json!({"id": "ai-1", "variant": "ai"})];
        assert!(accounts_checked_in_today(&ai_only, &[], &today, &json!({})));

        // 国内版已签 + 国际版无日志：国际版不拖累判定。
        let mixed = vec![
            json!({"id": "cn-1", "variant": "cn"}),
            json!({"id": "ai-1", "variant": "ai"}),
        ];
        assert!(accounts_checked_in_today(&mixed, &logs, &today, &json!({})));

        // 国内版未签 + 国际版无日志：仍需签到。
        let pending_cn = vec![
            json!({"id": "cn-2", "variant": "cn"}),
            json!({"id": "ai-1", "variant": "ai"}),
        ];
        assert!(!accounts_checked_in_today(
            &pending_cn,
            &logs,
            &today,
            &json!({})
        ));
    }

    /// 关闭自动签到的账号同样不参与判定；全部关闭时视为无需签到。
    #[test]
    fn checked_in_today_ignores_excluded_accounts() {
        let today = date_str(Some(1_700_000_000_000));
        let logs = vec![json!({
            "accountId": "cn-1",
            "result": "success",
            "ts": 1_700_000_000_000_i64
        })];

        // 全部关闭：没有待签到项，托盘显示「已签到」。
        let excluded_only = vec![json!({"id": "cn-1", "variant": "cn"})];
        assert!(accounts_checked_in_today(
            &excluded_only,
            &[],
            &today,
            &json!({"excluded_account_ids": ["cn-1"]})
        ));

        // 关闭的账号不参与判定，未关闭的账号未签时仍提示「可签到」。
        let mixed_excluded = vec![
            json!({"id": "cn-1", "variant": "cn"}),
            json!({"id": "cn-2", "variant": "cn"}),
        ];
        assert!(!accounts_checked_in_today(
            &mixed_excluded,
            &logs,
            &today,
            &json!({"excluded_account_ids": ["cn-1"]})
        ));
    }

    #[test]
    fn logs_are_tagged_with_variant_and_legacy_rows_fall_back_to_cn() {
        let accounts = vec![
            json!({"id": "cn-1"}),
            json!({"id": "ai-1", "variant": "ai"}),
        ];
        let logs = vec![
            // 新日志自带档位。
            json!({"accountId": "ai-1", "result": "success", "variant": "ai"}),
            // 历史日志按账号库回填。
            json!({"accountId": "ai-1", "result": "success"}),
            json!({"accountId": "cn-1", "result": "success"}),
            // 账号已删除：按缺省国内版解释。
            json!({"accountId": "gone", "result": "success"}),
            json!({"email": "legacy@example.com", "result": "success"}),
        ];

        let rows = checkin_logs_with_variant(&logs, &accounts);
        assert_eq!(rows.len(), logs.len());
        assert_eq!(rows[0]["variant"], "ai");
        assert_eq!(rows[1]["variant"], "ai");
        assert_eq!(rows[2]["variant"], "cn");
        assert_eq!(rows[3]["variant"], "cn");
        assert_eq!(rows[4]["variant"], "cn");
    }

    // -----------------------------------------------------------------------
    // 签到时间段：抽签 / 窗口 / due 判定 / 延迟
    // -----------------------------------------------------------------------

    /// 设计口径样例窗口：22:00–23:30。
    const TEST_WINDOW: (u32, u32) = (22 * 60, 23 * 60 + 30);

    fn local_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("test timestamp must be unambiguous")
            .timestamp_millis()
    }

    fn log_row(account_id: &str, result: &str, ts: i64) -> Value {
        json!({"accountId": account_id, "email": account_id, "result": result, "ts": ts})
    }

    /// 某天某账号在样例窗口内的目标时刻（ms）。
    fn day_target_ms(day: &str, key: &str) -> i64 {
        target_at(day, target_minute(day, key, TEST_WINDOW.0, TEST_WINDOW.1))
            .expect("目标时刻必须可解析")
    }

    /// 样例窗口的账号与今日目标时刻。
    fn due_fixture() -> (Value, i64) {
        (json!({"id": "due-a"}), day_target_ms("2026-09-21", "due-a"))
    }
