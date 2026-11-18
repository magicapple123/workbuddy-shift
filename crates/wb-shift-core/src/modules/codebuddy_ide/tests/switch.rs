    #[test]
    fn session_json_includes_uid_plus_token() {
        let acc = json!({
            "uid": "u-42",
            "nickname": "测试",
            "access_token": "tok-abc",
            "refresh_token": "rt-1",
            "domain": "www.codebuddy.cn",
            "expiresAt": 1234567890_i64,
        });
        let s = build_session_json(&acc);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["accessToken"], "u-42+tok-abc");
        assert_eq!(v["token"], "tok-abc");
        assert_eq!(v["auth"]["accessToken"], "tok-abc");
        assert_eq!(v["account"]["uid"], "u-42");
        assert_eq!(v["id"], "Tencent-Cloud.genie-ide");
    }

    #[test]
    fn parse_token_from_uid_plus_form() {
        let (uid, token) = parse_token_from_secret("uid-1+ACCESS").unwrap();
        assert_eq!(uid.as_deref(), Some("uid-1"));
        assert_eq!(token, "ACCESS");
    }

    #[test]
    fn parse_token_from_session_json() {
        let secret = r#"{"token":"T1","accessToken":"u9+T1","account":{"uid":"u9"}}"#;
        let (uid, token) = parse_token_from_secret(secret).unwrap();
        assert_eq!(uid.as_deref(), Some("u9"));
        assert_eq!(token, "T1");
    }

    #[test]
    fn secret_key_helper_reexported_path() {
        let key = crate::modules::vscode_cn_inject::secret_storage_item_key_for(
            &crate::modules::vscode_cn_inject::CODEBUDDY_INTL_TARGET,
        );
        assert!(key.contains("planning-genie.new.accessToken"));
        assert!(!key.contains("accessTokencn"));
        assert!(key.starts_with("secret://"));
    }

    #[test]
    fn ide_image_name_is_codebuddy_not_cn() {
        assert!(is_codebuddy_ide_image_name("CodeBuddy.exe"));
        assert!(is_codebuddy_ide_image_name("CodeBuddy"));
        assert!(!is_codebuddy_ide_image_name("CodeBuddy CN.exe"));
        assert!(!is_codebuddy_ide_image_name("WorkBuddy.exe"));
        assert!(!is_codebuddy_ide_image_name("workbuddy-shift.exe"));
        assert!(is_codebuddy_ide_windows_exe(
            r"C:\Users\Zhou\AppData\Local\Programs\CodeBuddy\CodeBuddy.exe"
        ));
        assert!(!is_codebuddy_ide_windows_exe(
            r"C:\Users\Zhou\AppData\Local\Programs\CodeBuddy CN\CodeBuddy.exe"
        ));
        assert!(!is_codebuddy_ide_windows_exe(
            r"C:\Users\Zhou\AppData\Local\Programs\CodeBuddy CN\CodeBuddy CN.exe"
        ));
    }

    #[test]
    fn windows_ide_rows_drop_self_cn_and_crashpad() {
        let stdout = "\
1001|workbuddy-shift|C:\\apps\\workbuddy-shift.exe
1002|CodeBuddy CN|D:\\Programs\\CodeBuddy CN\\CodeBuddy CN.exe
1003|CodeBuddy|D:\\Users\\Zhou\\AppData\\Local\\Programs\\CodeBuddy\\CodeBuddy.exe
1004|crashpad_handler|C:\\x\\crashpad_handler.exe
1005|workbuddy-shift|
1006|CodeBuddy|C:\\Users\\Zhou\\AppData\\Local\\Programs\\CodeBuddy\\CodeBuddy.exe
1007|CodeBuddy|C:\\Users\\Zhou\\AppData\\Local\\Programs\\CodeBuddy CN\\CodeBuddy.exe
";
        let kept: Vec<u32> = process::parse_windows_process_rows(stdout)
            .into_iter()
            .filter(keep_windows_ide_row)
            .map(|row| row.pid)
            .collect();
        assert_eq!(kept, vec![1003, 1006]);
    }

    #[test]
    fn windows_ide_fallback_candidates_include_local_and_program_files() {
        let cands = windows_ide_fallback_exe_candidates(
            Some(r"C:\Users\Zhou\AppData\Local"),
            Some(r"C:\Program Files"),
            Some(r"C:\Program Files (x86)"),
            Some("Zhou"),
            &['C'],
        );
        let s: Vec<String> = cands
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(s
            .iter()
            .any(|p| p.contains("Programs") && p.contains("CodeBuddy.exe")));
        assert!(s
            .iter()
            .any(|p| p.contains("CodeBuddy") && p.contains("CodeBuddy.exe")));
        assert!(s
            .iter()
            .any(|p| p.contains("Program Files") && p.contains("CodeBuddy.exe")));
        assert!(!s.iter().any(|p| path_contains_codebuddy_cn_dir(p)));
    }

    #[test]
    fn linux_cmdline_matcher_accepts_intl_not_cn_or_switcher() {
        assert!(linux_cmdline_is_codebuddy_ide(
            "/opt/codebuddy/codebuddy --foo"
        ));
        assert!(linux_cmdline_is_codebuddy_ide("/usr/bin/CodeBuddy"));
        assert!(linux_exe_is_codebuddy_ide(Path::new("/usr/bin/codebuddy")));
        assert!(!linux_cmdline_is_codebuddy_ide("/usr/bin/workbuddy-shift"));
        assert!(!linux_cmdline_is_codebuddy_ide(
            "/opt/CodeBuddy CN/codebuddy-cn"
        ));
        assert!(!linux_cmdline_is_codebuddy_ide(
            "/opt/codebuddy/codebuddy --type=gpu-process"
        ));
        assert!(!linux_exe_is_codebuddy_ide(Path::new(
            "/usr/bin/codebuddy-cn"
        )));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ide_pattern_fallback_does_not_match_international_codebuddy() {
        assert_eq!(
            macos_ide_main_patterns(None),
            vec!["CodeBuddy.app/Contents/MacOS".to_string()]
        );
        assert_eq!(
            macos_ide_bundle_patterns(None),
            vec!["CodeBuddy.app".to_string()]
        );
        assert_eq!(
            macos_ide_main_patterns(Some(Path::new("/Applications/CodeBuddy.app"))),
            vec!["/Applications/CodeBuddy.app/Contents/MacOS".to_string()]
        );
        let cands = macos_ide_app_candidates(Path::new("/Users/tester"));
        let s: Vec<String> = cands
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            s,
            vec![
                "/Applications/CodeBuddy.app".to_string(),
                "/Users/tester/Applications/CodeBuddy.app".to_string(),
            ]
        );
        assert!(s.iter().all(|p| p.ends_with("CodeBuddy.app")));
        assert!(!s.iter().any(|p| p.contains("CodeBuddy CN")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ide_ps_filter_excludes_self_and_cn() {
        let self_pid = std::process::id();
        let stdout = format!(
            "{self_pid} /Applications/workbuddy-shift.app/Contents/MacOS/workbuddy-shift\n\
             6001 /Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy --foo\n\
             6002 /Applications/CodeBuddy CN.app/Contents/MacOS/CodeBuddy CN\n\
             6003 /bin/zsh -c 'echo CodeBuddy.app mention via workbuddy-shift'\n\
             6004 /Applications/CodeBuddy.app/Contents/Resources/helper\n"
        );
        let main_kept = process::filter_ps_rows(&stdout, &macos_ide_main_patterns(None), self_pid);
        let main_pids: Vec<u32> = main_kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(main_pids, vec![6001]);

        let bundle_kept =
            process::filter_ps_rows(&stdout, &macos_ide_bundle_patterns(None), self_pid);
        let bundle_pids: Vec<u32> = bundle_kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(bundle_pids, vec![6001, 6004]);
    }

    /// secret 命中：`accessToken` 为 `uid+token` 形态时取前缀作为 uid。
    #[test]
    fn active_uid_prefers_secret() {
        let secret = Some(r#"{"accessToken":"uid-secret+TOKEN"}"#.to_string());
        let uid = resolve_active_uid(secret, Some("acc-from-state".to_string()), |_| {
            Some("uid-from-account".to_string())
        });
        assert_eq!(uid.as_deref(), Some("uid-secret"));
    }

    /// secret 读不出 uid（或没有 secret）时回退状态文件记录的账号 id → 账号库 uid。
    #[test]
    fn active_uid_falls_back_to_state_then_account_library() {
        let state_only = resolve_active_uid(None, Some("acc-from-state".to_string()), |id| {
            (id == "acc-from-state").then(|| "uid-from-account".to_string())
        });
        assert_eq!(state_only.as_deref(), Some("uid-from-account"));

        // secret 存在但没有可解析的 uid：同样走回退链（与国内版 `active_cn_ide_uid` 同序）。
        let no_uid_in_secret = resolve_active_uid(
            Some(r#"{"token":"T1"}"#.to_string()),
            Some("acc-from-state".to_string()),
            |_| Some("uid-from-account".to_string()),
        );
        assert_eq!(no_uid_in_secret.as_deref(), Some("uid-from-account"));

        // 状态文件指向的账号已不在账号库：视为未登录。
        assert_eq!(
            resolve_active_uid(None, Some("acc-gone".to_string()), |_| None),
            None
        );
    }

    /// 非法 uid 一律视为未登录，且**不得**回退到状态文件顶替（否则会复制错账号树）。
    #[test]
    fn active_uid_rejects_unsafe_uid_without_fallback() {
        for uid in ["default", "Public", "a/b", "a\\b", ".."] {
            // 用 json! 序列化，保证反斜杠等字符按 JSON 规则转义（手拼字符串会变成 \b 转义符）。
            let secret = json!({ "accessToken": format!("{uid}+TOKEN") }).to_string();
            let from_secret =
                resolve_active_uid(Some(secret), Some("acc-from-state".to_string()), |_| {
                    Some("uid-from-account".to_string())
                });
            assert_eq!(from_secret, None, "secret uid={uid:?} 应视为未登录");
        }
        // 账号库回退拿到非法 uid 时同样拒绝。
        assert_eq!(
            resolve_active_uid(None, Some("acc".to_string()), |_| Some(
                "default".to_string()
            )),
            None
        );
    }
