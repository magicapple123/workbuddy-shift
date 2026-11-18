    struct TempAuthDir(PathBuf);

    impl TempAuthDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "workbuddy-shift-auth-file-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn auth_path(&self, variant: WbVariant) -> PathBuf {
            self.0.join(variant.auth_file_path().file_name().unwrap())
        }

        fn marker_path(&self, variant: WbVariant) -> PathBuf {
            // 独立指定官方文件名，避免测试与生产实现共享错误的后缀拼接规则。
            self.0.join(match variant {
                WbVariant::Cn => "workbuddy-desktop.info.logged-out",
                WbVariant::Ai => "workbuddy-desktop-ai.info.logged-out",
            })
        }
    }

    impl Drop for TempAuthDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn successful_auth_write_clears_only_target_logout_marker() {
        for variant in [WbVariant::Cn, WbVariant::Ai] {
            for token in [
                json!("test-access-token"),
                json!({"$wbEncrypted": 1, "envelope": {"v": 1, "wrapped": "test"}}),
            ] {
                let dir = TempAuthDir::new();
                let path = dir.auth_path(variant);
                let marker = dir.marker_path(variant);
                let other_variant = match variant {
                    WbVariant::Cn => WbVariant::Ai,
                    WbVariant::Ai => WbVariant::Cn,
                };
                let other_path = dir.auth_path(other_variant);
                let other_marker = dir.marker_path(other_variant);
                std::fs::write(&marker, "logged-out").unwrap();
                std::fs::write(&other_marker, "other-logout").unwrap();
                std::fs::write(&other_path, "other-session").unwrap();
                let account = json!({"uid": "target", "access_token": token});

                write_account_to_auth_file_at(&account, &path).unwrap();

                assert!(
                    !marker.exists(),
                    "WorkBuddy 会忽略带退出标记的认证文件，切换成功必须清理标记"
                );
                let written = read_auth_file_at(&path).unwrap();
                assert_eq!(written["auth"]["accessToken"], token);
                assert_eq!(written["account"]["uid"], "target");
                assert_eq!(
                    std::fs::read_to_string(other_marker).unwrap(),
                    "other-logout"
                );
                assert_eq!(
                    std::fs::read_to_string(other_path).unwrap(),
                    "other-session"
                );
            }
        }
    }

    #[test]
    fn auth_write_without_logout_marker_remains_idempotent() {
        let dir = TempAuthDir::new();
        let path = dir.auth_path(WbVariant::Cn);
        let account = json!({"uid": "target", "access_token": "test-access-token"});

        write_account_to_auth_file_at(&account, &path).unwrap();
        write_account_to_auth_file_at(&account, &path).unwrap();

        assert!(!dir.marker_path(WbVariant::Cn).exists());
        let written = read_auth_file_at(&path).unwrap();
        assert_eq!(written["auth"]["accessToken"], "test-access-token");
        assert_eq!(written["allAccounts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn failed_auth_write_preserves_logout_marker() {
        let dir = TempAuthDir::new();
        let path = dir.auth_path(WbVariant::Cn);
        let marker = dir.marker_path(WbVariant::Cn);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "existing-data").unwrap();
        std::fs::write(&marker, "logged-out").unwrap();
        let account = json!({"uid": "target", "access_token": "test-access-token"});

        assert!(write_account_to_auth_file_at(&account, &path).is_err());

        assert_eq!(std::fs::read_to_string(marker).unwrap(), "logged-out");
        assert_eq!(
            std::fs::read_to_string(path.join("keep")).unwrap(),
            "existing-data"
        );
    }

    #[test]
    fn failed_logout_marker_removal_is_not_reported_as_success() {
        let dir = TempAuthDir::new();
        let path = dir.auth_path(WbVariant::Cn);
        let marker = dir.marker_path(WbVariant::Cn);
        // 用同名非空目录模拟无法 remove_file 的标记，不依赖平台权限行为。
        std::fs::create_dir(&marker).unwrap();
        std::fs::write(marker.join("keep"), "existing-data").unwrap();
        let account = json!({"uid": "target", "access_token": "test-access-token"});

        let error = write_account_to_auth_file_at(&account, &path).unwrap_err();

        assert!(
            error.contains("退出标记"),
            "错误应说明未清理退出标记：{error}"
        );
        assert!(marker.exists());
        assert_eq!(
            read_auth_file_at(&path).unwrap()["account"]["uid"],
            "target"
        );
    }

    #[test]
    fn auth_file_path_is_expected_location() {
        let p = WbVariant::Cn.auth_file_path();
        let s = p.to_string_lossy();
        assert!(
            s.contains("CodeBuddyExtension"),
            "路径应包含 CodeBuddyExtension: {s}"
        );
        assert!(
            s.ends_with("workbuddy-desktop.info"),
            "文件名应为 workbuddy-desktop.info: {s}"
        );

        // 国际版：同目录、不同文件
        let ai = WbVariant::Ai.auth_file_path();
        assert_eq!(ai.parent(), p.parent());
        assert!(ai.to_string_lossy().ends_with("workbuddy-desktop-ai.info"));
    }

    #[test]
    fn backup_file_name_is_distinguishable_per_variant() {
        let ts = "2026-09-15T00-00-00Z";
        assert_eq!(
            WbVariant::Cn.backup_file_name(ts),
            "workbuddy-desktop.2026-09-15T00-00-00Z.info"
        );
        assert_eq!(
            WbVariant::Ai.backup_file_name(ts),
            "workbuddy-desktop-ai.2026-09-15T00-00-00Z.info"
        );
    }

    #[test]
    fn import_from_auth_file_extracts_fields() {
        let root = json!({
            "account": {"uid": "u-1", "nickname": "小明", "email": "a@b.c"},
            "auth": {
                "accessToken": "AT-1",
                "refreshToken": "RT-1",
                "tokenType": "Bearer",
                "domain": "www.codebuddy.cn",
                "expiresAt": "1791912333558",
            },
            "domain": "www.codebuddy.cn",
        });
        // import_from_auth_file 从真实认证文件读取，此处直接测 parse_ts 与字段提取逻辑
        assert_eq!(parse_ts(root["auth"].get("expiresAt")), Some(1791912333558));
        assert_eq!(parse_ts(root["auth"].get("refreshToken")), None);
        assert_eq!(parse_ts(Some(&json!("1786728333"))), Some(1786728333));
    }

    #[test]
    fn import_without_email_does_not_synthesize_one() {
        let account = imported_account_from_root(
            json!({
                "account": {"uid": "u-1", "nickname": "同名用户"},
                "auth": {"accessToken": "test-token"}
            }),
            WbVariant::Cn,
        )
        .expect("auth payload should import");

        assert_eq!(account["uid"], "u-1");
        assert_eq!(account["nickname"], "同名用户");
        assert!(account["email"].is_null());
        assert_eq!(account["variant"], "cn");
    }

    /// 回归：`"accessToken": ""`（含纯空白）的登录态必须判为「没有 token」。
    /// 修复前 `secret_value` 对空串返回 `Some("")`，`imported_account_from_root` 里
    /// `access_token.is_none()` 的检查拦不住，导入会落库一条空凭据账号；
    /// `/api/import-local` 也因此不再返回 400「未读取到本地 WorkBuddy 登录信息」。
    #[test]
    fn blank_access_token_is_not_imported() {
        for blank in ["", "   ", "\t\n"] {
            let imported = imported_account_from_root(
                json!({
                    "account": {"uid": "u-1", "nickname": "小明"},
                    "auth": {"accessToken": blank, "refreshToken": "RT-1"}
                }),
                WbVariant::Cn,
            );
            assert!(imported.is_none(), "空 accessToken（{blank:?}）不得被导入");
        }

        // 加密信封不受影响：仍原样保留，切换写回依赖它。
        let envelope = json!({"$wbEncrypted": 1, "envelope": "enc"});
        let imported = imported_account_from_root(
            json!({
                "account": {"uid": "u-1"},
                "auth": {"accessToken": envelope.clone()}
            }),
            WbVariant::Cn,
        )
        .expect("加密信封 accessToken 必须可导入");
        assert_eq!(imported["access_token"], envelope);
    }

    #[test]
    fn imported_account_records_source_variant() {
        let account = imported_account_from_root(
            json!({
                "account": {"uid": "u-ai"},
                "auth": {"accessToken": "at-ai", "domain": "www.workbuddy.ai"}
            }),
            WbVariant::Ai,
        )
        .expect("auth payload should import");

        assert_eq!(account["variant"], "ai");
        assert_eq!(
            crate::modules::account::variant_of(&account),
            WbVariant::Ai,
            "导入账号自带档位，无需依赖域名兜底"
        );
    }
