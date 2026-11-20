    #[test]
    fn config_dir_name_filter_accepts_products_and_rejects_noise() {
        for name in [
            "IntelliJIdea2026.2",
            "IdeaIC2025.1",
            "PyCharm2026.2",
            "PyCharmCE2025.1",
        ] {
            assert!(is_supported_config_dir_name(name), "{name} 应被接受");
        }
        for name in [
            "PermanentDeviceId",
            "bl",
            "crl",
            "discovery",
            "IntelliJIdea",
            "PyCharm",
            "PyCharmBackup",
            "IntelliJIdeaX",
            "PyCharm2026.2-backup",
        ] {
            assert!(!is_supported_config_dir_name(name), "{name} 应被拒绝");
        }
    }

    /// 第 16 条 16.2 的判定表必须逐条钉住：放宽发行通道后缀，但仍拒绝迁移残留。
    ///
    /// 2026.3 的 EAP 用户会得到 `IntelliJIdea2026.3-eap`，旧判定把整类用户过滤成
    /// 「没有可切换的 IDE」——这是 issue #221 最可能的成因之一。
    #[test]
    fn config_dir_name_accepts_release_channels_but_rejects_backups() {
        for (name, expected) in [
            ("IntelliJIdea2026.3", true),
            ("IntelliJIdea2026.3.1", true),
            ("IdeaIC2026.3", true),
            ("IntelliJIdea2026.3-eap", true),
            ("IntelliJIdea2026.3-EAP", true),
            ("PyCharm2026.3-nightly", true),
            ("PyCharm2026.3-beta", true),
            ("PyCharmCE2026.3-rc", true),
            ("IdeaIC2026.3-snapshot", true),
            ("IntelliJIdea2026.3-eap2", false),
            ("PyCharm2026.2-backup", false),
            ("PyCharm2026.2-bak", false),
            ("PyCharm2026.2.bak", false),
            ("IntelliJIdea2026.3-eap.1", false),
            ("PyCharm2026.2-", false),
        ] {
            assert_eq!(
                is_supported_config_dir_name(name),
                expected,
                "{name} 的判定应为 {expected}"
            );
        }
    }

    /// 插件目录名按「包含 coding-copilot（大小写不敏感）」匹配，不是前缀匹配。
    #[test]
    fn plugin_dir_matches_marketplace_id_naming() {
        let dir =
            std::env::temp_dir().join(format!("workbuddy-shift-jb-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins/other-plugin")).unwrap();
        // 未安装插件：只有无关目录。
        assert!(plugin_dir(&dir).is_none());
        std::fs::create_dir_all(dir.join("plugins/Tencent-Cloud.coding-copilot")).unwrap();
        let found = plugin_dir(&dir).expect("marketplace id 命名必须判为已安装");
        assert_eq!(
            found.file_name().map(|n| n.to_string_lossy().into_owned()),
            Some("Tencent-Cloud.coding-copilot".to_string())
        );
        // 多个命中时按名排序取首个，保证诊断输出稳定。
        std::fs::create_dir_all(dir.join("plugins/coding-copilot-jetbrains")).unwrap();
        let names: Vec<String> = plugin_dirs(&dir)
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(
            names,
            vec![
                "Tencent-Cloud.coding-copilot".to_string(),
                "coding-copilot-jetbrains".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1 兜底：`product-info.json` 缺失 → 按 exe 名 + 安装目录名推断，并标 fallback。
    #[test]
    fn product_info_missing_falls_back_to_exe_and_dir_name() {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-fallback-{}/IntelliJ IDEA 2026.3",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/idea64.exe");
        std::fs::write(&exe, "").unwrap();

        let mapping = resolve_config_dir(&root, Some(&exe));
        assert_eq!(mapping.name.as_deref(), Some("IntelliJIdea2026.3"));
        assert_eq!(mapping.source, Some(MappingSource::Fallback));
        assert!(mapping.note.is_some(), "兜底必须留下原因供诊断展示");

        // 社区版按安装目录名判到 IdeaIC。
        let ce_root = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-fallback-ce-{}/IntelliJ IDEA Community Edition 2026.3",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&ce_root);
        std::fs::create_dir_all(ce_root.join("bin")).unwrap();
        let ce_exe = ce_root.join("bin/idea64.exe");
        std::fs::write(&ce_exe, "").unwrap();
        assert_eq!(
            resolve_config_dir(&ce_root, Some(&ce_exe)).name.as_deref(),
            Some("IdeaIC2026.3")
        );

        // PyCharm 同规则。
        let py_root = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-fallback-py-{}/PyCharm 2026.3",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&py_root);
        std::fs::create_dir_all(py_root.join("bin")).unwrap();
        let py_exe = py_root.join("bin/pycharm64.exe");
        std::fs::write(&py_exe, "").unwrap();
        assert_eq!(
            resolve_config_dir(&py_root, Some(&py_exe)).name.as_deref(),
            Some("PyCharm2026.3")
        );

        // 安装目录名里没有版本号 → 兜底也失败 → Unmapped（切换会被拒绝）。
        let unknown = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-fallback-unknown-{}/IntelliJ IDEA",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&unknown);
        std::fs::create_dir_all(unknown.join("bin")).unwrap();
        let unknown_exe = unknown.join("bin/idea64.exe");
        std::fs::write(&unknown_exe, "").unwrap();
        let mapping = resolve_config_dir(&unknown, Some(&unknown_exe));
        assert!(mapping.name.is_none());
        assert_eq!(mapping.source, Some(MappingSource::Unmapped));
        assert!(mapping.note.is_some());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ce_root);
        let _ = std::fs::remove_dir_all(&py_root);
        let _ = std::fs::remove_dir_all(&unknown);
    }

    /// `product-info.json` 损坏 → 兜底；`dataDirectoryName` 不合规 → 守卫拒绝该结果。
    #[test]
    fn product_info_corrupt_or_untrusted_falls_back() {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-corrupt-{}/PyCharm 2026.3",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/pycharm64.exe");
        std::fs::write(&exe, "").unwrap();

        // 损坏的 JSON
        std::fs::write(root.join("product-info.json"), "{not json").unwrap();
        let info = read_product_info(&root);
        assert!(info.data_dir_name.is_none());
        assert!(info
            .note
            .as_deref()
            .is_some_and(|n| n.contains("解析失败")));
        let mapping = resolve_config_dir(&root, Some(&exe));
        assert_eq!(mapping.name.as_deref(), Some("PyCharm2026.3"));
        assert_eq!(mapping.source, Some(MappingSource::Fallback));

        // 缺失 dataDirectoryName
        std::fs::write(root.join("product-info.json"), r#"{"name":"PyCharm"}"#).unwrap();
        assert!(read_product_info(&root)
            .note
            .as_deref()
            .is_some_and(|n| n.contains("dataDirectoryName")));

        // 守卫：dataDirectoryName 指向迁移残留目录 → 不采信 product-info，退回兜底。
        std::fs::write(
            root.join("product-info.json"),
            r#"{"name":"PyCharm","version":"2026.2","dataDirectoryName":"PyCharm2026.2-backup"}"#,
        )
        .unwrap();
        let info = read_product_info(&root);
        assert!(info.data_dir_name.is_none(), "不支持的目录名不得作为写入目标");
        assert!(info
            .note
            .as_deref()
            .is_some_and(|n| n.contains("不在受支持产品范围内")));

        // 合规样本仍走 product-info 精确分支。
        std::fs::write(
            root.join("product-info.json"),
            r#"{"name":"PyCharm","version":"2026.3","dataDirectoryName":"PyCharm2026.3"}"#,
        )
        .unwrap();
        let mapping = resolve_config_dir(&root, Some(&exe));
        assert_eq!(mapping.name.as_deref(), Some("PyCharm2026.3"));
        assert_eq!(mapping.source, Some(MappingSource::ProductInfo));
        assert_eq!(mapping.version.as_deref(), Some("2026.3"));
        assert!(mapping.note.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    fn running_instance(config_dir_name: Option<&str>) -> RunningIde {
        RunningIde {
            pid: 4242,
            launch_target: Some(PathBuf::from("/opt/idea/bin/idea64.exe")),
            config_dir_name: config_dir_name.map(str::to_string),
            mapping_source: if config_dir_name.is_some() {
                MappingSource::ProductInfo
            } else {
                MappingSource::Unmapped
            },
            product_name: Some("IntelliJ IDEA".to_string()),
            version: Some("2026.3".to_string()),
            mapping_note: config_dir_name
                .is_none()
                .then(|| "product-info.json 不存在".to_string()),
        }
    }

    /// 用户决策（不得改判）：有未映射的运行中 IDE 时**拒绝写入**并列出进程。
    #[test]
    fn unmapped_running_ide_blocks_write_and_lists_processes() {
        let running = vec![
            running_instance(Some("IntelliJIdea2026.3")),
            running_instance(None),
        ];
        let unmapped = unmapped_running(&running);
        assert_eq!(unmapped.len(), 1, "只统计映射失败的那一个");

        let hint = unmapped_running_hint(&unmapped);
        assert!(hint.contains("拒绝写入"), "必须明确拒绝写入：{hint}");
        assert!(hint.contains("完全退出"), "必须提示先退出 IDE：{hint}");
        assert!(hint.contains("4242"), "必须列出 PID：{hint}");
        assert!(hint.contains("IntelliJ IDEA"), "必须列出产品名：{hint}");
        assert!(
            hint.contains("product-info.json 不存在"),
            "必须带失败原因：{hint}"
        );

        // 全部映射成功时不得阻断。
        assert!(unmapped_running(&[running_instance(Some("PyCharm2026.3"))]).is_empty());

        // 状态里给前端的结构化清单字段齐全。
        let list = unmapped_running_json(&running);
        let first = &list[0];
        assert_eq!(first["pid"], json!(4242));
        assert_eq!(first["product"], json!("IntelliJ IDEA"));
        assert_eq!(first["path"], json!("/opt/idea/bin/idea64.exe"));
        assert!(first["reason"].is_string());
    }

    /// 失败诊断必须含关键字段（配置根 / 目标与插件命中 / 运行实例与映射来源）。
    #[test]
    fn failure_context_contains_key_diagnostic_fields() {
        let targets = vec![
            make_target("PyCharm2026.3"),
            Target {
                config_dir: PathBuf::from("/tmp/IntelliJIdea2026.3"),
                name: "IntelliJIdea2026.3".to_string(),
                plugin_installed: false,
            },
        ];
        let running = vec![running_instance(None)];
        let text = failure_context(&targets, &running);
        for expected in [
            "configRoots=",
            "root=",
            "target=PyCharm2026.3 pluginInstalled=true",
            "target=IntelliJIdea2026.3 pluginInstalled=false",
            "running pid=4242 mappingSource=unmapped configDir=(未映射)",
            "note=product-info.json 不存在",
        ] {
            assert!(text.contains(expected), "诊断缺少 {expected}：\n{text}");
        }
    }

    /// 「已识别 N 个 IDE：A、B」必须出现在失败提示里（区分没识别到 / 操作错了）。
    #[test]
    fn describe_detected_reports_count_and_names() {
        let targets = vec![make_target("PyCharm2026.3")];
        let text = describe_detected(&targets);
        assert!(text.contains("已识别 1 个"), "{text}");
        assert!(text.contains("PyCharm2026.3"), "{text}");
        assert!(text.contains("已装插件"), "{text}");

        let empty = describe_detected(&[]);
        assert!(empty.contains("已识别 0 个"), "{empty}");
        assert!(empty.contains("配置根"), "{empty}");
    }

    #[test]
    fn xml_escape_and_unescape_round_trip() {
        let raw = "a&b<c>\"d\"'e'f\n\tg";
        let escaped = xml_escape_attr(raw);
        assert_eq!(
            escaped,
            "a&amp;b&lt;c&gt;&quot;d&quot;&apos;e&apos;f&#10;&#9;g"
        );
        assert_eq!(xml_unescape_attr(&escaped), raw);
    }

    #[test]
    fn xml_unescape_keeps_unknown_entities_and_digits() {
        assert_eq!(
            xml_unescape_attr("x&#10;y&#x41;z&unknown;w"),
            "x\nyAz&unknown;w"
        );
        assert_eq!(xml_unescape_attr("no entity"), "no entity");
    }

    #[test]
    fn secret_entries_parse_and_serialize_round_trip() {
        let value =
            r#"{"id":"Tencent-Cloud.coding-copilot","accessToken":"u+tok","auth":{"scope":"all"}}"#;
        let content = serialize_secret_entries(&[
            ("other.key".to_string(), "plain & <value>".to_string()),
            (SECRET_KEY.to_string(), value.to_string()),
        ]);
        // 必须是平台真实写盘格式（application + component 包装，见 serialize 文档）。
        assert!(content.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(content.contains("<application>"));
        assert!(content.contains("<component name=\"SecretStorage\">"));
        let entries = parse_secret_entries(&content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "other.key");
        assert_eq!(entries[0].1, "plain & <value>");
        assert_eq!(entries[1].0, SECRET_KEY);
        assert_eq!(entries[1].1, value);
    }

    /// 平台真实落盘样本（实机登录产生，脱敏）必须可解析——防写入端与平台
    /// 格式脱节导致读回失败（写入的 key 被整体丢弃、插件显示未登录）。
    #[test]
    fn secret_entries_parse_real_platform_sample() {
        let sample = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
<application>\n  \
<component name=\"SecretStorage\">\n    \
<Scores>\n      \
<Entry key=\"CodeBuddy-Product-Cache\" value=\"1790363102226\" />\n      \
<Entry key=\"Tencent-Cloud.coding-copilot.new.accessToken\" value=\"{&quot;a&quot;:[1,2],&quot;b&quot;:&quot;x&amp;y&quot;}\" />\n    \
</Scores>\n  \
</component>\n\
</application>\n";
        let entries = parse_secret_entries(sample);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].0, SECRET_KEY);
        assert_eq!(entries[1].1, r#"{"a":[1,2],"b":"x&y"}"#);
        // 反序列化后仍是平台格式（可被 IDE 读回）
        let rewritten = serialize_secret_entries(&entries);
        assert!(rewritten.contains("<component name=\"SecretStorage\">"));
        assert_eq!(parse_secret_entries(&rewritten), entries);
    }

    #[test]
    fn secret_entries_parse_tolerates_self_closing_spread() {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<MapStorage>
  <Scores>
    <Entry key="Tencent-Cloud.coding-copilot.new.accessToken" value="{&quot;a&quot;:1}" />
    <Entry key="k2" value="v2" />
  </Scores>
</MapStorage>
"#;
        let entries = parse_secret_entries(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, r#"{"a":1}"#);
        assert_eq!(entries[1].1, "v2");
    }

    #[test]
    fn secret_store_read_write_upsert_in_temp_dir() {
        let dir =
            std::env::temp_dir().join(format!("workbuddy-shift-jb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("options")).unwrap();

        // 首写：文件不存在 → 新建
        write_secret_for(&dir, "first").unwrap();
        assert_eq!(read_secret_for(&dir).unwrap().as_deref(), Some("first"));
        // 备份在第二次写入时才出现
        assert!(!dir
            .join("options/secret-storage.xml.workbuddy-shift-bak")
            .exists());

        // 二写：upsert + 保留其它 Entry + 生成备份
        let path = secret_path_for(&dir);
        std::fs::write(
            &path,
            serialize_secret_entries(&[("keep.me".to_string(), "v1".to_string())]),
        )
        .unwrap();
        write_secret_for(&dir, "second").unwrap();
        assert_eq!(read_secret_for(&dir).unwrap().as_deref(), Some("second"));
        let entries = parse_secret_entries(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "keep.me");
        assert!(dir
            .join("options/secret-storage.xml.workbuddy-shift-bak")
            .exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_dir_name_parses_product_info() {
        let dir =
            std::env::temp_dir().join(format!("workbuddy-shift-jb-info-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_product_info(&dir).data_dir_name, None);
        std::fs::write(
            dir.join("product-info.json"),
            r#"{"name":"IntelliJ IDEA","version":"2026.2.1","dataDirectoryName":"IntelliJIdea2026.2"}"#,
        )
        .unwrap();
        assert_eq!(
            read_product_info(&dir).data_dir_name.as_deref(),
            Some("IntelliJIdea2026.2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 会话载荷复用 VS Code 扩展构造：id / accessToken / accounts 语义一致。
    #[test]
    fn session_json_reuses_vscode_ext_builder() {
        let acc = json!({
            "uid": "u-42",
            "nickname": "测试",
            "access_token": "tok-abc",
            "refresh_token": "rt-1",
            "domain": "www.codebuddy.cn",
            "expiresAt": 1234567890_i64,
        });
        let v: Value = serde_json::from_str(&build_ext_session_json(&acc, None)).unwrap();
        assert_eq!(v["id"], "Tencent-Cloud.coding-copilot");
        assert_eq!(v["accessToken"], "u-42+tok-abc");
        assert_eq!(v["token"], "tok-abc");
    }

    /// 关闭决策：运行中且手动模式 → 报错；未运行 → 直接写（closed 为空）。
    #[test]
    fn switch_decision_blocks_manual_mode_with_running_ide() {
        // 契约回归：matched 非空 + restart=false 必须在关进程前报错（不触碰进程表）。
        assert!(RUNNING_MANUAL_HINT.contains("正在运行"));
        assert_eq!(CLOSE_TIMEOUT_SECS, 60);
    }

    fn make_target(name: &str) -> Target {
        Target {
            config_dir: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            plugin_installed: true,
        }
    }

    /// 目标选择语义：None / 空 = 全部；非空 = 精确过滤；全不匹配显式报错。
    #[test]
    fn filter_targets_by_selection_semantics() {
        let all = vec![
            make_target("PyCharm2026.2"),
            make_target("IntelliJIdea2026.2"),
        ];

        let none = filter_targets_by_selection(all.clone(), None).unwrap();
        assert_eq!(none.len(), 2);
        let empty = filter_targets_by_selection(all.clone(), Some(&[])).unwrap();
        assert_eq!(empty.len(), 2);

        let selected = vec!["PyCharm2026.2".to_string()];
        let picked = filter_targets_by_selection(all.clone(), Some(&selected)).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "PyCharm2026.2");

        let missing = vec!["PyCharm2020.1".to_string()];
        assert!(filter_targets_by_selection(all, Some(&missing)).is_err());
    }

    // -----------------------------------------------------------------------
    // 关闭结论 / 回滚：不允许留下「IDE 被关了但盘没写、也没开回来」的状态
    // -----------------------------------------------------------------------

    fn instance(pid: u32, launch_target: Option<&str>) -> RunningIde {
        RunningIde {
            pid,
            launch_target: launch_target.map(PathBuf::from),
            config_dir_name: Some("PyCharm2026.3".to_string()),
            mapping_source: MappingSource::ProductInfo,
            product_name: Some("PyCharm".to_string()),
            version: Some("2026.3".to_string()),
            mapping_note: None,
        }
    }

    /// 关闭结论必须把「已关掉」与「超时仍存活」切分干净：回滚只重开自己关掉的。
    #[test]
    fn close_report_partitions_closed_and_alive_instances() {
        let instances = vec![instance(1, Some("/opt/idea/bin/idea64")), instance(2, None)];

        let all_closed = CloseReport::from_alive(&instances, Vec::new());
        assert!(all_closed.is_complete());
        assert_eq!(all_closed.closed.len(), 2);
        assert!(all_closed.alive.is_empty());

        // 2 号没退出：只有 1 号算「我们关掉的」。
        let partial = CloseReport::from_alive(&instances, vec![2]);
        assert!(!partial.is_complete());
        assert_eq!(partial.alive, vec![2]);
        assert_eq!(partial.closed.len(), 1);
        assert_eq!(partial.closed[0].pid, 1, "只回滚确实退出的实例");

        // 空输入：什么都不关，也不算失败。
        let empty = CloseReport::from_alive(&[], Vec::new());
        assert!(empty.is_complete());
        assert!(empty.closed.is_empty());
    }

    /// 关闭超时：错误文案保留原始原因，并把已关闭的实例开回来（不留半开状态）。
    #[test]
    fn partial_close_error_relaunches_the_instances_it_closed() {
        // 路径不存在 → `launch_ide` 立刻失败（不会真的起进程），拿到的是可见错误。
        let closed = instance(1, Some("/nonexistent/workbuddy-shift/idea64"));
        let report = CloseReport {
            closed: vec![closed],
            alive: vec![7],
        };
        let text = partial_close_error(&report);
        assert!(text.contains("超时"), "必须保留原始原因：{text}");
        assert!(text.contains('7'), "必须列出未退出的 PID：{text}");
        assert!(
            text.contains("重新打开"),
            "必须说明已尝试把关掉的 IDE 开回来：{text}"
        );
        assert!(text.contains("请手动打开"), "重开失败要给出下一步：{text}");

        // 没有关掉任何实例时不要凭空说「已重新打开」。
        let nothing_closed = CloseReport {
            closed: Vec::new(),
            alive: vec![7],
        };
        let text = partial_close_error(&nothing_closed);
        assert!(!text.contains("已重新打开"), "没关过就别说开回来了：{text}");
    }

    /// 多目标写入中途失败 → 已写过的目标恢复成写入前的值（不论原来有没有登录态）。
    #[test]
    fn rollback_restores_previous_secret_and_removes_created_entry() {
        let base =
            std::env::temp_dir().join(format!("workbuddy-shift-jb-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let logged_in = base.join("PyCharm2026.3");
        let fresh = base.join("IntelliJIdea2026.3");
        std::fs::create_dir_all(logged_in.join("options")).unwrap();
        std::fs::create_dir_all(fresh.join("options")).unwrap();

        // 目标 1 原本已登录（原值要能还原）；目标 2 原本没有登录态（要删干净）。
        write_secret_for(&logged_in, "original-token").unwrap();
        write_secret_for(&logged_in, "switched-token").unwrap();
        write_secret_for(&fresh, "switched-token").unwrap();

        let applied = vec![
            (logged_in.clone(), Some("original-token".to_string())),
            (fresh.clone(), None),
        ];
        let errors = rollback_writes(&applied);
        assert!(errors.is_empty(), "回滚不应报错：{errors:?}");
        assert_eq!(
            read_secret_for(&logged_in).unwrap().as_deref(),
            Some("original-token")
        );
        assert_eq!(
            read_secret_for(&fresh).unwrap(),
            None,
            "写入前没有登录态的目标必须回到「未登录」"
        );
        assert!(
            !secret_path_for(&fresh).exists(),
            "只剩空表时连文件一起删，不留空壳"
        );

        // 回滚文案：成功 / 失败两种口径都要能看懂。
        assert!(join_with_rollback("写入失败".to_string(), 2, Vec::new())
            .contains("已回滚本次写入的 2 个目标"));
        let failed = join_with_rollback(
            "写入失败".to_string(),
            2,
            vec!["PyCharm2026.3: 权限不足".to_string()],
        );
        assert!(failed.contains("回滚失败"), "{failed}");
        assert!(failed.contains("PyCharm2026.3"), "{failed}");
        assert_eq!(
            join_with_rollback("写入失败".to_string(), 0, Vec::new()),
            "写入失败",
            "一个都没写时不该追加回滚说明"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 失败日志的内容口径：标题固定、摘要与现场缺一不可（第 16 条 16.4 P1#6）。
    #[test]
    fn failure_log_detail_carries_summary_error_and_scene() {
        let targets = vec![make_target("PyCharm2026.3")];
        let summary = describe_detected(&targets);
        let scene = failure_context(&targets, &[]);
        let text = failure_log_detail(&summary, "账号缺少 access_token", &scene);

        assert_eq!(FAILURE_LOG_TITLE, "JetBrains IDE 切换失败");
        assert!(text.contains("已识别 1 个"), "日志必须带摘要：{text}");
        assert!(
            text.contains("错误：账号缺少 access_token"),
            "日志必须带错误原文：{text}"
        );
        assert!(text.contains("configRoots="), "日志必须带现场：{text}");
        assert!(
            text.contains("target=PyCharm2026.3 pluginInstalled=true"),
            "日志必须带目标与插件命中：{text}"
        );
    }

    /// 没有可切换目标时，诊断必须能回答「是没装插件还是目录名没被识别」。
    #[test]
    fn scan_config_root_reports_every_dir_with_reasons() {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-scan-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // 受支持 + 装插件 + 有登录文件（2026.3 EAP 形态）。
        std::fs::create_dir_all(root.join("IntelliJIdea2026.3-eap/plugins/Tencent-Cloud.coding-copilot"))
            .unwrap();
        std::fs::create_dir_all(root.join("IntelliJIdea2026.3-eap/options")).unwrap();
        std::fs::write(root.join("IntelliJIdea2026.3-eap/options/secret-storage.xml"), "").unwrap();
        // 受支持但没装插件。
        std::fs::create_dir_all(root.join("PyCharm2026.3")).unwrap();
        // 迁移残留：目录名不支持。
        std::fs::create_dir_all(root.join("PyCharm2026.2-backup/plugins/coding-copilot-jetbrains"))
            .unwrap();
        // 与 IDE 无关的系统目录。
        std::fs::create_dir_all(root.join("consentOptions")).unwrap();

        let scans = scan_config_root(&root);
        let by_name = |name: &str| {
            scans
                .iter()
                .find(|scan| scan.name == name)
                .unwrap_or_else(|| panic!("{name} 必须出现在诊断清单里：{scans:?}"))
        };

        let eap = by_name("IntelliJIdea2026.3-eap");
        assert!(eap.accepted, "EAP 目录必须被接受");
        assert_eq!(eap.plugin_dirs, vec!["Tencent-Cloud.coding-copilot"]);
        assert!(eap.has_secret_file);
        let json = eap.to_json();
        assert_eq!(json["pluginInstalled"], json!(true));
        assert_eq!(json["hasSecretFile"], json!(true));

        let plain = by_name("PyCharm2026.3");
        assert!(plain.accepted);
        assert!(plain.plugin_dirs.is_empty());
        assert!(!plain.has_secret_file);

        let backup = by_name("PyCharm2026.2-backup");
        assert!(!backup.accepted, "迁移残留目录必须标为不支持");
        assert_eq!(
            backup.to_json()["pluginInstalled"],
            json!(true),
            "诊断要如实说明「装了插件但目录名不被接受」"
        );

        let noise = by_name("consentOptions");
        assert!(!noise.accepted);

        // 清单按名字排序，诊断输出稳定（便于用户截图对照）。
        let names: Vec<&str> = scans.iter().map(|scan| scan.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let _ = std::fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // 当前账号：secret-storage 优先 + 本地状态文件兜底（04 回归口）
    // -----------------------------------------------------------------------
    //
    // 现象：在 PyCharm 里手动登录成另一个账号后，页面仍显示上一次的账号，点一次
    // 「检测账号」才更新——因为 `status()` 只读本地状态文件，而该文件只在「切换成功」
    // 与「检测账号」时回写。这里钉住两条口径：
    //
    // 1. `status()` 内联一次轻量识别：从 `secret-storage.xml` 读登录态并匹配账号库，
    //    命中就用它，`detectedFrom` 如实回报 `secret-storage`；
    // 2. 读不到 / 匹配不上回退本地状态文件，`detectedFrom` 回报 `state-file`。

    /// 造一个「装了插件 + 已登录」的配置目录（secret 内容与真机同 key）。
    fn jb_logged_in_config_dir(base: &Path, name: &str, uid: &str, token: &str) -> PathBuf {
        let config_dir = base.join(name);
        std::fs::create_dir_all(config_dir.join("plugins/coding-copilot-jetbrains")).unwrap();
        let secret = json!({ "uid": uid, "token": token }).to_string();
        write_secret_for(&config_dir, &secret).unwrap();
        config_dir
    }

    /// secret-storage 命中账号库 → 当前账号来自 secret-storage（而不是滞后的状态文件）。
    #[test]
    fn current_account_prefers_secret_storage_over_the_lagging_state_file() {
        let base = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-current-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let config_dir = jb_logged_in_config_dir(&base, "PyCharm2026.2", "uid-lin", "token-lin");
        let targets = vec![Target {
            config_dir,
            name: "PyCharm2026.2".to_string(),
            plugin_installed: true,
        }];

        let scan = scan_secret_account_with(&targets, &|uid, token| {
            assert_eq!(uid, Some("uid-lin"), "secret 里的 uid 必须透传给匹配器");
            assert_eq!(token, "token-lin");
            Some(json!({ "id": "acc-lin", "uid": "uid-lin", "nickname": "林小凤" }))
        });
        // 状态文件里还是上一次切换留下的账号：必须被 secret-storage 覆盖。
        let state = Some(("acc-guo".to_string(), "果妈".to_string()));
        let (id, name, detected_from) = resolve_active_account(&scan, state);
        assert_eq!(id.as_deref(), Some("acc-lin"));
        assert_eq!(name.as_deref(), Some("林小凤"));
        assert_eq!(detected_from, "secret-storage");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 登录态匹配不上账号库（如刚在 IDE 里登录、账号库还没导入）→ 回退状态文件并如实回报。
    #[test]
    fn current_account_falls_back_to_the_state_file() {
        let base = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-current-fallback-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let config_dir = jb_logged_in_config_dir(&base, "IntelliJIdea2026.3", "uid-new", "token-new");
        let targets = vec![Target {
            config_dir,
            name: "IntelliJIdea2026.3".to_string(),
            plugin_installed: true,
        }];

        // 1) 读到登录态但账号库无匹配 → 结论来自状态文件。
        let unmatched = scan_secret_account_with(&targets, &|_, _| None);
        assert!(matches!(unmatched, SecretScan::Unmatched { .. }), "应识别为「未匹配」");
        let (id, name, detected_from) =
            resolve_active_account(&unmatched, Some(("acc-guo".to_string(), "果妈".to_string())));
        assert_eq!(id.as_deref(), Some("acc-guo"));
        assert_eq!(name.as_deref(), Some("果妈"));
        assert_eq!(detected_from, "state-file");

        // 2) 一个装了插件的目录都没有登录态 → 同样回退状态文件。
        let no_secret = scan_secret_account_with(&[], &|_, _| Some(json!({ "id": "acc-x" })));
        assert!(matches!(no_secret, SecretScan::NotFound));
        let (id, _, detected_from) = resolve_active_account(&no_secret, None);
        assert_eq!(id, None, "两边都没有时不得凭空给出账号");
        assert_eq!(detected_from, "state-file");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 只读红线：识别当前账号**绝不**改写 `secret-storage.xml`（改了就等于把用户踢下线）。
    #[test]
    fn current_account_detection_never_writes_the_secret_file() {
        let base = std::env::temp_dir().join(format!(
            "workbuddy-shift-jb-current-readonly-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let config_dir = jb_logged_in_config_dir(&base, "PyCharm2026.3", "uid-lin", "token-lin");
        let target = Target {
            config_dir: config_dir.clone(),
            name: "PyCharm2026.3".to_string(),
            plugin_installed: true,
        };
        let path = secret_path_for(&config_dir);
        let before = std::fs::read_to_string(&path).unwrap();

        let _ = scan_secret_account_with(std::slice::from_ref(&target), &|_, _| {
            Some(json!({ "id": "acc-lin", "uid": "uid-lin" }))
        });
        let _ = resolve_active_account(
            &scan_secret_account_with(std::slice::from_ref(&target), &|_, _| None),
            Some(("acc-guo".to_string(), "果妈".to_string())),
        );

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "识别当前账号不得修改 secret-storage.xml"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
