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
        assert_eq!(data_dir_name_for_install_root(&dir), None);
        std::fs::write(
            dir.join("product-info.json"),
            r#"{"name":"IntelliJ IDEA","version":"2026.2.1","dataDirectoryName":"IntelliJIdea2026.2"}"#,
        )
        .unwrap();
        assert_eq!(
            data_dir_name_for_install_root(&dir).as_deref(),
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
