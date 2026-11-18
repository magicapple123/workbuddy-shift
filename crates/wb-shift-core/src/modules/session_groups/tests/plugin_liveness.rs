    // -----------------------------------------------------------------------
    // 插件 / IDE 的存活判定：会话目录证据 + 管理页按客户端过滤
    // -----------------------------------------------------------------------
    //
    // 两条本轮新增的语义，三处（预览 / 管理页 / 批量清理）共用同一份判定：
    //
    // 1. **目录证据优先**：`history/<workspaceHash>/<conversationId>/` 还在 → 会话确实存在，
    //    即使 `index.json` 里没有它的条目（损坏 / 半截写入 / 条目被改归属）。
    // 2. **管理页按客户端分派**：`Workbuddy` 用 `sessions` 表，插件 / IDE 用工作区索引 +
    //    目录证据，口径与清理完全一致（**隐藏的 = 可清理的**）。
    //
    // 旧实现下「索引条目缺失 + 目录还在」会被判成 Gone：管理页照样全展示（插件不过滤），
    // 预览会隐藏整组，清理会把组删掉——本轮把它修成 Alive。

    /// 索引条目缺失、但会话目录还在 → 清理**不**删这一组（旧实现必挂）。
    #[test]
    fn plugin_pruning_keeps_a_group_proven_alive_by_its_session_directory() {
        let store = TempStore::new("prune-ext-dir-evidence");
        let paths = store.ext_paths();
        // uid-live：索引可读但条目已丢（半截写入现场），正文目录还在 → 会话确实存在。
        store.write_workspace_index(VSCODE_STORE, "uid-live", WS_A, &[]);
        store.create_session_dir(VSCODE_STORE, "uid-live", WS_A, "session-uid-live");
        // uid-dead：索引可读、条目与目录都没有 → 可信消失，可以删。
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-dir",
                    WbVariant::Cn,
                    vec![stored_member("m-live", "uid-live", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(report["groupIds"], json!(["g-dead"]));
        let ids: Vec<String> = store
            .store_at(&paths)
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect();
        assert_eq!(ids, vec!["g-dir"], "目录还在的组不得被清理");
    }

    /// `Partial`（索引条目只可能在损坏分片里）但目录存在 → 正常处理，不再整次拒绝。
    ///
    /// 改造前这一现场是 `Err(无法判定…)` 且零写盘：一处损坏就永久挡住全部清理。
    #[test]
    fn plugin_pruning_processes_partial_shards_when_the_directory_proves_the_session() {
        let store = TempStore::new("prune-ext-partial-dir");
        let paths = store.ext_paths();
        // uid-1：WS_A 可读但为空、WS_B 损坏（条目可能在损坏分片里），会话目录在 WS_B 下。
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        store.create_session_dir(VSCODE_STORE, "uid-1", WS_B, "session-uid-1");
        // uid-2：两份都可读、都没有该会话 → 明确失效。
        store.write_workspace_index(VSCODE_STORE, "uid-2", WS_A, &[]);
        store.write_workspace_index(VSCODE_STORE, "uid-2", WS_B, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-partial-live",
                    WbVariant::Cn,
                    vec![stored_member("m-1", "uid-1", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-2", "uid-2", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();

        // 不再是「整次拒绝」：失效组照常删掉，目录证据救回的那组原样保留。
        assert_eq!(report["groupIds"], json!(["g-dead"]));
        let ids: Vec<String> = store
            .store_at(&paths)
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect();
        assert_eq!(ids, vec!["g-partial-live"]);
    }

    /// `Partial` 且目录也不存在 → Unknown：仍然拒绝整次清理、零写盘零备份。
    #[test]
    fn plugin_pruning_still_refuses_partial_shards_without_a_directory() {
        let store = TempStore::new("prune-ext-partial-no-dir");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        store.seed_at(
            &paths,
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );
        let before = store.links_digest_at(&paths);

        let error = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap_err();

        assert!(error.contains("只有部分工作区可读"), "{error}");
        assert_eq!(store.links_digest_at(&paths), before);
        assert!(store.backup_files().is_empty());
    }

    // -----------------------------------------------------------------------
    // 管理页：插件 / IDE 组也做存活过滤（隐藏的 = 可清理的）
    // -----------------------------------------------------------------------

    /// 插件组全失效 → 隐藏并计入 `dead`（`deadGroupCount` 从此对插件有效）。
    #[test]
    fn list_hides_plugin_groups_whose_sessions_are_all_gone() {
        let store = TempStore::new("view-ext-all-dead");
        let paths = store.ext_paths();
        // uid-live 的会话在索引里；uid-dead 的索引可读但空（条目与目录都没有）。
        store.write_workspace_index(VSCODE_STORE, "uid-live", WS_A, &[("session-uid-live", "在")]);
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-live",
                    WbVariant::Cn,
                    vec![stored_member("m-live", "uid-live", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let (shown, hidden) = visible_report(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            Some(&store.index_root()),
        );

        assert_eq!(shown, vec!["g-live"]);
        assert_eq!(hidden, vec!["g-dead"]);
    }

    /// 部分成员存活 → 展示；目录证据救回的组也展示（不能因为索引没条目就隐藏）。
    #[test]
    fn list_keeps_plugin_groups_with_a_live_member_or_a_session_directory() {
        let store = TempStore::new("view-ext-partial");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-live", WS_A, &[("session-uid-live", "在")]);
        // uid-dir：索引条目丢了但目录还在 → 靠目录证据存活。
        store.write_workspace_index(VSCODE_STORE, "uid-dir", WS_A, &[]);
        store.create_session_dir(VSCODE_STORE, "uid-dir", WS_A, "session-uid-dir");
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-partial",
                    WbVariant::Cn,
                    vec![
                        stored_member("m-live", "uid-live", MemberState::Active),
                        stored_member("m-dead", "uid-dead", MemberState::Active),
                    ],
                    Vec::new(),
                ),
                group(
                    "g-dir",
                    WbVariant::Cn,
                    vec![stored_member("m-dir", "uid-dir", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let (shown, hidden) = visible_report(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            Some(&store.index_root()),
        );

        assert_eq!(shown, vec!["g-partial", "g-dir"]);
        assert!(hidden.is_empty(), "没有全失效的组：{hidden:?}");
    }

    /// 索引不完整（`Partial`）且目录也没有 → 判不了 → 展示（读路径 fail-open）。
    #[test]
    fn list_fails_open_when_the_plugin_index_is_incomplete() {
        let store = TempStore::new("view-ext-fail-open");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        store.seed_at(
            &paths,
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );

        let (shown, hidden) = visible_report(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            Some(&store.index_root()),
        );

        assert_eq!(shown, vec!["g-1"]);
        assert!(hidden.is_empty());
    }

    /// 数据根解析不到 → 没有判定依据 → fail-open 全部展示（读路径不因缺依据而隐藏）。
    #[test]
    fn list_fails_open_when_the_plugin_data_root_cannot_be_resolved() {
        let store = TempStore::new("view-ext-no-root");
        let paths = store.ext_paths();
        // 索引侧确实「全都失效」（历史目录不存在也是判不了，这里用可读空索引把话说满）。
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );

        let (shown, hidden) = visible_report(&store, &paths, SessionClient::VscodeExt, None, None);

        assert_eq!(shown, vec!["g-1"]);
        assert!(hidden.is_empty());
    }

    /// WorkBuddy 组与插件组各按自己的索引判定，互不串用。
    #[test]
    fn list_judges_workbuddy_and_plugin_groups_independently() {
        let store = TempStore::new("view-mixed-clients");
        // WorkBuddy 侧：库里只有 uid-wb 的会话行。
        store.create_sessions_table(&store.paths.data_root);
        store.add_session_row(&store.paths.data_root, "session-uid-wb", "uid-wb", None);
        // 插件侧：索引里只有 uid-ext 的会话。
        store.write_workspace_index(VSCODE_STORE, "uid-ext", WS_A, &[("session-uid-ext", "插件会话")]);
        let group_for = |id: &str, uid: &str| {
            group(
                id,
                WbVariant::Cn,
                vec![stored_member(&format!("m-{uid}"), uid, MemberState::Active)],
                Vec::new(),
            )
        };
        store.seed_at(
            &store.paths,
            vec![group_for("g-ext", "uid-ext"), group_for("g-wb", "uid-wb")],
        );
        store.seed_at(
            &store.ext_paths(),
            vec![group_for("g-ext", "uid-ext"), group_for("g-wb", "uid-wb")],
        );

        // WorkBuddy 视角：库里没有 uid-ext 的行 → g-ext 失效；g-wb 存活。
        let (wb_shown, wb_hidden) = visible_report(
            &store,
            &store.paths,
            SessionClient::Workbuddy,
            None,
            Some(&store.index_root()),
        );
        assert_eq!(wb_shown, vec!["g-wb"]);
        assert_eq!(wb_hidden, vec!["g-ext"]);

        // 插件视角：索引里有 uid-ext → g-ext 存活；g-wb 在插件索引里「没有任何依据」
        // （Absent）→ 判不了 → fail-open 展示，绝不被 WorkBuddy 的库判成已失效。
        let (ext_shown, ext_hidden) = visible_report(
            &store,
            &store.ext_paths(),
            SessionClient::VscodeExt,
            None,
            Some(&store.index_root()),
        );
        assert_eq!(ext_shown, vec!["g-ext", "g-wb"]);
        assert!(ext_hidden.is_empty());
    }

    /// IDE 两档共用一份关联表：列表只判当前档位的组，另一档不受影响。
    #[test]
    fn list_respects_the_ide_variant_scope() {
        let store = TempStore::new("view-ide-scope");
        let paths = store.ide_paths();
        // 两档的会话都确实没了（索引可读、条目与目录都没有）。
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-cn", WS_A, &[]);
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-ai", WS_A, &[]);
        let mut ai_member = stored_member("m-ai", "uid-ai", MemberState::Active);
        ai_member.variant = Some(WbVariant::Ai);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-cn",
                    WbVariant::Cn,
                    vec![stored_member("m-cn", "uid-cn", MemberState::Active)],
                    Vec::new(),
                ),
                group("g-ai", WbVariant::Ai, vec![ai_member], Vec::new()),
            ],
        );

        let (shown, hidden) = visible_report(
            &store,
            &paths,
            SessionClient::CodebuddyIde,
            Some(WbVariant::Ai),
            Some(&store.index_root()),
        );

        // 只判 Ai：Cn 组不在这份列表的范围内（既不展示也不计入失效），Ai 组确实没了 → 隐藏。
        assert!(shown.is_empty(), "当前档位没有存活的组：{shown:?}");
        assert_eq!(hidden, vec!["g-ai"]);
    }

    /// 一致性：管理页隐藏的组 ≡ 清理能删的组（不允许「隐藏了但清理不删」或反过来）。
    #[test]
    fn list_hidden_groups_match_the_prune_targets() {
        let store = TempStore::new("view-prune-consistency");
        let paths = store.ext_paths();
        // 三种现场：索引里活着、靠目录证据活着、确实没了。
        store.write_workspace_index(VSCODE_STORE, "uid-index", WS_A, &[("session-uid-index", "在")]);
        store.write_workspace_index(VSCODE_STORE, "uid-dir", WS_A, &[]);
        store.create_session_dir(VSCODE_STORE, "uid-dir", WS_A, "session-uid-dir");
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-index",
                    WbVariant::Cn,
                    vec![stored_member("m-index", "uid-index", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dir",
                    WbVariant::Cn,
                    vec![stored_member("m-dir", "uid-dir", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let (shown, hidden) = visible_report(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            Some(&store.index_root()),
        );
        let dry = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            true,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(shown, vec!["g-index", "g-dir"]);
        assert_eq!(hidden, vec!["g-dead"]);
        // dry-run 的组 id 必须与隐藏集合完全相同（顺序都按存储顺序）。
        assert_eq!(dry["groupIds"], json!(hidden), "{dry}");
        assert_eq!(dry["removedGroups"], json!(hidden.len()), "{dry}");
    }

    // -----------------------------------------------------------------------
    // 空工作区目录不再让清理永久被拒（分母只数含 index.json 的工作区）
    // -----------------------------------------------------------------------

    /// 「有一个工作区目录没有 `index.json`」不得让整次清理被永久拒绝。
    ///
    /// 旧实现把工作区子目录数当分母：那种现场永远 `parsed < 分母` → 永远 `Partial` →
    /// 该客户端的清理永久 `Err`。目录在却没有索引文件通常是客户端正常状态（空工作区 /
    /// 尚未写入）；会话真的存在时目录证据会兜底，所以按 `Available` 处理是安全的。
    #[test]
    fn plugin_pruning_is_not_blocked_by_a_workspace_without_an_index_file() {
        let store = TempStore::new("prune-ext-empty-workspace");
        let paths = store.ext_paths();
        // uid-dead：WS_A 有可读索引（空），WS_B 只是目录（客户端还没写入索引）。
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.create_workspace_dir(VSCODE_STORE, "uid-dead", WS_B);
        // uid-dir：同上，但会话目录还在 → 靠目录证据存活，不该被删。
        store.write_workspace_index(VSCODE_STORE, "uid-dir", WS_A, &[]);
        store.create_workspace_dir(VSCODE_STORE, "uid-dir", WS_B);
        store.create_session_dir(VSCODE_STORE, "uid-dir", WS_A, "session-uid-dir");
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dir",
                    WbVariant::Cn,
                    vec![stored_member("m-dir", "uid-dir", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(report["groupIds"], json!(["g-dead"]));
        let ids: Vec<String> = store
            .store_at(&paths)
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect();
        assert_eq!(ids, vec!["g-dir"]);
    }

    // -----------------------------------------------------------------------
    // `pruneBlocked`：清理入口「可点必失败」→「禁用 + 说明」
    // -----------------------------------------------------------------------
    //
    // 管理页在 `Partial` / `Unavailable` 时仍会隐藏**已确认死亡**的组，但 `prune` 会整次
    // 拒绝（另一个组判不了）。旧实现让入口可点、点了必然失败；现在列表载荷带
    // `pruneBlocked`（原因与 `prune` 的 `Err` 逐字相同），前端据此禁用入口。

    /// 阻塞时：`pruneBlocked` 有值、`deadGroupCount` 仍然正确（已确认死亡的组照样被隐藏）。
    #[test]
    fn list_reports_prune_blocked_while_keeping_the_dead_count() {
        let store = TempStore::new("view-ext-blocked");
        let paths = store.ext_paths();
        // uid-1：WS_A 可读但空、WS_B 损坏 → 判不了（阻塞来源）。
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        // uid-dead：两份都可读、都没有该会话 → 已确认死亡。
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_B, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-unknown",
                    WbVariant::Cn,
                    vec![stored_member("m-1", "uid-1", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let (shown, hidden, blocked) =
            listing(&store, &paths, SessionClient::VscodeExt, None, Some(&store.index_root()));

        // 判不了的组照样展示（读路径 fail-open），已确认死亡的组照样隐藏。
        assert_eq!(shown, vec!["g-unknown"]);
        assert_eq!(hidden, vec!["g-dead"]);
        let reason = blocked.expect("有判不了的组时必须给出阻塞原因");
        assert!(reason.contains("VS Code 插件"), "{reason}");
        assert!(reason.contains("uid-1"), "{reason}");
        assert!(reason.contains("只有部分工作区可读"), "{reason}");
    }

    /// 不阻塞时：`pruneBlocked` 必须是 `null`（字段存在、值为 null），不是省略。
    #[test]
    fn list_reports_prune_blocked_as_null_when_every_group_is_decidable() {
        let store = TempStore::new("view-ext-not-blocked");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![group(
                "g-dead",
                WbVariant::Cn,
                vec![stored_member("m-dead", "uid-dead", MemberState::Active)],
                Vec::new(),
            )],
        );

        let (_shown, hidden, blocked) =
            listing(&store, &paths, SessionClient::VscodeExt, None, Some(&store.index_root()));

        assert_eq!(hidden, vec!["g-dead"]);
        assert_eq!(blocked, None);

        // 载荷契约：字段存在且为 `null`（`Option` 的 `None` 序列化成 null，不是省略键）。
        let payload = super::view::prune_status_payload(1, None);
        assert!(payload.get("pruneBlocked").is_some(), "{payload}");
        assert!(payload["pruneBlocked"].is_null(), "{payload}");
        assert_eq!(payload["deadGroupCount"], json!(1));
        // 有阻塞时是字符串（前端原样展示）。
        let blocked_payload = super::view::prune_status_payload(0, Some("无法判定会话"));
        assert_eq!(blocked_payload["pruneBlocked"], json!("无法判定会话"));
    }

    /// 阻塞判定与 `prune` 的实际拒绝**同一集合**：`blocked` 有值 ⇔ `prune` 返回 `Err`，
    /// 且两者文案逐字相同。
    #[test]
    fn list_prune_blocked_matches_the_actual_prune_rejection() {
        let store = TempStore::new("view-ext-blocked-parity");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        store.seed_at(
            &paths,
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );

        let (_shown, _hidden, blocked) =
            listing(&store, &paths, SessionClient::VscodeExt, None, Some(&store.index_root()));
        let error = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap_err();

        assert_eq!(
            blocked.as_deref(),
            Some(error.as_str()),
            "两处原因必须逐字相同"
        );
    }

    /// WorkBuddy 侧索引不可用时同样阻塞（`sessions` 表读不到就判不了，清理必被拒绝）。
    #[test]
    fn list_reports_prune_blocked_when_the_workbuddy_index_is_unavailable() {
        let store = TempStore::new("view-wb-blocked");
        // 没有 sessions 表：WorkBuddy 侧判不了。
        store.seed(vec![group(
            "g-1",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);

        let (shown, hidden, blocked) = listing(
            &store,
            &store.paths,
            SessionClient::Workbuddy,
            None,
            Some(&store.index_root()),
        );

        assert_eq!(shown, vec!["g-1"]);
        assert!(hidden.is_empty());
        assert!(
            blocked
                .as_deref()
                .unwrap_or_default()
                .contains("当前无法判定会话是否仍然存在"),
            "阻塞原因应说明无法判定：{blocked:?}"
        );
        // 与清理实际拒绝的原因一致。
        let error = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap_err();
        assert_eq!(blocked.as_deref(), Some(error.as_str()));
    }

    /// 插件数据根解析不到时同样阻塞（没有判定依据 → 清理必被拒绝），列表仍 fail-open。
    #[test]
    fn list_reports_prune_blocked_when_the_plugin_data_root_cannot_be_resolved() {
        let store = TempStore::new("view-ext-blocked-no-root");
        let paths = store.ext_paths();
        store.seed_at(
            &paths,
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );

        let (shown, hidden, blocked) = listing(&store, &paths, SessionClient::VscodeExt, None, None);

        assert_eq!(shown, vec!["g-1"]);
        assert!(hidden.is_empty());
        assert!(
            blocked
                .as_deref()
                .unwrap_or_default()
                .contains("未找到 VS Code CodeBuddy 插件的会话数据目录"),
            "{blocked:?}"
        );
    }

    // -----------------------------------------------------------------------
    // `list` 载荷装配（`list_at` / `list_at_with_index_root` / `list_payload`）
    // -----------------------------------------------------------------------
    //
    // 反例（QA 的精确变异）：把 `list()` 里的 `pruneBlocked` 装配改成恒 `null`，886 条单测
    // 一条都不挂——因为此前只测了 `prune_status_payload` 这个纯函数，**没有任何一条走
    // `list` 的装配路径**。现在装配只剩一处（`list_payload`），用例直接走它 + 走
    // `list_at` 的真实分支（注入临时关联存储路径，不碰真实客户端数据）。
    //
    // 第二个反例（本轮补上）：把 `Ready` 分支的 `visible.blocked.as_deref()` 改成 `None`
    // 也曾全绿——因为 `list_at` 只注入存储路径、数据根仍解析真实目录，用例没法造「`Ready`
    // 且有阻塞」的现场。现在多了 `list_at_with_index_root`，这条最后一跳也有用例兜住。

    /// 装配函数是唯一写入点：两个键都在，且 `pruneBlocked` 无阻塞时是 `null`、有阻塞时是原文。
    #[test]
    fn list_payload_always_writes_both_prune_fields() {
        let unblocked = super::view::list_payload(
            SessionClient::VscodeExt,
            None,
            "ready",
            None,
            3,
            None,
            Vec::new(),
        );
        assert_eq!(unblocked["storeStatus"], json!("ready"));
        assert_eq!(unblocked["deadGroupCount"], json!(3));
        assert!(
            unblocked.get("pruneBlocked").is_some(),
            "键必须存在（无阻塞时是 null）：{unblocked}"
        );
        assert!(unblocked["pruneBlocked"].is_null(), "{unblocked}");

        let blocked = super::view::list_payload(
            SessionClient::VscodeExt,
            None,
            "ready",
            None,
            1,
            Some("无法判定「VS Code 插件」账号 uid-1 的会话"),
            Vec::new(),
        );
        assert_eq!(blocked["deadGroupCount"], json!(1));
        assert_eq!(
            blocked["pruneBlocked"],
            json!("无法判定「VS Code 插件」账号 uid-1 的会话"),
            "阻塞原因必须原样写出（装配改成恒 null 时这条挂）：{blocked}"
        );
    }

    /// `Missing`（主表不存在 = 首次使用）：`prune` 返回 0 组、不拒绝，所以
    /// `pruneBlocked` 必须是 `null`。
    #[test]
    fn list_at_reports_no_prune_blocked_for_a_missing_store() {
        let store = TempStore::new("list-missing-store");
        let paths = store.ext_paths();
        assert!(!paths.session_links_file().exists());

        let payload = super::view::list_at(&paths, SessionClient::VscodeExt, None).unwrap();

        assert_eq!(payload["storeStatus"], json!("missing"));
        assert!(payload["pruneBlocked"].is_null(), "{payload}");
        assert_eq!(payload["deadGroupCount"], json!(0));
        // 对照：主表不存在时清理**不是**拒绝，返回 0 组。
        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();
        assert_eq!(report["removedGroups"], json!(0));
    }

    /// `Unavailable`（主表损坏读不出来）：`prune` 会 `Err`，所以 `pruneBlocked` 必须是
    /// 同一句文案——不变量「`pruneBlocked` 非空 ⇔ `prune` 会拒绝」无条件成立。
    #[test]
    fn list_at_reports_the_prune_reason_when_the_link_store_is_corrupt() {
        let store = TempStore::new("list-corrupt-store");
        let paths = store.ext_paths();
        // 主表损坏：内容不是合法 JSON。
        std::fs::write(paths.session_links_file(), "{ not json").unwrap();

        let payload = super::view::list_at(&paths, SessionClient::VscodeExt, None).unwrap();

        assert_eq!(payload["storeStatus"], json!("unavailable"));
        let blocked = payload["pruneBlocked"]
            .as_str()
            .unwrap_or_else(|| panic!("主表损坏时必须是阻塞：{payload}"));
        assert!(blocked.contains("同步记录已损坏"), "{blocked}");
        // 与清理的实际拒绝逐字相同。
        let error = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap_err();
        assert_eq!(blocked, error);
    }

    /// 空主表（`Ready` + 0 组）+ 可读的数据根：正常可清理，`pruneBlocked` 为 `null`。
    ///
    /// 数据根必须**显式注入临时目录**：`list_at` 会按客户端解析真实数据根，本机装没装扩展
    /// 决定了它是 `Some` 还是 `None`，而 `None` 本身就是一个阻塞原因——用例不能依赖这一点。
    #[test]
    fn list_at_reports_no_prune_blocked_for_an_empty_store() {
        let store = TempStore::new("list-empty-store");
        let paths = store.ext_paths();
        store.seed_at(&paths, Vec::new());
        let index_root = store.index_root();
        std::fs::create_dir_all(&index_root).unwrap();

        let payload =
            super::view::list_at_with_index_root(&paths, SessionClient::VscodeExt, None, Some(&index_root))
                .unwrap();

        assert_eq!(payload["storeStatus"], json!("ready"));
        assert!(payload["pruneBlocked"].is_null(), "{payload}");
        assert_eq!(payload["deadGroupCount"], json!(0));
        assert_eq!(payload["groups"], json!([]));
    }

    /// `Ready` + 数据根解析不到：`scan_groups` 会把「没有判定依据」记为阻塞，`prune` 随即
    /// 整次拒绝。列表的 `pruneBlocked` 必须**同一句**，否则前端的清理入口又会变回「可点但
    /// 必然失败」。
    ///
    /// 这条覆盖 `Ready` 分支从扫描结论到载荷的**最后一跳**（`visible.blocked` →
    /// `list_payload`）：把它改成 `None` 时本用例挂。主表里没有组，所以 0 张卡片、不读任何
    /// 客户端数据（AGENTS.md 第 8 节）。
    #[test]
    fn list_at_reports_the_prune_reason_when_the_client_has_no_data_root() {
        let store = TempStore::new("list-no-data-root");
        let paths = store.ext_paths();
        store.seed_at(&paths, Vec::new());

        // `index_root = None`：与真实环境「找不到插件数据目录」同一条分支。
        let payload = super::view::list_at_with_index_root(&paths, SessionClient::VscodeExt, None, None)
            .unwrap();

        assert_eq!(payload["storeStatus"], json!("ready"));
        assert_eq!(payload["deadGroupCount"], json!(0));
        assert_eq!(payload["groups"], json!([]));
        let blocked = payload["pruneBlocked"]
            .as_str()
            .unwrap_or_else(|| panic!("没有判定依据时必须阻塞：{payload}"));
        assert!(blocked.contains("未找到 VS Code CodeBuddy 插件的会话数据目录"), "{blocked}");
        // 与清理的实际拒绝逐字相同（同一次扫描口径）。
        let error = prune_at_paths(&store, &paths, SessionClient::VscodeExt, None, false, None)
            .unwrap_err();
        assert_eq!(blocked, error);
    }
