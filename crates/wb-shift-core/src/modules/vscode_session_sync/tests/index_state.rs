    // -----------------------------------------------------------------------
    // 工作区索引的三态判定（`index`）与预览的 fail-open 回归
    // -----------------------------------------------------------------------
    //
    // 三态是「会话还在不在」这条判定的地基：
    // - `Absent`：历史目录不存在（账号从未用过 / 数据根不对）；
    // - `Unavailable`：目录在，但一份 `index.json` 都没解析成功；
    // - `Available(ids)`：至少解析成功一份，此时「不在 ids 里」才可信。
    //
    // 读路径（预览）在前两态必须 fail-open，写路径（批量清理）必须拒绝——同一个判定，
    // 两种取舍。这里只覆盖判定与预览；清理侧见 `session_groups::tests::maintenance`。

    /// 某账号的工作区索引路径（造数用）。
    fn workspace_index(fixture: &Fixture, uid: &str, workspace: &str) -> PathBuf {
        vscode_session::history_root(&fixture.root, uid)
            .join(workspace)
            .join("index.json")
    }

    /// 某账号索引四态（测试只看四态，判定口径见 `liveness`）。
    fn index_state(fixture: &Fixture, uid: &str) -> ConversationIndexState {
        conversation_index_scan(VSCODE_STORE, &fixture.root, uid).state()
    }

    #[test]
    fn index_state_is_absent_without_a_history_directory() {
        let fixture = Fixture::new("index-absent");

        // 账号从未用过该客户端：连 `<uid>/VSCode/<uid>/history` 都不存在。
        assert_eq!(
            index_state(&fixture, "uid-never-used"),
            ConversationIndexState::Absent
        );
        // 定位表同样为空，且不 panic。
        assert!(conversation_index(VSCODE_STORE, &fixture.root, "uid-never-used").is_empty());
    }

    #[test]
    fn index_state_is_unavailable_when_every_index_file_is_broken() {
        let fixture = Fixture::new("index-broken");
        // 目录在、工作区子目录也在，但 index.json 不是合法 JSON。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            "{ this is not json",
        );

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Unavailable
        );
        assert!(conversation_index(VSCODE_STORE, &fixture.root, SRC_UID).is_empty());
    }

    #[test]
    fn index_state_is_unavailable_without_any_workspace_directory() {
        let fixture = Fixture::new("index-no-workspace");
        // 只建到 history 根：一个工作区子目录都没有 → 没有任何 index.json 可解析。
        std::fs::create_dir_all(vscode_session::history_root(&fixture.root, SRC_UID)).unwrap();

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Unavailable
        );
    }

    #[test]
    fn index_state_is_unavailable_when_the_conversations_field_is_not_an_array() {
        let fixture = Fixture::new("index-bad-field");
        // JSON 合法但结构不对：`conversations` 缺失 / 类型不对时这份索引同样用不上。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "current": CONV_SRC }).to_string(),
        );

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Unavailable
        );
    }

    #[test]
    fn index_state_is_available_with_an_empty_id_set() {
        let fixture = Fixture::new("index-empty");
        // 索引读得到、但客户端确实没有会话：这是「可用 + 空集」，不是「读不到」。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [], "current": null }).to_string(),
        );

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Available(BTreeSet::new())
        );
    }

    #[test]
    fn index_state_is_available_and_lists_the_live_ids() {
        let fixture = Fixture::new("index-available");
        seed_source(&fixture);

        let state = index_state(&fixture, SRC_UID);

        let ConversationIndexState::Available(ids) = state else {
            panic!("索引可读时必须是 Available：{state:?}");
        };
        assert_eq!(ids, BTreeSet::from([CONV_SRC.to_string()]));
        // 定位表与状态来自同一次遍历：ids 必须与定位表的键完全一致。
        let index = conversation_index(VSCODE_STORE, &fixture.root, SRC_UID);
        assert_eq!(ids, index.keys().cloned().collect::<BTreeSet<String>>());
        assert_eq!(index[CONV_SRC].title, "测试会话");
        assert_eq!(index[CONV_SRC].workspace_hash, WS);
    }

    #[test]
    fn index_state_is_partial_when_only_some_index_files_parsed() {
        let fixture = Fixture::new("index-partial");
        // 一份损坏 + 一份可读：`ids` 只是**可读分片**的并集，损坏那份里可能还有别的会话，
        // 所以是 `Partial` 而不是 `Available`——「不在 ids 里」不能当作「会话已消失」。
        write(&workspace_index(&fixture, SRC_UID, WS), "{ broken");
        write(
            &workspace_index(&fixture, SRC_UID, "ffffffffffffffffffffffffffffffff"),
            &json!({ "conversations": [{ "id": CONV_SRC, "name": "可读工作区" }] }).to_string(),
        );

        let state = index_state(&fixture, SRC_UID);

        let ConversationIndexState::Partial(ids) = state else {
            panic!("只有部分分片可读时必须是 Partial：{state:?}");
        };
        assert_eq!(ids, BTreeSet::from([CONV_SRC.to_string()]));
    }

    /// 工作区目录在、但**没有 `index.json`**：不参与四态分母，仍判 `Available`。
    ///
    /// 目录在却没有索引文件通常是客户端正常状态（空工作区 / 尚未写入）。旧实现把工作区
    /// 子目录数当分母，于是「有一个没索引的工作区」就永远 `parsed < 分母` → 永远 `Partial`
    /// → 该客户端的清理被永久拒绝。会话真的存在时由目录证据兜底（下一条用例），
    /// 所以把这个目录排除出分母是安全的。
    #[test]
    fn index_state_is_available_when_a_workspace_has_no_index_file() {
        let fixture = Fixture::new("index-available-empty-workspace");
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [{ "id": CONV_SRC, "name": "可读" }] }).to_string(),
        );
        // 另一个工作区目录里既没有 index.json 也没有会话目录（客户端还没写入）。
        std::fs::create_dir_all(
            vscode_session::history_root(&fixture.root, SRC_UID)
                .join("ffffffffffffffffffffffffffffffff"),
        )
        .unwrap();

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Available(BTreeSet::from([CONV_SRC.to_string()]))
        );
        // 分母只数「含 index.json 的工作区」：这一份被排除了。
        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);
        assert_eq!((scan.index_workspaces, scan.parsed), (1, 1));
    }

    /// 「有 `index.json` 但解析失败」仍然算 `Partial`：损坏**不得**当成可读。
    #[test]
    fn index_state_is_partial_when_an_index_file_cannot_be_parsed() {
        let fixture = Fixture::new("index-partial-unparsable");
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [{ "id": CONV_SRC, "name": "可读" }] }).to_string(),
        );
        write(
            &workspace_index(&fixture, SRC_UID, "ffffffffffffffffffffffffffffffff"),
            "{ broken",
        );

        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Partial(BTreeSet::from([CONV_SRC.to_string()]))
        );
        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);
        // 损坏的那份**计入**分母（它有 index.json），只是没解析成功。
        assert_eq!((scan.index_workspaces, scan.parsed), (2, 1));
    }

    /// 回归：`history` 目录存在、但 `index.json` 全损坏时，组**不得**被预览过滤掉。
    ///
    /// 旧判据是「历史目录在不在」（`history_root_in(..).is_dir()`），这种现场会被判成
    /// 「该账号没有任何会话」，把仍然存在的组整片过滤掉——用户会以为会话没了。
    /// 改动后三态给出 `Unavailable`，预览 fail-open（保留组）。
    #[test]
    fn preview_keeps_groups_when_the_workspace_index_is_unreadable() {
        let fixture = Fixture::new("preview-index-broken");
        seed_source(&fixture);
        copy_and_register(&fixture);
        let StoreState::Ready(store) = session_link::load_store(&fixture.paths()) else {
            panic!("关联存储应可读");
        };
        assert_eq!(store.groups.len(), 1);
        let group_id = store.groups[0].id.clone();
        // 造数：两侧工作区索引都可读时组会被列出（否则这条用例没有意义）。
        let before =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(before["groups"].as_array().unwrap().len(), 1, "{before}");

        // 现场：history 目录都在，两个账号的 index.json 全部损坏。
        for uid in [SRC_UID, DST_UID] {
            write(&workspace_index(&fixture, uid, WS), "{ broken");
            assert_eq!(
                index_state(&fixture, uid),
                ConversationIndexState::Unavailable
            );
        }

        let after =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        let groups = after["groups"].as_array().unwrap();
        assert_eq!(
            groups.len(),
            1,
            "索引不可判定时必须 fail-open（保留组），实际：{after}"
        );
        assert_eq!(groups[0]["groupId"], json!(group_id));
        // 标题退化也在预期内：索引读不到就没有标题，但组本身不能消失。
        assert_eq!(groups[0]["title"], json!("(无标题)"));
    }

    /// 部分分片损坏（`Partial`）时同样 fail-open：可读分片里没命中，不代表会话已消失。
    ///
    /// 反例现场（就是 QA 实测的那条）：目标账号的 WS_A 索引可读但为空、WS_B 的 `index.json`
    /// 损坏。若把 `Partial` 当成 `Available(空集)`，组会显示成「会话已失效」并被隐藏
    /// （清理侧更会直接删组），而损坏那份里可能正装着该会话。
    #[test]
    fn preview_keeps_groups_when_only_some_index_shards_are_readable() {
        let fixture = Fixture::new("preview-index-partial");
        seed_source(&fixture);
        copy_and_register(&fixture);
        // 目标账号：可读的那份是空的（副本「不在」），另一份损坏。
        write(
            &workspace_index(&fixture, DST_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        write(
            &workspace_index(&fixture, DST_UID, "ffffffffffffffffffffffffffffffff"),
            "{ broken",
        );
        // 源账号也补一个损坏分片（两侧都处于 `Partial`）。
        write(
            &workspace_index(&fixture, SRC_UID, "ffffffffffffffffffffffffffffffff"),
            "{ broken",
        );
        assert_eq!(
            index_state(&fixture, DST_UID),
            ConversationIndexState::Partial(BTreeSet::new())
        );

        let report =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        assert_eq!(
            report["groups"].as_array().unwrap().len(),
            1,
            "部分分片损坏时必须 fail-open（保留组），实际：{report}"
        );
    }

    /// 对照组：索引**可用**、且代表会话的索引条目与会话目录都不存在时，组仍要被过滤。
    ///
    /// 「可信消失」要求两条证据都缺：只删索引条目、目录还在的现场属于「目录证据」，
    /// 由下一条用例覆盖。
    #[test]
    fn preview_still_hides_groups_whose_sessions_are_really_gone() {
        let fixture = Fixture::new("preview-index-available");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        // 目标账号的工作区索引可读，但里面对话已被删除（空数组），正文目录也一起没了。
        write(
            &workspace_index(&fixture, DST_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        std::fs::remove_dir_all(fixture.dst_conv_dir(&target_conv)).unwrap();
        assert_eq!(
            index_state(&fixture, DST_UID),
            ConversationIndexState::Available(BTreeSet::new())
        );

        let report =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        assert_eq!(report["groups"].as_array().unwrap().len(), 0, "{report}");
    }
