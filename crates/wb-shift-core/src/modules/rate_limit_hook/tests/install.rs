    fn layout() -> HookLayout {
        let base = std::env::temp_dir().join(format!(
            "workbuddy-shift-hook-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&base).expect("临时主目录");
        HookLayout::under(&base)
    }

    /// 路径里带空格 / 中文 / 单引号的临时主目录：这些都是合法且真实存在的用户目录形态。
    fn awkward_layout() -> HookLayout {
        let base = std::env::temp_dir().join(format!(
            "wb switch 中文 o'brien {}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&base).expect("临时主目录");
        HookLayout::under(&base)
    }

    /// 删除临时主目录（`HookLayout` 不持有 Drop，测试结束手动清理）。
    fn cleanup(layout: &HookLayout) {
        if let Some(base) = layout.script.parent().and_then(Path::parent) {
            let _ = std::fs::remove_dir_all(base);
        }
    }

    /// 真正执行生成的脚本：`sh <script>`，把 payload 从 stdin 喂进去，返回（stdout, stderr, 退出码）。
    #[cfg(unix)]
    fn run_script(script: &Path, payload: &str) -> (String, String, i32) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("sh")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh 必须可用");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("写入 payload");
        let output = child.wait_with_output().expect("等待脚本结束");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.code().unwrap_or(-1),
        )
    }

    fn target<'a>(layout: &'a HookLayout, label: &str) -> &'a HookTarget {
        layout
            .targets
            .iter()
            .find(|target| target.label == label)
            .expect("目标配置必须存在")
    }

    fn write_settings(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("父目录")).expect("配置目录");
        std::fs::write(path, content).expect("写入配置");
    }

    /// 「本机装了该客户端」：创建数据根目录（存在性判据）。
    fn install_client(layout: &HookLayout, label: &str) {
        std::fs::create_dir_all(&target(layout, label).data_root).expect("客户端数据根");
    }

    /// 临时目录下的限额监听配置路径（绝不触碰真实 `~/.workbuddy-shift`）。
    fn config_path(layout: &HookLayout) -> PathBuf {
        layout.events.with_file_name("rate_limit_config.json")
    }

    #[test]
    fn script_body_appends_payload_and_always_returns_empty_object() {
        // 事件路径必须写死为绝对路径：执行器环境不可控，env 不可靠（2026-09-18 实证）。
        let events = Path::new("/tmp/base")
            .join(".workbuddy-shift")
            .join("hook-events.jsonl");
        let body = script_body(&events);
        assert!(body.contains("hook-events.jsonl"), "{body}");
        assert!(!body.contains("USERPROFILE"), "不得依赖环境变量：{body}");
        assert!(!body.contains("$HOME"), "不得依赖环境变量：{body}");
        // 路径必须经 shell_quote 包裹（含空格 / 中文 / 单引号都不能破坏重定向）。
        assert!(
            body.contains(&format!(
                ">> {}",
                shell_quote(&events.to_string_lossy().replace('\\', "/"))
            )),
            "{body}"
        );
        assert!(body.trim_end().ends_with("printf '{}'"), "{body}");
        // 不再注入 `_hookTs`：事件行就是原样 payload（模型归因只认当次 payload）。
        assert!(!body.contains("_hookTs"), "{body}");
        assert!(!body.contains("date"), "{body}");
    }

    #[test]
    fn hook_command_quotes_the_script_path() {
        let interpreter = if cfg!(windows) { "bash" } else { "sh" };
        let path = Path::new("/Users/a b/.workbuddy-shift/hook.sh");
        assert_eq!(
            hook_command(path),
            format!("{interpreter} '/Users/a b/.workbuddy-shift/hook.sh'")
        );
        // 单引号路径按 POSIX 写法转义 —— 裸拼会破坏引号配对，事件被静默丢弃
        // （2026-09-20 审查反例）。
        let quoted = Path::new("/Users/o'brien/.workbuddy-shift/hook.sh");
        assert_eq!(
            hook_command(quoted),
            format!("{interpreter} '/Users/o'\\''brien/.workbuddy-shift/hook.sh'")
        );
    }

    /// marker 必须与注册命令里的路径形态逐字节一致：否则含引号的路径会判定为「未注册」，
    /// 每次启动重复追加条目（2026-09-20 审查反例）。
    #[test]
    fn marker_matches_the_registered_command() {
        let layout = awkward_layout();
        let command = hook_command(&layout.script);
        assert!(
            command.contains(&layout.marker()),
            "marker {} 必须命中命令 {command}",
            layout.marker()
        );
        cleanup(&layout);
    }

    /// Windows 的注册命令用 bash + MSYS 路径（与插件 hook 同款）；
    /// `cmd /c` 在 HookExecutor 的 bash 环境里静默失败（2026-09-18 探针实证）。
    #[test]
    #[cfg(windows)]
    fn hook_command_uses_bash_on_windows() {
        let path = Path::new(r"C:\Users\a b\.workbuddy-shift\hook.sh");
        assert_eq!(
            hook_command(path),
            "bash '/c/Users/a b/.workbuddy-shift/hook.sh'"
        );
        let quoted = Path::new(r"C:\Users\o'brien\.workbuddy-shift\hook.sh");
        assert_eq!(
            hook_command(quoted),
            "bash '/c/Users/o'\\''brien/.workbuddy-shift/hook.sh'"
        );
    }

    /// 生成的脚本必须**真正跑得起来**：原样 append payload、stdout 恰好是 `{}`
    /// （客户端据此判定 hook 成功）。只断言脚本文本包含路径是不够的。
    #[cfg(unix)]
    #[test]
    fn generated_script_appends_the_payload_and_returns_empty_object() {
        let layout = awkward_layout();
        // 数据根存在 → `install_at` 会生成脚本并写入三处配置。
        install_client(&layout, "codebuddy");
        install_at(&layout).expect("安装");

        let payload = json!({
            "session_id": "s-1",
            "transcript_path": "/tmp/projects/s-1.jsonl",
            "hook_event_name": "Stop",
            "model": "hy3",
            "last_assistant_message": "429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置",
        })
        .to_string();
        let (stdout, stderr, code) = run_script(&layout.script, &payload);
        assert_eq!(stdout, "{}", "hook 必须回 `{{}}`（空 stdout 会被判失败）");
        assert_eq!(code, 0, "stderr：{stderr}");
        assert!(stderr.is_empty(), "路径转义正确时不该有 stderr：{stderr}");

        let written = std::fs::read_to_string(&layout.events).expect("事件文件必须被写入");
        assert_eq!(written, format!("{payload}\n"), "事件行必须是原样 payload");
        // JSON 保真（引号 / 中文 / 空格都不被 shell 改写）。
        let parsed: Value = serde_json::from_str(written.trim_end()).expect("事件行仍是合法 JSON");
        assert_eq!(parsed["model"], "hy3");

        // 大 payload（429 的完整助手消息可达数百 KB）同样一次写入、单行保真。
        let big = json!({
            "session_id": "s-2",
            "model": "hy3",
            "last_assistant_message": "429 ".repeat(150_000),
        })
        .to_string();
        let (stdout, stderr, code) = run_script(&layout.script, &big);
        assert_eq!(stdout, "{}");
        assert_eq!(code, 0, "stderr：{stderr}");
        let written = std::fs::read_to_string(&layout.events).expect("事件文件");
        assert_eq!(written, format!("{payload}\n{big}\n"));

        cleanup(&layout);
    }

    /// 单引号 / 空格 / 中文路径下安装必须幂等：marker 与注册命令形态一致，不会反复追加条目，
    /// 并且脚本真的能写入事件文件（2026-09-20 审查：单引号路径曾静默丢事件）。
    #[cfg(unix)]
    #[test]
    fn install_is_idempotent_for_awkward_paths_and_the_script_writes_events() {
        let layout = awkward_layout();
        install_client(&layout, "codebuddy");
        write_settings(&target(&layout, "codebuddy").settings, "{}");

        for _ in 0..2 {
            install_at(&layout).expect("安装必须成功");
        }
        let root = read_settings(&target(&layout, "codebuddy").settings).expect("配置");
        assert_eq!(
            root["hooks"]["Stop"].as_array().expect("Stop 数组").len(),
            1,
            "含引号 / 空格的路径不得重复追加条目：{root}"
        );
        assert!(layout.fully_installed());

        let payload = r#"{"session_id":"s-3","model":"hy3"}"#;
        let (stdout, _, _) = run_script(&layout.script, payload);
        assert_eq!(stdout, "{}");
        assert_eq!(
            std::fs::read_to_string(&layout.events).expect("事件文件"),
            format!("{payload}\n")
        );

        cleanup(&layout);
    }

    /// 旧模板升级 / 用户改过的脚本保护：
    /// - 已知旧模板（依赖 `$HOME` 的 sh 脚本）→ 安装时升级为当前模板；
    /// - 内容未知（用户改过）→ 安装不覆盖、卸载不删除，并明确报错。
    #[test]
    fn install_upgrades_known_templates_and_never_overwrites_user_edits() {
        let layout = layout();
        install_client(&layout, "codebuddy");
        std::fs::create_dir_all(layout.script.parent().expect("store")).expect("store 目录");

        // ① 已知旧模板 → 升级。
        std::fs::write(&layout.script, LEGACY_SH_BODY).expect("写旧脚本");
        install_at(&layout).expect("旧模板升级必须成功");
        assert_eq!(
            std::fs::read_to_string(&layout.script).expect("脚本"),
            script_body(&layout.events),
            "已知旧模板必须升级为当前模板"
        );

        // ② 用户改过的脚本 → 不覆盖、不删除，并报出。
        let mine = "#!/bin/sh\necho mine\n";
        std::fs::write(&layout.script, mine).expect("写用户脚本");
        let error = install_at(&layout).expect_err("未知内容不得静默覆盖");
        assert!(error.contains("未覆盖"), "{error}");
        assert_eq!(std::fs::read_to_string(&layout.script).expect("脚本"), mine);
        // 「已接管」的脚本不再触发自动重装。
        assert!(
            !auto_install_at(&layout, true, false),
            "用户改过的脚本按已接管处理，不反复重装"
        );
        uninstall_at(&layout).expect("卸载");
        assert_eq!(
            std::fs::read_to_string(&layout.script).expect("脚本仍应保留"),
            mine
        );
    }

    /// 旧版 `hook.cmd` 只在**内容匹配已知生成版本**时清理；用户改写过的必须保留
    /// （2026-09-20 审查反例：仅凭 `is_file()` 无条件删除）。
    #[test]
    fn legacy_cmd_script_is_removed_only_when_it_matches_the_known_template() {
        let layout = layout();
        let legacy = layout.script.with_file_name(LEGACY_CMD_NAME);
        std::fs::create_dir_all(layout.script.parent().expect("store")).expect("store 目录");

        std::fs::write(&legacy, LEGACY_CMD_BODY).expect("写旧 cmd 脚本");
        uninstall_at(&layout).expect("卸载");
        assert!(!legacy.exists(), "本工具生成的旧 cmd 脚本随卸载清理");

        let mine = "@echo off\r\necho mine\r\n";
        std::fs::write(&legacy, mine).expect("写用户 cmd 脚本");
        uninstall_at(&layout).expect("卸载");
        assert_eq!(
            std::fs::read_to_string(&legacy).expect("用户脚本必须保留"),
            mine
        );

        cleanup(&layout);
    }

    #[test]
    fn legacy_migration_only_removes_the_current_tools_hook_command() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        let legacy = format!("cmd /c \"{}\"", layout.legacy_marker());
        let current = hook_command(&layout.script);
        let third_party = format!(
            "cmd /c \"{}-third-party/hook.cmd\"",
            layout.script.parent().expect("store").display()
        );
        write_settings(
            &codebuddy.settings,
            &json!({
                "hooks": {
                    "Stop": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": legacy }] },
                        { "matcher": "", "hooks": [{ "type": "command", "command": third_party }] },
                    ]
                }
            })
            .to_string(),
        );

        install_at(&layout).expect("安装必须成功");

        let root = read_settings(&codebuddy.settings).expect("配置");
        let stop = root["hooks"]["Stop"].as_array().expect("Stop 数组");
        assert_eq!(stop.len(), 2, "旧条目应替换为当前条目，第三方条目保留");
        assert!(
            stop.iter().any(|entry| {
                entry["hooks"][0]["command"].as_str() == Some(third_party.as_str())
            }),
            "第三方 hook 不得被迁移逻辑认领"
        );
        assert!(
            stop.iter()
                .any(|entry| entry["hooks"][0]["command"].as_str() == Some(current.as_str())),
            "本工具旧 hook 应替换为当前命令"
        );

        cleanup(&layout);
    }

    #[test]
    fn install_is_idempotent_and_preserves_third_party_hooks() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        write_settings(
            &codebuddy.settings,
            &serde_json::to_string_pretty(&json!({
                "model": "deepseek-v4.1-flash",
                "hooks": {
                    "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "echo user" }] }],
                    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo tool" }] }],
                },
            }))
            .expect("序列化"),
        );

        for _ in 0..2 {
            install_at(&layout).expect("安装必须成功");
        }

        let root = read_settings(&codebuddy.settings).expect("配置可解析");
        let stop = root["hooks"]["Stop"].as_array().expect("Stop 数组");
        assert_eq!(stop.len(), 2, "只应追加一条我们的条目：{stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "echo user", "用户条目保留");
        // 全平台统一 sh 脚本名（Windows 执行器是 bash，不再用 cmd）。
        assert!(stop[1]["hooks"][0]["command"]
            .as_str()
            .expect("命令")
            .contains(SCRIPT_NAME_SH));
        assert_eq!(
            root["hooks"]["FinalStop"]
                .as_array()
                .expect("FinalStop 数组")
                .len(),
            1
        );
        assert_eq!(
            root["hooks"]["PreToolUse"]
                .as_array()
                .expect("PreToolUse 数组")
                .len(),
            1,
            "其它事件不得被动"
        );
        assert_eq!(root["model"], "deepseek-v4.1-flash");
        // 未注册的事件一个都不能多。
        assert_eq!(
            root["hooks"].as_object().expect("hooks 对象").len(),
            3,
            "只有 Stop / FinalStop 是本工具新增的"
        );
    }
