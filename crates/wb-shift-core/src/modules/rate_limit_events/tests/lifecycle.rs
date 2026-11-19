    #[test]
    fn workbuddy_events_use_the_auth_file_uid_then_the_session_fallback() {
        let fixture = Fixture::new();
        // ① 登录态文件给了 uid → 直接命中账号库。
        std::fs::write(
            fixture.auth_file(),
            json!({ "account": { "uid": UID_A }, "auth": { "accessToken": "t" } }).to_string(),
        )
        .expect("登录态文件");
        let accounts = vec![
            json!({ "id": "acc-wb", "uid": UID_A, "variant": "cn" }),
            json!({ "id": "acc-session", "uid": "other-uid", "variant": "cn" }),
        ];
        let lookup = absent_lookup;
        let ctx = fixture.context(accounts.clone(), &lookup);
        fixture.append(&[stop_payload(
            "s-wb",
            "hy3",
            &fixture.workbuddy_transcript("s-wb"),
            Some(T0 + 900),
        )]);
        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events[0].account_id, "acc-wb");

        // ② 登录态文件缺失 → 会话兜底；③ 兜底也拿不到 → 丢弃。
        std::fs::remove_file(fixture.auth_file()).expect("移除登录态文件");
        let fallback = |variant: WbVariant, session: &str| {
            (variant == WbVariant::Cn && session == "s-fallback").then(|| "acc-session".to_string())
        };
        let ctx = fixture.context(accounts, &fallback);
        fixture.append(&[
            stop_payload(
                "s-fallback",
                "hy3",
                &fixture.workbuddy_transcript("s-fallback"),
                Some(T0 + 900),
            ),
            stop_payload(
                "s-unknown",
                "hy3",
                &fixture.workbuddy_transcript("s-unknown"),
                Some(T0 + 900),
            ),
        ]);
        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events.len(), 1, "只有兜底命中的那条入账");
        assert_eq!(state.events[0].account_id, "acc-session");
    }

    #[test]
    fn entries_expire_at_their_reset_time() {
        let mut state = State {
            offset: 0,
            last_event_at: None,
            sources_migrated: true,
            events: vec![StoredEntry {
                account_id: "a".to_string(),
                model: Some("hy3".to_string()),
                reset_at: 1_500,
                first_seen_at: 1_000,
                hit_count: 1,
                source: None,
            }],
        };
        assert_eq!(
            live_entries(&mut state, 1_400).len(),
            1,
            "未到恢复时刻仍保留"
        );
        assert!(
            live_entries(&mut state, 1_500).is_empty(),
            "到恢复时刻即清掉"
        );
    }

    #[test]
    fn truncated_events_file_resets_the_offset_without_duplicating_entries() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-1";
        fixture.write_session_registry(4242, session, T0 + 500);
        fixture.append(&[stop_payload(
            session,
            "hy3",
            &fixture.cli_transcript(session),
            Some(T0 + 900),
        )]);
        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert!(state.offset > 0);

        // 用户/编辑器把文件截短（或轮转掉）→ 偏移必须回退到可读范围，且不产生重复条目。
        std::fs::write(fixture.events(), "").expect("清空事件文件");
        assert_eq!(consume_with(&ctx, &mut state, T0 + 2_000), None);
        assert_eq!(state.offset, 0);
        assert_eq!(state.events.len(), 1, "同一事件不得重复入账");
    }

    #[test]
    fn large_event_files_are_rotated_without_losing_the_tail() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-1";
        fixture.write_session_registry(4242, session, T0 + 500);
        let payload = stop_payload(
            session,
            "hy3",
            &fixture.cli_transcript(session),
            Some(T0 + 900),
        );
        // 先把文件撑到轮转阈值，再在末尾追一条事件。
        let filler = "x".repeat(COMPACT_AFTER_BYTES as usize);
        std::fs::write(fixture.events(), format!("{filler}\n{payload}\n")).expect("大事件文件");

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.offset, 0, "轮转后偏移归零");
        assert!(!fixture.events().exists(), "旧文件已轮转删除");
        assert!(
            !ctx.events.with_extension("jsonl.1").exists(),
            "临时轮转文件已清理"
        );
    }

    #[test]
    fn watcher_state_path_lives_in_the_store_directory() {
        assert!(state_path().ends_with(STATE_FILE_NAME));
    }
