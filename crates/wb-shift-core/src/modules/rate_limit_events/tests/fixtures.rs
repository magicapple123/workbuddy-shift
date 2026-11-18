    const UID_A: &str = "f0ae9eeb-8476-4ef5-9a3e-1d6339d545da";

    /// 测试用时刻基准（2026-09-10）：晚于 requestId 时刻的合理下界（2024-01-01），
    /// 早于 fixture 文案里的恢复时刻，便于用 `T0 + 偏移` 构造先后关系。
    const T0: i64 = 1_789_000_000_000;

    /// 文案尾部身份段里的 sessionId（本模块不解析它，只要求形态与真实文案一致）。
    const QUOTA_SESSION_ID: &str = "01a0c392-24cf-7696-90d4-24785d3743ce";

    /// 默认 fixture 文案的官方恢复时刻原文（本机实测句式）。
    const QUOTA_RESET_TEXT: &str = "2026-09-17 17:59:27 UTC+8";

    /// 本机实测的陈旧 429 文案的恢复时刻原文（2026-09-21 19:30，见 research 文档）。
    const STALE_RESET_TEXT: &str = "2026-09-22 15:06:49 UTC+8";

    /// 按毫秒生成 32 位十六进制的 UUIDv7 `requestId`：前 12 位十六进制 = 毫秒时刻，
    /// version nibble `7`、variant `0b10`（与真实文案同形，见 research 文档）。
    fn request_id_at(ms: i64) -> String {
        let ms = (ms as u64) & 0x0000_ffff_ffff_ffff;
        format!("{ms:012x}7cebb43642413d3ad593")
    }

    /// 429 文案：`reset_text` = 官方恢复时刻原文；`request_at` 给出时补上尾部
    /// `(requestId/sessionId)` 身份段，`None` = 文案里没有身份段的形态（证据不足用例）。
    fn quota_message(reset_text: &str, request_at: Option<i64>) -> String {
        let mut message = format!(
            "429 您的使用量已超出频率限制，将在 {reset_text} 重置，您也可以切换其他模型继续使用。"
        );
        if let Some(ms) = request_at {
            message.push_str(&format!(" ({}/{QUOTA_SESSION_ID})", request_id_at(ms)));
        }
        message
    }

    /// 本机实测的 WorkBuddy payload 骨架（`Stop` 行，字段已替换）。
    ///
    /// `request_at` = 该 429 请求自身的时刻（CLI 归因判据；WorkBuddy 归因不用它）。
    fn stop_payload(
        session: &str,
        model: &str,
        transcript: &Path,
        request_at: Option<i64>,
    ) -> String {
        json!({
            "session_id": session,
            "transcript_path": transcript.to_string_lossy(),
            "hook_event_name": "Stop",
            "model": model,
            "last_assistant_message": quota_message(QUOTA_RESET_TEXT, request_at),
        })
        .to_string()
    }

    /// 非限额轮次（正常回答）的 payload。
    fn ok_payload(session: &str, transcript: &Path) -> String {
        json!({
            "session_id": session,
            "transcript_path": transcript.to_string_lossy(),
            "hook_event_name": "Stop",
            "model": "deepseek-v4.1-flash",
            "last_assistant_message": "已完成。",
        })
        .to_string()
    }

    /// 带废弃字段 `_hookTs` 的限额 payload（旧脚本写入的历史事件行形态）。
    ///
    /// 该字段已从消费端移除，但事件文件是 append-only 的：历史行必须照常消费。
    fn legacy_stop_payload_with_hook_ts(
        session: &str,
        model: &str,
        transcript: &Path,
        hook_ts: i64,
        request_at: Option<i64>,
    ) -> String {
        json!({
            "session_id": session,
            "transcript_path": transcript.to_string_lossy(),
            "hook_event_name": "Stop",
            "model": model,
            "_hookTs": hook_ts,
            "last_assistant_message": quota_message(QUOTA_RESET_TEXT, request_at),
        })
        .to_string()
    }

    fn absent_lookup(_variant: WbVariant, _session: &str) -> Option<String> {
        None
    }

    /// 临时环境：所有路径都在 tempdir 下，绝不触碰真实用户配置。
    struct Fixture {
        root: PathBuf,
    }

    /// 测试结束自动清理 tempdir：不清理的话每轮 cargo test 留 20+ 个残留目录
    /// （2026-09-18 实证 127 个，且目录名与事件文件形态酷似「沙箱写入」，误导过排查方向）。
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "workbuddy-shift-events-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&root).expect("临时目录");
            Self { root }
        }

        fn events(&self) -> PathBuf {
            self.root.join("hook-events.jsonl")
        }

        fn state_file(&self) -> PathBuf {
            self.root.join("rate_limit_state.json")
        }

        fn cli_state(&self) -> PathBuf {
            self.root.join("codebuddy-rotate.json")
        }

        fn cli_sessions_dir(&self) -> PathBuf {
            self.root.join("codebuddy-sessions")
        }

        fn auth_file(&self) -> PathBuf {
            self.root.join("workbuddy-desktop.info")
        }

        /// CLI transcript 路径（前缀与真实形态一致，只是根在 tempdir）。
        fn cli_transcript(&self, session: &str) -> PathBuf {
            self.root
                .join(".codebuddy/projects")
                .join(format!("{session}.jsonl"))
        }

        fn workbuddy_transcript(&self, session: &str) -> PathBuf {
            self.root
                .join(".workbuddy/projects")
                .join(format!("{session}.jsonl"))
        }

        fn context<'a>(
            &self,
            accounts: Vec<Value>,
            lookup: &'a dyn Fn(WbVariant, &str) -> Option<String>,
        ) -> IngestContext<'a> {
            let mut source_roots = vec![(self.root.join(".codebuddy"), HookSource::Cli)];
            source_roots.push((
                self.root.join(".workbuddy"),
                HookSource::WorkBuddy(WbVariant::Cn),
            ));
            source_roots.push((
                self.root.join(".workbuddy-ai"),
                HookSource::WorkBuddy(WbVariant::Ai),
            ));
            IngestContext {
                events: self.events(),
                state_path: self.state_file(),
                accounts,
                source_roots,
                cli_state_path: self.cli_state(),
                cli_sessions_dir: self.cli_sessions_dir(),
                auth_file_paths: vec![
                    (WbVariant::Cn, self.auth_file()),
                    (WbVariant::Ai, self.root.join("workbuddy-desktop-ai.info")),
                ],
                session_lookup: lookup,
            }
        }

        fn append(&self, lines: &[String]) {
            let mut text = std::fs::read_to_string(self.events()).unwrap_or_default();
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
            std::fs::write(self.events(), text).expect("追加事件");
        }

        fn state(&self) -> State {
            load_state(&self.state_file())
        }

        /// 预置一份 CLI 会话注册表（`sessionId → 进程启动时刻`）。
        fn write_session_registry(&self, pid: u32, session: &str, started_at: i64) {
            let dir = self.cli_sessions_dir();
            std::fs::create_dir_all(&dir).expect("会话注册表目录");
            std::fs::write(
                dir.join(format!("{pid}.json")),
                json!({ "pid": pid, "sessionId": session, "startedAt": started_at }).to_string(),
            )
            .expect("会话注册表");
        }

        /// 写 `state.json`（当前账号 + 其后由 `set_state_mtime` 钉住的写入时刻）。
        fn write_cli_state(&self, active_account_id: &str) {
            std::fs::write(
                self.cli_state(),
                json!({ "activeAccountId": active_account_id }).to_string(),
            )
            .expect("状态文件");
        }
    }

    /// 一条 CLI 限额事件（只用于直接调 `attribute` 的用例）。
    ///
    /// `request_at` = 该 429 请求自身的时刻（归因判据，由文案里的 requestId 解码而来）。
    fn cli_quota_event(fixture: &Fixture, session: &str, request_at: i64) -> QuotaEvent {
        QuotaEvent {
            transcript_path: fixture
                .cli_transcript(session)
                .to_string_lossy()
                .to_string(),
            session_id: Some(session.to_string()),
            model: Some("deepseek-v4.1-flash".to_string()),
            reset_at: 1_789_670_683_000,
            request_at: Some(request_at),
        }
    }

    /// 本机实测的陈旧 429 payload（2026-09-21 19:30，恢复时刻 2026-09-22 15:06:49 UTC+8）：
    /// codeg 恢复会话后新进程重放的就是这条文案（`request_at` 仍是旧进程那一刻）。
    fn stale_429_payload(session: &str, transcript: &Path, request_at: i64) -> String {
        json!({
            "session_id": session,
            "transcript_path": transcript.to_string_lossy(),
            "hook_event_name": "Stop",
            "model": "hy3",
            "last_assistant_message": quota_message(STALE_RESET_TEXT, Some(request_at)),
        })
        .to_string()
    }

    fn cli_account() -> Value {
        json!({ "id": "acc-cli", "uid": UID_A, "variant": "cn" })
    }
