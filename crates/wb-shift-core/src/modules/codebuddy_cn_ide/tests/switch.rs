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
        assert_eq!(v["id"], "Tencent-Cloud.genie-ide-cn");
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
        let key = crate::modules::vscode_cn_inject::secret_storage_item_key();
        assert!(key.contains("planning-genie.new.accessTokencn"));
        assert!(key.starts_with("secret://"));
    }

    #[test]
    fn cn_image_name_is_exact_not_international_codebuddy() {
        assert!(is_codebuddy_cn_image_name("CodeBuddy CN.exe"));
        assert!(is_codebuddy_cn_image_name("codebuddy cn"));
        assert!(is_codebuddy_cn_image_name(
            r"D:\Users\Zhou\AppData\Local\Programs\CodeBuddy CN\CodeBuddy CN.exe"
        ));
        assert!(!is_codebuddy_cn_image_name("CodeBuddy.exe"));
        assert!(!is_codebuddy_cn_image_name("CodeBuddy"));
        assert!(!is_codebuddy_cn_image_name("WorkBuddy.exe"));
        assert!(!is_codebuddy_cn_image_name("workbuddy-shift.exe"));
        assert!(!is_codebuddy_cn_image_name("workbuddy-shift"));
        assert!(is_codebuddy_cn_windows_exe(
            r"C:\Users\Zhou\AppData\Local\Programs\CodeBuddy CN\CodeBuddy.exe"
        ));
        assert!(!is_codebuddy_cn_windows_exe(
            r"C:\Users\Zhou\AppData\Local\Programs\CodeBuddy\CodeBuddy.exe"
        ));
    }

    #[test]
    fn windows_cn_rows_drop_self_international_and_crashpad() {
        let stdout = "\
1001|workbuddy-shift|C:\\apps\\workbuddy-shift.exe
1002|CodeBuddy|D:\\Programs\\CodeBuddy\\CodeBuddy.exe
1003|CodeBuddy CN|D:\\Users\\Zhou\\AppData\\Local\\Programs\\CodeBuddy CN\\CodeBuddy CN.exe
1004|crashpad_handler|C:\\x\\crashpad_handler.exe
1005|workbuddy-shift|
1006|CodeBuddy|C:\\Users\\Zhou\\AppData\\Local\\Programs\\CodeBuddy CN\\CodeBuddy.exe
";
        let kept: Vec<u32> = process::parse_windows_process_rows(stdout)
            .into_iter()
            .filter(keep_windows_cn_row)
            .map(|row| row.pid)
            .collect();
        assert_eq!(kept, vec![1003, 1006]);
    }

    #[test]
    fn windows_cn_fallback_candidates_include_local_and_program_files() {
        let cands = windows_cn_fallback_exe_candidates(
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
            .any(|p| p.contains("Programs") && p.contains("CodeBuddy CN.exe")));
        assert!(s
            .iter()
            .any(|p| p.contains("CodeBuddy CN") && p.contains("CodeBuddy.exe")));
        assert!(s
            .iter()
            .any(|p| p.contains("Program Files") && p.contains("CodeBuddy CN.exe")));
        assert!(!s
            .iter()
            .any(|p| { p.contains("CodeBuddy.exe") && !path_contains_codebuddy_cn_dir(p) }));
    }

    #[test]
    fn linux_cmdline_matcher_accepts_cn_not_switcher() {
        assert!(linux_cmdline_is_codebuddy_cn(
            "/opt/codebuddy-cn/codebuddy-cn --foo"
        ));
        assert!(linux_cmdline_is_codebuddy_cn("/usr/bin/CodeBuddy CN"));
        assert!(linux_exe_is_codebuddy_cn(Path::new(
            "/usr/bin/codebuddy-cn"
        )));
        assert!(!linux_cmdline_is_codebuddy_cn("/usr/bin/workbuddy-shift"));
        assert!(!linux_cmdline_is_codebuddy_cn(
            "/opt/codebuddy-cn/codebuddy-cn --type=gpu-process"
        ));
        assert!(!linux_exe_is_codebuddy_cn(Path::new("/usr/bin/codebuddy")));
    }

    #[test]
    fn linux_exe_matcher_accepts_real_package_names() {
        // deb 包（/usr/share/buddycn/buddycn）与 AppImage 命名都要认。
        assert!(linux_exe_is_codebuddy_cn(Path::new(
            "/usr/share/buddycn/buddycn"
        )));
        assert!(linux_exe_is_codebuddy_cn(Path::new(
            "/home/u/Applications/CodeBuddy_CN-1.2.3.AppImage"
        )));
        assert!(linux_exe_is_codebuddy_cn(Path::new(
            "/snap/bin/codebuddy-cn"
        )));
        assert!(linux_cmdline_is_codebuddy_cn(
            "/usr/share/buddycn/bin/buddycn --unity-launch"
        ));
        // 国际版与自家程序不算
        assert!(!linux_exe_is_codebuddy_cn(Path::new(
            "/opt/CodeBuddy/codebuddy"
        )));
        assert!(!linux_exe_is_codebuddy_cn(Path::new(
            "/opt/WorkBuddy/workbuddy"
        )));
        assert!(!linux_exe_is_codebuddy_cn(Path::new(
            "/home/u/Dev/workbuddy-shift/target/debug/workbuddy-shift-desktop"
        )));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_exec_program_drops_field_codes_and_env_prefix() {
        assert_eq!(
            linux_exec_program("/usr/share/buddycn/bin/buddycn %F").as_deref(),
            Some("/usr/share/buddycn/bin/buddycn")
        );
        assert_eq!(
            linux_exec_program("\"/opt/CodeBuddy CN/codebuddy-cn\" --flag %U").as_deref(),
            Some("/opt/CodeBuddy CN/codebuddy-cn")
        );
        assert_eq!(
            linux_exec_program("env FOO=1 /usr/bin/buddycn %U").as_deref(),
            Some("/usr/bin/buddycn")
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_desktop_entry_resolves_cn_and_skips_lookalikes() {
        let dir = std::env::temp_dir().join(format!("wb-cn-desktop-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("buddycn");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let write = |name: &str, name_field: &str, exec: &str| {
            let path = dir.join(name);
            let body =
                format!("[Desktop Entry]\nType=Application\nName={name_field}\nExec={exec}\n");
            std::fs::write(&path, body).unwrap();
            path
        };

        // 真实 deb 包条目：Exec=/usr/share/buddycn/bin/buddycn %F
        let cn = write(
            "buddycn.desktop",
            "CodeBuddy CN",
            &format!("{} %F", exe.display()),
        );
        assert_eq!(linux_cn_exe_from_desktop_entry(&cn), Some(exe.clone()));

        // URL handler 同名同程序，但需要 URL 参数才能启动，不能当主入口
        let url = write(
            "buddycn-url-handler.desktop",
            "CodeBuddy CN - URL Handler",
            &format!("{} --open-url %U", exe.display()),
        );
        assert_eq!(linux_cn_exe_from_desktop_entry(&url), None);

        // 国际版 CodeBuddy 与自家 workbuddy-shift 都不算
        let intl = write(
            "codebuddy.desktop",
            "CodeBuddy",
            &format!("{} %U", exe.display()),
        );
        assert_eq!(linux_cn_exe_from_desktop_entry(&intl), None);
        let own = write(
            "workbuddy-shift.desktop",
            "workbuddy-shift",
            &format!("{} %U", exe.display()),
        );
        assert_eq!(linux_cn_exe_from_desktop_entry(&own), None);

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 回归：真实发行包必须被发现并认成 CN。
    ///
    /// 只在装了 CN 桌面条目且条目可解析的机器上验证（CI 与未安装环境直接跳过）。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_finds_installed_cn_executable_from_desktop_entry() {
        let desktop_exe = linux_application_dirs().iter().find_map(|dir| {
            let entries = std::fs::read_dir(dir).ok()?;
            entries
                .flatten()
                .find_map(|entry| linux_cn_exe_from_desktop_entry(&entry.path()))
        });
        let Some(desktop_exe) = desktop_exe else {
            return;
        };
        assert!(
            linux_cn_exe_candidates().contains(&desktop_exe),
            "桌面条目指向的 {} 未出现在候选列表里",
            desktop_exe.display()
        );
        assert!(
            linux_exe_is_codebuddy_cn(&desktop_exe),
            "{} 未被认成 CodeBuddy CN",
            desktop_exe.display()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_cn_pattern_fallback_does_not_match_international_codebuddy() {
        assert_eq!(
            macos_cn_main_patterns(None),
            vec!["CodeBuddy CN.app/Contents/MacOS".to_string()]
        );
        assert_eq!(
            macos_cn_bundle_patterns(None),
            vec!["CodeBuddy CN.app".to_string()]
        );
        assert_eq!(
            macos_cn_main_patterns(Some(Path::new("/Applications/CodeBuddy CN.app"))),
            vec!["/Applications/CodeBuddy CN.app/Contents/MacOS".to_string()]
        );
        let cands = macos_cn_app_candidates(Path::new("/Users/tester"));
        let s: Vec<String> = cands
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            s,
            vec![
                "/Applications/CodeBuddy CN.app".to_string(),
                "/Users/tester/Applications/CodeBuddy CN.app".to_string(),
            ]
        );
        assert!(!s.iter().any(|p| p.ends_with("CodeBuddy.app")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_cn_ps_filter_excludes_self_and_international() {
        let self_pid = std::process::id();
        let stdout = format!(
            "{self_pid} /Applications/workbuddy-shift.app/Contents/MacOS/workbuddy-shift\n\
             6001 /Applications/CodeBuddy CN.app/Contents/MacOS/CodeBuddy CN --foo\n\
             6002 /Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy\n\
             6003 /bin/zsh -c 'echo CodeBuddy CN.app mention via workbuddy-shift'\n\
             6004 /Applications/CodeBuddy CN.app/Contents/Resources/helper\n"
        );
        let main_kept = process::filter_ps_rows(&stdout, &macos_cn_main_patterns(None), self_pid);
        let main_pids: Vec<u32> = main_kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(main_pids, vec![6001]);

        let bundle_kept =
            process::filter_ps_rows(&stdout, &macos_cn_bundle_patterns(None), self_pid);
        let bundle_pids: Vec<u32> = bundle_kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(bundle_pids, vec![6001, 6004]);
    }
