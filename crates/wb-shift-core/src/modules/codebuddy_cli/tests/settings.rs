    #[test]
    fn windows_current_account_is_derived_from_persisted_token() {
        let accounts = vec![
            json!({"id": "a1", "access_token": "TOKEN_ONE"}),
            json!({"id": "a2", "access_token": "TOKEN_TWO"}),
        ];
        assert_eq!(
            account_index_by_token(&accounts, "Bearer TOKEN_TWO"),
            Some((1, "a2".to_string()))
        );
        assert_eq!(account_index_by_token(&accounts, "UNKNOWN"), None);
    }

    #[test]
    fn settings_account_token_rejects_empty_bearer_value() {
        let error = settings_account_token(&json!({"access_token": "Bearer "})).unwrap_err();
        assert!(error.contains("没有可用的认证信息"));
        assert!(!error.contains("Bearer"));
    }

    #[test]
    fn invalid_settings_file_is_not_overwritten_or_leaked() {
        let test_dir = helper_test_dir();
        let settings = test_dir.join("settings.json");
        fs::create_dir_all(&test_dir).unwrap();
        fs::write(&settings, "not-json SECRET_ON_DISK").unwrap();

        let error =
            prepare_settings_env_update(&settings, "NEW_SECRET", WbVariant::Cn).unwrap_err();
        assert!(error.contains("不是有效 JSON"));
        assert!(!error.contains("NEW_SECRET"));
        assert!(!error.contains("SECRET_ON_DISK"));
        assert_eq!(
            fs::read_to_string(&settings).unwrap(),
            "not-json SECRET_ON_DISK"
        );
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn recognizes_legacy_windows_path_and_direct_cjs_path() {
        assert_eq!(
            command_path(r"C:\Users\tester\.codebuddy-rotate\helper.cmd"),
            Some(PathBuf::from(
                r"C:\Users\tester\.codebuddy-rotate\helper.cmd"
            ))
        );
        assert_eq!(
            command_path(r"C:\Users\test user\.codebuddy-rotate\helper.cjs"),
            Some(PathBuf::from(
                r"C:\Users\test user\.codebuddy-rotate\helper.cjs"
            ))
        );
        assert!(command_path("node helper.cjs").is_none());
    }

    #[test]
    fn windows_paths_compare_case_and_separator_insensitively() {
        assert!(same_path(
            Path::new(r"C:\Users\Tester\.codebuddy-rotate\helper.cmd"),
            Path::new("c:/users/tester/.codebuddy-rotate/helper.cmd"),
            true,
        ));
    }

    #[test]
    fn windows_space_path_requires_a_space_free_short_path() {
        let original = Path::new(r"C:\Users\Test User\.codebuddy-rotate\helper.cjs");
        assert_eq!(
            select_windows_configured_path(
                original,
                Some(PathBuf::from(
                    r"C:\Users\TESTUS~1\.codebuddy-rotate\helper.cjs"
                )),
            )
            .unwrap(),
            PathBuf::from(r"C:\Users\TESTUS~1\.codebuddy-rotate\helper.cjs")
        );
        let error = select_windows_configured_path(original, None).unwrap_err();
        assert!(error.contains("配置阶段"));
        assert!(error.contains("shell 不安全字符"));
    }

    #[test]
    fn windows_shell_metacharacters_also_require_a_safe_short_path() {
        for path in [
            r"C:\Users\Test&User\.codebuddy-rotate\helper.cjs",
            r"C:\Users\Test(User)\.codebuddy-rotate\helper.cjs",
            r#"C:\Users\Test'User\.codebuddy-rotate\helper.cjs"#,
        ] {
            assert!(!path_is_posix_eval_safe(Path::new(path)));
            assert!(select_windows_configured_path(Path::new(path), None).is_err());
        }
        assert!(path_is_posix_eval_safe(Path::new(
            r"C:\Users\TESTUS~1\.codebuddy-rotate\helper.cjs"
        )));
    }
