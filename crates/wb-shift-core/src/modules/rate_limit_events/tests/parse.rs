    #[test]
    fn parses_quota_lines_and_ignores_everything_else() {
        let transcript = Path::new("/x/.workbuddy/projects/s.jsonl");
        assert!(
            parse_quota_line(&ok_payload("s-1", transcript)).is_none(),
            "非限额轮次忽略"
        );
        assert!(parse_quota_line("{ not json").is_none(), "损坏行忽略");
        assert!(
            parse_quota_line(
                &json!({
                    "transcript_path": "/x/.workbuddy/projects/s.jsonl",
                    "last_assistant_message": "429 您的使用量已超出频率限制"
                })
                .to_string()
            )
            .is_none(),
            "只有文案没有恢复时刻 → 不入账"
        );
        assert!(
            parse_quota_line(
                &json!({ "last_assistant_message": "429 您的使用量已超出频率限制" }).to_string()
            )
            .is_none(),
            "缺 transcript_path → 不入账"
        );

        // 模型哨兵值不得作为模型名；文案没有身份段时 request_at 为空（归因侧会丢弃）。
        let event = parse_quota_line(
            &json!({
                "transcript_path": "/x/.workbuddy/projects/s.jsonl",
                "session_id": "s-1",
                "model": "auto",
                "last_assistant_message": "429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置"
            })
            .to_string(),
        )
        .expect("限额行");
        assert_eq!(event.model, None);
        assert_eq!(event.session_id.as_deref(), Some("s-1"));
        assert_eq!(event.reset_at, 1_789_639_167_000);
        assert_eq!(event.request_at, None);

        // 尾部身份段 → 请求时刻（毫秒）由 requestId 解码得到。
        let event = parse_quota_line(&stop_payload("s-1", "hy3", transcript, Some(T0 + 900)))
            .expect("限额行");
        assert_eq!(event.request_at, Some(T0 + 900));
        assert_eq!(event.model.as_deref(), Some("hy3"));
        assert_eq!(event.reset_at, 1_789_639_167_000);
    }

    /// requestId 解码：合法 UUIDv7 → 精确毫秒；形态 / 版本 / variant / 下界不符 → None
    /// （调用方丢弃该事件，不回退到消费时刻）。
    #[test]
    fn request_id_decodes_to_the_request_time_or_is_rejected() {
        // 本机实测样例：19:30:03.868 发出的 429（requestId 前 12 位十六进制 = 该毫秒时刻）。
        let sample = "429 您的使用量已超出频率限制，将在 2026-09-22 15:06:49 UTC+8 重置。 \
                      (01a0c3baf1dc7cebb43642413d3ad593/01a0c392-24cf-7696-90d4-24785d3743ce)";
        assert_eq!(request_time_of(sample), Some(1_789_990_203_868));
        // 正文里还有别的括号时取「最后一个 `(`」（身份段永远在尾部）。
        assert_eq!(
            request_time_of("已用额度 (约 3 次) 后限流 (01a0c3baf1dc7cebb43642413d3ad593/x)"),
            Some(1_789_990_203_868)
        );

        let id = request_id_at(T0 + 900);
        let decode = |value: &str| request_time_of(&format!("429 超出频率限制 ({value}/x)"));
        assert_eq!(decode(&id), Some(T0 + 900));
        assert_eq!(request_time_of("429 超出频率限制"), None, "无括号");
        assert_eq!(
            request_time_of("429 超出频率限制 (01a0c3baf1dc7cebb43642413d3ad593)"),
            None,
            "有括号但没有 `/` 分隔的身份段"
        );
        assert_eq!(
            decode("01a0c3baf1dc7cebb43642413d3ad59g"),
            None,
            "非十六进制"
        );
        assert_eq!(decode(&id[..31]), None, "不足 32 位");
        // version nibble ≠ 7（UUIDv4 等）与 variant ≠ 0b10（`c` = 0b11）都拒绝。
        assert_eq!(decode(&format!("{}{}{}", &id[..12], '6', &id[13..])), None);
        assert_eq!(decode(&format!("{}{}{}", &id[..16], 'c', &id[17..])), None);
        // 时刻下界：2024-01-01 之前（UUIDv7 不存在）拒绝，恰在下界通过。
        assert_eq!(decode(&request_id_at(1_704_067_199_999)), None);
        assert_eq!(
            decode(&request_id_at(1_704_067_200_000)),
            Some(1_704_067_200_000)
        );
    }

    #[test]
    fn source_is_decided_by_transcript_prefix() {
        let roots = vec![
            (PathBuf::from("/home/u/.codebuddy"), HookSource::Cli),
            (
                PathBuf::from("/home/u/.workbuddy"),
                HookSource::WorkBuddy(WbVariant::Cn),
            ),
            (
                PathBuf::from("/home/u/.workbuddy-ai"),
                HookSource::WorkBuddy(WbVariant::Ai),
            ),
        ];
        assert_eq!(
            source_of("/home/u/.codebuddy/projects/p/s.jsonl", &roots),
            Some(HookSource::Cli)
        );
        assert_eq!(
            source_of("/home/u/.workbuddy/projects/p/s.jsonl", &roots),
            Some(HookSource::WorkBuddy(WbVariant::Cn))
        );
        assert_eq!(
            source_of("/home/u/.workbuddy-ai/projects/p/s.jsonl", &roots),
            Some(HookSource::WorkBuddy(WbVariant::Ai))
        );
        // IDE 的 transcript 在 Application Support 下，不属于任一来源。
        assert_eq!(
            source_of(
                "/Users/u/Library/Application Support/CodeBuddyIDE/x.json",
                &roots
            ),
            None
        );
        assert_eq!(source_of("", &roots), None);
    }

    #[test]
    fn only_complete_lines_are_consumed() {
        let (lines, consumed) = complete_lines(b"{\"a\":1}\n{\"b\"");
        assert_eq!(lines, vec!["{\"a\":1}"]);
        assert_eq!(consumed, 8);
        // 一行都没有完整时不得消费任何字节。
        let (lines, consumed) = complete_lines(b"{\"a\"");
        assert!(lines.is_empty());
        assert_eq!(consumed, 0);
        // 空行忽略，但换行本身要计入已消费长度。
        let (lines, consumed) = complete_lines(b"\n\n{\"a\":1}\n");
        assert_eq!(lines, vec!["{\"a\":1}"]);
        assert_eq!(consumed, 10);
    }

    /// 事件文件首行带 UTF-8 BOM（Windows PowerShell 5.1 `Add-Content -Encoding UTF8`
    /// 建文件时的实测行为）不得吞掉第一个限额事件，也不得打乱按**字节**推进的消费偏移。
    #[test]
    fn utf8_bom_on_the_first_line_does_not_kill_the_first_event() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-bom";
        fixture.write_session_registry(4242, session, T0 + 500);
        std::fs::write(
            fixture.events(),
            format!(
                "\u{feff}{}\n",
                stop_payload(
                    session,
                    "hy3",
                    &fixture.cli_transcript(session),
                    Some(T0 + 900)
                )
            ),
        )
        .expect("带 BOM 的事件文件");

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_000), Some(true));
        assert_eq!(state.events.len(), 1, "BOM 前缀不得让首行解析失败");
        // 偏移以字节计（BOM 也算在内）：文件被消费完，下一轮不会重复入账。
        assert_eq!(
            state.offset,
            std::fs::metadata(fixture.events()).expect("事件文件").len()
        );
        assert_eq!(
            consume_with(&ctx, &mut state, T0 + 1_001),
            None,
            "无新字节不重复消费"
        );
    }

    /// 事件模型来自**当次 payload**：切到 B 后的首个请求被限（429 轮次没有响应行），
    /// 延迟消费期间会话又切到 C —— 事件必须仍记为 B。
    ///
    /// 回归 2026-09-20 审查反例：曾按 transcript 全文重判模型，会取到上一轮的 A
    /// （时间上界只截到「最新一条模型行」），把 payload 里正确的 B 覆盖掉。
    #[test]
    fn event_model_comes_from_the_current_payload_not_the_transcript() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-switched";
        let transcript = fixture.cli_transcript(session);
        std::fs::create_dir_all(transcript.parent().expect("父目录")).expect("transcript 目录");
        std::fs::write(
            &transcript,
            format!(
                "{}\n{}\n",
                // 上一轮用 A 成功（事件之前的最后一个 assistant 轮次）。
                json!({ "type": "message", "timestamp": 900_000, "providerData": { "model": "model-a" } }),
                // 事件之后主人切到 C 继续会话（延迟消费时已在文件里）。
                json!({ "type": "message", "timestamp": 1_060_000, "providerData": { "model": "model-c" } })
            ),
        )
        .expect("写 transcript");
        fixture.write_session_registry(4242, session, T0 + 500);
        std::fs::write(
            fixture.events(),
            format!(
                "{}\n",
                stop_payload(session, "model-b", &transcript, Some(T0 + 1_000_000))
            ),
        )
        .expect("事件文件");

        let mut state = State::default();
        // 请求发生在 T0 + 1 000 000，消费推迟到 T0 + 1 200 000（transcript 已含切到 C 的新轮次）。
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_200_000), Some(true));
        assert_eq!(state.events.len(), 1);
        assert_eq!(
            state.events[0].model.as_deref(),
            Some("model-b"),
            "模型必须取当次 payload，不得被 transcript 里的其它轮次覆盖"
        );
    }

    /// 旧脚本写入的事件行带已废弃字段 `_hookTs`：仍须照常消费（事件文件是 append-only 的）。
    #[test]
    fn legacy_events_with_the_retired_hook_ts_field_are_still_consumed() {
        let fixture = Fixture::new();
        let lookup = absent_lookup;
        let ctx = fixture.context(vec![cli_account()], &lookup);
        fixture.write_cli_state("acc-cli");
        set_state_mtime(&fixture, T0 + 1);
        let session = "s-legacy";
        fixture.write_session_registry(4242, session, T0 + 500);
        std::fs::write(
            fixture.events(),
            format!(
                "{}\n",
                legacy_stop_payload_with_hook_ts(
                    session,
                    "hy3",
                    &fixture.cli_transcript(session),
                    1_000_000,
                    Some(T0 + 1_000_000)
                )
            ),
        )
        .expect("事件文件");

        let mut state = State::default();
        assert_eq!(consume_with(&ctx, &mut state, T0 + 1_200_000), Some(true));
        assert_eq!(state.events.len(), 1, "未知字段不得让整行解析失败");
        assert_eq!(state.events[0].model.as_deref(), Some("hy3"));
    }
