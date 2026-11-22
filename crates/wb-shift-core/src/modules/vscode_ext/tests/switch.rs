    #[test]
    fn ext_session_json_uses_uid_plus_token_and_vscode_id() {
        let acc = json!({
            "uid": "u-42",
            "nickname": "测试",
            "access_token": "tok-abc",
            "refresh_token": "rt-1",
            "domain": "www.codebuddy.cn",
            "expiresAt": 1234567890_i64,
        });
        let s = build_ext_session_json(&acc, None);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], "Tencent-Cloud.coding-copilot");
        assert_eq!(v["accessToken"], "u-42+tok-abc");
        assert_eq!(v["token"], "tok-abc");
        assert_eq!(v["auth"]["accessToken"], "tok-abc");
        assert_eq!(v["account"]["uid"], "u-42");
        assert_eq!(v["converted"], true);
        // 未登录既有 secret 时补 accounts 单元素数组
        assert_eq!(v["accounts"].as_array().map(|a| a.len()), Some(1));
    }

    /// WorkBuddy 产品域注入 CodeBuddy 扩展前必须规范化：否则扩展把域归为 `selfhosted`
    /// （该分支会读「企业版端点」设置）；企业自建域保持原样。
    #[test]
    fn ext_session_json_normalizes_workbuddy_domain() {
        let acc = json!({
            "uid": "u-9",
            "nickname": "张佳",
            "access_token": "tok-9",
            "domain": "www.workbuddy.cn",
        });
        let v: Value = serde_json::from_str(&build_ext_session_json(&acc, None)).unwrap();
        assert_eq!(v["domain"], "www.codebuddy.cn");
        assert_eq!(v["auth"]["domain"], "www.codebuddy.cn");

        let corp = json!({
            "uid": "u-c",
            "access_token": "tok-c",
            "domain": "corp.example.com",
        });
        let vc: Value = serde_json::from_str(&build_ext_session_json(&corp, None)).unwrap();
        assert_eq!(vc["domain"], "corp.example.com");
    }

    #[test]
    fn ext_session_json_merge_upserts_accounts_and_preserves_fields() {
        let acc = json!({
            "uid": "u-99",
            "nickname": "新账号",
            "access_token": "tok-new",
            "refresh_token": "rt-new",
            "domain": "www.codebuddy.cn",
            "expiresAt": 42_i64,
            "refreshExpiresAt": 1_700_000_000_000_i64,
        });
        let existing = r#"{"accounts":[{"uid":"old","lastLogin":true}],"id":"Tencent-Cloud.coding-copilot","craftSettings":{"x":1},"converted":true,"auth":{"accessToken":"tok-old","scope":"all","sessionState":"logged_in","notBeforePolicy":0,"refreshExpiresAt":111,"expiresIn":5184000}}"#;
        let s = build_ext_session_json(&acc, Some(existing));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["accessToken"], "u-99+tok-new");
        assert_eq!(v["account"]["uid"], "u-99");
        assert_eq!(v["account"]["lastLogin"], true);
        // 旧账号条目仍在，但被标记为非当前登录
        assert_eq!(v["accounts"][0]["uid"], "old");
        assert_eq!(v["accounts"][0]["lastLogin"], false);
        // 目标账号被追加并标为当前登录
        assert_eq!(v["accounts"][1]["uid"], "u-99");
        assert_eq!(v["accounts"][1]["lastLogin"], true);
        assert_eq!(v["accounts"].as_array().map(|a| a.len()), Some(2));
        // 扩展私有字段保留，不被覆盖
        assert_eq!(v["craftSettings"]["x"], 1);
        // auth 的扩展私有键原样保留（F4）
        assert_eq!(v["auth"]["scope"], "all");
        assert_eq!(v["auth"]["sessionState"], "logged_in");
        assert_eq!(v["auth"]["notBeforePolicy"], 0);
        // auth 的身份/凭据键被覆盖
        assert_eq!(v["auth"]["accessToken"], "tok-new");
        assert_eq!(v["auth"]["refreshToken"], "rt-new");
        assert_eq!(v["auth"]["tokenType"], "Bearer");
        assert_eq!(v["auth"]["domain"], "www.codebuddy.cn");
        assert_eq!(v["auth"]["expiresAt"], 42);
        // refreshExpiresAt 取账号库的值（不再写 0）
        assert_eq!(v["auth"]["refreshExpiresAt"], 1_700_000_000_000_i64);
        // expiresIn 刻意沿用「写绝对时间」的既有写法（与 CN IDE 已验证实现一致）
        assert_eq!(v["auth"]["expiresIn"], 42);
        assert_eq!(v["auth"]["refreshExpiresIn"], 0);
    }

    /// F4：账号库没有 `refreshExpiresAt` 时保留既有 auth 里的值（而非写 0）。
    #[test]
    fn ext_session_json_keeps_existing_auth_refresh_expires_at_without_account_value() {
        let acc = json!({
            "uid": "u-7",
            "nickname": "无 refreshExpiresAt",
            "access_token": "tok-7",
            "domain": "www.codebuddy.cn",
            "expiresAt": 7_i64,
        });
        let existing = r#"{"auth":{"refreshExpiresAt":777,"scope":"all"}}"#;
        let s = build_ext_session_json(&acc, Some(existing));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["auth"]["refreshExpiresAt"], 777);
        assert_eq!(v["auth"]["scope"], "all");
        assert_eq!(v["auth"]["accessToken"], "tok-7");
    }

    /// F4：既无账号库值也无既有 auth（未登录）时维持旧载荷形状（键在、值为 0）。
    #[test]
    fn ext_session_json_without_existing_auth_keeps_zero_refresh_expires_at() {
        let acc = json!({
            "uid": "u-8",
            "access_token": "tok-8",
            "domain": "www.codebuddy.cn",
            "expiresAt": 8_i64,
        });
        let s = build_ext_session_json(&acc, None);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["auth"]["refreshExpiresAt"], 0);
    }

    #[test]
    fn ext_session_json_upsert_replaces_existing_entry_in_place() {
        let acc = json!({
            "uid": "u-99",
            "nickname": "新账号",
            "access_token": "tok-new",
            "domain": "www.codebuddy.cn",
            "expiresAt": 7_i64,
        });
        let existing =
            r#"{"accounts":[{"uid":"u-99","lastLogin":false,"label":"旧"},{"uid":"other"}]}"#;
        let s = build_ext_session_json(&acc, Some(existing));
        let v: Value = serde_json::from_str(&s).unwrap();
        // 命中 uid：原地替换，不产生重复条目
        assert_eq!(v["accounts"].as_array().map(|a| a.len()), Some(2));
        assert_eq!(v["accounts"][0]["uid"], "u-99");
        assert_eq!(v["accounts"][0]["lastLogin"], true);
        assert_eq!(v["accounts"][1]["uid"], "other");
        assert_eq!(v["accounts"][1]["lastLogin"], false);
    }

    #[test]
    fn code_image_name_is_exact_not_insiders_or_codebuddy() {
        assert!(is_code_image_name("Code.exe"));
        assert!(is_code_image_name("code"));
        assert!(is_code_image_name(
            r"C:\Users\Zhou\AppData\Local\Programs\Microsoft VS Code\Code.exe"
        ));
        assert!(!is_code_image_name("Code - Insiders.exe"));
        assert!(!is_code_image_name("CodeBuddy CN.exe"));
        assert!(!is_code_image_name("CodeBuddy.exe"));
        assert!(!is_code_image_name("workbuddy-shift.exe"));
        assert!(!is_code_image_name("workbuddy-shift"));
    }

    #[test]
    fn windows_code_rows_drop_self_insiders_and_codebuddy() {
        let stdout = "\
2001|workbuddy-shift|C:\\apps\\workbuddy-shift.exe
2002|Code - Insiders|C:\\Users\\Zhou\\AppData\\Local\\Programs\\Microsoft VS Code Insiders\\Code - Insiders.exe
2003|Code|C:\\Users\\Zhou\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe
2004|crashpad_handler|C:\\x\\crashpad_handler.exe
2005|CodeBuddy CN|D:\\Programs\\CodeBuddy CN\\CodeBuddy CN.exe
2006|Code|C:\\Users\\Zhou\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe
";
        let kept: Vec<u32> = process::parse_windows_process_rows(stdout)
            .into_iter()
            .filter(keep_windows_code_row)
            .map(|row| row.pid)
            .collect();
        assert_eq!(kept, vec![2003, 2006]);
    }

    #[test]
    fn linux_cmdline_matcher_accepts_code_not_variants() {
        assert!(linux_cmdline_is_code("/usr/share/code/code --unity-launch"));
        assert!(linux_cmdline_is_code("/usr/bin/code --no-sandbox"));
        assert!(linux_exe_is_code(std::path::Path::new(
            "/usr/share/code/code"
        )));
        assert!(!linux_cmdline_is_code("/usr/bin/workbuddy-shift"));
        assert!(!linux_cmdline_is_code(
            "/usr/share/code/code --type=gpu-process"
        ));
        assert!(!linux_cmdline_is_code("/usr/bin/code-insiders"));
        assert!(!linux_cmdline_is_code("/opt/codebuddy-cn/codebuddy-cn"));
        assert!(!linux_exe_is_code(std::path::Path::new(
            "/usr/bin/code-insiders"
        )));
    }

    #[test]
    fn state_db_path_ends_with_state_vscdb() {
        let Some(db) = vscode_ext_state_db_path() else {
            return;
        };
        assert!(db.ends_with("state.vscdb"));
        assert!(db.to_string_lossy().contains("globalStorage"));
    }

    #[test]
    fn target_secret_key_matches_vscode_extension() {
        use crate::modules::vscode_cn_inject::secret_storage_item_key_for;
        assert_eq!(
            secret_storage_item_key_for(&VSCODE_TARGET),
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"Tencent-Cloud.coding-copilot.new.accessToken"}"#
        );
    }

    /// macOS 重开依赖关闭前从主进程 args 回溯的 `.app` bundle 路径。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_code_bundle_is_recovered_from_main_args() {
        assert_eq!(
            process::extract_app_bundle_from_args(
                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron -psn_0_123"
            ),
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );
        assert_eq!(
            process::extract_app_bundle_from_args(
                "/Users/zhou/Applications/Visual Studio Code.app/Contents/MacOS/Electron \
                 --type=renderer"
            ),
            Some(PathBuf::from(
                "/Users/zhou/Applications/Visual Studio Code.app"
            ))
        );
        // 非 VS Code 进程不得被当成目标
        assert_eq!(
            process::extract_app_bundle_from_args("/usr/bin/ssh host"),
            None
        );
    }

    /// 主进程模式必须只命中官方 VS Code（不含 Insiders / CodeBuddy 系）。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_code_main_patterns_match_code_not_variants() {
        let patterns = macos_code_main_patterns();
        let hits = |args: &str| patterns.iter().any(|p| args.contains(p.as_str()));
        assert!(hits(
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron -psn_0_1"
        ));
        assert!(!hits(
            "/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Electron"
        ));
        assert!(!hits(
            "/Applications/CodeBuddy CN.app/Contents/MacOS/CodeBuddy CN"
        ));
        assert!(!hits(
            "/Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy"
        ));
    }

    #[test]
    fn close_timeout_error_lists_pids_and_manual_hint() {
        let message = close_timeout_error(&[4242, 4243]);
        assert!(message.contains("4242, 4243"), "{message}");
        assert!(message.contains("请在 VS Code 中处理保存提示"), "{message}");
        // 等待期间被重新打开也会表现为「仍有进程」→ 文案要点出这条排查方向。
        assert!(message.contains("重新打开过 VS Code"), "{message}");
    }

    /// 超时文案点名窗口标题：PID 保留（便于排查），标题让用户认出是哪个窗口没关掉。
    #[test]
    fn close_timeout_error_with_titles_names_windows_and_keeps_pids() {
        let titles = vec![
            "main.ts - 项目 - Visual Studio Code".to_string(),
            // 重复 / 空白标题必须折叠，否则文案会变成噪音。
            "main.ts - 项目 - Visual Studio Code".to_string(),
            "   ".to_string(),
            "另一个窗口 - Visual Studio Code".to_string(),
        ];
        let message = close_timeout_error_with_titles(&[4242], &titles);
        assert!(message.contains("4242"), "{message}");
        assert!(
            message.contains("main.ts - 项目 - Visual Studio Code"),
            "{message}"
        );
        assert!(message.contains("另一个窗口 - Visual Studio Code"), "{message}");
        assert_eq!(
            message.matches("main.ts - 项目").count(),
            1,
            "重复标题只点名一次: {message}"
        );
        // 排查提示一字不改地保留（两条路径话术不能漂移）。
        assert!(message.contains("请在 VS Code 中处理保存提示"), "{message}");
        assert!(message.contains("重新打开过 VS Code"), "{message}");
        assert!(!message.contains("「   」"), "空白标题不得出现: {message}");
    }

    /// 拿不到窗口标题时必须退化到只报 PID（不能出现空的「窗口: 」）。
    #[test]
    fn close_timeout_error_with_titles_degrades_to_pids_only() {
        assert_eq!(
            close_timeout_error_with_titles(&[4242], &[]),
            close_timeout_error(&[4242])
        );
        assert_eq!(
            close_timeout_error_with_titles(&[4242], &[" ".to_string()]),
            close_timeout_error(&[4242])
        );
    }

    /// 标题过长（含完整文件路径）时截断，不把界面撑爆；也最多点名 3 个窗口。
    #[test]
    fn close_timeout_error_with_titles_truncates_and_caps() {
        let long = "a".repeat(500);
        let message = close_timeout_error_with_titles(&[1], &[long]);
        // 字符上限的唯一来源在 `process`（四条关闭路径共用），这里不重复写死数字。
        let cap = process::MAX_WINDOW_TITLE_CHARS;
        assert!(message.contains(&"a".repeat(cap)), "{message}");
        assert!(!message.contains(&"a".repeat(cap + 1)), "{message}");

        let many: Vec<String> = (0..6).map(|i| format!("窗口{i}")).collect();
        let message = close_timeout_error_with_titles(&[1], &many);
        assert!(message.contains("窗口0"), "{message}");
        assert!(message.contains("窗口2"), "{message}");
        assert!(!message.contains("窗口3"), "最多点名 3 个: {message}");
    }

    fn instance_with_pids(pids: Vec<u32>) -> RunningInstance {
        RunningInstance {
            pids,
            main_exe: None,
            macos_bundle: None,
        }
    }

    /// 关闭决策纯逻辑（注入实例，不碰真实进程）：
    /// 未运行 → `Ok(None)`（不主动拉起）；运行中 + 手动模式 → 报错且不关闭（AC10）。
    #[test]
    fn close_for_switch_decides_without_touching_processes() {
        assert!(
            close_vscode_for_switch_with(instance_with_pids(Vec::new()), false)
                .unwrap()
                .is_none()
        );
        assert!(
            close_vscode_for_switch_with(instance_with_pids(Vec::new()), true)
                .unwrap()
                .is_none()
        );

        let error =
            close_vscode_for_switch_with(instance_with_pids(vec![4242]), false).unwrap_err();
        assert_eq!(error, RUNNING_MANUAL_HINT);
    }

    /// 未运行时 [`close_vscode_for_switch`] 必须是空操作。本机可能真的开着 VS Code，
    /// 因此只在确认未运行时断言，其它情况跳过（**绝不在测试里关闭编辑器**）。
    #[test]
    fn close_for_switch_is_noop_when_vscode_not_running() {
        if is_vscode_running() {
            eprintln!("skip: 本机 VS Code 正在运行，无法安全断言 Ok(None)");
            return;
        }
        assert!(close_vscode_for_switch(false).unwrap().is_none());
    }

    /// 新入口 [`switch_account_after_close`] 的校验分支：账号不存在时返回可读错误，
    /// 且此时不会去读/写 state.vscdb（`None` 表示编辑器本来没运行）。
    #[test]
    fn switch_account_after_close_rejects_unknown_account() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let error = switch_account_after_close(&missing, None).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    /// 失败兜底（D4）：编辑器是本次我们关的 → 任何 Err 路径都要尽力开回来；本来没关的 → 一次都不许调。
    ///
    /// 重开动作由参数注入（真实实现会真的启动用户的 VS Code，测试里**绝不能**走到那一步）。
    #[test]
    fn error_paths_relaunch_only_editor_we_closed() {
        let closed = ClosedEditor(instance_with_pids(vec![4242]));
        let mut relaunch_calls = 0usize;

        // ① 编辑器是本次关的（如关闭后才发现的 access_token 为空 / 读 secret 失败）→ 必须开回来
        let error = relaunch_closed_on_error(
            Some(&closed),
            |_| {
                relaunch_calls += 1;
                Ok(())
            },
            "账号 access_token 为空".to_string(),
        );
        assert_eq!(relaunch_calls, 1, "关掉的编辑器必须被尽力开回来");
        assert_eq!(error, "账号 access_token 为空");

        // ② 编辑器本来没运行 → 不得尝试启动（否则会凭空拉起用户的 VS Code）
        let error = relaunch_closed_on_error(
            None,
            |_| {
                relaunch_calls += 1;
                Ok(())
            },
            "账号不存在".to_string(),
        );
        assert_eq!(relaunch_calls, 1, "未关闭编辑器时不得调用重开");
        assert_eq!(error, "账号不存在");

        // ③ 重开也失败 → 两条信息都要保留，用户才知道编辑器还关着
        let error = relaunch_closed_on_error(
            Some(&closed),
            |_| Err("重新打开 VS Code 失败: open 超时".to_string()),
            "注入登录状态失败".to_string(),
        );
        assert!(error.contains("注入登录状态失败"), "{error}");
        assert!(error.contains("open 超时"), "{error}");
    }

    /// 成功文案三态（design §5）：不再出现「重载窗口」这类与真实行为不符的提示。
    #[test]
    fn switch_message_matches_design_states() {
        let restarted = switch_message("测试号", true, None);
        assert!(restarted.contains("VS Code 已重新打开"), "{restarted}");
        assert!(!restarted.contains("重载"), "{restarted}");

        let relaunch_failed = switch_message("测试号", false, Some("open 超时"));
        assert!(
            relaunch_failed.contains("自动重新打开 VS Code 失败"),
            "{relaunch_failed}"
        );

        let not_running = switch_message("测试号", false, None);
        assert!(not_running.contains("请打开 VS Code 生效"), "{not_running}");
        assert!(!not_running.contains("重载"), "{not_running}");
    }
