    /// 延迟规则：30 分钟基线、5 分钟快速重试（预算递减）、耗尽回落、1 分钟下限。
    #[test]
    fn plan_next_delay_covers_window_and_retry_rules() {
        let (account, target) = due_fixture();
        let accounts = vec![account];

        // 窗口未生效：既有 30 分钟节奏，快速重试预算不参与也不消耗。
        assert_eq!(
            plan_next_delay(true, 3, target, &accounts, None),
            (CHECKIN_RECOVERY_INTERVAL, 3)
        );
        assert_eq!(
            plan_next_delay(true, 3, target, &[], None),
            (CHECKIN_RECOVERY_INTERVAL, 3)
        );

        // 上一轮有 error 且预算未耗尽：5 分钟，并消耗一次预算。
        assert_eq!(
            plan_next_delay(true, FAST_RETRY_MAX, target, &accounts, Some(TEST_WINDOW)),
            (FAST_RETRY_INTERVAL, FAST_RETRY_MAX - 1)
        );
        assert_eq!(
            plan_next_delay(true, 1, target, &accounts, Some(TEST_WINDOW)),
            (FAST_RETRY_INTERVAL, 0)
        );

        // 预算耗尽 → 回落；此时最近目标是明日 → 上限 30 分钟（当天继续尝试）。
        assert_eq!(
            plan_next_delay(true, 0, target + 60_000, &accounts, Some(TEST_WINDOW)),
            (CHECKIN_RECOVERY_INTERVAL, 0)
        );

        // 无待办（最近目标是明日的 22:00–23:30）→ 30 分钟上限，本地复核周期。
        assert_eq!(
            plan_next_delay(
                false,
                FAST_RETRY_MAX,
                target + 60_000,
                &accounts,
                Some(TEST_WINDOW)
            ),
            (CHECKIN_RECOVERY_INTERVAL, FAST_RETRY_MAX)
        );

        // 到点未办：目标已在 1 分钟内 → 1 分钟下限，立刻处理。
        assert_eq!(
            plan_next_delay(
                false,
                FAST_RETRY_MAX,
                target - 30_000,
                &accounts,
                Some(TEST_WINDOW)
            ),
            (MIN_CYCLE_DELAY, FAST_RETRY_MAX)
        );
        assert_eq!(
            plan_next_delay(false, FAST_RETRY_MAX, target, &accounts, Some(TEST_WINDOW)),
            (CHECKIN_RECOVERY_INTERVAL, FAST_RETRY_MAX)
        );

        // 中间值：目标在 10 分钟后 → 正好睡到目标时刻。
        assert_eq!(
            plan_next_delay(
                false,
                FAST_RETRY_MAX,
                target - 10 * 60_000,
                &accounts,
                Some(TEST_WINDOW)
            ),
            (Duration::from_secs(10 * 60), FAST_RETRY_MAX)
        );

        // 无账号 / 窗口生效但无账号 → 回落到 30 分钟，不 panic。
        assert_eq!(
            plan_next_delay(false, FAST_RETRY_MAX, target, &[], Some(TEST_WINDOW)),
            (CHECKIN_RECOVERY_INTERVAL, FAST_RETRY_MAX)
        );
    }

    /// 干净轮次重置预算；有错误时保留（由 `next_cycle_delay` 逐次消耗）。
    #[test]
    fn cycle_health_resets_fast_retry_budget_only_when_clean() {
        let _serial = RETRY_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        record_cycle_health(true);
        assert!(LAST_CYCLE_HAD_ERRORS.load(Ordering::Relaxed));
        assert_eq!(FAST_RETRY_BUDGET.load(Ordering::Relaxed), FAST_RETRY_MAX);

        FAST_RETRY_BUDGET.store(2, Ordering::Relaxed);
        record_cycle_health(true);
        assert_eq!(
            FAST_RETRY_BUDGET.load(Ordering::Relaxed),
            2,
            "有 error 的轮次不得重置预算"
        );

        record_cycle_health(false);
        assert!(!LAST_CYCLE_HAD_ERRORS.load(Ordering::Relaxed));
        assert_eq!(FAST_RETRY_BUDGET.load(Ordering::Relaxed), FAST_RETRY_MAX);
    }
