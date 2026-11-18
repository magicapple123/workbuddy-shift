    #[test]
    fn legacy_windows_helper_requires_migration_even_when_logic_supports_ids() {
        let directory = Path::new(r"C:\Users\tester\.codebuddy-rotate");
        assert!(is_legacy_helper_path(
            &directory.join(LEGACY_WINDOWS_HELPER_FILE),
            directory,
            true,
        ));
        assert!(is_legacy_helper_path(
            &directory.join(LEGACY_HELPER_FILE),
            directory,
            true,
        ));
        assert!(!is_legacy_helper_path(
            &directory.join(HELPER_FILE),
            directory,
            true,
        ));
    }

    #[test]
    fn helper_validation_errors_never_include_stdout_or_token() {
        let secret = "SECRET_ACCESS_TOKEN";
        let error =
            validate_helper_result(true, Some(0), b"Bearer OTHER_SECRET\n", secret).unwrap_err();
        assert!(error.contains("输出阶段"));
        assert!(!error.contains(secret));
        assert!(!error.contains("OTHER_SECRET"));

        let error =
            validate_helper_result(false, Some(127), b"Bearer LEAKED\n", secret).unwrap_err();
        assert!(error.contains("退出码为 127"));
        assert!(!error.contains(secret));
        assert!(!error.contains("LEAKED"));
    }

    #[test]
    fn extracts_node_path_without_accepting_shell_noise() {
        // 判据是「绝对路径 + 文件名为 node」，而绝对路径的形态按平台不同
        // （Windows 需盘符或 UNC 前缀），fixture 必须按平台给。
        let node = if cfg!(windows) {
            r"C:\Users\test\node-v22\node"
        } else {
            "/Users/test/.nvm/versions/node/v22/bin/node"
        };
        let output = format!("welcome to the shell\n{node}\n");
        assert_eq!(
            node_path_from_shell_output(output.as_bytes()),
            Some(PathBuf::from(node))
        );
        assert_eq!(node_path_from_shell_output(b"node\nwelcome\n"), None);
    }

    #[test]
    fn helper_selects_active_account_and_fails_without_selected_token() {
        let test_dir = helper_test_dir();
        let rotate_dir = test_dir.join("rotate");
        let accounts_file = test_dir.join("accounts.json");
        fs::create_dir_all(&rotate_dir).unwrap();
        fs::write(
            &accounts_file,
            serde_json::to_vec(&json!([
                {"id": "a1", "access_token": "SECRET_ONE"},
                {"id": "a2", "access_token": "SECRET_TWO"}
            ]))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            rotate_dir.join(STATE_FILE),
            serde_json::to_vec(&json!({"activeAccountId": "a2", "active": 0})).unwrap(),
        )
        .unwrap();

        let output = Command::new("node")
            .arg("-e")
            .arg(STANDARD_HELPER)
            .env("CODEBUDDY_ROTATE_DIR", &rotate_dir)
            .env("WB_SHIFT_ACCOUNTS_FILE", &accounts_file)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "Bearer SECRET_TWO\n"
        );

        fs::write(
            &accounts_file,
            serde_json::to_vec(&json!([
                {"id": "a1", "access_token": "SECRET_ONE"},
                {"id": "a2"}
            ]))
            .unwrap(),
        )
        .unwrap();
        let output = Command::new("node")
            .arg("-e")
            .arg(STANDARD_HELPER)
            .env("CODEBUDDY_ROTATE_DIR", &rotate_dir)
            .env("WB_SHIFT_ACCOUNTS_FILE", &accounts_file)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_ONE"));

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn restores_previous_file_after_failed_validation() {
        let test_dir = helper_test_dir();
        fs::create_dir_all(&test_dir).unwrap();
        let existing = test_dir.join("existing.json");
        fs::write(&existing, "old").unwrap();
        restore_file(&existing, Some("old"));
        assert_eq!(fs::read_to_string(&existing).unwrap(), "old");

        let newly_created = test_dir.join("new.json");
        fs::write(&newly_created, "temporary").unwrap();
        restore_file(&newly_created, None);
        assert!(!newly_created.exists());
        fs::remove_dir_all(test_dir).unwrap();
    }
