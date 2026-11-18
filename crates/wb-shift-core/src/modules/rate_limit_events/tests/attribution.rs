    #[test]
    fn cli_events_are_attributed_to_the_active_account_and_persisted() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        // 写入时刻钉在会话启动之前 ⇒ 该进程取到的 key 就是当前账号。
        set_state_mtime(&fixture, T0 + 1);
        let session = "01a0ad53-0c95-7767-bd6a-d43356edb644";
        fixture.write_session_registry(4242, session, T0 + 500);
        fixture.append(&[stop_payload(
            session,
            "deepseek-v4.1-flash",
            &fixture.cli_transcript(session),
            Some(T0 + 900),
        )]);

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].account_id, "acc-cli");
        assert_eq!(
            state.events[0].model.as_deref(),
            Some("deepseek-v4.1-flash")
        );
        assert_eq!(state.last_event_at, Some(T0 + 1_000));
        assert!(state.offset > 0);
        assert_eq!(live_entries(&mut state, T0 + 1_100).len(), 1);

        // 重复消费同一文件：偏移已推进 → 不重复入账。
        let mut reloaded = fixture.state();
        assert_eq!(consume_with(&ctx, &mut reloaded, T0 + 2_000), None);
        assert_eq!(reloaded.events.len(), 1);

        // 同一次事件的重复书写（重试轮）：hitCount 累加、不新增条目。
        fixture.append(&[stop_payload(
            session,
            "deepseek-v4.1-flash",
            &fixture.cli_transcript(session),
            Some(T0 + 900),
        )]);
        let mut again = fixture.state();
        assert_eq!(consume_with(&ctx, &mut again, T0 + 3_000), Some(true));
        assert_eq!(again.events.len(), 1);
        assert_eq!(again.events[0].hit_count, 2);
    }

    /// 本案时间线（research/2026-09-17-misattribution-evidence.md，本机实测，CST）。
    const CLI_SESSION: &str = "01a0aeb3-1f70-7d85-b356-0faa85650c1f";
    /// 17:29:29 旧进程 pid=9312 启动（此后取到张佳的 key）。
    const OLD_PROCESS_STARTED_AT: i64 = 1_789_637_369_000;
    /// 18:44:02 新进程 pid=60705 启动（取到 wkbdtest 的 key）。
    const NEW_PROCESS_STARTED_AT: i64 = 1_789_641_842_256;
    /// 18:00:30 state.json 切到 wkbdtest。
    const SWITCHED_TO_WKBDTEST_AT: i64 = 1_789_639_230_009;
    /// 18:43:56 旧进程触发 429。
    const EVENT_AT: i64 = 1_789_639_436_000;

    /// 陈旧 429 重放时间线（research/2026-09-21-stale-429-replay-evidence.md，本机实测，CST）：
    /// 同一会话先由旧进程持有（发出 429），切换账号后被新进程接管并重放那条旧文案。
    const STALE_SESSION: &str = "01a0c392-24cf-7696-90d4-24785d3743ce";
    /// 18:45:29.320 旧进程 pid=32745 启动（持旧账号 key）。
    const STALE_OLD_PROCESS_STARTED_AT: i64 = 1_789_987_529_320;
    /// 旧账号那次切换写入 state.json 的时刻（早于旧进程启动；实测值未留存，取启动前 1 分钟）。
    const STALE_PREVIOUS_SWITCH_AT: i64 = 1_789_987_469_320;
    /// 19:30:03.868 旧进程发出的 429 请求（requestId 解码；日志里 `Sending request` 为 .947）。
    const STALE_REQUEST_AT: i64 = 1_789_990_203_868;
    /// 19:30:06.389 旧账号那条被当轮消费（对照：修复后仍照旧入账）。
    const STALE_OLD_CONSUME_AT: i64 = 1_789_990_206_389;
    /// 19:31:09.492 切到新账号（state.json mtime / `updatedAt`）。
    const STALE_SWITCH_AT: i64 = 1_789_990_269_492;
    /// 19:31:22.122 新进程 pid=64440 启动（持新账号 key）。
    const STALE_NEW_PROCESS_STARTED_AT: i64 = 1_789_990_282_122;
    /// 19:31:26.185 新进程自己发出的请求时刻（AC2 的正常路径）。
    const STALE_NEW_REQUEST_AT: i64 = 1_789_990_286_185;
    /// 19:31:28.481 App 消费那条被新进程重放的陈旧事件。
    const STALE_CONSUME_AT: i64 = 1_789_990_288_481;

    /// `state.json` 的最后写入时刻（毫秒）：构造「快照早于 / 等于 state 写入」用。
    fn state_mtime_ms(fixture: &Fixture) -> i64 {
        std::fs::metadata(fixture.cli_state())
            .and_then(|metadata| metadata.modified())
            .expect("状态文件 mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime 晚于 UNIX_EPOCH")
            .as_millis() as i64
    }

    /// 把 state.json 的 mtime 钉到指定毫秒，避免用例依赖墙钟。
    fn set_state_mtime(fixture: &Fixture, ms: i64) {
        let file = std::fs::File::options()
            .write(true)
            .open(fixture.cli_state())
            .expect("打开状态文件以设置 mtime");
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64))
            .expect("设置状态文件 mtime");
    }

    /// 同一会话先后被两个进程持有：取「事件前最后一个持有者」的启动时刻。
    #[test]
    fn session_snapshot_prefers_the_latest_holder_before_the_event() {
        let fixture = Fixture::new();
        fixture.write_session_registry(9312, CLI_SESSION, OLD_PROCESS_STARTED_AT);
        fixture.write_session_registry(60705, CLI_SESSION, NEW_PROCESS_STARTED_AT);
        let dir = fixture.cli_sessions_dir();
        assert_eq!(
            session_snapshot_time(&dir, Some(CLI_SESSION), EVENT_AT),
            Some(OLD_PROCESS_STARTED_AT),
            "18:43:56 的事件：60705 尚未启动，取 9312"
        );
        assert_eq!(
            session_snapshot_time(&dir, Some(CLI_SESSION), NEW_PROCESS_STARTED_AT + 1_000),
            Some(NEW_PROCESS_STARTED_AT),
            "接管后的事件取 60705"
        );
        // 不匹配 / 缺 sessionId / 目录缺失 → 无快照。
        assert_eq!(session_snapshot_time(&dir, Some("other"), EVENT_AT), None);
        assert_eq!(session_snapshot_time(&dir, None, EVENT_AT), None);
        assert_eq!(
            session_snapshot_time(&fixture.root.join("missing"), Some(CLI_SESSION), EVENT_AT),
            None
        );
        // 坏文件跳过，不影响其它候选。
        std::fs::write(dir.join("broken.json"), "not-json").expect("坏注册表");
        assert_eq!(
            session_snapshot_time(&dir, Some(CLI_SESSION), EVENT_AT),
            Some(OLD_PROCESS_STARTED_AT)
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 注册表缺失（或坏文件被跳过）→ 兜底 transcript 首行的会话创建时刻。
    #[test]
    fn session_snapshot_falls_back_to_the_transcript_first_line() {
        let fixture = Fixture::new();
        let transcript = fixture.cli_transcript(CLI_SESSION);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            format!(
                "{}\n{}\n",
                json!({
                    "type": "session-meta",
                    "sessionId": CLI_SESSION,
                    "timestamp": OLD_PROCESS_STARTED_AT + 769
                }),
                json!({ "type": "user" })
            ),
        )
        .unwrap();
        fixture.write_cli_state("acc-cli");
        // 注册表目录在但文件坏：跳过，仍走 transcript。
        std::fs::create_dir_all(fixture.cli_sessions_dir()).expect("会话注册表目录");
        std::fs::write(fixture.cli_sessions_dir().join("9312.json"), "{").expect("坏注册表");
        // state 的写入时刻早于会话创建时刻 ⇒ 这个进程取到的就是当前账号。
        set_state_mtime(&fixture, OLD_PROCESS_STARTED_AT);
        let ctx = fixture.context(vec![cli_account()], &absent_lookup);
        assert_eq!(
            session_snapshot_time(&fixture.cli_sessions_dir(), Some(CLI_SESSION), EVENT_AT),
            None,
            "坏注册表不提供快照时刻"
        );
        assert_eq!(
            session_created_at(&transcript.to_string_lossy()),
            Some(OLD_PROCESS_STARTED_AT + 769)
        );
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, EVENT_AT),
                HookSource::Cli,
                &ctx,
                EVENT_AT
            )
            .as_deref(),
            Some("acc-cli")
        );
        // transcript 也不可读 → 丢弃。
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, "missing-transcript", EVENT_AT),
                HookSource::Cli,
                &ctx,
                EVENT_AT
            ),
            None
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 切换之后仍有旧进程触发限额：按不变式这种进程本该已经被关闭 ⇒ 丢弃（不猜账号）。
    #[test]
    fn old_process_event_after_switch_is_dropped() {
        let fixture = Fixture::new();
        fixture.write_session_registry(9312, CLI_SESSION, OLD_PROCESS_STARTED_AT);
        fixture.write_cli_state("acc-wkbdtest");
        // state 在旧进程启动之后才被改写：它持有的是切换前的 key。
        set_state_mtime(&fixture, SWITCHED_TO_WKBDTEST_AT);
        let ctx = fixture.context(vec![json!({ "id": "acc-wkbdtest" })], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, EVENT_AT),
                HookSource::Cli,
                &ctx,
                EVENT_AT
            ),
            None,
            "不变式被破坏时应丢弃，不得猜切换前的账号"
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 快照时刻恰等于 state 写入时刻 ⇒ 进程在最后一次切换之后启动，归当前账号。
    #[test]
    fn new_process_event_after_the_last_switch_uses_the_state_account() {
        let fixture = Fixture::new();
        fixture.write_cli_state("acc-wkbdtest");
        let mtime = state_mtime_ms(&fixture);
        fixture.write_session_registry(60705, CLI_SESSION, mtime);
        let ctx = fixture.context(vec![json!({ "id": "acc-wkbdtest" })], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, mtime + 500),
                HookSource::Cli,
                &ctx,
                mtime + 1_000
            )
            .as_deref(),
            Some("acc-wkbdtest")
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 快照时刻比 state 写入时刻早 1ms ⇒ 归因边界收紧到「丢弃」。
    #[test]
    fn snapshot_one_millisecond_before_the_state_write_is_dropped() {
        let fixture = Fixture::new();
        fixture.write_cli_state("acc-wkbdtest");
        let mtime = state_mtime_ms(&fixture);
        fixture.write_session_registry(9312, CLI_SESSION, mtime - 1);
        let ctx = fixture.context(vec![json!({ "id": "acc-wkbdtest" })], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, mtime + 500),
                HookSource::Cli,
                &ctx,
                mtime + 1_000
            ),
            None,
            "无法区分旧进程/新进程接管时不得猜账号"
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 快照时刻不可得（注册表与 transcript 都不可用）→ 丢弃，即使 state.json 可读。
    #[test]
    fn missing_snapshot_evidence_is_dropped() {
        let fixture = Fixture::new();
        fixture.write_cli_state("acc-cli");
        let ctx = fixture.context(vec![cli_account()], &absent_lookup);
        // 注册表目录不存在 + transcript 不存在 + session_id 缺失。
        let mut event = cli_quota_event(&fixture, CLI_SESSION, T0 + 900);
        event.session_id = None;
        assert_eq!(attribute(&event, HookSource::Cli, &ctx, T0 + 10_000), None);

        // state.json 缺失时同样丢弃（即使快照时刻可得）。
        let fixture = Fixture::new();
        fixture.write_session_registry(9312, CLI_SESSION, T0 + 500);
        let ctx = fixture.context(vec![cli_account()], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, T0 + 900),
                HookSource::Cli,
                &ctx,
                T0 + 10_000
            ),
            None
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 当前账号已被删除 → 丢弃（不得改判到任何别的账号）。
    #[test]
    fn deleted_current_account_is_dropped() {
        let fixture = Fixture::new();
        fixture.write_session_registry(60705, CLI_SESSION, NEW_PROCESS_STARTED_AT);
        fixture.write_cli_state("acc-deleted");
        set_state_mtime(&fixture, SWITCHED_TO_WKBDTEST_AT);
        let ctx = fixture.context(vec![json!({ "id": "acc-demo" })], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, NEW_PROCESS_STARTED_AT + 500),
                HookSource::Cli,
                &ctx,
                NEW_PROCESS_STARTED_AT + 1_000
            ),
            None,
            "当前账号不在账号库时丢弃，不得改判到账号库里仍存在的旧账号"
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// AC1 回归（2026-09-21 本机实测时间线）：codeg 恢复会话后**新进程**触发 Stop，
    /// 重放的却是旧进程上一轮（旧账号）的 429 文案 —— 判据是请求时刻，不是消费时刻。
    ///
    /// 同一时间线里：① 切换前消费（旧进程当轮触发）仍归旧账号（不丢那条 chip）；
    /// ② 切换后消费同一条陈旧文案 → 丢弃；③ 消费路径不入账。
    #[test]
    fn stale_429_replayed_by_a_new_process_is_dropped() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(
            vec![json!({ "id": "acc-old" }), json!({ "id": "acc-new" })],
            &lookup,
        );
        // 同一会话先后被两个进程持有：旧进程发出 429，新进程接管会话并重放那条文案。
        fixture.write_session_registry(32745, STALE_SESSION, STALE_OLD_PROCESS_STARTED_AT);
        fixture.write_session_registry(64440, STALE_SESSION, STALE_NEW_PROCESS_STARTED_AT);
        fixture.append(&[stale_429_payload(
            STALE_SESSION,
            &fixture.cli_transcript(STALE_SESSION),
            STALE_REQUEST_AT,
        )]);
        let event = cli_quota_event(&fixture, STALE_SESSION, STALE_REQUEST_AT);

        // ① 19:30:06 旧进程当轮触发 hook 时消费 → 归旧账号。
        fixture.write_cli_state("acc-old");
        set_state_mtime(&fixture, STALE_PREVIOUS_SWITCH_AT);
        assert_eq!(
            attribute(&event, HookSource::Cli, &ctx, STALE_OLD_CONSUME_AT).as_deref(),
            Some("acc-old"),
            "旧账号那条必须照旧入账（修复不得连带丢掉它）"
        );

        // ② 19:31:28 新进程重放同一条陈旧文案 → 丢弃，不记到新账号。
        fixture.write_cli_state("acc-new");
        set_state_mtime(&fixture, STALE_SWITCH_AT);
        assert_eq!(
            attribute(&event, HookSource::Cli, &ctx, STALE_CONSUME_AT),
            None,
            "请求时刻早于切换时刻的陈旧 429 不得归到新账号"
        );

        // ③ 消费路径：该事件不入账（行照常消费，不重放）。
        let mut state = State::default();
        assert_eq!(
            consume_with(&ctx, &mut state, STALE_CONSUME_AT),
            Some(false)
        );
        assert!(state.events.is_empty());
        assert!(state.offset > 0);
    }

    /// AC2：切换之后由新进程发出的 429（请求时刻 ≥ 切换时刻）→ 仍归当前账号（正常路径不回退）。
    #[test]
    fn fresh_429_after_the_switch_is_attributed_to_the_current_account() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![json!({ "id": "acc-new" })], &lookup);
        fixture.write_cli_state("acc-new");
        set_state_mtime(&fixture, STALE_SWITCH_AT);
        fixture.write_session_registry(64440, STALE_SESSION, STALE_NEW_PROCESS_STARTED_AT);
        fixture.append(&[stale_429_payload(
            STALE_SESSION,
            &fixture.cli_transcript(STALE_SESSION),
            STALE_NEW_REQUEST_AT,
        )]);

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, STALE_CONSUME_AT), Some(true));
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].account_id, "acc-new");
        assert_eq!(
            state.events[0].reset_at, 1_790_060_809_000,
            "恢复时刻仍取文案原文（2026-09-22 15:06:49 UTC+8）"
        );
    }

    /// AC3：文案里没有 `(requestId/sessionId)` 身份段（已知边界）→ CLI 事件丢弃 + 告警，
    /// 不 panic，且不影响同一批里的其它来源（WorkBuddy 走登录态文件归因）。
    #[test]
    fn cli_events_without_a_request_id_are_dropped_without_affecting_other_sources() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        std::fs::write(
            fixture.auth_file(),
            json!({ "account": { "uid": UID_A } }).to_string(),
        )
        .expect("登录态文件");
        let ctx = fixture.context(
            vec![json!({ "id": "acc-wb", "uid": UID_A, "variant": "cn" })],
            &lookup,
        );
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-no-request-id";
        fixture.write_session_registry(4242, session, T0 + 500);
        fixture.append(&[
            // CLI：证据不足（无身份段）→ 丢弃。
            stop_payload(session, "hy3", &fixture.cli_transcript(session), None),
            // 同批次的 WorkBuddy 事件照常入账。
            stop_payload("s-wb", "hy3", &fixture.workbuddy_transcript("s-wb"), None),
        ]);

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events.len(), 1, "只有 WorkBuddy 那条入账");
        assert_eq!(state.events[0].account_id, "acc-wb");
    }

    /// 时钟异常（快照时刻晚于事件时刻）时不猜。
    #[test]
    fn future_snapshot_time_is_dropped() {
        let fixture = Fixture::new();
        let transcript = fixture.cli_transcript(CLI_SESSION);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        // transcript 首行（兜底快照）的时间在「当前时刻」之后：时钟异常，不猜。
        std::fs::write(
            &transcript,
            format!("{}\n", json!({ "timestamp": T0 + 20_000 })),
        )
        .unwrap();
        fixture.write_cli_state("acc-cli");
        let ctx = fixture.context(vec![cli_account()], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, T0 + 9_000),
                HookSource::Cli,
                &ctx,
                T0 + 10_000
            ),
            None
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    /// 时钟异常（请求时刻晚于当前时刻）时不猜：不得回退到消费时刻归因。
    #[test]
    fn future_request_time_is_dropped() {
        let fixture = Fixture::new();
        fixture.write_session_registry(4242, CLI_SESSION, T0 + 500);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let ctx = fixture.context(vec![cli_account()], &absent_lookup);
        assert_eq!(
            attribute(
                &cli_quota_event(&fixture, CLI_SESSION, T0 + 11_000),
                HookSource::Cli,
                &ctx,
                T0 + 10_000
            ),
            None,
            "request_at 晚于当前时刻必须丢弃，即使快照与 mtime 本来会归到当前账号"
        );
    }

    /// 归因到的账号不在账号库（用户删了账号）→ 丢弃，不误归。
    #[test]
    fn events_are_dropped_when_attribution_fails() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![json!({ "id": "acc-other" })], &lookup);
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
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(false));
        assert!(state.events.is_empty());
        assert_eq!(state.last_event_at, None, "没有入账就不算事件时刻");
        assert!(state.offset > 0, "已消费的行不得反复重读");
    }
