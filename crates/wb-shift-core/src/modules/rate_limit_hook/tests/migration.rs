    // 历史形态的识别与迁移：上一代数据目录 / 同代模板 / 旧注册条目。
    //
    // 这一组的共同前提：**不写死旧目录名**，一律经实现里的 `LEGACY_STORE_DIRS` 与
    // `legacy_scripts()` 推导，改名或下线旧目录时测试跟着走。

    /// 上一代数据目录里的 hook 脚本路径（不写死目录名：用实现里的登记表推导）。
    fn legacy_script(layout: &HookLayout) -> PathBuf {
        layout
            .legacy_scripts()
            .into_iter()
            .next()
            .expect("必须登记至少一处上一代目录")
    }

    /// 「同代模板、只是把事件写到了别的目录」必须被认成**本工具的旧模板**。
    ///
    /// 认不出的代价（2026-11-17 本机实证）：`script_ownership` 返回 `Foreign`
    /// ⇒ `write_script` 永久失败 ⇒ `install_at` 在写 `settings.json` 之前就返回
    /// ⇒ hook 再也不会被注册，限额只能靠日志扫描兜底（卡片缺 chip、模型名归因失败）。
    #[test]
    fn same_generation_template_pointing_elsewhere_is_recognised_as_outdated() {
        let layout = layout();
        let base = layout
            .script
            .parent()
            .and_then(Path::parent)
            .expect("主目录");
        let legacy_events = base.join(LEGACY_STORE_DIRS[0]).join(EVENTS_FILE_NAME);
        let expected = legacy_events.to_string_lossy().replace('\\', "/");

        assert_eq!(
            template_events_target(&script_body(&legacy_events)).as_deref(),
            Some(expected.as_str()),
            "同代模板（只有事件路径不同）必须被认出来"
        );
        // 目标不是事件文件 → 不认（可能是用户改成写别的地方，不属于升级范围）。
        assert!(
            template_events_target(&script_body(&legacy_events.with_file_name("other.jsonl")))
                .is_none()
        );
        // 骨架被改过 → 不认（按「用户改过」保护）。
        assert!(template_events_target("#!/bin/sh\necho mine\n").is_none());
        // `$HOME` 形态（最早一版）同样由骨架识别（目标保留 `$HOME/` 前缀）：G1 时代
        // 生成的、指向上一代数据目录的 `"$HOME/…"` 也是这一形态，认不出它会在那类
        // 机器上重现「Foreign ⇒ 永久装不上」的同一个 bug。
        // 与 `script_ownership` 的 `LEGACY_SH_BODY` 字节匹配分支行为等价（同为 `Outdated`）。
        assert_eq!(
            template_events_target(LEGACY_SH_BODY).as_deref(),
            Some("$HOME/.workbuddy-shift/hook-events.jsonl"),
        );

        std::fs::create_dir_all(layout.script.parent().expect("store")).expect("store 目录");
        std::fs::write(&layout.script, script_body(&legacy_events)).expect("写上一代模板");
        assert!(
            matches!(layout.script_ownership(), ScriptOwnership::Outdated),
            "同代模板必须判为待升级，而不是「用户改过」"
        );

        install_client(&layout, "codebuddy");
        install_at(&layout).expect("上一代模板必须能升级并注册");
        assert_eq!(
            std::fs::read_to_string(&layout.script).expect("脚本"),
            script_body(&layout.events),
            "升级后事件必须写回本工具的数据目录"
        );
        assert!(layout.fully_installed(), "升级后必须判定为装全");

        cleanup(&layout);
    }

    /// 指向上一代目录的注册条目必须被认领并替换。
    ///
    /// 留着只有坏处：旧脚本继续被客户端拉起 ⇒ 同一个 429 被写两份，且事件落进已废弃的目录
    /// （消费端读的是新目录，那部分记录等于凭空消失）。命令形态取自真实配置
    /// （`bash '/c/…/hook.sh'`），第三方同类命令仍不得被认领。
    #[test]
    fn install_replaces_legacy_store_entries_and_keeps_third_party_hooks() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        install_client(&layout, "codebuddy");
        let legacy_command = hook_command(&legacy_script(&layout));
        let third_party = hook_command(Path::new("/opt/other/hook.sh"));
        assert_ne!(legacy_command, third_party);
        write_settings(
            &codebuddy.settings,
            &json!({
                "hooks": {
                    "Stop": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": legacy_command }] },
                        { "matcher": "", "hooks": [{ "type": "command", "command": third_party }] },
                    ],
                    "FinalStop": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": legacy_command }] },
                    ],
                }
            })
            .to_string(),
        );

        install_at(&layout).expect("安装必须成功");

        let root = read_settings(&codebuddy.settings).expect("配置");
        let stop = root["hooks"]["Stop"].as_array().expect("Stop 数组");
        let commands: Vec<&str> = stop
            .iter()
            .filter_map(|entry| entry["hooks"][0]["command"].as_str())
            .collect();
        assert_eq!(
            commands.len(),
            2,
            "上一代条目应被替换为当前条目，第三方条目保留：{commands:?}"
        );
        assert!(
            commands.contains(&third_party.as_str()),
            "第三方 hook 不得被认领：{commands:?}"
        );
        assert!(
            commands
                .iter()
                .all(|command| *command != legacy_command.as_str()),
            "指向上一代目录的条目必须移除：{commands:?}"
        );
        let final_stop = root["hooks"]["FinalStop"]
            .as_array()
            .expect("FinalStop 数组");
        assert_eq!(final_stop.len(), 1, "FinalStop 也必须是替换而不是追加");
        assert_eq!(
            final_stop[0]["hooks"][0]["command"].as_str(),
            Some(hook_command(&layout.script).as_str()),
            "FinalStop 只留当前命令"
        );

        cleanup(&layout);
    }

    /// 2026-11-17 现场状态的端到端回归：脚本是同代模板但指向上一代数据目录，三处配置里只有
    /// 指向上一代目录的条目。
    ///
    /// 修复前：脚本被判 `Foreign` ⇒ `install_at` 在写配置之前就失败 ⇒ `auto_install_at`
    /// 每启动一次失败一次、settings.json 一个字节都没变，于是 hook 从未生效，限额只能靠日志
    /// 扫描兜底（缺 chip + 模型名归因失败）。这里断言启动自动接入能把现场修回来。
    #[test]
    fn startup_auto_install_repairs_the_legacy_store_state() {
        let layout = layout();
        let legacy_events = legacy_script(&layout).with_file_name(EVENTS_FILE_NAME);
        let legacy_command = hook_command(&legacy_script(&layout));
        std::fs::create_dir_all(layout.script.parent().expect("store")).expect("store 目录");
        std::fs::write(&layout.script, script_body(&legacy_events)).expect("写上一代模板");
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            install_client(&layout, label);
            write_settings(
                &target(&layout, label).settings,
                &json!({
                    "hooks": {
                        "Stop": [
                            { "matcher": "", "hooks": [{ "type": "command", "command": legacy_command }] },
                        ],
                        "FinalStop": [
                            { "matcher": "", "hooks": [{ "type": "command", "command": legacy_command }] },
                        ],
                    }
                })
                .to_string(),
            );
        }
        assert!(!layout.fully_installed(), "现场状态不是「已装全」");

        assert!(
            auto_install_at(&layout, true, false),
            "启动自动接入必须真的改动注册状态"
        );

        assert!(layout.fully_installed(), "修复后必须判定为装全");
        assert_eq!(
            std::fs::read_to_string(&layout.script).expect("脚本"),
            script_body(&layout.events),
            "脚本事件路径必须指回本工具的数据目录"
        );
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            let root = read_settings(&target(&layout, label).settings).expect("配置");
            assert!(
                config_has_marker(&root, &layout.marker()),
                "{label} 必须注册当前命令"
            );
            assert!(
                !pretty(&root).contains(&legacy_command),
                "{label} 不得残留指向上一代目录的条目：{root}"
            );
        }
        // 幂等：再跑一次不再改动。
        assert!(!auto_install_at(&layout, true, false));

        cleanup(&layout);
    }
