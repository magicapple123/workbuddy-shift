    fn helper_test_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "workbuddy-shift-codebuddy-helper-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn resolves_account_by_id_or_uid_and_returns_canonical_id() {
        let accounts = vec![
            json!({"id": "a1", "uid": "u1"}),
            json!({"id": "a2", "uid": "u2"}),
        ];
        assert_eq!(account_index(&accounts, "a2"), Some((1, "a2".to_string())));
        assert_eq!(account_index(&accounts, "u1"), Some((0, "a1".to_string())));
        assert_eq!(account_index(&accounts, "missing"), None);
    }

    #[test]
    fn state_prefers_account_id_over_legacy_index() {
        let accounts = vec![
            json!({"id": "a1", "uid": "u1"}),
            json!({"id": "a2", "uid": "u2"}),
        ];
        let state = json!({"active": 0, "activeAccountId": "a2"});
        assert_eq!(
            state_account_index(&state, &accounts),
            Some((1, "a2".to_string()))
        );
    }

    #[test]
    fn legacy_index_wraps_without_panicking() {
        let accounts = vec![json!({"id": "a1"}), json!({"id": "a2"})];
        let state = json!({"active": 5});
        assert_eq!(
            state_account_index(&state, &accounts),
            Some((1, "a2".to_string()))
        );
    }

    #[test]
    fn empty_accounts_have_no_active_account() {
        assert_eq!(state_account_index(&json!({"active": 0}), &[]), None);
    }

    // -----------------------------------------------------------------------
    // 会话存活判据（`has_live_session`）
    // -----------------------------------------------------------------------

    /// 一个临时 sessions 目录；用例结束后删除 `parent()` 之外的兄弟文件。
    fn sessions_fixture() -> (PathBuf, PathBuf) {
        let root = helper_test_dir();
        let dir = root.join("sessions");
        fs::create_dir_all(&dir).unwrap();
        (root, dir)
    }

    /// 写一份注册表文件（`<pid>.json` / `manual-*.json` / 其它名字都由 `name` 决定）。
    fn write_session_file(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    /// 心跳为 `heartbeat` 的注册表内容（字段与客户端形态一致）。
    fn session_json(heartbeat: i64) -> String {
        json!({
            "pid": 9312,
            "sessionId": "01a0aeb3-1f70-7d85-b356-0faa85650c1f",
            "startedAt": heartbeat,
            "lastHeartbeat": heartbeat,
        })
        .to_string()
    }

    #[test]
    fn sessions_dir_is_the_single_join_point_under_codebuddy_home() {
        let dir = sessions_dir();
        assert_eq!(
            dir.file_name().and_then(|name| name.to_str()),
            Some(SESSIONS_DIR_NAME)
        );
        assert_eq!(
            dir.parent().and_then(|parent| parent.file_name()),
            Some(std::ffi::OsStr::new(SETTINGS_DIR))
        );
        assert_eq!(
            dir.parent().and_then(|parent| parent.parent()),
            Some(crate::modules::config::home_dir().as_path())
        );
    }

    #[test]
    fn fresh_heartbeat_marks_the_session_alive_and_stale_one_does_not() {
        let (root, dir) = sessions_fixture();
        let now = 1_800_000_000_000;

        // 恰好等于阈值：仍算活着（`<=` 与客户端超时判定一致）。
        write_session_file(
            &dir,
            "9312.json",
            &session_json(now - LIVE_SESSION_STALE_MS),
        );
        assert!(has_live_session_in(&dir, now));

        // 超过阈值 1ms：过期。
        write_session_file(
            &dir,
            "9312.json",
            &session_json(now - LIVE_SESSION_STALE_MS - 1),
        );
        assert!(!has_live_session_in(&dir, now));

        // 未来心跳（时钟回拨 / 客户端时钟偏差）：保守视为活着。
        write_session_file(&dir, "9312.json", &session_json(now + 5 * 60_000));
        assert!(has_live_session_in(&dir, now));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn manual_session_files_count_and_other_names_are_ignored() {
        let (root, dir) = sessions_fixture();
        let now = 1_800_000_000_000;
        let fresh = now - 1_000;

        // `manual-<name>.json` 是客户端另一种合法形态。
        write_session_file(&dir, "manual-review.json", &session_json(fresh));
        assert!(has_live_session_in(&dir, now));
        fs::remove_file(dir.join("manual-review.json")).unwrap();

        // 其它形态都不认（含空 stem、缺 `-` 后缀、非 .json 扩展名）。
        for name in [
            "session.json",
            "manual-.json",
            ".json",
            "9312.json.bak",
            "9312",
            "notes.txt",
        ] {
            write_session_file(&dir, name, &session_json(fresh));
        }
        assert!(!has_live_session_in(&dir, now));
        // 认形态但内容坏 / 缺字段 → 跳过，不阻塞其它文件。
        write_session_file(&dir, "9313.json", "not-json");
        write_session_file(&dir, "9314.json", &json!({"pid": 9314}).to_string());
        assert!(!has_live_session_in(&dir, now));
        // 同目录里加一份新鲜的心跳文件 → 活着。
        write_session_file(&dir, "9315.json", &session_json(fresh));
        assert!(has_live_session_in(&dir, now));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_sessions_dir_is_not_an_error() {
        let (root, dir) = sessions_fixture();
        let missing = root.join("missing-sessions");
        assert!(!has_live_session_in(&missing, 1_800_000_000_000));
        assert!(!missing.exists(), "不得因为探测而创建目录");
        // 路径存在但不是目录（例如同名文件）同样按无会话处理。
        write_session_file(&root, "not-a-dir", "x");
        assert!(!has_live_session_in(&root.join("not-a-dir"), 1));
        assert!(!has_live_session_in(&dir, 1_800_000_000_000));
        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_sessions_dir_is_reported_as_no_session() {
        use std::os::unix::fs::PermissionsExt;

        let (root, dir) = sessions_fixture();
        write_session_file(&dir, "9312.json", &session_json(1_800_000_000_000));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        // root 不受权限位约束（CI 容器常见）：此时构造不出该场景，跳过而不是假失败。
        if std::fs::read_dir(&dir).is_ok() {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir_all(root).ok();
            return;
        }
        assert!(!has_live_session_in(&dir, 1_800_000_000_000));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(root).ok();
    }

    // -----------------------------------------------------------------------
    // 切换顺序：先关进程，再写 state.json
    // -----------------------------------------------------------------------

    #[test]
    fn state_is_written_only_after_the_cli_is_closed() {
        let root = helper_test_dir();
        fs::create_dir_all(&root).unwrap();
        let state_file = root.join("state.json");
        let order = std::cell::RefCell::new(Vec::new());

        let (closed, count) = close_then_write_state(&state_file, "{\"active\":1}", || {
            order.borrow_mut().push("close");
            // 关闭的瞬间不能已经存在新 state：否则旧进程会读到新账号。
            assert!(!state_file.exists(), "写 state 必须发生在关闭进程之后");
            (true, 2)
        })
        .expect("切换写入");
        order.borrow_mut().push("write");

        assert_eq!(*order.borrow(), vec!["close", "write"]);
        assert!(closed);
        assert_eq!(count, 2);
        assert_eq!(fs::read_to_string(&state_file).unwrap(), "{\"active\":1}");

        // 生产形态：`state.json` 在上一次切换时就已经存在。此时"没写出新内容"才是顺序证据 ——
        // 若先写后关，关闭的回调里读到的会是新内容（或至少不再是旧内容）。
        fs::write(&state_file, "{\"active\":0}").unwrap();
        let (_, count) = close_then_write_state(&state_file, "{\"active\":2}", || {
            assert_eq!(
                fs::read_to_string(&state_file).unwrap(),
                "{\"active\":0}",
                "关闭进程时 state.json 必须还是旧内容"
            );
            (true, 1)
        })
        .expect("切换写入");
        assert_eq!(count, 1);
        assert_eq!(fs::read_to_string(&state_file).unwrap(), "{\"active\":2}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_write_failure_reports_the_already_closed_cli() {
        let root = helper_test_dir();
        fs::create_dir_all(&root).unwrap();
        // 父路径是普通文件 → 写入必然失败，且失败发生在关闭之后。
        fs::write(root.join("blocker"), "x").unwrap();
        let state_file = root.join("blocker").join("state.json");

        let error = close_then_write_state(&state_file, "{}", || (true, 3)).unwrap_err();
        assert!(
            error.contains("已关闭 3 个"),
            "必须说明 CLI 已被关闭: {error}"
        );

        // 没有关到进程时不追加那句话（错误保持原样）。
        let error = close_then_write_state(&state_file, "{}", || (false, 0)).unwrap_err();
        assert!(!error.contains("已关闭"), "无进程可关时不追加: {error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_env_token_preserves_helper_and_other_env_values() {
        let mut settings = json!({
            "apiKeyHelper": "C:/Users/tester/bin/wb-helper.bat",
            "trustedDirectories": ["C:/Users/tester"],
            "env": { "HTTPS_PROXY": "http://127.0.0.1:7890" }
        });
        write_settings_env_token(&mut settings, "RAW_SECRET").unwrap();
        assert_eq!(settings_env_token(&settings), Some("RAW_SECRET"));
        assert_eq!(
            settings["apiKeyHelper"],
            "C:/Users/tester/bin/wb-helper.bat"
        );
        assert_eq!(settings["env"]["HTTPS_PROXY"], "http://127.0.0.1:7890");
        assert_eq!(settings["env"][CODEBUDDY_AUTH_TOKEN], "RAW_SECRET");
    }

    #[test]
    fn settings_env_token_rejects_non_object_env() {
        let mut settings = json!({"env": "invalid"});
        let error = write_settings_env_token(&mut settings, "SECRET").unwrap_err();
        assert!(error.contains("env 字段不是对象"));
        assert!(!error.contains("SECRET"));
    }

    #[test]
    fn settings_env_file_update_roundtrips_and_preserves_existing_fields() {
        let test_dir = helper_test_dir();
        let settings = test_dir.join(".codebuddy").join("settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(
            &settings,
            r#"{
  "apiKeyHelper": "C:/Users/tester/bin/wb-helper.bat",
  "trustedDirectories": ["C:/Users/tester"],
  "env": { "HTTPS_PROXY": "http://127.0.0.1:7890" }
}"#,
        )
        .unwrap();

        let (previous, value) =
            prepare_settings_env_update(&settings, "Bearer RAW_SECRET", WbVariant::Cn).unwrap();
        commit_settings_env_update(
            &settings,
            previous.as_deref(),
            &value,
            "RAW_SECRET",
            WbVariant::Cn,
        )
        .unwrap();

        let persisted = read_json_file(&settings).unwrap();
        assert_eq!(settings_env_token(&persisted), Some("RAW_SECRET"));
        assert_eq!(
            persisted["apiKeyHelper"],
            "C:/Users/tester/bin/wb-helper.bat"
        );
        assert_eq!(persisted["trustedDirectories"][0], "C:/Users/tester");
        assert_eq!(persisted["env"]["HTTPS_PROXY"], "http://127.0.0.1:7890");
        assert_eq!(
            persisted["env"][CODEBUDDY_INTERNET_ENVIRONMENT],
            CN_INTERNET_ENVIRONMENT
        );
        fs::remove_dir_all(test_dir).unwrap();
    }
