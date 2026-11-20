    fn record(
        id: &str,
        uid: Option<&str>,
        nickname: &str,
        email: Option<&str>,
        token: bool,
    ) -> Value {
        json!({
            "id": id,
            "uid": uid,
            "nickname": nickname,
            "email": email,
            "access_token": if token { format!("token-{id}") } else { String::new() },
            "createdAt": 1,
        })
    }

    #[test]
    fn parse_rejects_empty_invalid_and_non_array() {
        assert!(parse_accounts_json("").is_err());
        assert!(parse_accounts_json("not json").is_err());
        assert!(parse_accounts_json(r#"{ "a": 1 }"#).is_err());
    }

    #[test]
    fn parse_rejects_non_object_element() {
        let err = parse_accounts_json(r#"[{ "uid": "u1" }, 42]"#).unwrap_err();
        assert!(err.contains("第 2 项"), "错误应带位置：{err}");
    }

    #[test]
    fn parse_accepts_object_array() {
        let parsed = parse_accounts_json(r#"[{ "uid": "u1" }, {}]"#).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn merge_overwrites_same_uid_preserving_imported_record() {
        let mut accounts = vec![record("local", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "id": "imported", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1, "同 uid 应覆盖而不是新增");
        assert_eq!(accounts[0]["id"], "imported", "覆盖保留导入记录原样");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok-new");
    }

    #[test]
    fn merge_preserves_local_id_when_imported_record_has_none() {
        let mut accounts = vec![record("local-id", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["id"], "local-id", "导入记录缺 id 时保留本地 id");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok");
    }

    /// R5-04：导入**旧备份**（不含 `mark`）不得冲掉本地标记。
    ///
    /// 覆盖分支会用导入记录整条替换本地记录，所以 `mark` 必须加进保留列表，
    /// 与 `note` / `displayField` 同一处理。
    #[test]
    fn merge_keeps_local_mark_when_backup_has_none() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("设计组-主号");
        local["mark"] = json!({
            "text": "即将限额",
            "color": "warning",
            "createdAt": 1_763_654_321_000_i64,
            "expiresAt": null,
        });
        let mut accounts = vec![local];

        let text = r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts[0]["mark"]["text"], "即将限额", "旧备份缺 mark 时保留本地标记");
        assert_eq!(accounts[0]["mark"]["color"], "warning");
        assert_eq!(accounts[0]["note"], "设计组-主号", "note 同样保留");
        assert_eq!(accounts[0]["nickname"], "新名称", "官方字段仍以导入记录为准");
    }

    /// R5-04：备份自带 `mark` 时以备份为准（导入即恢复用户当时的展示配置）。
    #[test]
    fn merge_restores_mark_from_backup() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["mark"] = json!({"text": "暂不使用", "color": "neutral", "createdAt": 1, "expiresAt": null});
        let mut accounts = vec![local];

        let text = r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new",
            "mark": { "text": "积分将过期", "color": "danger", "createdAt": 2, "expiresAt": 1800000000000 } }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(accounts[0]["mark"]["text"], "积分将过期");
        assert_eq!(accounts[0]["mark"]["color"], "danger");
        assert_eq!(accounts[0]["mark"]["expiresAt"], json!(1_800_000_000_000_i64));    }

    #[test]
    fn merge_appends_when_uid_missing_or_unmatched() {
        let mut accounts = vec![record("local", Some("u1"), "甲", None, true)];
        let text = r#"[
            { "id": "n1", "nickname": "无uid", "access_token": "t1" },
            { "id": "n2", "uid": "u-other", "nickname": "uid不匹配", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 0);
        assert_eq!(accounts.len(), 3);
    }

    #[test]
    fn merge_skips_missing_token() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "id": "no-token", "uid": "u9", "nickname": "缺token" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
        assert!(accounts.is_empty(), "缺 token 的记录不得进入账号库");
    }

    /// 回归 issue #100：加密信封凭据按采集口径放行，且导入后原样保留（不落空串）。
    #[test]
    fn merge_imports_envelope_token_as_is() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{
            "id": "env-1",
            "uid": "u-env",
            "nickname": {"$wbEncrypted": 1, "envelope": "nick"},
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
            "refresh_token": {"$wbEncrypted": 1, "envelope": "r"}
        }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.imported, 1, "信封凭据应计入 imported");
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0]["access_token"],
            json!({"$wbEncrypted": 1, "envelope": "a"}),
            "信封凭据必须原样保留"
        );
        assert_eq!(accounts[0]["refresh_token"]["envelope"], "r");
    }

    /// 空串与普通对象（非信封）仍按无凭据处理：预览「缺少 token」、导入计 skipped。
    #[test]
    fn merge_still_skips_blank_and_plain_object_token() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[
            { "id": "blank", "uid": "u-blank", "access_token": "   " },
            { "id": "object", "uid": "u-object", "access_token": { "token": "t" } }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 2);
        assert!(accounts.is_empty(), "空串 / 普通对象不得进入账号库");
    }

    #[test]
    fn merge_counts_out_of_range_index_as_skipped() {
        let mut accounts: Vec<Value> = vec![];
        let result =
            merge_import_records(&mut accounts, r#"[{ "access_token": "t" }]"#, &[5]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn merge_deduplicates_within_file() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[
            { "id": "f1", "uid": "u9", "access_token": "t1" },
            { "id": "f2", "uid": "u9", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1, "文件内重复 uid 也不得产生重复账号");
        assert_eq!(accounts[0]["id"], "f2");
    }

    /// 回归 issue #111：导入记录缺 id 时必须补一个，否则刷新回写会反复追加副本。
    #[test]
    fn merge_generates_id_for_imported_record_without_id() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "uid": "u-new", "nickname": "无id", "access_token": "t1" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.imported, 1);
        assert_eq!(accounts.len(), 1);
        assert!(
            accounts[0]["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "追加入库的记录必须带 id"
        );
    }

    /// 本地同 uid 记录也是无 id 的历史脏数据时，覆盖后同样要补 id。
    #[test]
    fn merge_generates_id_when_neither_imported_nor_local_record_has_one() {
        let mut accounts = vec![json!({
            "uid": "u1",
            "nickname": "旧名称",
            "access_token": "t-old",
        })];
        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "t-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert!(
            accounts[0]["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "本地也无 id 时覆盖后必须补 id"
        );
    }

    #[test]
    fn merge_repairs_empty_local_id_when_imported_record_has_no_id() {
        let mut accounts = vec![json!({
            "id": "  ",
            "uid": "u1",
            "access_token": "t-old",
        })];
        let text = r#"[{ "uid": "u1", "access_token": "t-new" }]"#;

        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert!(account::get_str(&accounts[0], "id").is_some());
        assert_eq!(accounts[0]["access_token"], "t-new");
    }

    #[test]
    fn export_requires_selection() {
        let accounts = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(select_export_records(&accounts, &[]).is_err());
        assert!(select_export_records(&accounts, &["missing".to_string()]).is_err());
    }

    #[test]
    fn export_returns_full_records_with_tokens() {
        let accounts = vec![
            record("a1", Some("u1"), "甲", Some("a@b.c"), true),
            record("a2", Some("u2"), "乙", None, true),
        ];
        let exported =
            select_export_records(&accounts, &["a1".to_string(), "missing".to_string()]).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0]["id"], "a1");
        assert_eq!(exported[0]["access_token"], "token-a1");
    }

    #[test]
    fn exported_records_roundtrip_import() {
        let accounts = vec![json!({
            "id": "a1",
            "uid": "u1",
            "nickname": "甲",
            "email": "a@b.c",
            "access_token": "tok-1",
            "refresh_token": "ref-1",
            "auth_raw": { "k": "v" },
            "createdAt": 1,
        })];
        let exported = select_export_records(&accounts, &["a1".to_string()]).unwrap();
        let text = serde_json::to_string(&exported).unwrap();
        let mut target: Vec<Value> = vec![];
        let result = merge_import_records(&mut target, &text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(target.len(), 1);
        assert_eq!(
            target[0]["access_token"], "tok-1",
            "round-trip 保留 access_token"
        );
        assert_eq!(target[0]["refresh_token"], "ref-1");
        assert_eq!(target[0]["auth_raw"]["k"], "v");
    }

    #[test]
    fn preview_exposes_only_desensitized_fields() {
        let text = r#"[{
            "id": "a1",
            "uid": "u1",
            "nickname": "小明",
            "email": "x@y.z",
            "access_token": "SECRET"
        }]"#;
        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["total"], 1);
        assert_eq!(preview["accounts"][0]["index"], 0);
        assert_eq!(preview["accounts"][0]["uid"], "u1");
        assert_eq!(preview["accounts"][0]["nickname"], "小明");
        assert_eq!(preview["accounts"][0]["email"], "x@y.z");
        assert_eq!(preview["accounts"][0]["hasToken"], true);
        assert!(
            preview["accounts"][0].get("access_token").is_none(),
            "预览不得泄露 token"
        );
    }

    /// 回归 issue #100：预览的对象字段必须折叠为 null（防 React error #31 白屏），
    /// 信封凭据标记 encrypted，明文凭据不标记。
    #[test]
    fn preview_folds_object_fields_and_marks_envelope_token() {
        let text = r#"[
            {
                "id": "env-1",
                "uid": "u-env",
                "nickname": {"$wbEncrypted": 1, "envelope": "nick"},
                "email": {"$wbEncrypted": 1, "envelope": "mail"},
                "access_token": {"$wbEncrypted": 1, "envelope": "a"}
            },
            {
                "id": "plain-1",
                "uid": "u-plain",
                "nickname": "明文昵称",
                "email": "x@y.z",
                "access_token": "SECRET"
            }
        ]"#;
        let preview = preview_accounts(text).unwrap();

        assert_eq!(preview["total"], 2);
        let env = &preview["accounts"][0];
        assert_eq!(env["uid"], "u-env");
        assert!(
            env["nickname"].is_null(),
            "信封 nickname 必须折叠为 null，不得透传对象"
        );
        assert!(
            env["email"].is_null(),
            "信封 email 必须折叠为 null，不得透传对象"
        );
        assert_eq!(env["hasToken"], true, "信封凭据应视为有凭据");
        assert_eq!(env["encrypted"], true, "信封凭据应标记 encrypted");
        assert!(env.get("access_token").is_none(), "预览不得泄露 token");

        let plain = &preview["accounts"][1];
        assert_eq!(plain["nickname"], "明文昵称");
        assert_eq!(plain["email"], "x@y.z");
        assert_eq!(plain["hasToken"], true);
        assert_eq!(plain["encrypted"], false, "明文凭据不得误标 encrypted");
    }

    #[test]
    fn export_file_name_rejects_traversal_and_non_json() {
        assert!(validate_export_file_name("../../etc/passwd.json").is_err());
        assert!(validate_export_file_name("a/b.json").is_err());
        assert!(validate_export_file_name("a\\b.json").is_err());
        assert!(validate_export_file_name("out.txt").is_err());
        assert!(validate_export_file_name("").is_err());
        assert!(validate_export_file_name("workbuddy-shift-accounts-2026-08-21.json").is_ok());
    }

    #[test]
    fn export_path_validation() {
        assert!(
            validate_export_path("relative/out.json").is_err(),
            "必须绝对路径"
        );
        // 绝对路径的形态按平台不同（Windows 需盘符或 UNC 前缀），fixture 必须取自
        // 当前平台：写死 `/tmp/...` 会在 Windows 上被判为相对路径而误报。
        let dir = std::env::temp_dir();
        let txt = dir.join("out.txt").to_string_lossy().to_string();
        let upper = dir.join("out.JSON").to_string_lossy().to_string();
        let json = dir.join("out.json").to_string_lossy().to_string();
        assert!(validate_export_path(&txt).is_err(), "必须 .json");
        assert!(validate_export_path(&upper).is_ok(), "扩展名不区分大小写");
        assert!(validate_export_path(&json).is_ok());
    }

    #[test]
    fn export_accounts_to_file_writes_json_with_tokens() {
        let dir = std::env::temp_dir();
        let file_name = format!(
            "workbuddy-shift-accounts-test-{}.json",
            uuid::Uuid::new_v4().simple()
        );
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        let path = write_records_to_file(&dir, &records, &file_name).unwrap();
        let written: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0]["access_token"], "token-a1", "导出文件保留 token");
    }

    #[test]
    fn write_records_to_file_rejects_bad_name() {
        let dir = std::env::temp_dir();
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(write_records_to_file(&dir, &records, "../escape.json").is_err());
        assert!(write_records_to_file(&dir, &records, "no-ext").is_err());
    }

    /// 导入备份：记录未携带本地展示字段（备注 / 显示选择）时保留本地值。
    #[test]
    fn merge_preserves_local_display_fields_when_absent() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("本地备注");
        local["displayField"] = json!("note");
        let mut accounts = vec![local];

        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["note"], "本地备注");
        assert_eq!(accounts[0]["displayField"], "note");
        assert_eq!(accounts[0]["nickname"], "新名称");
    }

    /// 导入备份：记录自带展示字段时以导入为准（导入即恢复当时的展示配置）。
    #[test]
    fn merge_uses_imported_display_fields_when_present() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("本地备注");
        local["displayField"] = json!("note");
        let mut accounts = vec![local];

        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok-new",
            "note": "备份备注", "displayField": "phone" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["note"], "备份备注");
        assert_eq!(accounts[0]["displayField"], "phone");
    }
