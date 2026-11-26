use super::*;
use companion_runtime::hook_script;

#[test]
fn cleanup_preserves_mixed_groups_and_opt_out_survives_restart() {
    let home = std::env::temp_dir().join(format!("integrations-{}", uuid::Uuid::new_v4()));
    for dir in [".codex", ".workbuddy-ai", ".codebuddy"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    let mut c = Collector::new(home.clone()).unwrap();
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        let file = files(&c, source)[0].clone();
        let mine = command(&c, source, &file);
        let other = json!({"type":"command","command":format!("echo {mine}"),"custom":42});
        atomic_json(&file,&json!({"custom":true,"hooks":{"Stop":[{"matcher":"custom","metadata":"keep","hooks":[{"command":mine},other.clone()]},{"hooks":[]}]}})).unwrap();
        set(&mut c, &json!({"source":source,"action":"uninstall"})).unwrap();
        let doc = read(&file).unwrap();
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"], json!([other]));
        assert_eq!(doc["hooks"]["Stop"][0]["metadata"], "keep");
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(doc["custom"], true);
    }
    let mut restarted = Collector::new(home.clone()).unwrap();
    restarted
        .request("settings_set", &restarted.settings.clone())
        .unwrap();
    install_hooks(&restarted).unwrap();
    assert!(get(&restarted)["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| !matches!(r["source"].as_str(), Some("codeg" | "dsh")))
        .all(|r| r["status"] == "not_installed" && r["automatic"] == false));
    // 第 5 个来源 DSH 是只读日志接入：目录不存在即「暂不可用」，
    // 而且 install / uninstall 都不能写盘（没有 hook 可装）。
    restarted.settings["sources"]["dsh"]["path"] = json!(home.join("no-dsh"));
    let dsh = get(&restarted)["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source"] == "dsh")
        .cloned()
        .unwrap();
    assert_eq!(dsh["kind"], "log");
    assert_eq!(dsh["status"], "unavailable");
    assert!(dsh["locations"].as_array().unwrap().is_empty());
    let entries_before = std::fs::read_dir(&home).unwrap().count();
    set(&mut restarted, &json!({"source":"dsh","action":"install"})).unwrap();
    assert_eq!(get(&restarted)["sources"][4]["automatic"], true);
    set(
        &mut restarted,
        &json!({"source":"dsh","action":"uninstall"}),
    )
    .unwrap();
    assert_eq!(get(&restarted)["sources"][4]["automatic"], false);
    assert_eq!(
        std::fs::read_dir(&home).unwrap().count(),
        entries_before,
        "DSH 接入不得在用户目录里新建任何文件"
    );
    // 有 sessions 目录时是「已连接（只读）」。
    std::fs::create_dir_all(home.join("no-dsh/sessions/-D-proj-/session-a")).unwrap();
    std::fs::write(
        home.join("no-dsh/sessions/-D-proj-/session-a/session.v4.jsonl.zstd"),
        b"",
    )
    .unwrap();
    let dsh = get(&restarted)["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source"] == "dsh")
        .cloned()
        .unwrap();
    assert_eq!(dsh["status"], "installed");
    assert_eq!(dsh["locations"].as_array().unwrap().len(), 1);
    set(
        &mut restarted,
        &json!({"source":"codex","action":"install"}),
    )
    .unwrap();
    assert_eq!(get(&restarted)["sources"][0]["status"], "installed");
    let codex_file = home.join(".codex/hooks.json");
    let codex_hooks = read(&codex_file).unwrap();
    let installed_command = command(&restarted, "codex", &codex_file);
    for event in CODEX_EVENTS {
        // 第一组是测试中预置的第三方 hook；新装的命令在最后一组。
        let installed = codex_hooks["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
            .find(|h| h["command"] == installed_command)
            .unwrap_or_else(|| panic!("{event} 缺少已安装的 hook"));
        assert_eq!(
            installed["async"],
            Value::Null,
            "{event} must complete before Codex leaves the turn"
        );
        assert_eq!(installed["timeout"], 3, "Codex 结束事件仍需等待采集器确认");
    }
    std::fs::write(home.join(".codex/hooks.json"), "malformed").unwrap();
    assert_eq!(get(&restarted)["sources"][0]["status"], "error");
    assert!(set(
        &mut restarted,
        &json!({"source":"codex","action":"uninstall"})
    )
    .is_err());
    assert!(!Collector::new(home.clone())
        .unwrap()
        .integration_automatic("codex"));
    assert_eq!(
        std::fs::read_to_string(home.join(".codex/hooks.json")).unwrap(),
        "malformed"
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn install_removes_legacy_and_edition_variants_but_keeps_foreign_hooks() {
    let home = std::env::temp_dir().join(format!("integrations-legacy-{}", uuid::Uuid::new_v4()));
    for dir in [".workbuddy", ".codebuddy"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    // `~/.workbuddy` 只有目录还不够：适配器要求 settings.json 或 workbuddy.db
    // 才认定它是国内版 WorkBuddy 的配置目录。
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    let c = Collector::new(home.clone()).unwrap();
    for source in ["workbuddy", "codebuddy-ide"] {
        let file = files(&c, source)[0].clone();
        let mine = command(&c, source, &file);
        // v0.4.2 的 exe 命令：升级后必须被换成脚本，否则 10 条 exe 命令会继续触发超时。
        let exe_v042 = hook_command(&binary(&c), &c.home, source, edition_of(source, &file));
        let no_edition = hook_command(&binary(&c), &c.home, source, None);
        let other_edition = hook_command(&binary(&c), &c.home, source, Some("domestic"));
        let legacy = hook_command(
            &home.join(".agent-studio/bin/agent-studio-runtime-v1"),
            &c.home,
            source,
            None,
        );
        let third_party = json!({"type":"command","command":format!("echo {mine}"),"custom":42});
        // 积分签到 rate_limit_hook 的真实命令形态（Unix / Windows）必须原样保留。
        let hook_sh = json!({"type":"command","command":"bash hook.sh"});
        let rate_limit_bash = json!({
            "type":"command",
            "command":"bash '/c/Users/example/.workbuddy-shift/hook.sh'"
        });
        let rate_limit_cmd = json!({
            "type":"command",
            "command":"cmd /c \"C:\\Users\\example\\.workbuddy-shift\\hook.cmd\""
        });
        atomic_json(
            &file,
            &json!({"custom":true,"hooks":{"Stop":[{"matcher":"","metadata":"keep","hooks":[
                {"command":mine},{"command":exe_v042},{"command":no_edition},
                {"command":other_edition},{"command":legacy},third_party.clone(),
                hook_sh.clone(),rate_limit_bash.clone(),rate_limit_cmd.clone()
            ]}]}}),
        )
        .unwrap();
        edit(&c, source, true).unwrap();
        // 重复安装必须幂等：不得为同一事件追加第二条脚本命令。
        edit(&c, source, true).unwrap();
        let doc = read(&file).unwrap();
        let groups = doc["hooks"]["Stop"].as_array().unwrap();
        // 原来的分组只留下第三方 / 积分签到 hook；新装的命令追加为独立分组。
        let hooks = groups[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 4, "{source}: {hooks:?}");
        assert!(hooks.contains(&third_party), "第三方 hook 必须原样保留");
        assert!(hooks.contains(&hook_sh), "积分签到 hook.sh 必须原样保留");
        assert!(
            hooks.contains(&rate_limit_bash) && hooks.contains(&rate_limit_cmd),
            "rate_limit_hook 的真实注册形态必须原样保留"
        );
        let installed: Vec<&Value> = groups
            .iter()
            .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
            .filter(|h| h["command"] == mine)
            .collect();
        assert_eq!(installed.len(), 1, "{source} 幂等安装不得重复注册");
        let installed = installed[0];
        assert_eq!(installed["timeout"], 3, "宿主预算是 3000ms，不得再收紧");
        // 注册的是脚本，不再是 5MB 的 exe（issue #20 回归的直接原因）。
        assert!(
            mine.starts_with("bash ") || mine.starts_with("sh "),
            "{mine}"
        );
        assert!(!mine.contains("companion-runtime"), "{mine}");
        let script = hook_script::script_path(&c.home, source, edition_of(source, &file).unwrap());
        let body = std::fs::read_to_string(&script).expect("安装必须生成 hook 脚本");
        assert!(body.contains(companion_runtime::hook_frame::SOURCE_DIRECTIVE));
        assert!(body.contains("printf '{}'"));
        if source == "workbuddy" {
            assert_eq!(installed["async"], true);
        }
        assert_eq!(doc["hooks"]["Stop"][0]["metadata"], "keep");
        assert_eq!(doc["custom"], true);
        assert!(!doc.to_string().contains("agent-studio-runtime"));
        assert!(!doc.to_string().contains("companion-runtime"));
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn uninstall_all_hooks_clears_both_brands_and_blocks_auto_reinstall() {
    let home =
        std::env::temp_dir().join(format!("integrations-uninstall-{}", uuid::Uuid::new_v4()));
    for dir in [".workbuddy", ".codebuddy", ".codex", ".agent-studio"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    let mut c = Collector::new(home.clone()).unwrap();
    install_hooks(&c).unwrap();
    let workbuddy_file = files(&c, "workbuddy")[0].clone();
    let workbuddy_script = hook_script::script_path(&c.home, "workbuddy", "domestic");
    assert!(workbuddy_script.is_file(), "安装必须落地脚本");
    let legacy = hook_command(
        &home.join(".agent-studio/bin/agent-studio-runtime-v1"),
        &c.home,
        "workbuddy",
        None,
    );
    // v0.4.2 遗留在配置里的 exe 命令（用户机器上实际是 10 条）。
    let exe_v042 = hook_command(&binary(&c), &c.home, "workbuddy", Some("domestic"));
    let mut doc = read(&workbuddy_file).unwrap();
    doc["hooks"]["UserPromptSubmit"]
        .as_array_mut()
        .unwrap()
        .push(json!({"matcher":"","hooks":[{"type":"command","command":legacy}]}));
    doc["hooks"]["Stop"].as_array_mut().unwrap().push(json!({
        "matcher":"","hooks":[{"type":"command","command":exe_v042}]
    }));
    doc["hooks"]["Stop"].as_array_mut().unwrap().push(json!({
        "matcher":"","hooks":[{"type":"command","command":"bash hook.sh"}]
    }));
    doc["hooks"]["Stop"].as_array_mut().unwrap().push(json!({
        "matcher":"","hooks":[{
            "type":"command",
            "command":"bash '/c/Users/example/.workbuddy-shift/hook.sh'"
        }]
    }));
    atomic_json(&workbuddy_file, &doc).unwrap();
    atomic_json(
        &home.join(".agent-studio/settings.json"),
        &json!({"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":legacy}]}]}}),
    )
    .unwrap();
    uninstall_all_hooks(&mut c).unwrap();
    let after = read(&workbuddy_file).unwrap().to_string();
    assert!(!after.contains("companion-runtime"), "{after}");
    assert!(!after.contains("agent-studio-runtime"), "{after}");
    assert!(!after.contains("hook-workbuddy-domestic.sh"), "{after}");
    assert!(after.contains("bash hook.sh"));
    assert!(after.contains(".workbuddy-shift/hook.sh"));
    // 脚本文件一并删除；spool 队列与积分签到的数据一律保留。
    assert!(!workbuddy_script.exists(), "卸载必须删除生成的脚本");
    assert!(!hook_script::scripts_dir(&c.home).exists());
    assert!(!read(&home.join(".agent-studio/settings.json"))
        .unwrap()
        .to_string()
        .contains("agent-studio-runtime"));
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        assert!(
            !c.integration_automatic(source),
            "{source} 必须退出自动接入"
        );
    }
    let restarted = Collector::new(home.clone()).unwrap();
    install_hooks(&restarted).unwrap();
    assert!(!read(&workbuddy_file)
        .unwrap()
        .to_string()
        .contains("companion-runtime"));
    // opt-out 落盘：重启后不得静默重装脚本。
    assert!(!hook_script::script_path(&home, "workbuddy", "domestic").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_unquoted_spelling_of_our_script_is_still_claimed_by_ownership() {
    // 逐字节比对是主判据；同一条命令的另一种书写（少一层引号）也必须认领，
    // 否则会被当成第三方 hook 保留下来 ⇒ 同一事件触发两次。
    let home = std::env::temp_dir().join(format!("integrations-claim-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join(".workbuddy")).unwrap();
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    let c = Collector::new(home.clone()).unwrap();
    let file = files(&c, "workbuddy")[0].clone();
    let script = hook_script::script_path(&c.home, "workbuddy", "domestic");
    let unquoted = format!(
        "bash {}",
        script
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "")
    );
    assert_ne!(unquoted, command(&c, "workbuddy", &file));
    assert!(owned(&c, "workbuddy", &file, &json!({"command": unquoted})));
    // 红线不受影响：积分签到与第三方命令永远不进删除判定。
    for keep in [
        "bash hook.sh",
        "bash '/c/Users/example/.workbuddy-shift/hook.sh'",
        "node '/c/Users/example/plugins/tracker/index.js'",
    ] {
        assert!(
            !owned(&c, "workbuddy", &file, &json!({"command": keep})),
            "{keep} 不得被认领"
        );
    }
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn a_failed_script_removal_does_not_hide_the_cleared_registration() {
    // P3-5：脚本删不掉时不得让调用方以为「整个卸载被回滚了」。配置此刻已经落盘，
    // 错误语义必须可读成「注册项已清、脚本可能残留」。
    let home = std::env::temp_dir().join(format!(
        "integrations-uninstall-partial-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(home.join(".workbuddy")).unwrap();
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    let mut c = Collector::new(home.clone()).unwrap();
    edit(&c, "workbuddy", true).unwrap();
    let file = files(&c, "workbuddy")[0].clone();
    assert!(contains(
        &c,
        "workbuddy",
        &file,
        &read(&file).unwrap()["hooks"]["Stop"]
    ));
    // 把脚本路径换成「非空目录」：`remove_file` 必然失败，模拟脚本删不掉。
    let script = hook_script::script_path(&c.home, "workbuddy", "domestic");
    std::fs::remove_file(&script).unwrap();
    std::fs::create_dir_all(script.join("busy")).unwrap();

    let error = set(&mut c, &json!({"source":"workbuddy","action":"uninstall"})).unwrap_err();
    assert!(
        error.contains("hook-workbuddy-domestic.sh"),
        "错误必须点名残留的脚本：{error}"
    );
    // 关键断言：注册项已经清干净，错误只描述残留，不是整体回滚。
    let doc = read(&file).unwrap();
    for event in events("workbuddy") {
        assert!(
            !contains(&c, "workbuddy", &file, &doc["hooks"][event]),
            "{event} 注册项必须已清：{doc}"
        );
    }
    assert!(
        !c.integration_automatic("workbuddy"),
        "卸载必须落盘 opt-out（否则重启会静默重装）"
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn both_editions_register_their_own_script_and_status_follows_it() {
    let home = std::env::temp_dir().join(format!("integrations-run-{}", uuid::Uuid::new_v4()));
    for dir in [".workbuddy", ".workbuddy-ai"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    std::fs::write(home.join(".workbuddy-ai/settings.json"), "{}").unwrap();
    let c = Collector::new(home.clone()).unwrap();
    edit(&c, "workbuddy", true).unwrap();
    // 两份 edition 各一份脚本：国际版不能串写国内版的来源。
    let domestic = hook_script::script_path(&c.home, "workbuddy", "domestic");
    let international = hook_script::script_path(&c.home, "workbuddy", "international");
    assert!(domestic.is_file() && international.is_file());
    assert_ne!(
        std::fs::read_to_string(&domestic).unwrap(),
        std::fs::read_to_string(&international).unwrap()
    );
    let files_registered: Vec<String> = files(&c, "workbuddy")
        .iter()
        .map(|file| command(&c, "workbuddy", file))
        .collect();
    assert_eq!(files_registered.len(), 2);
    assert!(files_registered
        .iter()
        .any(|command| command.contains("hook-workbuddy-international.sh")));
    assert!(files_registered
        .iter()
        .any(|command| command.contains("hook-workbuddy-domestic.sh")));
    // 状态判定看脚本是否就位：脚本被删 → partial / not_installed 而不是 installed。
    assert_eq!(get(&c)["sources"][1]["status"], "installed");
    std::fs::remove_file(&domestic).unwrap();
    assert_eq!(get(&c)["sources"][1]["status"], "partial");
    std::fs::remove_dir_all(home).unwrap();
}
