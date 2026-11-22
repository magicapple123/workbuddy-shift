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
        // 未设置 / 缺失 / 只填一个 / 非法 / start >= end：`window_minutes` 仍返回 None
        // （语义不变），但自动调度必须退化为默认窗口 8:00–22:00，而不是「不限制」。
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
            assert_eq!(window_minutes(&cfg), None, "必须按未配置处理: {cfg}");
            let schedule = schedule_window(&cfg);
            assert_eq!(
                schedule.window,
                (8 * 60, 22 * 60),
                "未配置时间段时必须退化为默认 8:00–22:00: {cfg}"
            );
            assert_eq!(
                schedule.scope,
                DueScope::TodayOnly,
                "默认兜底窗口必须用「只看今日」口径，否则跨日补签会抵消分散: {cfg}"
            );
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

        // 有效配置优先于默认窗口，且保留跨日补签口径。
        for (cfg, window) in [
            (
                json!({"checkin_start": "22:00", "checkin_end": "23:30"}),
                (22 * 60, 23 * 60 + 30),
            ),
            (json!({"checkin_start": "0:0", "checkin_end": "1:0"}), (0, 60)),
        ] {
            let schedule = schedule_window(&cfg);
            assert_eq!(schedule.window, window);
            assert_eq!(schedule.scope, DueScope::CatchUpAcrossDays);
        }
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

    /// 未配置时间段 → 默认窗口 + 今日口径：首轮（含首次安装、毫无日志）只处理
    /// 今天目标已过的账号，不再一轮打完全部。
    #[test]
    fn default_window_first_cycle_only_handles_accounts_past_their_today_target() {
        // 默认窗口必须是 8:00–22:00（产品口径），改这个常量就等于改签到分布。
        assert_eq!(DEFAULT_CHECKIN_WINDOW, (8 * 60, 22 * 60));
        let schedule = schedule_window(&json!({}));
        assert_eq!(schedule.window, DEFAULT_CHECKIN_WINDOW);
        assert_eq!(schedule.scope, DueScope::TodayOnly);

        let accounts: Vec<Value> = (0..40)
            .map(|index| json!({"id": format!("acc-{index}")}))
            .collect();
        let now = local_ms(2026, 9, 21, 14, 0);

        // 期望集合按同一抽签规则独立算出（今日目标 ≤ now），避免测试依赖具体哈希值。
        let expected: Vec<String> = accounts
            .iter()
            .filter(|acc| {
                target_ms_for_day("2026-09-21", acc, DEFAULT_CHECKIN_WINDOW)
                    .is_some_and(|target| target <= now)
            })
            .map(|acc| acc["id"].as_str().unwrap_or_default().to_string())
            .collect();

        // 关键回归：日志为空（首次安装 / 当天首次启动）时，昨日未回应不再让账号提前到点。
        let due: Vec<String> = due_accounts(
            accounts.clone(),
            now,
            Some(schedule.window),
            &[],
            schedule.scope,
        )
        .iter()
        .map(|acc| acc["id"].as_str().unwrap_or_default().to_string())
        .collect();
        assert_eq!(due, expected, "只有今日目标已过的账号可以进入本轮");

        // 30 分钟后（上一轮已把到点的那批签完）：只剩这半小时内新到点的账号，
        // 单轮处理量落到个位数量级——这才是「不像脚本」的稳态。
        let half_hour_later = local_ms(2026, 9, 21, 14, 30);
        let first_cycle_logs: Vec<Value> = expected
            .iter()
            .map(|id| log_row(id, "success", now))
            .collect();
        let next_cycle = due_accounts(
            accounts.clone(),
            half_hour_later,
            Some(schedule.window),
            &first_cycle_logs,
            schedule.scope,
        );
        assert!(
            next_cycle.len() <= 5,
            "稳态单轮应是个位数量级，不能再次批量提交: {}",
            next_cycle.len()
        );
        // 14:00 时窗口已走过 6/14，期望约 40 × 6/14 ≈ 17 个。
        assert!(!due.is_empty(), "到点账号不得为空");
        assert!(
            due.len() < 25,
            "首轮处理量必须落在十几的量级，不能接近全部 40 个: {}",
            due.len()
        );
        assert!(
            due.len() > OUTSIDE_WINDOW_BATCH,
            "窗口内不加数量上限（限量补签只作用于窗口外）: {}",
            due.len()
        );

        // 已回应的账号被剔除（今日回执生效）。
        let first_due = expected[0].clone();
        let answered = vec![log_row(&first_due, "success", now)];
        let due = due_accounts(
            accounts.clone(),
            now,
            Some(schedule.window),
            &answered,
            schedule.scope,
        );
        assert!(
            !due
                .iter()
                .any(|acc| acc["id"].as_str() == Some(first_due.as_str())),
            "今日已回应的账号不得重复提交"
        );
        assert_eq!(due.len(), expected.len() - 1);

        // 对照：`due_accounts` 的 None 分支（不限制）仍原样返回全部账号，
        // 供手动/兜底调用方使用。
        assert_eq!(
            due_accounts(accounts.clone(), now, None, &[], DueScope::TodayOnly),
            accounts
        );
    }

    /// 默认兜底窗口外：不是「不签」，而是每轮限量补签（最多 `OUTSIDE_WINDOW_BATCH` 个），
    /// 且按今日 target 升序取最早的几个。
    #[test]
    fn default_window_outside_hours_catches_up_in_capped_batches() {
        let schedule = schedule_window(&json!({"checkin_start": "", "checkin_end": ""}));
        let accounts: Vec<Value> = (0..40)
            .map(|index| json!({"id": format!("acc-{index}")}))
            .collect();
        // 白天已签 6 个 → 剩 34 个未签（与线上反馈的量级一致）。
        let signed: Vec<String> = accounts[..6]
            .iter()
            .map(|acc| acc["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let answered: Vec<Value> = signed
            .iter()
            .map(|id| log_row(id, "success", local_ms(2026, 9, 21, 10, 0)))
            .collect();

        // 期望：`day` 当天未回应（`answered_today` 之外）的账号，按目标时刻升序取前 N。
        let earliest = |day: &str, answered_today: &[String]| -> Vec<String> {
            let mut rows: Vec<(i64, String)> = accounts
                .iter()
                .filter_map(|acc| {
                    let id = acc["id"].as_str()?.to_string();
                    if answered_today.contains(&id) {
                        return None;
                    }
                    let target = target_ms_for_day(day, acc, DEFAULT_CHECKIN_WINDOW)?;
                    Some((target, id))
                })
                .collect();
            rows.sort_by_key(|(target, _)| *target);
            rows.into_iter()
                .take(OUTSIDE_WINDOW_BATCH)
                .map(|(_, id)| id)
                .collect()
        };
        let ids_of = |rows: &[Value]| -> Vec<String> {
            rows.iter()
                .map(|acc| acc["id"].as_str().unwrap_or_default().to_string())
                .collect()
        };

        // 当天 22:30（窗口后）：34 个未签里 target 最早的 3 个，顺序也要一致。
        let due = due_accounts(
            accounts.clone(),
            local_ms(2026, 9, 21, 22, 30),
            Some(schedule.window),
            &answered,
            schedule.scope,
        );
        let expected_today = earliest("2026-09-21", &signed);
        assert_eq!(expected_today.len(), OUTSIDE_WINDOW_BATCH);
        assert_eq!(ids_of(&due), expected_today, "窗口后必须按今日 target 升序取前 3");
        assert_eq!(due.len(), 3, "单轮不得批量提交");

        // 次日 02:00（跨零点）：昨天的回应不算今天 → 40 个都是候选，仍是 target 最早的 3 个。
        let overnight = due_accounts(
            accounts.clone(),
            local_ms(2026, 9, 22, 2, 0),
            Some(schedule.window),
            &answered,
            schedule.scope,
        );
        let expected_overnight = earliest("2026-09-22", &[]);
        assert_eq!(
            ids_of(&overnight),
            expected_overnight,
            "跨零点后仍按今日 target 升序补签，不得退化为空集"
        );
        assert_eq!(overnight.len(), 3);

        // 窗口起点前的同一逻辑（07:30）同样只补 3 个。
        let early = due_accounts(
            accounts.clone(),
            local_ms(2026, 9, 21, 7, 30),
            Some(schedule.window),
            &answered,
            schedule.scope,
        );
        assert_eq!(ids_of(&early), expected_today);

        // 今日全部已回应 → 没有候补，不能重复提交。
        let all_answered: Vec<Value> = accounts
            .iter()
            .map(|acc| {
                log_row(
                    acc["id"].as_str().unwrap_or_default(),
                    "success",
                    local_ms(2026, 9, 21, 10, 0),
                )
            })
            .collect();
        assert!(due_accounts(
            accounts.clone(),
            local_ms(2026, 9, 21, 22, 30),
            Some(schedule.window),
            &all_answered,
            schedule.scope,
        )
        .is_empty());

        // 无账号 → 空集（不 panic）。
        assert!(due_accounts(
            Vec::new(),
            local_ms(2026, 9, 21, 22, 30),
            Some(schedule.window),
            &[],
            schedule.scope,
        )
        .is_empty());

        // 窗口起点整点属窗口内（`[start, end)`），所以 08:00 只有目标正好是 08:00 的账号，
        // 不会因为「限量」被误判成窗口外补签。
        let inside_start = due_accounts(
            accounts,
            local_ms(2026, 9, 21, 8, 0),
            Some(schedule.window),
            &answered,
            schedule.scope,
        );
        assert!(
            inside_start.len() <= 1,
            "08:00 至多一个账号（目标分钟抽到 08:00）: {}",
            inside_start.len()
        );
    }

    /// 用户显式配置的窗口保留跨日补签（既有能力，不能被默认口径改坏）。
    #[test]
    fn configured_window_keeps_cross_day_catch_up() {
        // 今天的 09:00–10:00 已过且无日志 → 全部到点。
        let schedule = schedule_window(&json!({"checkin_start": "09:00", "checkin_end": "10:00"}));
        assert_eq!(schedule.window, (9 * 60, 10 * 60));
        assert_eq!(schedule.scope, DueScope::CatchUpAcrossDays);

        let accounts: Vec<Value> = (0..40)
            .map(|index| json!({"id": format!("cfg-{index}")}))
            .collect();
        let now = local_ms(2026, 9, 21, 14, 0);
        let due = due_accounts(
            accounts.clone(),
            now,
            Some(schedule.window),
            &[],
            schedule.scope,
        );
        assert_eq!(
            due.len(),
            accounts.len(),
            "配置窗口必须保留跨日补签，且不受窗口外限量补签影响"
        );
        assert!(
            due.len() > OUTSIDE_WINDOW_BATCH,
            "限量补签不得污染配置窗口路径"
        );

        // 今天的目标还没到（20:00–21:00 的窗口、现在 14:00）：配置窗口按昨天那轮补签
        // （全部 40 个），默认兜底口径只走窗口外限量补签（3 个）——两种口径的差异在这里。
        let evening_window = (20 * 60, 21 * 60);
        assert_eq!(
            due_accounts(
                accounts.clone(),
                now,
                Some(evening_window),
                &[],
                DueScope::CatchUpAcrossDays
            )
            .len(),
            accounts.len()
        );
        assert_eq!(
            due_accounts(
                accounts,
                now,
                Some(evening_window),
                &[],
                DueScope::TodayOnly
            )
            .len(),
            OUTSIDE_WINDOW_BATCH,
            "今日口径下今天的目标未到就不处理；窗口外只保留限量补签"
        );
    }

    /// 今日口径的边界：正好到点算到点，早 1 分钟不算；已被回应（success/already）才剔除。
    #[test]
    fn today_only_due_boundary_is_the_target_minute_itself() {
        // 固定 10:00–11:00，边界前后一分钟都落在窗口内，避免与窗口边界混淆。
        let win = (10 * 60, 11 * 60);
        let account = json!({"id": "boundary"});
        let target = target_ms_for_day("2026-09-21", &account, win).expect("目标时刻必须可解析");

        assert!(
            account_due_today_only(target, &account, win, &[]),
            "到点整时刻必须判为到点"
        );
        assert!(
            !account_due_today_only(target - 60_000, &account, win, &[]),
            "早 1 分钟不得提前签"
        );
        assert!(
            !account_due_today_only(
                target + 60_000,
                &account,
                win,
                &[log_row("boundary", "success", target + 1)]
            ),
            "今日已签不再到点"
        );
        assert!(
            !account_due_today_only(
                target + 60_000,
                &account,
                win,
                &[log_row("boundary", "already", target + 1)]
            ),
            "already 回执同样算已回应"
        );
        assert!(
            account_due_today_only(
                target + 60_000,
                &account,
                win,
                &[log_row("boundary", "error", target + 1)]
            ),
            "error 不算回应"
        );
        // 昨天已回应不影响今天：今日口径只看今日目标。
        assert!(
            account_due_today_only(
                target + 60_000,
                &account,
                win,
                &[log_row("boundary", "success", target - 24 * 3600 * 1000)]
            ),
            "昨天那轮不能顶替今天"
        );
    }
