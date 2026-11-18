    #[test]
    fn uninstall_restores_the_settings_file_byte_for_byte() {
        let layout = layout();
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            let target = target(&layout, label);
            // 故意用非标准格式（紧凑 + 无缩进）验证「逐字节还原」，而不是「重排后相等」。
            write_settings(
                &target.settings,
                r#"{"language":"简体中文","hooks":{"Stop":[{"matcher":"x","hooks":[{"type":"command","command":"echo keep"}]}]},"model":"hy3"}"#,
            );
        }

        install_at(&layout).expect("安装");
        let installed = read_settings(&target(&layout, "codebuddy").settings).expect("已安装");
        assert!(config_has_marker(&installed, &layout.marker()));

        uninstall_at(&layout).expect("卸载");
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            let target = target(&layout, label);
            assert_eq!(
                std::fs::read_to_string(&target.settings).expect("配置存在"),
                r#"{"language":"简体中文","hooks":{"Stop":[{"matcher":"x","hooks":[{"type":"command","command":"echo keep"}]}]},"model":"hy3"}"#,
                "{label} 必须逐字节还原"
            );
        }
        assert!(!layout.script.is_file(), "本工具生成的脚本在卸载后应清理");
    }

    #[test]
    fn uninstall_keeps_changes_the_user_made_after_install() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        write_settings(&codebuddy.settings, "{}");
        install_at(&layout).expect("安装");

        // 安装之后用户（或客户端）改了别的键。
        let mut root = read_settings(&codebuddy.settings).expect("配置");
        root["statusLine"] = json!({ "type": "command" });
        write_settings(&codebuddy.settings, &pretty(&root));

        uninstall_at(&layout).expect("卸载");
        let root = read_settings(&codebuddy.settings).expect("配置");
        assert_eq!(root["statusLine"]["type"], "command", "用户的改动必须保留");
        assert!(
            !config_has_marker(&root, &layout.marker()),
            "本工具条目必须移除"
        );
        assert!(root.get("hooks").is_none(), "空 hooks 键一并摘掉");
    }

    #[test]
    fn install_creates_missing_settings_and_uninstall_removes_them_again() {
        let layout = layout();
        let workbuddy = target(&layout, "workbuddy");
        install_client(&layout, "workbuddy");
        assert!(!workbuddy.settings.exists());

        install_at(&layout).expect("安装");
        let root = read_settings(&workbuddy.settings).expect("新配置可解析");
        assert!(config_has_marker(&root, &layout.marker()));

        uninstall_at(&layout).expect("卸载");
        assert!(
            !workbuddy.settings.exists(),
            "安装前不存在 → 卸载后也不存在"
        );
    }

    /// 不存在的数据根不参与安装：不创建目录、不写配置，状态里 `exists = false`。
    #[test]
    fn install_skips_clients_whose_data_root_is_missing() {
        let layout = layout();
        install_client(&layout, "codebuddy");
        let codebuddy = target(&layout, "codebuddy");
        write_settings(&codebuddy.settings, "{}");

        let status = install_at(&layout).expect("只对已安装的客户端安装，不是错误");
        assert_eq!(status["installed"], json!(true));
        for label in ["workbuddy", "workbuddy-ai"] {
            let target = target(&layout, label);
            assert!(!target.data_root.exists(), "{label} 的数据根不得被创建");
            assert!(!target.settings.exists(), "{label} 不得被写入配置");
        }
        for entry in status["targets"].as_array().expect("targets") {
            let expected = entry["label"] == "codebuddy";
            assert_eq!(entry["exists"], json!(expected), "{entry}");
            assert_eq!(entry["installed"], json!(expected), "{entry}");
        }
    }

    /// 一处客户端都没装：安装是「什么都不做」，不生成脚本、不建目录，也不是错误。
    #[test]
    fn install_without_any_client_does_nothing() {
        let layout = layout();
        let status = install_at(&layout).expect("没有客户端不是错误");
        assert_eq!(status["installed"], json!(false));
        assert_eq!(status["scriptExists"], json!(false), "不生成脚本");
        for target in &layout.targets {
            assert!(!target.exists(), "{} 的数据根不得被创建", target.label);
            assert!(!target.settings.exists(), "{} 不得被写入配置", target.label);
        }
    }

    /// `exists` 只反映数据根存在性（客户端可能还没写过 `settings.json`）。
    #[test]
    fn status_exists_reflects_the_data_root_not_the_settings_file() {
        let layout = layout();
        install_client(&layout, "workbuddy");
        let status = status_at(&layout);
        let workbuddy = status["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|entry| entry["label"] == "workbuddy")
            .expect("workbuddy 目标")
            .clone();
        assert_eq!(workbuddy["exists"], json!(true), "数据根在即视为已安装");
        assert_eq!(workbuddy["installed"], json!(false));
        assert!(!target(&layout, "workbuddy").settings.exists());

        let codebuddy = status["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|entry| entry["label"] == "codebuddy")
            .expect("codebuddy 目标")
            .clone();
        assert_eq!(codebuddy["exists"], json!(false));
        assert_eq!(status["installed"], json!(false), "一处都没注册");
    }

    /// 逐来源扫描判定：数据根存在 且 该处配置未注册 hook 才扫日志。
    ///
    /// 关键场景：某处配置注册失败（非法 JSON / 用户手删条目）时**只有那一处**回退扫描。
    #[test]
    fn log_scan_is_decided_per_source() {
        let layout = layout();
        let marker = layout.marker();
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            install_client(&layout, label);
        }
        install_at(&layout).expect("安装");

        // 全部注册成功 → 三处都不需要日志扫描。
        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            let target = target(&layout, label);
            assert!(
                !needs_log_scan(&target.data_root, &target.settings, &marker),
                "{label} 已注册 hook 时不该扫日志"
            );
        }

        // 用户手删了 workbuddy-ai 的条目（其余两处仍在）→ 只有这一处回退扫描。
        let ai = target(&layout, "workbuddy-ai");
        write_settings(&ai.settings, "{}");
        assert!(
            needs_log_scan(&ai.data_root, &ai.settings, &marker),
            "条目被删的来源必须回退日志扫描"
        );
        for label in ["codebuddy", "workbuddy"] {
            let target = target(&layout, label);
            assert!(
                !needs_log_scan(&target.data_root, &target.settings, &marker),
                "{label} 不受其它来源影响"
            );
        }

        // 配置写坏（非法 JSON）同样按「未注册」处理 → 该来源回退扫描。
        write_settings(&ai.settings, "{ not json");
        assert!(needs_log_scan(&ai.data_root, &ai.settings, &marker));

        // 数据根不存在 → 不扫描（无论配置长什么样）。
        let missing = layout.script.parent().expect("store").join("missing");
        assert!(!needs_log_scan(
            &missing,
            &missing.join(SETTINGS_FILE_NAME),
            &marker
        ));
    }

    /// 「卸载 = opt-out，接入 = 清除」：配置文件只保留已知字段，且不动 `enabled`。
    #[test]
    fn uninstall_records_opt_out_and_install_clears_it() {
        let layout = layout();
        let config = config_path(&layout);
        install_client(&layout, "codebuddy");
        write_settings(&target(&layout, "codebuddy").settings, "{}");

        // 用户关掉了限额监听（enabled=false），卸载时必须原样保留。
        config::save_rate_limit_config_at(&config, &json!({ "enabled": false })).expect("写入配置");
        uninstall_and_opt_out(&layout, &config).expect("卸载");
        let saved = config::load_rate_limit_config_at(&config);
        assert_eq!(saved["hookOptOut"], json!(true), "卸载即记录拒绝");
        assert_eq!(saved["enabled"], json!(false), "不得改动限额监听开关");
        // 卸载后重启：opt-out 让自动接入直接放弃（不会又被装回）。
        assert!(
            !auto_install_at(&layout, true, true),
            "卸载过就不再自动装回"
        );

        install_and_clear_opt_out(&layout, &config).expect("接入");
        let saved = config::load_rate_limit_config_at(&config);
        assert_eq!(saved["hookOptOut"], json!(false), "接入即恢复默认接入语义");
        assert_eq!(saved["enabled"], json!(false));
        // 重新接入后恢复默认接入：hook 失效（脚本被删）时启动会重新装。
        std::fs::remove_file(&layout.script).expect("删除脚本");
        assert!(
            auto_install_at(&layout, true, false),
            "清除 opt-out 后恢复默认接入"
        );

        // 安装失败也要记下用户意图（否则下次启动又会自动装回）。
        let broken = target(&layout, "workbuddy");
        install_client(&layout, "workbuddy");
        write_settings(&broken.settings, "{ not json");
        install_and_clear_opt_out(&layout, &config).expect_err("非法 JSON 必须报错");
        assert_eq!(
            config::load_rate_limit_config_at(&config)["hookOptOut"],
            json!(false)
        );
        uninstall_and_opt_out(&layout, &config).expect("卸载");
        assert_eq!(
            config::load_rate_limit_config_at(&config)["hookOptOut"],
            json!(true)
        );
    }

    /// 默认接入的前置条件：开关开启 && 未 opt-out && 存在客户端 && hook 未装全。
    #[test]
    fn auto_install_respects_its_preconditions() {
        let layout = layout();

        // ① 一处客户端都没有 → 什么都不做。
        assert!(!auto_install_at(&layout, true, false));
        assert!(!layout.script.exists());
        assert!(!layout.any_target_exists());

        // ② 开关关闭 / 用户已卸载过 → 不安装。
        install_client(&layout, "codebuddy");
        assert!(!auto_install_at(&layout, false, false));
        assert!(!auto_install_at(&layout, true, true));
        assert!(!layout.script.exists(), "前置条件不满足时不得生成脚本");

        // ③ 条件满足 → 自动安装（幂等）。
        assert!(auto_install_at(&layout, true, false));
        assert!(layout.fully_installed());

        // ④ 已装全 → 不再重复动文件。
        assert!(!auto_install_at(&layout, true, false));

        // ⑤ 只装了一半（新客户端在安装之后出现）→ 补齐。
        install_client(&layout, "workbuddy");
        assert!(!layout.fully_installed());
        assert!(auto_install_at(&layout, true, false));
        assert!(layout.fully_installed());

        // ⑥ 脚本被删（hook 失效）→ 重新生成。
        std::fs::remove_file(&layout.script).expect("删除脚本");
        assert!(auto_install_at(&layout, true, false));
        assert!(layout.fully_installed());
    }

    #[test]
    fn malformed_settings_are_left_untouched() {
        let layout = layout();
        let workbuddy = target(&layout, "workbuddy");
        write_settings(&workbuddy.settings, "{ not json");
        let codebuddy = target(&layout, "codebuddy");
        write_settings(&codebuddy.settings, "{}");

        let error = install_at(&layout).expect_err("非法 JSON 必须报错");
        assert!(error.contains("workbuddy"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&workbuddy.settings).expect("原文件仍在"),
            "{ not json",
            "损坏的配置不得被覆盖"
        );
        // 其它目标照常安装（单个失败不阻断）。
        assert!(config_has_marker(
            &read_settings(&codebuddy.settings).expect("配置"),
            &layout.marker()
        ));
    }

    #[test]
    fn malformed_hooks_shape_is_reported_without_overwriting() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        write_settings(
            &codebuddy.settings,
            &json!({ "hooks": ["not-an-object"] }).to_string(),
        );
        let error = install_at(&layout).expect_err("`hooks` 不是对象必须报错");
        assert!(error.contains("hooks"), "{error}");
        assert_eq!(
            read_settings(&codebuddy.settings).expect("配置")["hooks"][0],
            "not-an-object"
        );
    }

    #[test]
    fn status_tracks_the_marker_and_the_script() {
        let layout = layout();
        let status = status_at(&layout);
        assert_eq!(status["installed"], json!(false));
        assert_eq!(status["scriptExists"], json!(false));
        assert_eq!(status["targets"].as_array().expect("targets").len(), 3);
        for target in status["targets"].as_array().expect("targets") {
            assert_eq!(target["exists"], json!(false), "{target}");
        }

        for label in ["codebuddy", "workbuddy", "workbuddy-ai"] {
            install_client(&layout, label);
        }
        install_at(&layout).expect("安装");
        let status = status_at(&layout);
        assert_eq!(status["installed"], json!(true));
        assert_eq!(status["scriptExists"], json!(true));
        for target in status["targets"].as_array().expect("targets") {
            assert_eq!(target["installed"], json!(true), "{target}");
        }

        // 配置里手删 marker（模拟客户端升级冲掉了注册）→ 不再视为已安装。
        let codebuddy = target(&layout, "codebuddy");
        std::fs::remove_file(&codebuddy.settings).expect("删除配置");
        assert_eq!(
            status_at(&layout)["installed"],
            json!(true),
            "还有两处配置在"
        );
        for label in ["workbuddy", "workbuddy-ai"] {
            std::fs::remove_file(&target(&layout, label).settings).expect("删除配置");
        }
        assert_eq!(status_at(&layout)["installed"], json!(false));
    }

    #[test]
    fn events_path_lives_in_the_store_directory() {
        assert!(events_path().ends_with(EVENTS_FILE_NAME));
        assert!(events_path().to_string_lossy().contains(STORE_DIR_NAME));
    }

    /// 卸载必须连「指向上一代目录的条目」一起摘掉。
    ///
    /// 否则用户点完「卸载」旧脚本仍被客户端拉起（继续往废弃目录写事件），而界面上已经
    /// 显示「未接入」——状态与实际行为不一致，也正是这次缺 chip 的现场形态。
    #[test]
    fn uninstall_removes_entries_pointing_at_the_legacy_store_directory() {
        let layout = layout();
        let codebuddy = target(&layout, "codebuddy");
        let legacy_command = hook_command(&legacy_script(&layout));
        write_settings(
            &codebuddy.settings,
            &json!({
                "hooks": {
                    "Stop": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": legacy_command }] },
                    ],
                }
            })
            .to_string(),
        );

        uninstall_at(&layout).expect("卸载");

        let root = read_settings(&codebuddy.settings).expect("配置");
        assert!(root.get("hooks").is_none(), "旧条目与空 hooks 键都应摘掉：{root}");

        cleanup(&layout);
    }
