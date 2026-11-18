    #[test]
    fn secret_key_matches_verified_format() {
        let key = secret_storage_item_key();
        assert_eq!(
            key,
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessTokencn"}"#
        );
        assert_eq!(
            secret_storage_item_key_for(&CODEBUDDY_INTL_TARGET),
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessToken"}"#
        );
        assert_eq!(
            CodeBuddyIdeFlavor::Intl.keychain_service(),
            "CodeBuddy Safe Storage"
        );
        assert_ne!(
            CodeBuddyIdeFlavor::Cn.secret_key(),
            CodeBuddyIdeFlavor::Intl.secret_key()
        );
    }

    #[test]
    fn flavor_target_matches_flavor_fields() {
        for flavor in [CodeBuddyIdeFlavor::Cn, CodeBuddyIdeFlavor::Intl] {
            let target = flavor.target();
            assert_eq!(target.secret_key, flavor.secret_key());
            assert_eq!(target.macos_keychain_service, flavor.keychain_service());
            assert_eq!(
                target.linux_secret_tool_app_names,
                flavor.linux_secret_apps()
            );
            assert_eq!(target.display_name, flavor.product_label());
        }
    }

    #[test]
    fn secret_key_for_custom_target_uses_its_fields() {
        let target = VscodeSafeStorageTarget {
            data_dir_resolver: codebuddy_cn_data_dir,
            display_name: "Test",
            secret_item_prefix_extension_id: "tencent-cloud.coding-copilot",
            secret_key: "Tencent-Cloud.coding-copilot.new.accessToken",
            macos_keychain_service: "Code Safe Storage",
            linux_secret_tool_app_names: &["Code"],
        };
        assert_eq!(
            secret_storage_item_key_for(&target),
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"Tencent-Cloud.coding-copilot.new.accessToken"}"#
        );
    }

    #[test]
    fn data_dir_contains_codebuddy_cn() {
        let Some(dir) = codebuddy_cn_data_dir() else {
            return;
        };
        let s = dir.to_string_lossy();
        assert!(
            s.contains("CodeBuddy CN"),
            "data dir should contain CodeBuddy CN: {s}"
        );
        let Some(intl) = codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Intl) else {
            return;
        };
        let intl_s = intl.to_string_lossy();
        assert!(
            intl_s.contains("CodeBuddy") && !intl_s.contains("CodeBuddy CN"),
            "intl data dir should be CodeBuddy not CN: {intl_s}"
        );
    }

    #[test]
    fn state_db_path_ends_with_state_vscdb() {
        let Some(db) = codebuddy_cn_state_db_path() else {
            return;
        };
        assert!(db.ends_with("state.vscdb"));
        assert!(db.to_string_lossy().contains("globalStorage"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn cbc_roundtrip_with_known_password() {
        let key = pbkdf2_sha1_key("test-password", 1003);
        let plain = br#"{"token":"abc","accessToken":"uid+abc"}"#;
        let encrypted = encrypt_cbc_prefixed(V10_PREFIX, &key, plain).unwrap();
        assert!(encrypted.starts_with(V10_PREFIX));
        let decrypted = decrypt_cbc_prefixed(&encrypted, V10_PREFIX, &key).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn resolve_prefers_existing_candidate() {
        let dir =
            std::env::temp_dir().join(format!("wb-cn-ide-path-test-{}", uuid::Uuid::new_v4()));
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, b"").unwrap();
        let resolved = resolve_state_db_path(Some(&dir)).unwrap();
        assert_eq!(resolved, db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `has_secret_row_for`：只看 key 是否存在（有行 / 无行 / 无文件三态），
    /// 且不读值、不解密、不创建任何目录或文件。
    #[test]
    fn has_secret_row_detects_row_without_creating_files() {
        let dir =
            std::env::temp_dir().join(format!("wb-cn-secret-row-test-{}", uuid::Uuid::new_v4()));
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();

        // ① 库文件不存在 → false，且不得顺手创建目录/文件
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());
        assert!(!db.exists(), "只读探测不得创建 state.vscdb");

        let missing_root = dir.join("no-such-data-dir");
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&missing_root)).unwrap());
        assert!(!missing_root.exists(), "只读探测不得创建数据目录");

        // ①b 空库（0 字节文件）：ItemTable 尚未初始化 → false，且不得报错
        std::fs::write(&db, b"").unwrap();
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        // ② 表存在但无该 key → false
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('other', 'x')",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        // ③ 有该 key → true（值是不可解密的占位串，证明只查存在性、不走解密）
        let key = secret_storage_item_key();
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, 'not-a-ciphertext')",
            [key.as_str()],
        )
        .unwrap();
        drop(conn);
        assert!(has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 回归 issue #91：只读探测（`read_secret_for`）不得有任何建目录副作用，
    /// 否则 `installed` 会把探测自建的空数据目录误判为「已接入」，且用户删掉后
    /// 下次探测又会重建，形成自我维持的误报。国内版与国际版 target 一并断言。
    #[test]
    fn read_secret_probe_creates_nothing() {
        let base =
            std::env::temp_dir().join(format!("wb-cn-readonly-probe-{}", uuid::Uuid::new_v4()));

        for (label, target, root) in [
            (
                "cn/数据目录整体缺失",
                &CODEBUDDY_CN_TARGET,
                base.join("cn-missing"),
            ),
            (
                "cn/目录存在但无 globalStorage",
                &CODEBUDDY_CN_TARGET,
                base.join("cn-empty-root"),
            ),
            (
                "intl/数据目录整体缺失",
                &CODEBUDDY_INTL_TARGET,
                base.join("intl-missing"),
            ),
        ] {
            if label.ends_with("无 globalStorage") {
                std::fs::create_dir_all(&root).unwrap();
            }
            assert_eq!(
                read_secret_for(target, Some(&root)).unwrap(),
                None,
                "{label}: 无 db 时应返回 Ok(None)"
            );
            assert!(!root.join("User").exists(), "{label}: 不得创建 User/");
            assert!(
                !root.join("User").join("globalStorage").exists(),
                "{label}: 不得创建 User/globalStorage/"
            );
            assert!(
                !root.join("globalStorage").exists(),
                "{label}: 不得创建 globalStorage/"
            );
            assert!(
                !root.join("state.vscdb").exists(),
                "{label}: 不得创建 state.vscdb"
            );
        }

        std::fs::remove_dir_all(base).ok();
    }

    /// 回归 issue #91（写入路径不受影响）：db 不存在时注入仍会自动准备目录并写库成功。
    #[test]
    fn inject_still_prepares_directory() {
        let dir = std::env::temp_dir().join(format!("wb-cn-inject-mkdir-{}", uuid::Uuid::new_v4()));
        let plaintext = r#"{"token":"tok","accessToken":"uid+tok"}"#;
        let result = inject_secret_for(&CODEBUDDY_CN_TARGET, plaintext, Some(&dir));
        // Windows 上加密依赖系统 Safe Storage，无该条目会失败；只断言目录已准备好。
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        if result.is_ok() {
            assert!(db.exists(), "注入应创建 state.vscdb");
            let read = read_secret_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap();
            assert_eq!(read.as_deref(), Some(plaintext));
        } else {
            assert!(
                db.parent().map(|p| p.exists()).unwrap_or(false),
                "注入路径解析应准备 globalStorage 目录"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// 回归 issue #80：密钥环里确实有密码时，读本机登录信息不能再报
    /// 「无法加载 Linux secret storage key（v11）」。
    ///
    /// 只在本机既有密钥环条目、又有 CodeBuddy CN 数据目录时才验证，
    /// 无桌面环境（CI）直接跳过。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_v11_secret_is_readable_when_keyring_has_password() {
        let Some(_password) = crate::modules::linux_keyring::find_password(
            CODEBUDDY_CN_TARGET.linux_secret_tool_app_names,
        ) else {
            return;
        };
        let Some(db_path) = codebuddy_cn_state_db_path() else {
            return;
        };
        if !db_path.exists() {
            return;
        }
        read_secret_for(&CODEBUDDY_CN_TARGET, None)
            .expect("密钥环里有密码时，读取 CodeBuddy CN secret 不应失败");
    }

    /// Linux 写入 → 读回必须还原原文：有密钥环走 v11、没有则退回 peanuts 的 v10，
    /// 两条路都要能自洽。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_inject_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wb-linux-inject-{}", uuid::Uuid::new_v4()));
        let plaintext = r#"{"token":"tok","accessToken":"uid+tok"}"#;
        inject_secret_for(&CODEBUDDY_CN_TARGET, plaintext, Some(&dir)).unwrap();
        let read = read_secret_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap();
        assert_eq!(read.as_deref(), Some(plaintext));
        std::fs::remove_dir_all(dir).unwrap();
    }
