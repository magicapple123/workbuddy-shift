    const WS: &str = "0123456789abcdef0123456789abcdef";
    const SRC_UID: &str = "uid-src-0001";
    const DST_UID: &str = "uid-dst-0002";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wb_shift_vscode_link_{}_{name}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// 造一个会话目录：`conv_id` 与各类 id 都由调用方给定，用来模拟「同一会话的源与副本」。
    struct Seed<'a> {
        conv_id: &'a str,
        msg_ids: [&'a str; 2],
        req_id: &'a str,
        /// 第一条消息 `extra.traceId`（不在「副本特异 id」集合内的 32-hex）。
        trace_id: &'a str,
        /// 追加到第一条消息 `extra` 的额外字段（例如会话自身 id 的引用）。
        extra_fields: Vec<(&'a str, &'a str)>,
        /// 第二条消息的正文，用于「改写一条消息」的用例。
        second_message: &'a str,
    }

    impl<'a> Default for Seed<'a> {
        fn default() -> Self {
            Self {
                conv_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                msg_ids: [
                    "11111111111111111111111111111111",
                    "22222222222222222222222222222222",
                ],
                req_id: "33333333333333333333333333333333",
                trace_id: "ffffffffffffffffffffffffffffffff",
                extra_fields: Vec::new(),
                second_message: "在的",
            }
        }
    }

    fn seed_conversation(root: &Path, seed: &Seed<'_>) -> PathBuf {
        let dir = root.join(WS).join(seed.conv_id);
        let index = json!({
            "messages": [
                {"id": seed.msg_ids[0], "type": "text", "role": "user", "isComplete": true},
                {"id": seed.msg_ids[1], "type": "text", "role": "assistant", "isComplete": false},
            ],
            "requests": [{
                "id": seed.req_id,
                "type": "craft",
                "messages": [seed.msg_ids[0], seed.msg_ids[1]],
                "state": "complete",
                "startedAt": 1789532362052_i64,
            }],
        });
        write(&dir.join("index.json"), &index.to_string());

        // `extra` 用字符串化 JSON（实测形态）：复制路径会解析后再序列化，归一化走同一处理。
        let mut first_extra = json!({
            "requestId": seed.req_id,
            "responseId": seed.msg_ids[0],
            "modelId": "deepseek-v4",
            "traceId": seed.trace_id,
        });
        for (key, value) in &seed.extra_fields {
            first_extra[*key] = json!(value);
        }
        write(
            &dir.join(format!("messages/{}.json", seed.msg_ids[0])),
            &json!({
                "role": "user",
                // `message` 是字符串化 JSON：复制路径不改它，归一化同样不改（两侧口径一致）。
                "message": json!({"role": "user", "content": "你好"}).to_string(),
                "id": seed.msg_ids[0],
                "extra": first_extra.to_string(),
                "createdAt": "2026-09-16T05:05:29.751Z",
            })
            .to_string(),
        );
        let second_extra = json!({
            "requestId": seed.req_id,
            "responseId": seed.msg_ids[1],
            "modelId": "deepseek-v4",
        });
        write(
            &dir.join(format!("messages/{}.json", seed.msg_ids[1])),
            &json!({
                "role": "assistant",
                "message": json!({"role": "assistant", "content": seed.second_message}).to_string(),
                "id": seed.msg_ids[1],
                "extra": second_extra.to_string(),
                "createdAt": "2026-09-16T05:05:31.001Z",
            })
            .to_string(),
        );
        dir
    }

    fn digest_of(state: &ContentState) -> Vec<String> {
        match state {
            ContentState::Ready(snapshot) => snapshot.normalized.line_digests.clone(),
            other => panic!("期望 Ready，实际 {other:?}"),
        }
    }

    /// 判定的地基：同一逻辑会话的源与副本（id 全不同）归一化后逐条摘要相等。
    #[test]
    fn copies_normalize_to_identical_digests() {
        let root = temp_dir("identical");
        let source = seed_conversation(&root, &Seed::default());
        let copy = seed_conversation(
            &root,
            &Seed {
                conv_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                msg_ids: [
                    "44444444444444444444444444444444",
                    "55555555555555555555555555555555",
                ],
                req_id: "66666666666666666666666666666666",
                ..Seed::default()
            },
        );
        let source_state = read_session_content(&source, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let copy_state = read_session_content(&copy, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(digest_of(&source_state), digest_of(&copy_state));
        // 原始字节摘要必须能区分源与副本（预览凭据按副本校验）。
        let (ContentState::Ready(source_snapshot), ContentState::Ready(copy_snapshot)) =
            (&source_state, &copy_state)
        else {
            panic!("双方内容都应可读");
        };
        assert_ne!(source_snapshot.full_digest, copy_snapshot.full_digest);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 会话自身 id 参与归一化：消息里引用了会话 id 时，源与副本（id 全不同）摘要仍相等。
    #[test]
    fn conversation_id_is_normalized_too() {
        let root = temp_dir("conv-id");
        let source = seed_conversation(
            &root,
            &Seed {
                extra_fields: vec![("sessionId", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")],
                ..Seed::default()
            },
        );
        let copy = seed_conversation(
            &root,
            &Seed {
                conv_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                msg_ids: [
                    "44444444444444444444444444444444",
                    "55555555555555555555555555555555",
                ],
                req_id: "66666666666666666666666666666666",
                extra_fields: vec![("sessionId", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")],
                ..Seed::default()
            },
        );
        let a = read_session_content(&source, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = read_session_content(&copy, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(digest_of(&a), digest_of(&b));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 未列入 id 集合的 32-hex（如 `extra.traceId`）原样保留：两侧不同即视为差异（保守方向）。
    #[test]
    fn unknown_hex_ids_are_kept_and_make_digests_differ() {
        let root = temp_dir("trace-id");
        let source = seed_conversation(&root, &Seed::default());
        let copy = seed_conversation(
            &root,
            &Seed {
                conv_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                msg_ids: [
                    "44444444444444444444444444444444",
                    "55555555555555555555555555555555",
                ],
                req_id: "66666666666666666666666666666666",
                // traceId 是真实数据里存在的 32-hex，但不在「副本特异 id」集合内。
                trace_id: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                ..Seed::default()
            },
        );
        let a = read_session_content(&source, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = read_session_content(&copy, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_ne!(digest_of(&a), digest_of(&b));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 改写一条消息内容 → 该条摘要变化（其它条不变）。
    #[test]
    fn changed_message_changes_only_that_digest() {
        let root = temp_dir("changed");
        let source = seed_conversation(&root, &Seed::default());
        let before = digest_of(&read_session_content(
            &source,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ));
        let dir = seed_conversation(
            &root,
            &Seed {
                second_message: "在的，改过了",
                ..Seed::default()
            },
        );
        let after = digest_of(&read_session_content(
            &dir,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ));
        assert_eq!(before[0], after[0]);
        assert_ne!(before[1], after[1]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 缺消息文件 / 缺索引 / 空会话一律 `Unavailable` 或 `Missing`，不猜。
    #[test]
    fn missing_pieces_are_not_guessed() {
        let root = temp_dir("missing");
        let dir = seed_conversation(&root, &Seed::default());
        let conv_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(matches!(
            read_session_content(&dir, conv_id),
            ContentState::Ready(_)
        ));
        std::fs::remove_file(dir.join("messages/22222222222222222222222222222222.json")).unwrap();
        assert!(matches!(
            read_session_content(&dir, conv_id),
            ContentState::Unavailable(_)
        ));
        std::fs::remove_file(dir.join("index.json")).unwrap();
        assert!(matches!(
            read_session_content(&dir, conv_id),
            ContentState::Missing
        ));
        write(&dir.join("index.json"), r#"{"messages":[],"requests":[]}"#);
        assert!(matches!(
            read_session_content(&dir, conv_id),
            ContentState::Unavailable(_)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 派生 id：32 位小写 hex、确定性（同输入同结果）、序号区分重复内容、salt 换值。
    #[test]
    fn derived_ids_are_deterministic_hex32() {
        let digest = "d".repeat(64);
        let first = derive_message_id(DST_UID, "conv-b", 7, &digest, 0);
        assert_eq!(first.len(), 32);
        assert!(is_hex32(&first));
        assert_eq!(first, derive_message_id(DST_UID, "conv-b", 7, &digest, 0));
        // 内容相同的重复消息：序号不同 → id 不同（不撞车）。
        assert_ne!(first, derive_message_id(DST_UID, "conv-b", 8, &digest, 0));
        // 换目标会话 / 换 salt / 换内容摘要都会换 id。
        assert_ne!(first, derive_message_id(DST_UID, "conv-c", 7, &digest, 0));
        assert_ne!(first, derive_message_id(DST_UID, "conv-b", 7, &digest, 1));
        assert_ne!(
            first,
            derive_message_id(DST_UID, "conv-b", 7, &"e".repeat(64), 0)
        );
        assert_ne!(first, derive_message_id(SRC_UID, "conv-b", 7, &digest, 0));
        // 请求 id 与消息 id 在同一序号上不撞。
        let request = derive_request_id(DST_UID, "conv-b", 7, &digest, 0);
        assert!(is_hex32(&request));
        assert_ne!(first, request);
    }

    /// 位置对齐只翻译前缀，不做「按内容猜 id」。
    #[test]
    fn positional_translation_covers_prefix_only() {
        let source = vec!["s0".to_string(), "s1".to_string(), "s2".to_string()];
        let target = vec!["t0".to_string(), "t1".to_string()];
        let map = positional_translation(&source, &target);
        assert_eq!(map.get("s0"), Some(&"t0".to_string()));
        assert_eq!(map.get("s1"), Some(&"t1".to_string()));
        assert_eq!(map.get("s2"), None);
    }

    /// 索引 / 消息 / 请求的重写范围与复制路径一致（只动 id 与引用）。
    #[test]
    fn translation_touches_ids_and_references_only() {
        let root = temp_dir("translate");
        let dir = seed_conversation(&root, &Seed::default());
        let index = read_session_index(&dir).unwrap();
        let translation: BTreeMap<String, String> = [
            (
                "11111111111111111111111111111111".to_string(),
                "aaaa1111111111111111111111111111".to_string(),
            ),
            (
                "22222222222222222222222222222222".to_string(),
                "aaaa2222222222222222222222222222".to_string(),
            ),
            (
                "33333333333333333333333333333333".to_string(),
                "aaaa3333333333333333333333333333".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let translated = translate_session_index(&index, &translation);
        assert_eq!(
            message_ids_of(&translated),
            vec![
                "aaaa1111111111111111111111111111".to_string(),
                "aaaa2222222222222222222222222222".to_string()
            ]
        );
        let request = translated
            .get("requests")
            .and_then(Value::as_array)
            .unwrap()[0]
            .clone();
        assert_eq!(
            request.get("id").and_then(Value::as_str),
            Some("aaaa3333333333333333333333333333")
        );
        assert_eq!(
            request
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            request.get("startedAt").and_then(Value::as_i64),
            Some(1789532362052),
            "非 id 字段必须原样保留"
        );

        let message_path = dir.join("messages/11111111111111111111111111111111.json");
        let message: Value =
            serde_json::from_str(&std::fs::read_to_string(&message_path).unwrap()).unwrap();
        let translated_message = translate_message(&message, &translation);
        assert_eq!(
            translated_message.get("id").and_then(Value::as_str),
            Some("aaaa1111111111111111111111111111")
        );
        let extra: Value = serde_json::from_str(
            translated_message
                .get("extra")
                .and_then(Value::as_str)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            extra.get("requestId").and_then(Value::as_str),
            Some("aaaa3333333333333333333333333333")
        );
        assert_eq!(
            extra.get("traceId").and_then(Value::as_str),
            Some("ffffffffffffffffffffffffffffffff"),
            "不在翻译表里的 hex 原样保留"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 请求摘要：请求序号不同 / 内容不同都会改变派生 id 的输入。
    #[test]
    fn request_digests_follow_index_order() {
        let root = temp_dir("req-digest");
        let dir = seed_conversation(&root, &Seed::default());
        let index = read_session_index(&dir).unwrap();
        let digests = request_digests(&index, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(digests.len(), 1);
        assert_eq!(digests[0].len(), 64);
        assert!(request_digests(&json!({}), "x").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
