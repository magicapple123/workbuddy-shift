    const GAP: i64 = 24 * 3_600_000; // 防抖动差异阈值
    const URG: i64 = 72 * 3_600_000; // 紧迫度阈值
    const HOUR_MS: i64 = 3_600_000;

    fn cand(id: &str, expire_at: Option<i64>, remaining: f64, valid: bool) -> Candidate {
        Candidate {
            account_id: id.to_string(),
            display_name: id.to_string(),
            soonest_expire_at: expire_at,
            total_remaining: remaining,
            valid,
            error: None,
        }
    }

    /// 默认参数调用：无冷却、无存活会话、无价值过滤。
    fn dt(candidates: &[Candidate], current: Option<&str>) -> Decision {
        decide_target(candidates, current, None, 0, GAP, URG, false, 0.0)
    }

    /// 同一套 CLI：候选辅助仍可按档位切分，但轮换主路径不再只用国内账号。
    #[test]
    fn rotation_candidates_can_be_split_or_combined_by_variant() {
        let accounts = vec![
            json!({"id": "cn-1", "uid": "u-1"}),
            json!({"id": "ai-1", "uid": "u-2", "variant": "ai"}),
            json!({"id": "ai-2", "uid": "u-3", "domain": "www.workbuddy.ai"}),
        ];

        let cn = candidates_for_variant(&accounts, WbVariant::Cn);
        assert_eq!(cn.len(), 1);
        assert_eq!(cn[0]["id"], "cn-1");

        let ai = candidates_for_variant(&accounts, WbVariant::Ai);
        assert_eq!(ai.len(), 2);
        assert_eq!(accounts.len(), 3);
        assert!(ai
            .iter()
            .all(|a| a["variant"] == "ai" || a["domain"] == "www.workbuddy.ai"));
        assert!(ai.iter().all(|a| a["id"] != "cn-1"));
    }

    #[test]
    fn switches_to_most_urgent_account() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 30 * 24 * 3_600_000), 100.0, true),
            cand("b", Some(now + 24 * 3_600_000), 50.0, true),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Switch("b".to_string())
        );
    }

    #[test]
    fn noop_when_current_is_most_urgent() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 24 * 3_600_000), 50.0, true),
            cand("b", Some(now + 30 * 24 * 3_600_000), 100.0, true),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Skip("当前账号已是最紧迫账号".to_string())
        );
    }

    #[test]
    fn skips_when_gap_below_threshold() {
        // 目标 c 比当前 a 早到期，但差异 < 24h → 不切（防抖动）
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 24 * 3_600_000), 50.0, true),
            cand("b", Some(now + 22 * 3_600_000), 60.0, true),
            cand("c", Some(now + 21 * 3_600_000), 70.0, true),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Skip("目标到期仅早 3 小时，未达切换阈值（24 小时）".to_string())
        );
    }

    #[test]
    fn switches_when_gap_above_threshold() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 30 * 24 * 3_600_000), 100.0, true),
            cand("b", Some(now + 24 * 3_600_000), 50.0, true),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Switch("b".to_string())
        );
    }

    #[test]
    fn respects_cooldown() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 30 * 24 * 3_600_000), 100.0, true),
            cand("b", Some(now + 24 * 3_600_000), 50.0, true),
        ];
        // 刚切过（10 分钟前），冷却 30 分钟 → 不切
        assert_eq!(
            decide_target(
                &candidates,
                Some("a"),
                Some(now - 10 * 60_000),
                30 * 60_000,
                GAP,
                URG,
                false,
                0.0
            ),
            Decision::Skip("处于切换冷却期".to_string())
        );
        // 冷却结束 → 切
        assert_eq!(
            decide_target(
                &candidates,
                Some("a"),
                Some(now - 40 * 60_000),
                30 * 60_000,
                GAP,
                URG,
                false,
                0.0
            ),
            Decision::Switch("b".to_string())
        );
    }

    #[test]
    fn expired_and_failed_accounts_excluded() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 2 * 24 * 3_600_000), 100.0, true),
            cand("expired", Some(now - 3_600_000), 0.0, false),
            cand("failed", None, 0.0, false),
        ];
        assert_eq!(
            dt(&candidates, Some("expired")),
            Decision::Switch("a".to_string())
        );
    }

    #[test]
    fn no_valid_candidates_skips() {
        let candidates = vec![
            cand("a", Some(now_ms()), 0.0, false),
            cand("b", None, 0.0, false),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Skip("没有可用账号（查询失败/已过期/无剩余积分）".to_string())
        );
    }

    #[test]
    fn skips_when_nothing_urgent() {
        // 所有账号 5 天后才过期：最紧迫剩余 > 72h → 不切。
        // 目标账号额外多给 1 分钟余量：判定函数会重新采样 now_ms()，
        // 若两次采样跨过 1 毫秒，截断除法会把「5 天」打成「4 天」。
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 6 * 24 * 3_600_000), 100.0, true),
            cand("b", Some(now + 5 * 24 * 3_600_000 + 60_000), 50.0, true),
        ];
        assert_eq!(
            dt(&candidates, Some("a")),
            Decision::Skip("所有账号到期都还早（最紧迫的还剩 5 天），无需切换".to_string())
        );
    }

    /// 存活门控：有 CLI 会话在跑 → 固定 reason 跳过；放开后同一条链会切。
    #[test]
    fn live_session_gate_skips_and_switches_once_the_session_is_gone() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 30 * DAY_MS), 100.0, true),
            cand("b", Some(now + DAY_MS), 50.0, true),
        ];
        assert_eq!(
            decide_target(&candidates, Some("a"), None, 0, GAP, URG, true, 0.0),
            Decision::Skip(SKIP_LIVE_SESSION_REASON.to_string()),
            "门控命中时 reason 必须固定，提示判定依赖它"
        );
        assert_eq!(
            decide_target(&candidates, Some("a"), None, 0, GAP, URG, false, 0.0),
            Decision::Switch("b".to_string()),
            "放开门控后其余条件满足 → 会切"
        );
    }

    /// 门控被拦下但"放开也不会切"时不产生提示（这里覆盖两种典型原因）。
    #[test]
    fn gate_skip_without_a_real_target_does_not_prompt() {
        let now = now_ms();
        // ① 两个账号到期差 < min_gap_hours：放开也只会 Skip（防抖动）。
        let close_expiry = vec![
            cand("a", Some(now + DAY_MS), 50.0, true),
            cand("b", Some(now + 21 * HOUR_MS), 70.0, true),
        ];
        assert_eq!(
            decide_target(&close_expiry, Some("a"), None, 0, GAP, URG, true, 0.0),
            Decision::Skip(SKIP_LIVE_SESSION_REASON.to_string())
        );
        assert!(
            !matches!(
                decide_target(&close_expiry, Some("a"), None, 0, GAP, URG, false, 0.0),
                Decision::Switch(_)
            ),
            "差异未达阈值 → 不该提示"
        );

        // ② 当前账号已是最紧迫账号：决策链在门控之前就拦下了，同样"不是仅因门控被拦"。
        let already_target = vec![
            cand("a", Some(now + DAY_MS), 50.0, true),
            cand("b", Some(now + 30 * DAY_MS), 100.0, true),
        ];
        assert_eq!(
            decide_target(&already_target, Some("a"), None, 0, GAP, URG, true, 0.0),
            Decision::Skip("当前账号已是最紧迫账号".to_string()),
            "已是目标时不会走到门控，也就不会按门控文案提示"
        );
        assert_eq!(
            decide_target(&already_target, Some("a"), None, 0, GAP, URG, false, 0.0),
            Decision::Skip("当前账号已是最紧迫账号".to_string())
        );
    }

    /// AC7 的判定本身：用真实决策链验证「放开门控后得 Switch 才提示」。
    #[test]
    fn prompt_only_when_releasing_the_gate_would_switch() {
        let now = now_ms();
        let run = |candidates: &[Candidate], gated: bool| {
            decide_target(candidates, Some("a"), None, 0, GAP, URG, gated, 0.0)
        };

        // 当前账号 3 天后到期、另一账号 1 天后到期 + 有存活会话 → 提示目标 b。
        let expiring = vec![
            cand("a", Some(now + 3 * DAY_MS), 100.0, true),
            cand("b", Some(now + DAY_MS), 50.0, true),
        ];
        assert_eq!(
            run(&expiring, true),
            Decision::Skip(SKIP_LIVE_SESSION_REASON.to_string())
        );
        assert_eq!(
            deferred_prompt_target(SKIP_LIVE_SESSION_REASON, || run(&expiring, false)).as_deref(),
            Some("b"),
            "除门控外本来会切换 → 提示"
        );

        // 两个账号到期接近（差异未达 min_gap_hours）→ 真实原因仍是门控，放开也不切 → 不提示。
        let close_expiry = vec![
            cand("a", Some(now + DAY_MS), 50.0, true),
            cand("b", Some(now + 21 * HOUR_MS), 70.0, true),
        ];
        assert_eq!(
            run(&close_expiry, true),
            Decision::Skip(SKIP_LIVE_SESSION_REASON.to_string())
        );
        assert_eq!(
            deferred_prompt_target(SKIP_LIVE_SESSION_REASON, || run(&close_expiry, false)),
            None,
            "差异未达阈值 → 不提示"
        );

        // 真实原因不是门控时不该重跑决策链（重跑不免费，而且结论无关）。
        let mut replayed = false;
        assert_eq!(
            deferred_prompt_target("处于切换冷却期", || {
                replayed = true;
                Decision::Switch("b".to_string())
            }),
            None
        );
        assert!(!replayed, "非门控原因不得重跑决策链");
    }

    /// 提示文案：唯一构造点，标题固定，且不得写成"正在使用中"（会话可能是闲置但存活）。
    #[test]
    fn deferred_notify_text_is_built_in_one_place() {
        let notify = rotate_deferred_notify("账号B", Some(1));
        assert_eq!(notify["title"], json!("workbuddy-shift"));
        assert_eq!(
            notify["body"],
            json!("「账号B」的积分 1 天后到期，但检测到有 CodeBuddy CLI 会话在运行；重启 CLI 后新账号才会生效。")
        );
        assert!(
            !notify["body"].as_str().unwrap().contains("正在使用中"),
            "会话可能是闲置但存活"
        );
        // 到期时刻未知（目标没有 soonestExpireAt）时不谎报天数。
        assert_eq!(
            rotate_deferred_notify("B", None)["body"],
            json!("「B」的积分即将到期，但检测到有 CodeBuddy CLI 会话在运行；重启 CLI 后新账号才会生效。")
        );
    }

    #[test]
    fn remaining_days_round_up_and_never_report_zero() {
        let now = 1_800_000_000_000;
        assert_eq!(remaining_days_until(now + DAY_MS, now), 1);
        assert_eq!(remaining_days_until(now + DAY_MS + 1, now), 2);
        assert_eq!(remaining_days_until(now + 1, now), 1, "不足一天按一天算");
        assert_eq!(
            remaining_days_until(now - 5 * DAY_MS, now),
            1,
            "已过期也不报 0 天"
        );
    }

    #[test]
    fn skips_when_target_low_remaining() {
        let now = now_ms();
        let candidates = vec![
            cand("a", Some(now + 30 * 24 * 3_600_000), 100.0, true),
            cand("b", Some(now + 24 * 3_600_000), 10.0, true),
        ];
        // 目标剩余 10 < 阈值 30 → 不值得切
        assert_eq!(
            decide_target(&candidates, Some("a"), None, 0, GAP, URG, false, 30.0),
            Decision::Skip("目标账号剩余积分不足（10，阈值 30），不值得切换".to_string())
        );
        // 阈值 5：目标剩余 10 达标 → 切
        assert_eq!(
            decide_target(&candidates, Some("a"), None, 0, GAP, URG, false, 5.0),
            Decision::Switch("b".to_string())
        );
    }
