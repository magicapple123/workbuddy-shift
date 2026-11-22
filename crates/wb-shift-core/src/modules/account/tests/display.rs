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

    /// R5-07：`displayField` 缺失时按「备注优先」取第一个非空字段
    /// （备注 → 昵称 → 邮箱 → uid → id）。历史上这里是「邮箱优先」，本轮统一改判。
    #[test]
    fn display_name_without_display_field_is_note_first() {
        let with_email = account("a-1", Some("uid-1"), "小明", Some("a@b.c"));
        assert_eq!(account_display_name(&with_email), "小明", "昵称优先于邮箱");
        let no_email = account("a-2", Some("uid-2"), "小明", None);
        assert_eq!(account_display_name(&no_email), "小明");

        let mut with_note = account("a-3", Some("uid-3"), "小明", Some("a@b.c"));
        with_note["note"] = json!("设计组-主号");
        assert_eq!(account_display_name(&with_note), "设计组-主号", "有备注时显示备注");

        let email_only = json!({"id": "a-4", "email": "a@b.c"});
        assert_eq!(account_display_name(&email_only), "a@b.c", "无昵称回退邮箱");
        let uid_only = json!({"id": "a-5", "uid": "u-5"});
        assert_eq!(account_display_name(&uid_only), "u-5", "无邮箱回退 uid");
        let id_only = json!({"id": "a-6"});
        assert_eq!(account_display_name(&id_only), "a-6", "最后回退 id");
    }

    /// R5-07：手机号是唯一例外——显式选「手机号」时压过备注；没有手机号则继续往下走。
    #[test]
    fn display_name_phone_preference_beats_note() {
        let mut acc = account("a-1", Some("uid-1"), "小明", Some("a@b.c"));
        acc["note"] = json!("设计组-主号");
        acc["displayField"] = json!("phone");
        assert_eq!(account_display_name(&acc), "设计组-主号", "无手机号时回退备注");

        acc["profile_raw"] = json!({"phoneNumber": "13800138000"});
        assert_eq!(account_display_name(&acc), "13800138000", "手机号压过备注");

        // `note` 偏好与新默认等价（保留只为兼容存量数据）。
        acc["displayField"] = json!("note");
        assert_eq!(account_display_name(&acc), "设计组-主号");
        // `nickname` 不再压制备注。
        acc["displayField"] = json!("nickname");
        assert_eq!(account_display_name(&acc), "设计组-主号");
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

    // -----------------------------------------------------------------------
    // R5-04 账号标记：校验、下发、持久化保留
    // -----------------------------------------------------------------------

    /// 未来时刻（毫秒），供「到期时间」用例复用。
    fn in_hours(hours: i64) -> i64 {
        now_ms() + hours * 3_600_000
    }

    /// 保存标记：`createdAt` 由后端生成，客户端传值被忽略。
    #[test]
    fn update_account_display_sets_mark_ignoring_client_created_at() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        let updated = update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"mark": {
                "text": "  即将限额  ",
                "color": "warning",
                "expiresAt": in_hours(2),
                "createdAt": 1,
            }}),
        )
        .expect("valid mark should apply");

        assert_eq!(updated["mark"]["text"], "即将限额", "文本应 trim");
        assert_eq!(updated["mark"]["color"], "warning");
        assert_ne!(updated["mark"]["createdAt"], json!(1), "createdAt 必须由后端生成");
        assert!(
            updated["mark"]["createdAt"].as_i64().unwrap_or_default() > 0,
            "createdAt 应为当前毫秒"
        );
        assert_eq!(accounts[0]["mark"], updated["mark"], "应真正写回账号记录");
    }

    /// `expiresAt: null` = 不自动过期（默认档），必须能存下来。
    #[test]
    fn mark_without_expiry_means_manual_removal() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"mark": {"text": "暂不使用", "color": "neutral", "expiresAt": null}}),
        )
        .expect("null expiry should apply");
        assert!(accounts[0]["mark"]["expiresAt"].is_null());
        assert_eq!(accounts[0]["mark"]["color"], "neutral");
    }

    /// 单账号只保留一个标记：重复保存即覆盖；`null` 立即解除。
    #[test]
    fn mark_is_single_and_can_be_removed() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"mark": {"text": "即将限额", "color": "warning"}}),
        )
        .expect("first mark should apply");
        update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"mark": {"text": "积分将过期", "color": "danger"}}),
        )
        .expect("second mark should overwrite");
        assert_eq!(accounts[0]["mark"]["text"], "积分将过期");
        assert_eq!(accounts[0]["mark"]["color"], "danger");

        update_account_display_in(&mut accounts, "a-1", &json!({"mark": null}))
            .expect("null should clear");
        assert!(accounts[0]["mark"].is_null(), "解除标记后字段应为 null");
    }

    /// 校验：超长 / 非法色 / 过去时间 / 换行都拒绝，且不留下部分更新。
    #[test]
    fn mark_validation_rejects_invalid_input() {
        let long = "字".repeat(MARK_TEXT_MAX_CHARS + 1);
        let cases = [
            ("超长", json!({"mark": {"text": long, "color": "warning"}}), "字符"),
            (
                "非法色",
                json!({"mark": {"text": "即将限额", "color": "purple"}}),
                "颜色",
            ),
            (
                "过去时间",
                json!({"mark": {"text": "即将限额", "color": "warning", "expiresAt": now_ms() - 1000}}),
                "晚于当前时间",
            ),
            (
                "换行",
                json!({"mark": {"text": "即将\n限额", "color": "warning"}}),
                "换行",
            ),
            (
                "空文本",
                json!({"mark": {"text": "   ", "color": "warning"}}),
                "不能为空",
            ),
            (
                "非对象",
                json!({"mark": "即将限额"}),
                "格式无效",
            ),
        ];
        for (case, patch, expect) in cases {
            let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
            let error = update_account_display_in(&mut accounts, "a-1", &patch)
                .expect_err(&format!("{case} 应被拒绝"));
            assert!(error.contains(expect), "{case} 的错误文案应含「{expect}」：{error}");
            assert!(accounts[0].get("mark").is_none(), "{case} 不得留下部分更新");
        }
    }

    /// 旧数据缺字段 / mark 损坏时整体下发 `null`（前端按「无标记」处理）。
    #[test]
    fn account_meta_mark_is_null_for_missing_or_broken_mark() {
        let legacy = account("a-1", Some("uid-1"), "小明", None);
        assert!(account_meta(&legacy)["mark"].is_null(), "旧账号库无 mark → null");

        let mut broken = account("a-2", Some("uid-2"), "小明", None);
        broken["mark"] = json!({"text": "即将限额", "color": "purple", "createdAt": now_ms()});
        assert!(account_meta(&broken)["mark"].is_null(), "非法色整体降级为 null");

        let mut no_created = account("a-3", Some("uid-3"), "小明", None);
        no_created["mark"] = json!({"text": "即将限额", "color": "warning"});
        assert!(account_meta(&no_created)["mark"].is_null(), "缺 createdAt 视为损坏");

        let mut not_object = account("a-4", Some("uid-4"), "小明", None);
        not_object["mark"] = json!("即将限额");
        assert!(account_meta(&not_object)["mark"].is_null(), "非对象视为损坏");
    }

    /// 合法标记按原样下发（含已过期的一项，前端要用来显示「已过期」与续期）。
    #[test]
    fn account_meta_exposes_valid_mark_including_expired() {
        let mut acc = account("a-1", Some("uid-1"), "小明", None);
        acc["mark"] = json!({
            "text": "即将限额",
            "color": "warning",
            "createdAt": 1_763_654_321_000_i64,
            "expiresAt": 1_000,
        });
        let meta = account_meta(&acc);
        assert_eq!(meta["mark"]["text"], "即将限额");
        assert_eq!(meta["mark"]["color"], "warning");
        assert_eq!(meta["mark"]["createdAt"], 1_763_654_321_000_i64);
        assert_eq!(meta["mark"]["expiresAt"], 1_000, "已过期也要下发，交给前端判定");
    }

    /// 刷新 Token（upsert_account_in）不得冲掉标记。
    #[test]
    fn refresh_upsert_preserves_mark() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["mark"] = json!({"text": "即将限额", "color": "warning", "createdAt": 1, "expiresAt": null});
        let mut accounts = vec![existing];

        upsert_account_in(&mut accounts, &account("a-1", Some("uid-1"), "小明", None));
        assert_eq!(accounts[0]["mark"]["text"], "即将限额");
        assert_eq!(accounts[0]["mark"]["color"], "warning");
    }

    /// 扫码重登 / 自动导入本机登录态（upsert_collected_account）同样保留标记。
    #[test]
    fn collected_upsert_preserves_mark() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["mark"] = json!({"text": "即将限额", "color": "warning", "createdAt": 1, "expiresAt": null});
        let mut accounts = vec![existing];

        let saved = upsert_collected_account(&mut accounts, account("a-1", Some("uid-1"), "小明（官方更新）", None));
        assert_eq!(saved["mark"]["text"], "即将限额");
        assert_eq!(saved["nickname"], "小明（官方更新）");
    }

    /// 标记落盘后重新读取仍在：这是「刷新 Token / 重启应用不丢」的持久化前提。
    #[test]
    fn mark_survives_disk_roundtrip() {
        let test_dir = std::env::temp_dir().join(format!(
            "workbuddy-shift-mark-roundtrip-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = test_dir.join("accounts.json");

        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        update_account_display_in(
            &mut accounts,
            "a-1",
            &json!({"mark": {"text": "即将限额", "color": "danger", "expiresAt": in_hours(2)}}),
        )
        .expect("valid mark should apply");
        save_accounts_to_path(&path, &accounts).expect("accounts with mark should persist");

        let reloaded = load_accounts_from_path(&path);
        assert_eq!(reloaded.len(), 1);
        let meta = account_meta(&reloaded[0]);
        assert_eq!(meta["mark"]["text"], "即将限额", "重新读取后标记仍在");
        assert_eq!(meta["mark"]["color"], "danger");
        assert_eq!(meta["mark"]["expiresAt"], accounts[0]["mark"]["expiresAt"]);
        assert_eq!(meta["mark"]["createdAt"], accounts[0]["mark"]["createdAt"]);

        // 再次写回（模拟重启后再改备注）也不得丢掉标记。
        let mut rewritten = reloaded.clone();
        update_account_display_in(&mut rewritten, "a-1", &json!({"note": "工作号"}))
            .expect("note update should apply");
        save_accounts_to_path(&path, &rewritten).expect("rewrite should succeed");
        let again = load_accounts_from_path(&path);
        assert_eq!(again[0]["note"], "工作号");
        assert_eq!(account_meta(&again[0])["mark"]["text"], "即将限额");

        std::fs::remove_dir_all(&test_dir).expect("temporary account store should clean up");
    }

    // -----------------------------------------------------------------------
    // issue #184「最近切换」：lastUsedAt / lastUsedClient 的写入与保留
    // -----------------------------------------------------------------------

    /// 旧账号库（没有这两个键）下发 `null`：前端据此不显示徽标，不出现「从未」噪音。
    #[test]
    fn account_meta_reports_null_when_last_used_is_absent() {
        let acc = account("a-1", Some("uid-1"), "小明", None);
        let meta = account_meta(&acc);
        assert!(meta["lastUsedAt"].is_null());
        assert!(meta["lastUsedClient"].is_null());
    }

    /// 写入助手：只写这两个键，时间戳与来源端都按入参落库，其余字段原样保留。
    #[test]
    fn touch_last_used_writes_timestamp_and_client_only() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        let before = accounts[0].clone();

        touch_last_used_in(&mut accounts, "a-1", "vscode", 1_763_654_321_000)
            .expect("known account should be touched");

        assert_eq!(accounts[0]["lastUsedAt"], 1_763_654_321_000_i64);
        assert_eq!(accounts[0]["lastUsedClient"], "vscode");
        // 除这两个键外，账号记录必须逐字段不变（不碰 token / 官方数据）。
        let mut expected = before.clone();
        expected["lastUsedAt"] = json!(1_763_654_321_000_i64);
        expected["lastUsedClient"] = json!("vscode");
        assert_eq!(accounts[0], expected);
    }

    /// 未知来源端（未来版本的客户端名）只写时间戳，来源端留空：老版本前端读不懂的值不入库。
    #[test]
    fn touch_last_used_ignores_unknown_client_label() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        touch_last_used_in(&mut accounts, "a-1", "atom-plugin", 42).expect("account exists");

        assert_eq!(accounts[0]["lastUsedAt"], 42);
        assert!(accounts[0]["lastUsedClient"].is_null());
    }

    /// 账号不存在时返回错误且不改动任何记录（调用方忽略错误即可，不影响切换结果）。
    #[test]
    fn touch_last_used_on_missing_account_does_not_mutate() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        let before = accounts.clone();

        assert!(touch_last_used_in(&mut accounts, "nope", "workbuddy", 42).is_err());
        assert_eq!(accounts, before);
    }

    /// 官方资料重采（刷新 Token）保留 `lastUsedAt/lastUsedClient`。
    #[test]
    fn refresh_upsert_preserves_last_used() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["lastUsedAt"] = json!(1_763_654_321_000_i64);
        existing["lastUsedClient"] = json!("jetbrains");
        let mut accounts = vec![existing];

        // 官方刷新对象天然没有这两个键。
        upsert_account_in(
            &mut accounts,
            &json!({"id": "a-1", "uid": "uid-1", "access_token": "refreshed"}),
        );

        assert_eq!(accounts[0]["lastUsedAt"], 1_763_654_321_000_i64);
        assert_eq!(accounts[0]["lastUsedClient"], "jetbrains");
        assert_eq!(accounts[0]["access_token"], "refreshed", "官方字段仍应更新");
    }

    /// 扫码重登 / 自动导入本机登录态同样保留 `lastUsedAt/lastUsedClient`。
    #[test]
    fn collected_upsert_preserves_last_used() {
        let mut existing = account("a-1", Some("uid-1"), "小明", None);
        existing["lastUsedAt"] = json!(1_763_654_321_000_i64);
        existing["lastUsedClient"] = json!("rotate");
        let mut accounts = vec![existing];

        let saved = upsert_collected_account(
            &mut accounts,
            json!({"uid": "uid-1", "nickname": "小明（官方更新）", "access_token": "collected"}),
        );

        assert_eq!(saved["lastUsedAt"], 1_763_654_321_000_i64);
        assert_eq!(saved["lastUsedClient"], "rotate");
    }

    /// 改备注（`update_account_display`）不得抹掉「最近切换」。
    #[test]
    fn display_update_preserves_last_used() {
        let mut accounts = vec![account("a-1", Some("uid-1"), "小明", None)];
        touch_last_used_in(&mut accounts, "a-1", "workbuddy", 1_763_654_321_000)
            .expect("account exists");

        update_account_display_in(&mut accounts, "a-1", &json!({"note": "工作号"}))
            .expect("note update should apply");

        assert_eq!(accounts[0]["note"], "工作号");
        assert_eq!(accounts[0]["lastUsedAt"], 1_763_654_321_000_i64);
        assert_eq!(accounts[0]["lastUsedClient"], "workbuddy");
        // 下发口径与落库口径一致。
        let meta = account_meta(&accounts[0]);
        assert_eq!(meta["lastUsedAt"], 1_763_654_321_000_i64);
        assert_eq!(meta["lastUsedClient"], "workbuddy");
    }
