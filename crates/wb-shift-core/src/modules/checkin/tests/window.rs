    /// `LAST_CYCLE_HAD_ERRORS` / `FAST_RETRY_BUDGET` 是进程内全局状态，
    /// 只有本文件的健康度用例会写它，串行化即可。
    static RETRY_STATE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn target_minute_is_stable_inside_window_and_spread_across_accounts_and_days() {
        let (start, end) = TEST_WINDOW;
        let day = "2026-09-21";
        let anchor = target_minute(day, "acc-a", start, end);
        assert!(
            (start..end).contains(&anchor),
            "抽签必须落在窗口内: {anchor}"
        );
        // 同一天跨重启不变（确定性抽签，与进程内状态无关）。
        assert_eq!(anchor, target_minute(day, "acc-a", start, end));
        // 单分钟窗口只能落在该分钟（不 panic、不越界）。
        assert_eq!(target_minute(day, "acc-a", 60, 61), 60);
        assert_eq!(target_minute(day, "acc-a", 1439, 1440), 1439);

        let accounts: HashSet<u32> = ["a", "b", "c", "d", "e", "f", "g", "h"]
            .iter()
            .map(|key| target_minute(day, key, start, end))
            .collect();
        assert!(accounts.len() > 1, "不同账号必须错峰: {accounts:?}");
        assert!(accounts.iter().all(|minute| (start..end).contains(minute)));

        let days: HashSet<u32> = ["2026-09-21", "2026-09-22", "2026-09-23", "2026-09-24"]
            .iter()
            .map(|day| target_minute(day, "acc-a", start, end))
            .collect();
        assert!(days.len() > 1, "不同日期必须重新抽签: {days:?}");
    }

    #[test]
    fn window_minutes_requires_both_bounds_and_start_before_end() {
        // 未设置 / 缺失 / 只填一个 / 非法 / start >= end 一律按不限制处理。
        for cfg in [
            json!({}),
            json!({"checkin_start": "", "checkin_end": ""}),
            json!({"checkin_start": "22:00", "checkin_end": ""}),
            json!({"checkin_start": "", "checkin_end": "23:30"}),
            json!({"checkin_start": "22:00"}),
            json!({"checkin_end": "23:30"}),
            json!({"checkin_start": "22:00", "checkin_end": "25:00"}),
            json!({"checkin_start": "22:00", "checkin_end": "abc"}),
            json!({"checkin_start": 2200, "checkin_end": 2330}),
            json!({"checkin_start": "23:30", "checkin_end": "22:00"}),
            json!({"checkin_start": "22:00", "checkin_end": "22:00"}),
        ] {
            assert_eq!(window_minutes(&cfg), None, "必须按不限制处理: {cfg}");
        }

        assert_eq!(
            window_minutes(&json!({"checkin_start": "22:00", "checkin_end": "23:30"})),
            Some((22 * 60, 23 * 60 + 30))
        );
        // 1–2 位写法同样生效；想签在 0 点后填 00:00–01:00（不支持跨午夜）。
        assert_eq!(
            window_minutes(&json!({"checkin_start": "9:5", "checkin_end": "10:00"})),
            Some((9 * 60 + 5, 10 * 60))
        );
        assert_eq!(
            window_minutes(&json!({"checkin_start": "0:0", "checkin_end": "1:0"})),
            Some((0, 60))
        );
    }

    /// 手动入口窗口判定边界：`[start, end)`，end 整点已在窗口外。
    #[test]
    fn now_in_window_covers_window_boundaries() {
        let win = TEST_WINDOW;
        assert!(
            !now_in_window(local_ms(2026, 9, 21, 21, 59), win),
            "start 前必须判为窗口外"
        );
        assert!(
            now_in_window(local_ms(2026, 9, 21, 22, 0), win),
            "start 整点必须判为窗口内"
        );
        assert!(
            now_in_window(local_ms(2026, 9, 21, 22, 45), win),
            "窗口内必须判为窗口内"
        );
        assert!(
            now_in_window(local_ms(2026, 9, 21, 23, 29), win),
            "end-1 分必须判为窗口内"
        );
        assert!(
            !now_in_window(local_ms(2026, 9, 21, 23, 30), win),
            "end 整点是开区间，必须判为窗口外"
        );
        assert!(
            !now_in_window(local_ms(2026, 9, 21, 23, 31), win),
            "end 后必须判为窗口外"
        );
    }

    /// 手动批量签到的窗口跳过行：仅 `respect_window + 窗口生效 + 窗口外` 才整轮跳过。
    #[test]
    fn manual_window_skip_rows_only_when_respecting_effective_window_outside() {
        let cfg = json!({"checkin_start": "22:00", "checkin_end": "23:30"});
        let accounts = vec![
            json!({"id": "cn-a", "email": "a@example.com"}),
            json!({"id": "cn-b"}),
        ];
        let outside = local_ms(2026, 9, 21, 12, 0);
        let inside = local_ms(2026, 9, 21, 22, 30);

        // respect_window=false：任何时刻都走立即签路径。
        assert!(manual_window_skip_rows(&accounts, &cfg, false, outside).is_none());
        // 窗口未生效（未设置）：不限制，照常立即签。
        assert!(manual_window_skip_rows(&accounts, &json!({}), true, outside).is_none());
        // 窗口生效且在窗口内：照常立即签。
        assert!(manual_window_skip_rows(&accounts, &cfg, true, inside).is_none());

        // 窗口生效且在窗口外：逐账号 skipped 行，整轮不发任何签到请求。
        let rows = manual_window_skip_rows(&accounts, &cfg, true, outside).expect("窗口外必须跳过");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["accountId"], "cn-a");
        assert_eq!(rows[0]["email"], "a@example.com");
        assert_eq!(rows[1]["accountId"], "cn-b");
        for row in &rows {
            assert_eq!(row["result"], "skipped");
            assert_eq!(row["reason"], "outside_checkin_window");
            assert_eq!(row["error"], Value::Null);
        }

        // 空账号列表同样按跳过返回空行集。
        assert_eq!(
            manual_window_skip_rows(&[], &cfg, true, outside),
            Some(vec![])
        );
    }

    #[test]
    fn target_boundaries_are_stable_across_restarts() {
        let (account, target) = due_fixture();
        let yesterday = day_target_ms("2026-09-20", "due-a");
        let tomorrow = day_target_ms("2026-09-22", "due-a");

        // 未到今日目标 → 最近目标是昨日，下一个目标是今日。
        assert_eq!(
            latest_target_ms(target - 1, &account, TEST_WINDOW),
            Some(yesterday)
        );
        assert_eq!(
            next_target_ms(target - 1, &account, TEST_WINDOW),
            Some(target)
        );
        // 已过今日目标 → 最近目标是今日，下一个目标是明日。
        assert_eq!(
            latest_target_ms(target + 1, &account, TEST_WINDOW),
            Some(target)
        );
        assert_eq!(
            next_target_ms(target + 1, &account, TEST_WINDOW),
            Some(tomorrow)
        );
        // 目标本身 = 到点时刻。
        assert_eq!(
            latest_target_ms(target, &account, TEST_WINDOW),
            Some(target)
        );
    }

    /// 常驻场景：窗口内到点前不 due（不发请求）；到点未办即 due。
    #[test]
    fn account_due_waits_for_today_target_then_fires() {
        let (account, target) = due_fixture();
        let answered_yesterday = vec![log_row(
            "due-a",
            "success",
            day_target_ms("2026-09-20", "due-a") + 60_000,
        )];

        assert!(
            !account_due(target - 60_000, &account, TEST_WINDOW, &answered_yesterday),
            "到点前不得发请求"
        );
        assert!(account_due(
            target + 60_000,
            &account,
            TEST_WINDOW,
            &answered_yesterday
        ));

        let mut answered = answered_yesterday;
        answered.push(log_row("due-a", "success", target + 30_000));
        assert!(!account_due(
            target + 60_000,
            &account,
            TEST_WINDOW,
            &answered
        ));
    }

    /// 整夜未运行 / 每天仅在窗口外开机 / 目标已过的新账号 → 立即补一轮。
    #[test]
    fn missed_window_and_new_accounts_are_due_immediately() {
        let (account, target) = due_fixture();
        assert!(account_due(
            local_ms(2026, 9, 22, 8, 0),
            &account,
            TEST_WINDOW,
            &[]
        ));
        assert!(account_due(
            local_ms(2026, 9, 22, 10, 0),
            &account,
            TEST_WINDOW,
            &[]
        ));
        assert!(account_due(target + 60_000, &account, TEST_WINDOW, &[]));

        // 昨日已回应不顶替今日目标：次日凌晨仍要先补昨日那一轮。
        let answered_yesterday = vec![log_row(
            "due-a",
            "success",
            day_target_ms("2026-09-20", "due-a") + 60_000,
        )];
        assert!(account_due(
            local_ms(2026, 9, 22, 8, 0),
            &account,
            TEST_WINDOW,
            &answered_yesterday
        ));
    }

    /// 目标前手动签到：到点仍 due（复核后写 already 回执），回执使窗口跨重启成立。
    #[test]
    fn manual_checkin_before_target_is_answered_by_window_receipt() {
        let (account, target) = due_fixture();
        let manual = vec![log_row("due-a", "success", local_ms(2026, 9, 21, 14, 0))];
        assert!(account_due(target + 60_000, &account, TEST_WINDOW, &manual));

        let mut with_receipt = manual;
        with_receipt.push(log_row("due-a", "already", target + 60_000));
        assert!(!account_due(
            target + 60_000,
            &account,
            TEST_WINDOW,
            &with_receipt
        ));
        // 跨重启后（次日同一目标时刻之前）仍不 due，不会被拉回 00:0x。
        assert!(!account_due(
            local_ms(2026, 9, 22, 0, 30),
            &account,
            TEST_WINDOW,
            &with_receipt
        ));

        // error 行与其它账号的行都不算回应。
        assert!(account_due(
            target + 60_000,
            &account,
            TEST_WINDOW,
            &[log_row("due-a", "error", target + 60_000)]
        ));
        assert!(account_due(
            target + 60_000,
            &account,
            TEST_WINDOW,
            &[log_row("other", "success", target + 60_000)]
        ));
    }

    /// 窗口回执判定：只认该账号在该窗口之后的 success/already，不跨账号串味。
    #[test]
    fn window_receipt_predicate_matches_same_account_after_target_only() {
        let (account, target) = due_fixture();
        assert!(!logs_answered_window(&[], &account, target));
        assert!(
            !logs_answered_window(
                &[log_row("due-a", "success", target - 60_000)],
                &account,
                target
            ),
            "目标之前的记录不算本窗口回应"
        );
        assert!(logs_answered_window(
            &[log_row("due-a", "already", target + 60_000)],
            &account,
            target
        ));

        // 无稳定 id 的账号：日志行 accountId 为空，按展示名匹配。
        let legacy = json!({"email": "legacy@example.com"});
        let legacy_rows = vec![json!({
            "accountId": null,
            "email": "legacy@example.com",
            "result": "success",
            "ts": target + 1
        })];
        assert!(logs_answered_window(&legacy_rows, &legacy, target));
        assert!(!logs_answered_window(
            &legacy_rows,
            &json!({"email": "other@example.com"}),
            target
        ));

        // 空串 accountId 与缺字段 / null 同口径（无稳定 id），按展示名匹配。
        let empty_id_account = json!({"id": "", "email": "legacy@example.com"});
        let empty_id_rows = vec![json!({
            "accountId": "",
            "email": "legacy@example.com",
            "result": "already",
            "ts": target + 1
        })];
        assert!(logs_answered_window(
            &empty_id_rows,
            &empty_id_account,
            target
        ));
        assert!(logs_answered_window(&empty_id_rows, &legacy, target));
        assert!(!logs_answered_window(
            &empty_id_rows,
            &json!({"id": "due-a", "email": "legacy@example.com"}),
            target
        ));
    }

    #[test]
    fn unresolvable_target_is_treated_as_due() {
        assert_eq!(target_at("not-a-date", 0), None);
        assert_eq!(target_at("2026-09-21", 24 * 60), None);

        // minute=1440 无法表示为本地 HMS（hour=24），与 DST 缺口一样让 target_at 返回 None。
        let win = (24 * 60, 24 * 60 + 1);
        let account = json!({"id": "gap"});
        let now = local_ms(2026, 9, 21, 12, 0);
        assert_eq!(latest_target_ms(now, &account, win), None);
        assert!(
            account_due(now, &account, win, &[]),
            "target_at 失败必须保守当作到点未办，避免漏天"
        );
        assert_eq!(
            plan_next_delay(false, FAST_RETRY_MAX, now, &[account], Some(win)),
            (CHECKIN_RECOVERY_INTERVAL, FAST_RETRY_MAX),
            "无未来目标时回落 30 分钟，不得退化成更短空转"
        );
    }

    /// 未设置（或非法）窗口时不得过滤任何账号：与改动前「每轮处理全部账号」一致。
    #[test]
    fn unset_or_invalid_window_never_filters_accounts() {
        let accounts = vec![json!({"id": "due-a"}), json!({"id": "due-b"})];
        let (account, target) = due_fixture();
        let answered = vec![log_row("due-a", "success", target + 60_000)];
        let now = target + 120_000;

        for cfg in [
            json!({}),
            json!({"checkin_start": "", "checkin_end": ""}),
            json!({"checkin_start": "22:00"}),
            json!({"checkin_start": "23:30", "checkin_end": "22:00"}),
            json!({"checkin_start": "22:00", "checkin_end": "25:00"}),
        ] {
            assert_eq!(
                due_accounts(accounts.clone(), now, window_minutes(&cfg), &answered),
                accounts,
                "未设置窗口必须原样返回全部账号: {cfg}"
            );
        }

        // 对照：窗口生效时，已回应的账号被剔除，其余保留。
        let due = due_accounts(accounts, now, Some(TEST_WINDOW), &answered);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0]["id"], json!("due-b"));
        assert!(!account_due(now, &account, TEST_WINDOW, &answered));
    }
