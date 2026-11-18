    #[test]
    fn collected_uid_match_repairs_missing_local_id() {
        let mut accounts = vec![json!({
            "id": "",
            "uid": "uid-1",
            "access_token": "old",
        })];

        let saved = upsert_collected_account(
            &mut accounts,
            json!({"uid": "uid-1", "access_token": "new"}),
        );

        assert_eq!(accounts.len(), 1);
        assert!(get_str(&saved, "id").is_some());
        assert_eq!(accounts[0]["id"], saved["id"]);
    }

    /// 既无 id 也无 uid 的记录：首次追加时补 id，之后按 id 覆盖不追加。
    #[test]
    fn upsert_without_id_or_uid_appends_once_with_generated_id() {
        let mut accounts: Vec<Value> = vec![];
        upsert_account_in(&mut accounts, &json!({"access_token": "t1"}));

        assert_eq!(accounts.len(), 1);
        let id = accounts[0]["id"].as_str().unwrap_or_default().to_string();
        assert!(!id.is_empty(), "追加时必须补 id");

        let mut again = accounts[0].clone();
        again["access_token"] = json!("t2");
        upsert_account_in(&mut accounts, &again);

        assert_eq!(accounts.len(), 1, "补 id 后必须按 id 覆盖");
        assert_eq!(accounts[0]["access_token"], "t2");
        assert_eq!(accounts[0]["id"], id, "覆盖不得改变 id");
    }

    /// 回归 issue #111 的增长曲线：模拟刷新回写循环，无 id 记录连续多轮
    /// upsert 后账号库长度恒为 1，且首轮即自愈出 id。
    #[test]
    fn repeated_refresh_upsert_of_id_less_record_does_not_grow_store() {
        let mut accounts = vec![json!({"uid": "uid-1", "access_token": "t0"})];
        for round in 0..5 {
            let mut refreshed = accounts[0].clone();
            refreshed["access_token"] = json!(format!("t{round}"));
            upsert_account_in(&mut accounts, &refreshed);
        }

        assert_eq!(accounts.len(), 1, "刷新循环不得复制无 id 记录");
        assert!(
            accounts[0]["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "首轮覆盖后必须自愈出 id"
        );
        assert_eq!(accounts[0]["access_token"], "t4");
    }

    /// 采集路径的追加分支同样保证入库记录带 id（纵深防御）。
    #[test]
    fn collected_account_without_identity_gets_generated_id() {
        let mut accounts: Vec<Value> = vec![];
        let saved = upsert_collected_account(&mut accounts, json!({"access_token": "t"}));

        assert_eq!(accounts.len(), 1);
        let id = saved["id"].as_str().unwrap_or_default();
        assert!(!id.is_empty(), "采集追加分支必须补 id");
        assert_eq!(accounts[0]["id"], saved["id"], "返回值与库中记录一致");
    }

    #[test]
    fn persisted_same_name_accounts_can_be_found_and_deleted_independently() {
        let test_dir = std::env::temp_dir().join(format!(
            "workbuddy-shift-same-name-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = test_dir.join("accounts.json");
        let mut accounts = vec![];
        upsert_collected_account(
            &mut accounts,
            account("account-1", Some("uid-1"), "同名用户", None),
        );
        upsert_collected_account(
            &mut accounts,
            account("account-2", Some("uid-2"), "同名用户", None),
        );
        save_accounts_to_path(&path, &accounts).expect("same-name accounts should persist");

        let persisted = load_accounts_from_path(&path);
        assert_eq!(
            find_account_in(&persisted, "account-1").unwrap()["uid"],
            "uid-1"
        );
        assert_eq!(
            find_account_in(&persisted, "account-2").unwrap()["uid"],
            "uid-2"
        );

        delete_account_from_path(&path, "account-1").expect("first account should delete");
        let after_first_delete = load_accounts_from_path(&path);
        assert!(find_account_in(&after_first_delete, "account-1").is_none());
        assert_eq!(
            find_account_in(&after_first_delete, "account-2").unwrap()["uid"],
            "uid-2"
        );

        delete_account_from_path(&path, "account-2").expect("second account should delete");
        assert!(load_accounts_from_path(&path).is_empty());
        std::fs::remove_dir_all(&test_dir).expect("temporary account store should clean up");
    }

    // -----------------------------------------------------------------------
    // 本地展示字段：备注与显示字段选择
    // -----------------------------------------------------------------------

    /// displayField 缺失时展示名保持改造前行为（email → nickname → uid）。
    #[test]
    fn display_name_without_display_field_keeps_legacy_fallback() {
        let with_email = account("a-1", Some("uid-1"), "小明", Some("a@b.c"));
        assert_eq!(account_display_name(&with_email), "a@b.c");
        let no_email = account("a-2", Some("uid-2"), "小明", None);
        assert_eq!(account_display_name(&no_email), "小明");
    }

    /// displayField = note / phone 时优先取对应字段，取不到回退原兜底链。
    #[test]
    fn display_name_follows_display_field_with_fallback() {
        let mut acc = account("a-1", Some("uid-1"), "小明", None);
        acc["note"] = json!("工作号");
        acc["displayField"] = json!("note");
        assert_eq!(account_display_name(&acc), "工作号");

        acc["note"] = Value::Null;
        assert_eq!(account_display_name(&acc), "小明", "备注为空应回退昵称");

        acc["displayField"] = json!("phone");
        assert_eq!(account_display_name(&acc), "小明", "无手机号应回退昵称");
        acc["profile_raw"] = json!({"phoneNumber": "13800138000"});
        assert_eq!(account_display_name(&acc), "13800138000");

        acc["displayField"] = json!("nickname");
        assert_eq!(account_display_name(&acc), "小明");
    }

    /// account_meta 下发手机号（实时读 profile_raw）、备注与显示字段。
    #[test]
    fn account_meta_exposes_phone_note_and_display_field() {
        let mut acc = account("a-1", Some("uid-1"), "小明", None);
        acc["profile_raw"] = json!({"phoneNumber": "13800138000"});
        acc["note"] = json!("工作号");
        acc["displayField"] = json!("phone");
        let meta = account_meta(&acc);
        assert_eq!(meta["phoneNumber"], "13800138000");
        assert_eq!(meta["note"], "工作号");
        assert_eq!(meta["displayField"], "phone");

        // profile_raw 缺失或异常形态：手机号折叠为 null，不泄露对象。
        let bare = account("a-2", Some("uid-2"), "小红", None);
        let meta = account_meta(&bare);
        assert!(meta["phoneNumber"].is_null());
        assert!(meta["note"].is_null());
        assert!(meta["displayField"].is_null());
    }

    /// 更新接口：设置 / 清空备注、切换显示字段、只更新传入项。
    #[test]
    fn update_account_display_sets_note_and_field() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];

        let updated = update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"note": "  工作号  ", "displayField": "note"}),
        )
        .expect("valid patch should apply");
        assert_eq!(updated["note"], "工作号", "备注应 trim");
        assert_eq!(updated["displayField"], "note");

        // 只改显示字段：备注保持。
        update_account_display_in(&mut accounts, "a-1", &json!({"displayField": "nickname"}))
            .expect("field-only patch should apply");
        assert_eq!(accounts[0]["note"], "工作号");
        assert_eq!(accounts[0]["displayField"], "nickname");

        // 显式 null 与纯空白都等价于清空。
        update_account_display_in(&mut accounts, "a-1", &json!({"note": null}))
            .expect("null note should clear");
        assert!(accounts[0]["note"].is_null());
        update_account_display_in(&mut accounts, "a-1", &json!({"note": "   "}))
            .expect("blank note should clear");
        assert!(accounts[0]["note"].is_null());
    }

    /// 更新接口的校验：非法字段 / 未知 id / 超长备注都不写入。
    #[test]
    fn update_account_display_rejects_invalid_patch() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];

        assert!(
            update_account_display_in(&mut accounts, "a-1", &json!({"displayField": "email"}))
                .is_err(),
            "非白名单显示字段应拒绝"
        );
        assert!(
            update_account_display_in(&mut accounts, "a-1", &json!({"note": 42})).is_err(),
            "非字符串备注应拒绝"
        );
        assert!(
            update_account_display_in(&mut accounts, "missing", &json!({"note": "x"})).is_err(),
            "未知账号应报错"
        );
        let long = "字".repeat(NOTE_MAX_CHARS + 1);
        assert!(
            update_account_display_in(&mut accounts, "a-1", &json!({"note": long})).is_err(),
            "超长备注应拒绝"
        );
        // 任一失败都不应留下部分更新。
        assert!(accounts[0].get("note").is_none());
        assert!(accounts[0].get("displayField").is_none());
    }

    /// 采集合并（扫码重登）保留本地备注与显示选择。
    #[test]
    fn collected_upsert_preserves_local_display_fields() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["note"] = json!("工作号");
        existing["displayField"] = json!("note");
        let mut accounts = vec![existing];

        let collected = account("a-1", Some("uid-1"), "小明（官方更新）", None);
        let saved = upsert_collected_account(&mut accounts, collected);
        assert_eq!(saved["note"], "工作号");
        assert_eq!(saved["displayField"], "note");
        assert_eq!(saved["nickname"], "小明（官方更新）");
    }

    /// token 刷新路径（upsert_account_in）保留本地展示字段。
    #[test]
    fn refresh_upsert_preserves_local_display_fields() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["note"] = json!("工作号");
        existing["displayField"] = json!("phone");
        let mut accounts = vec![existing];

        let refreshed = account("a-1", Some("uid-1"), "小明", None);
        upsert_account_in(&mut accounts, &refreshed);
        assert_eq!(accounts[0]["note"], "工作号");
        assert_eq!(accounts[0]["displayField"], "phone");
    }
