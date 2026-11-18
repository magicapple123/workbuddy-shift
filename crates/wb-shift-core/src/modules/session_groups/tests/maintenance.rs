    // -----------------------------------------------------------------------
    // 失效关联组的批量清理（`maintenance`）
    // -----------------------------------------------------------------------
    //
    // 判定口径：组内**所有**成员指向的会话行都已不存活才清理；任意一个成员还活着就不许删。

    /// 清理用的成员数据根解析：国内版读测试数据根，国际版读注入的第二档数据根。
    fn prune_resolve(store: &TempStore) -> impl Fn(WbVariant) -> SessionPaths + '_ {
        move |variant| match variant {
            WbVariant::Cn => store.paths.clone(),
            WbVariant::Ai => store.ai_paths(),
        }
    }

    /// WorkBuddy 侧清理（默认命名空间 + 注入的插件数据根）。
    fn prune_at(
        store: &TempStore,
        client: SessionClient,
        scope: Option<WbVariant>,
        dry_run: bool,
    ) -> Result<Value, String> {
        prune_at_paths(
            store,
            &store.paths,
            client,
            scope,
            dry_run,
            Some(store.index_root()),
        )
    }

    /// 指定关联存储命名空间（插件 / IDE 各有自己的主表文件名）。
    fn prune_at_paths(
        store: &TempStore,
        paths: &SessionPaths,
        client: SessionClient,
        scope: Option<WbVariant>,
        dry_run: bool,
        index_root: Option<PathBuf>,
    ) -> Result<Value, String> {
        let resolve = prune_resolve(store);
        super::maintenance::prune_dead_groups_at(
            paths,
            client,
            scope,
            dry_run,
            resolve,
            index_root.as_deref(),
        )
    }

    fn group(
        id: &str,
        variant: WbVariant,
        members: Vec<LinkMember>,
        pair_bases: Vec<crate::modules::session_link::PairBase>,
    ) -> LinkGroup {
        LinkGroup {
            id: id.to_string(),
            variant,
            created_at: 1,
            members,
            pair_bases,
        }
    }

    fn pair_base(left: &str, right: &str, reference: &str) -> crate::modules::session_link::PairBase {
        let (first, second) = session_link::pair_key(left, right);
        crate::modules::session_link::PairBase {
            member_ids: [first, second],
            baseline_ref: reference.to_string(),
            normalization_version: 1,
        }
    }

    /// 指定 sessionId 的成员（`stored_member` 的 sessionId 固定为 `session-<uid>`，
    /// 而同一账号在两个组里必须是不同的会话，否则违反 `validate_store` 的归组唯一性）。
    fn member_with_session(
        member_id: &str,
        uid: &str,
        session_id: &str,
        state: MemberState,
    ) -> LinkMember {
        let mut member = stored_member(member_id, uid, state);
        member.session_id = session_id.to_string();
        member
    }

    #[test]
    fn pruning_removes_only_groups_whose_members_are_all_gone() {
        let store = TempStore::new("prune-all-gone");
        store.create_sessions_table(&store.paths.data_root);
        // uid-a 的会话行仍然存活；uid-dead1 / uid-dead2 的行不存在（会话已删除）。
        store.add_session_row(&store.paths.data_root, "session-uid-a", "uid-a", None);
        store.add_session_row(&store.paths.data_root, "session-uid-a2", "uid-a", None);
        store.seed(vec![
            group(
                "g-dead",
                WbVariant::Cn,
                vec![
                    stored_member("m-dead1", "uid-dead1", MemberState::Active),
                    stored_member("m-dead2", "uid-dead2", MemberState::Active),
                ],
                vec![pair_base("m-dead1", "m-dead2", "base-dead")],
            ),
            group(
                "g-live",
                WbVariant::Cn,
                vec![stored_member("m-a", "uid-a", MemberState::Active)],
                Vec::new(),
            ),
            group(
                "g-partial",
                WbVariant::Cn,
                vec![
                    member_with_session("m-a2", "uid-a", "session-uid-a2", MemberState::Active),
                    stored_member("m-dead3", "uid-dead3", MemberState::Active),
                ],
                vec![pair_base("m-a2", "m-dead3", "base-partial")],
            ),
        ]);

        let report = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        assert_eq!(report["dryRun"], false);
        assert_eq!(report["removedGroups"], 1);
        // 只清掉「所有成员都失效」的那一组。
        assert_eq!(report["removedMembers"], 2);
        assert_eq!(report["groupIds"], json!(["g-dead"]));
        let saved = store.store();
        let ids: Vec<&str> = saved.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, vec!["g-live", "g-partial"]);
        assert_eq!(saved.revision, 2);
        // 删组后存储不变量仍然成立（配对基线不会悬空引用已删除的成员）。
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn pruning_counts_soft_deleted_and_reassigned_rows_as_gone() {
        let store = TempStore::new("prune-soft-deleted");
        store.create_sessions_table(&store.paths.data_root);
        // 软删除（deleted_at 非空）与归属变更都算失效。
        store.add_session_row(&store.paths.data_root, "session-uid-soft", "uid-soft", Some(1_700_000_000_000));
        store.add_session_row(&store.paths.data_root, "session-uid-moved", "uid-other", None);
        store.seed(vec![
            group(
                "g-soft",
                WbVariant::Cn,
                vec![stored_member("m-soft", "uid-soft", MemberState::Active)],
                Vec::new(),
            ),
            group(
                "g-moved",
                WbVariant::Cn,
                vec![stored_member("m-moved", "uid-moved", MemberState::Active)],
                Vec::new(),
            ),
        ]);

        let report = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        assert_eq!(report["removedGroups"], 2);
        assert_eq!(report["removedMembers"], 2);
        assert!(store.store().groups.is_empty());
    }

    #[test]
    fn pruning_judges_each_member_in_its_own_variant_root() {
        let store = TempStore::new("prune-cross-variant");
        // 会话行只在国际版数据根里：按成员档位判定 → 组活着；若误读国内根会被整组删掉。
        store.create_sessions_table(&store.ai_paths().data_root);
        store.add_session_row(&store.ai_paths().data_root, "session-uid-ai", "uid-ai", None);
        let mut member = stored_member("m-ai", "uid-ai", MemberState::Active);
        member.variant = Some(WbVariant::Ai);
        store.seed(vec![group("g-ai", WbVariant::Ai, vec![member], Vec::new())]);

        let report = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        assert_eq!(report["removedGroups"], 0);
        assert_eq!(store.store().groups.len(), 1);
    }

    #[test]
    fn pruning_ignores_groups_outside_the_variant_scope() {
        let store = TempStore::new("prune-scope");
        // 国内版成员的索引可读但会话已不在（→ 该组确为失效）；国际版账号完全没有数据根
        // （→ 索引 Absent，判定不了）。scope 只覆盖国内版：若 scope 过滤失效，这次清理会被
        // 国际版账号的 Absent 拦下并报错，而不是正常删掉一组。
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-cn", "ws-a", &[]);
        let mut ai_member = stored_member("m-ai", "uid-ai", MemberState::Active);
        ai_member.variant = Some(WbVariant::Ai);
        store.seed_at(
            &store.ide_paths(),
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
        let resolve = prune_resolve(&store);
        let saved = store.store_at(&store.ide_paths());

        let dead = super::maintenance::dead_groups(
            &saved,
            SessionClient::CodebuddyIde,
            Some(WbVariant::Cn),
            &resolve,
            Some(&store.index_root()),
        )
        .unwrap();

        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].id, "g-cn");
    }

    #[test]
    fn dry_run_reports_counts_without_touching_the_store() {
        let store = TempStore::new("prune-dry-run");
        store.create_sessions_table(&store.paths.data_root);
        store.seed(vec![group(
            "g-dead",
            WbVariant::Cn,
            vec![
                stored_member("m-1", "uid-1", MemberState::Active),
                stored_member("m-2", "uid-2", MemberState::Superseded),
            ],
            Vec::new(),
        )]);
        let before = store.links_file_digest();

        let report = prune_at(&store, SessionClient::Workbuddy, None, true).unwrap();

        assert_eq!(report["dryRun"], true);
        assert_eq!(report["removedGroups"], 1);
        assert_eq!(report["removedMembers"], 2);
        assert_eq!(report["groupIds"], json!(["g-dead"]));
        // 只读：主表内容、revision 与备份目录都不变。
        assert_eq!(store.links_file_digest(), before);
        assert_eq!(store.store().revision, 1);
        assert!(store.backup_files().is_empty());
    }

    #[test]
    fn pruning_aborts_without_writing_when_the_index_is_unavailable() {
        let store = TempStore::new("prune-unavailable");
        // 没有 sessions 表：存活不可判定 → 破坏性操作必须中止，不能按「全部失效」清空。
        store.seed(vec![group(
            "g-1",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);
        let before = store.links_file_digest();

        let error = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap_err();

        assert!(
            error.contains("当前无法判定会话是否仍然存在"),
            "错误文案应说明无法判定：{error}"
        );
        assert_eq!(store.links_file_digest(), before);
        assert_eq!(store.store().revision, 1);
        assert!(store.backup_files().is_empty());
    }

    #[test]
    fn pruning_writes_a_timestamped_backup_before_saving() {
        let store = TempStore::new("prune-backup");
        store.create_sessions_table(&store.paths.data_root);
        store.seed(vec![group(
            "g-dead",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);
        let before = store.links_file_digest();

        prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        let backups = store.backup_files();
        assert_eq!(backups.len(), 1, "应生成一份时间戳备份：{backups:?}");
        assert!(
            backups[0].starts_with("session_links.json.bak-"),
            "备份命名应为 session_links.json.bak-<YYYYMMDD-HHMMSS>：{}",
            backups[0]
        );
        // 备份是**清理前**的原文（否则等于没备份）。
        let backup_bytes = std::fs::read(store.paths.store_root.join(&backups[0])).unwrap();
        assert_eq!(
            format!("{}:{}", backup_bytes.len(), simple_checksum(&backup_bytes)),
            before
        );
        assert!(store.store().groups.is_empty());
    }

    /// 主表字节的 sha256（`None` = 文件不存在）。
    fn links_file_sha256(path: &Path) -> Option<String> {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(path).ok()?;
        Some(format!("{:x}", Sha256::digest(&bytes)))
    }

    /// 没有失效组时清理必须**真的不碰主表**：不写盘、不备份、revision 不变、字节不变。
    ///
    /// 反例（改造前）：`with_link_store_write` 无条件落盘 + `revision + 1`，于是一次空转的
    /// 清理会静默重写主表并给出新的 revision / mtime——用户什么都没点掉，却看到存储被动过。
    #[test]
    fn pruning_no_op_returns_without_touching_the_store() {
        let store = TempStore::new("prune-nothing");
        store.create_sessions_table(&store.paths.data_root);
        store.add_session_row(&store.paths.data_root, "session-uid-1", "uid-1", None);
        store.seed(vec![group(
            "g-live",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);
        let path = store.paths.session_links_file();
        let bytes_before = std::fs::read(&path).unwrap();
        let sha_before = links_file_sha256(&path).unwrap();
        let revision_before = store.store().revision;
        let mtime_before = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();

        let report = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        assert_eq!(report["dryRun"], false);
        assert_eq!(report["removedGroups"], 0);
        assert_eq!(report["removedMembers"], 0);
        assert_eq!(report["groupIds"], json!([]));
        // 不需要改动就不该留下备份（有备份=文件被动过）。
        assert!(store.backup_files().is_empty());
        assert_eq!(store.store().groups.len(), 1);
        // 主表**字节级**未变：没有重写、没有 revision 递增。
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
        assert_eq!(store.store().revision, revision_before);
        let sha_after = links_file_sha256(&path).unwrap();
        assert_eq!(sha_after, sha_before, "无失效组时主表字节不该变");
        let mtime_after = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
        assert_eq!(mtime_after, mtime_before, "无失效组时主表连 mtime 都不该变");
    }

    #[test]
    fn pruning_a_fresh_store_does_not_create_the_links_file() {
        let store = TempStore::new("prune-missing");
        // 没有 seed：关联主表不存在（首次使用）。
        assert!(!store.paths.session_links_file().exists());

        let report = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();

        assert_eq!(report["removedGroups"], 0);
        // 清理不是「初始化」：不能顺带把主表建出来。
        assert!(!store.paths.session_links_file().exists());
    }

    #[test]
    fn pruning_is_rejected_only_when_the_client_data_root_cannot_be_resolved() {
        // 「暂不支持」收窄为「数据根解析不到」：没有数据根就没有任何判定依据。
        let store = TempStore::new("prune-no-root");
        store.seed_at(
            &store.ext_paths(),
            vec![group(
                "g-1",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );
        let before = store.links_digest_at(&store.ext_paths());

        let error = prune_at_paths(
            &store,
            &store.ext_paths(),
            SessionClient::VscodeExt,
            None,
            false,
            None,
        )
        .unwrap_err();

        assert!(
            error.contains("未找到 VS Code CodeBuddy 插件的会话数据目录"),
            "错误应指出数据目录解析不到：{error}"
        );
        assert_eq!(store.links_digest_at(&store.ext_paths()), before);
        assert!(store.backup_files().is_empty());
    }

    // -----------------------------------------------------------------------
    // 插件 / IDE 侧的批量清理（索引三态 + variant 过滤）
    // -----------------------------------------------------------------------

    /// 工作区目录名（不透明字符串，测试里固定两个即可）。
    const WS_A: &str = "0123456789abcdef0123456789abcdef";
    const WS_B: &str = "ffffffffffffffffffffffffffffffff";

    /// 全失效的插件组被删；有成员存活的组保留；备份按插件命名空间命名。
    #[test]
    fn plugin_pruning_removes_only_groups_whose_sessions_are_gone() {
        let store = TempStore::new("prune-ext-mixed");
        let paths = store.ext_paths();
        // uid-live 的索引里有它的两个会话；uid-dead1 的索引可读但会话已不在；
        // uid-dead2 的索引是空数组（客户端确实没有会话，仍然可判定）。
        store.write_workspace_index(
            VSCODE_STORE,
            "uid-live",
            WS_A,
            &[
                ("session-uid-live", "存活会话"),
                ("session-uid-live2", "另一条存活会话"),
            ],
        );
        store.write_workspace_index(VSCODE_STORE, "uid-dead1", WS_A, &[("other-conv", "别的会话")]);
        store.write_workspace_index(VSCODE_STORE, "uid-dead2", WS_A, &[]);
        store.write_workspace_index(VSCODE_STORE, "uid-dead3", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-dead",
                    WbVariant::Cn,
                    vec![
                        stored_member("m-dead1", "uid-dead1", MemberState::Active),
                        stored_member("m-dead2", "uid-dead2", MemberState::Active),
                    ],
                    vec![pair_base("m-dead1", "m-dead2", "base-dead")],
                ),
                group(
                    "g-live",
                    WbVariant::Cn,
                    vec![stored_member("m-live", "uid-live", MemberState::Active)],
                    Vec::new(),
                ),
                // 部分存活：只要有一个账号的会话还在就不许删。
                group(
                    "g-partial",
                    WbVariant::Cn,
                    vec![
                        member_with_session(
                            "m-live2",
                            "uid-live",
                            "session-uid-live2",
                            MemberState::Active,
                        ),
                        stored_member("m-dead3", "uid-dead3", MemberState::Active),
                    ],
                    vec![pair_base("m-live2", "m-dead3", "base-partial")],
                ),
            ],
        );
        let before = store.links_digest_at(&paths);

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(report["dryRun"], false);
        assert_eq!(report["groupIds"], json!(["g-dead"]));
        assert_eq!(report["removedMembers"], 2);
        let saved = store.store_at(&paths);
        let ids: Vec<&str> = saved.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, vec!["g-live", "g-partial"]);
        assert_eq!(saved.revision, 2);
        assert!(session_link::validate_store(&saved).is_ok());
        // 备份是清理前的插件主表原文，命名随命名空间走。
        let backups = store.backup_files();
        assert_eq!(backups.len(), 1, "应生成一份时间戳备份：{backups:?}");
        assert!(
            backups[0].starts_with("vscode_session_links.json.bak-"),
            "插件侧备份命名应为 vscode_session_links.json.bak-<YYYYMMDD-HHMMSS>：{}",
            backups[0]
        );
        let backup_bytes = std::fs::read(store.paths.store_root.join(&backups[0])).unwrap();
        assert_eq!(
            format!("{}:{}", backup_bytes.len(), simple_checksum(&backup_bytes)),
            before
        );
    }

    /// 索引不可判定（目录在但 index.json 全损坏）→ 拒绝整次清理，主表与备份都不动。
    #[test]
    fn plugin_pruning_is_rejected_when_the_index_is_unreadable() {
        let store = TempStore::new("prune-ext-unavailable");
        let paths = store.ext_paths();
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_A);
        store.seed_at(
            &paths,
            vec![group(
                "g-dead",
                WbVariant::Cn,
                vec![
                    stored_member("m-1", "uid-1", MemberState::Active),
                    // 第二个成员看起来「确实没有会话」，也不能单独成立：
                    // 一次清理只要有一处判不了，就整次拒绝（不允许部分删除）。
                    stored_member("m-2", "uid-2", MemberState::Active),
                ],
                Vec::new(),
            )],
        );
        store.write_workspace_index(VSCODE_STORE, "uid-2", WS_A, &[]);
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

        assert!(
            error.contains("无法判定「VS Code 插件」账号 uid-1 的会话"),
            "错误应指出是哪个账号、判不了：{error}"
        );
        assert!(error.contains("索引无法解析"), "{error}");
        assert_eq!(store.links_digest_at(&paths), before);
        assert_eq!(store.store_at(&paths).revision, 1);
        assert!(store.backup_files().is_empty());
    }

    /// 历史目录不存在（`Absent`）同样没有判定依据：拒绝，而不是「当全死」。
    #[test]
    fn plugin_pruning_is_rejected_when_the_history_directory_is_absent() {
        let store = TempStore::new("prune-ext-absent");
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

        assert!(error.contains("找不到该账号的会话历史目录"), "{error}");
        assert_eq!(store.links_digest_at(&paths), before);
        assert!(store.backup_files().is_empty());
    }

    /// 已确定存活的组不因另一个账号索引损坏而连带拒绝（否则一处损坏会永远挡住清理）。
    #[test]
    fn plugin_pruning_still_works_when_an_alive_group_has_an_unreadable_member() {
        let store = TempStore::new("prune-ext-alive-plus-broken");
        let paths = store.ext_paths();
        // g-alive：uid-live 的会话在索引里 → 组活着，无需再判定 uid-broken。
        store.write_workspace_index(VSCODE_STORE, "uid-live", WS_A, &[("session-uid-live", "在")]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-broken", WS_A);
        // g-dead：uid-dead 的索引可读、会话已不在 → 可以删。
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-alive",
                    WbVariant::Cn,
                    vec![
                        stored_member("m-live", "uid-live", MemberState::Active),
                        stored_member("m-broken", "uid-broken", MemberState::Active),
                    ],
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
        assert_eq!(ids, vec!["g-alive"]);
    }

    /// `Partial`（部分分片损坏）且代表会话在**可读分片**里命中 → 明确存活，组保留。
    ///
    /// 这条是「一处损坏不永久挡住清理」的落点：命中可读分片已足以证明会话还在，
    /// 不需要为损坏的那份拒绝整次清理。
    #[test]
    fn plugin_pruning_counts_a_hit_in_a_readable_shard_as_alive() {
        let store = TempStore::new("prune-ext-partial-hit");
        let paths = store.ext_paths();
        // uid-live：WS_A 可读且装着会话，WS_B 损坏 → Partial，但会话确实在。
        store.write_workspace_index(VSCODE_STORE, "uid-live", WS_A, &[("session-uid-live", "在")]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-live", WS_B);
        // uid-dead：两份都可读、都没有该会话 → 明确失效，可以删。
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_A, &[]);
        store.write_workspace_index(VSCODE_STORE, "uid-dead", WS_B, &[]);
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
        assert_eq!(ids, vec!["g-live"]);
    }

    /// `Partial` 且代表会话只在损坏分片里可能 → 判不了，**拒绝整次清理**（零写盘、零备份）。
    ///
    /// 反例现场：uid-1 的 WS_A 索引可读但为空、WS_B 的 `index.json` 损坏。若把 `Partial`
    /// 当成「可用 + 空集」，整组会被误删，而损坏那份里可能正装着该会话。
    #[test]
    fn plugin_pruning_is_rejected_when_the_session_may_live_in_a_broken_shard() {
        let store = TempStore::new("prune-ext-partial-miss");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.write_broken_workspace_index(VSCODE_STORE, "uid-1", WS_B);
        // 同一台机器上的另一个账号索引完好（用来证明拒绝不是「全都不行」导致的一刀切）。
        store.write_workspace_index(VSCODE_STORE, "uid-2", WS_A, &[]);
        store.write_workspace_index(VSCODE_STORE, "uid-2", WS_B, &[]);
        store.seed_at(
            &paths,
            vec![
                group(
                    "g-1",
                    WbVariant::Cn,
                    vec![stored_member("m-1", "uid-1", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-2",
                    WbVariant::Cn,
                    vec![stored_member("m-2", "uid-2", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );
        let before = store.links_digest_at(&paths);
        let revision_before = store.store_at(&paths).revision;

        let error = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap_err();

        assert!(
            error.contains("无法判定「VS Code 插件」账号 uid-1 的会话"),
            "{error}"
        );
        assert!(error.contains("只有部分工作区可读"), "{error}");
        // 拒绝 = 一个字都不写：主表内容、revision、备份目录全部不变。
        assert_eq!(store.links_digest_at(&paths), before);
        assert_eq!(store.store_at(&paths).revision, revision_before);
        assert!(store.backup_files().is_empty());
    }

    /// IDE 的档位隔离在 `Partial` 下同样成立：另一档索引分片损坏不牵连本次档位。
    #[test]
    fn ide_pruning_partial_shards_respect_the_variant_scope() {
        let store = TempStore::new("prune-ide-partial-scope");
        let paths = store.ide_paths();
        // 国内版：WS_A 可读但为空、WS_B 损坏 → Partial(空集)，且没命中 → 判定不了。
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-cn", WS_A, &[]);
        store.write_broken_workspace_index(CODEBUDDY_IDE_STORE, "uid-cn", WS_B);
        // 国际版：两份都可读、都没有会话 → 明确失效。
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-ai", WS_A, &[]);
        store.write_workspace_index(CODEBUDDY_IDE_STORE, "uid-ai", WS_B, &[]);
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

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::CodebuddyIde,
            Some(WbVariant::Ai),
            false,
            Some(store.index_root()),
        )
        .unwrap();

        // 只删国际版那一组：国内版的 Partial 不在本次判定范围内，不该拦住清理。
        assert_eq!(report["groupIds"], json!(["g-ai"]));
        let ids: Vec<String> = store
            .store_at(&paths)
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect();
        assert_eq!(ids, vec!["g-cn"]);
    }

    /// dry-run 在插件侧同样只统计：零写盘、不生成备份。
    #[test]
    fn plugin_dry_run_reports_counts_without_writing() {
        let store = TempStore::new("prune-ext-dry-run");
        let paths = store.ext_paths();
        store.write_workspace_index(VSCODE_STORE, "uid-1", WS_A, &[]);
        store.seed_at(
            &paths,
            vec![group(
                "g-dead",
                WbVariant::Cn,
                vec![stored_member("m-1", "uid-1", MemberState::Active)],
                Vec::new(),
            )],
        );
        let before = store.links_digest_at(&paths);

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::VscodeExt,
            None,
            true,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(report["dryRun"], true);
        assert_eq!(report["removedGroups"], 1);
        assert_eq!(report["groupIds"], json!(["g-dead"]));
        assert_eq!(store.links_digest_at(&paths), before);
        assert_eq!(store.store_at(&paths).revision, 1);
        assert!(store.backup_files().is_empty());
    }

    /// IDE 两档共用一份关联表：清理只碰当前档位的组，另一档原样保留。
    #[test]
    fn ide_pruning_only_touches_the_scoped_variant() {
        let store = TempStore::new("prune-ide-scope-clean");
        let paths = store.ide_paths();
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

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::CodebuddyIde,
            Some(WbVariant::Cn),
            false,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(report["groupIds"], json!(["g-cn"]));
        let saved = store.store_at(&paths);
        let ids: Vec<&str> = saved.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, vec!["g-ai"]);
        let backups = store.backup_files();
        assert_eq!(backups.len(), 1);
        assert!(
            backups[0].starts_with("codebuddy_ide_session_links.json.bak-"),
            "IDE 侧备份命名应为 codebuddy_ide_session_links.json.bak-<Ymd-HMS>：{}",
            backups[0]
        );
    }

    /// IDE 侧索引不可判定 → 同样拒绝（两档共用表，更不允许按猜测删）。
    #[test]
    fn ide_pruning_is_rejected_when_the_index_is_unreadable() {
        let store = TempStore::new("prune-ide-unavailable");
        let paths = store.ide_paths();
        // 历史目录在、但没有工作区子目录（也解析不出任何 index.json）。
        store.create_history_dir(CODEBUDDY_IDE_STORE, "uid-1");
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
            SessionClient::CodebuddyIde,
            Some(WbVariant::Cn),
            false,
            Some(store.index_root()),
        )
        .unwrap_err();

        assert!(
            error.contains("无法判定「CodeBuddy IDE」账号 uid-1 的会话"),
            "{error}"
        );
        assert_eq!(store.links_digest_at(&paths), before);
        assert!(store.backup_files().is_empty());
    }

    /// 同一 scope 下 WorkBuddy 组与插件组各按自己的索引判定，互不串用。
    ///
    /// `sessions` 表里没有插件账号的行，工作区索引里也没有 WorkBuddy 的会话：任一方向串用
    /// 都会把对方的组判成「全部失效」并误删。
    #[test]
    fn pruning_never_mixes_workbuddy_and_plugin_judgements() {
        let store = TempStore::new("prune-mixed-clients");
        // WorkBuddy 侧：只有 uid-wb 的会话行。
        store.create_sessions_table(&store.paths.data_root);
        store.add_session_row(&store.paths.data_root, "session-uid-wb", "uid-wb", None);
        // 插件侧：只有 uid-ext 的会话，且它确实在索引里。
        store.write_workspace_index(
            VSCODE_STORE,
            "uid-ext",
            WS_A,
            &[("session-uid-ext", "插件会话")],
        );

        // 同一个组各写一份：WorkBuddy 视角看它是死的（库里没有 uid-ext 的行），
        // 插件视角看它是活的（索引里有 session-uid-ext）。
        store.seed_at(
            &store.paths,
            vec![group(
                "g-ext",
                WbVariant::Cn,
                vec![stored_member("m-ext", "uid-ext", MemberState::Active)],
                Vec::new(),
            )],
        );
        store.seed_at(
            &store.ext_paths(),
            vec![group(
                "g-ext",
                WbVariant::Cn,
                vec![stored_member("m-ext", "uid-ext", MemberState::Active)],
                Vec::new(),
            )],
        );

        let wb = prune_at(&store, SessionClient::Workbuddy, None, false).unwrap();
        assert_eq!(wb["groupIds"], json!(["g-ext"]));

        let ext = prune_at_paths(
            &store,
            &store.ext_paths(),
            SessionClient::VscodeExt,
            None,
            false,
            Some(store.index_root()),
        )
        .unwrap();

        assert_eq!(ext["removedGroups"], 0);
        assert_eq!(ext["groupIds"], json!([]));
        // 插件主表没有被删任何组，也没有生成插件侧的备份（备份 = 真的动过表）。
        assert_eq!(store.store_at(&store.ext_paths()).groups.len(), 1);
        let plugin_backups: Vec<String> = store
            .backup_files()
            .into_iter()
            .filter(|name| name.starts_with("vscode_session_links.json.bak-"))
            .collect();
        assert!(
            plugin_backups.is_empty(),
            "判定为「没有失效组」时不该动插件主表：{plugin_backups:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 管理页列表过滤（`view::visible_groups_at`）
    // -----------------------------------------------------------------------

    /// 管理页判定的完整结果：`(展示的组 id, 被隐藏的失效组 id, 清理阻塞原因)`。
    ///
    /// 一律走可注入数据根的入口（[`super::view::visible_groups_at`]）并把数据根指向临时目录：
    /// 不得让测试读到本机真实的客户端数据（AGENTS.md 第 8 节）。
    fn listing(
        store: &TempStore,
        paths: &SessionPaths,
        client: SessionClient,
        scope: Option<WbVariant>,
        index_root: Option<&Path>,
    ) -> (Vec<String>, Vec<String>, Option<String>) {
        let resolve = prune_resolve(store);
        let member_paths = |group: &LinkGroup, member: &LinkMember| {
            resolve(session_link::member_variant(group, member))
        };
        let saved = store.store_at(paths);
        let visible =
            super::view::visible_groups_at(&saved, client, scope, &member_paths, index_root);
        let blocked = visible.blocked.clone();
        let ids = |groups: Vec<&LinkGroup>| {
            groups
                .into_iter()
                .map(|group| group.id.clone())
                .collect::<Vec<String>>()
        };
        let shown = ids(visible.groups);
        // `dead` 只是计数：为了能比较「隐藏的 = 可清理的」，这里按同一口径反推出隐藏项。
        let hidden: Vec<String> = saved
            .groups
            .iter()
            .filter(|group| super::view::group_is_in_scope(client, scope, group))
            .map(|group| group.id.clone())
            .filter(|id| !shown.contains(id))
            .collect();
        assert_eq!(hidden.len(), visible.dead, "隐藏计数与反推集合必须一致");
        (shown, hidden, blocked)
    }

    /// 管理页判定结果里的「展示 / 隐藏」两项（阻塞原因见 [`listing`]）。
    fn visible_report(
        store: &TempStore,
        paths: &SessionPaths,
        client: SessionClient,
        scope: Option<WbVariant>,
        index_root: Option<&Path>,
    ) -> (Vec<String>, Vec<String>) {
        let (shown, hidden, _blocked) = listing(store, paths, client, scope, index_root);
        (shown, hidden)
    }

    fn visible(store: &TempStore, client: SessionClient) -> Vec<String> {
        visible_in(store, client, None)
    }

    /// 默认走临时数据根（`store.index_root()`），绝不读真实客户端目录。
    fn visible_in(
        store: &TempStore,
        client: SessionClient,
        scope: Option<WbVariant>,
    ) -> Vec<String> {
        visible_report(
            store,
            &store.paths,
            client,
            scope,
            Some(&store.index_root()),
        )
        .0
    }

    /// 隐藏的失效组数量：列表为空时前端仍要能触达清理入口。
    fn hidden(store: &TempStore, client: SessionClient) -> usize {
        visible_report(
            store,
            &store.paths,
            client,
            None,
            Some(&store.index_root()),
        )
        .1
        .len()
    }

    #[test]
    fn list_hides_groups_without_any_live_member() {
        let store = TempStore::new("view-all-dead");
        store.create_sessions_table(&store.paths.data_root);
        store.add_session_row(&store.paths.data_root, "session-uid-live", "uid-live", None);
        store.add_session_row(&store.paths.data_root, "session-uid-live2", "uid-live", None);
        store.seed(vec![
            group(
                "g-dead",
                WbVariant::Cn,
                vec![
                    stored_member("m-1", "uid-1", MemberState::Active),
                    stored_member("m-2", "uid-2", MemberState::Stale),
                ],
                Vec::new(),
            ),
            group(
                "g-live",
                WbVariant::Cn,
                vec![stored_member("m-live", "uid-live", MemberState::Active)],
                Vec::new(),
            ),
            // 部分存活：仍要列出（卡片按既有「内容缺失」文案提示），不能整组隐藏。
            group(
                "g-partial",
                WbVariant::Cn,
                vec![
                    member_with_session("m-live2", "uid-live", "session-uid-live2", MemberState::Active),
                    stored_member("m-gone", "uid-gone", MemberState::Active),
                ],
                Vec::new(),
            ),
        ]);

        assert_eq!(visible(&store, SessionClient::Workbuddy), vec!["g-live", "g-partial"]);
        // 全失效组仍要能被数出来：否则列表为空时清理入口无从触达。
        assert_eq!(hidden(&store, SessionClient::Workbuddy), 1);
    }

    #[test]
    fn list_fails_open_when_the_session_index_is_unavailable() {
        let store = TempStore::new("view-fail-open");
        // 没有 sessions 表：WorkBuddy 侧也无法判定存活 → 不过滤，保持改造前列表。
        store.seed(vec![group(
            "g-1",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);

        assert_eq!(visible(&store, SessionClient::Workbuddy), vec!["g-1"]);
        assert_eq!(hidden(&store, SessionClient::Workbuddy), 0);
    }

    /// 插件 / IDE 的会话行不在 `sessions` 表里：绝不能拿 WorkBuddy 的库去判它们的组。
    ///
    /// 同一台机器装了 WorkBuddy 时库一定读得到，若照搬 SQL 索引，插件组成员 uid 全被判成
    /// 「不存在」，整片列表会被误隐藏。
    #[test]
    fn list_never_hides_plugin_groups_by_the_workbuddy_index() {
        let store = TempStore::new("view-plugin-not-judged");
        // 建一张「有行但都没有成员 uid」的库：如果误用该索引，组会被判成全部失效。
        store.create_sessions_table(&store.paths.data_root);
        store.add_session_row(&store.paths.data_root, "session-someone-else", "uid-other", None);
        store.seed(vec![group(
            "g-plugin",
            WbVariant::Cn,
            vec![stored_member("m-1", "uid-1", MemberState::Active)],
            Vec::new(),
        )]);

        assert_eq!(visible(&store, SessionClient::VscodeExt), vec!["g-plugin"]);
        assert_eq!(hidden(&store, SessionClient::VscodeExt), 0);
        // IDE 必须带档位范围（既有契约）。
        assert_eq!(
            visible_in(&store, SessionClient::CodebuddyIde, Some(WbVariant::Cn)),
            vec!["g-plugin"]
        );
        // 同一个组在 WorkBuddy 视角下确实已失效（口径按客户端分开）。
        assert!(visible(&store, SessionClient::Workbuddy).is_empty());
        assert_eq!(hidden(&store, SessionClient::Workbuddy), 1);
    }

    #[test]
    fn list_judges_cross_variant_members_in_their_own_root() {
        let store = TempStore::new("view-cross-variant");
        store.create_sessions_table(&store.ai_paths().data_root);
        store.add_session_row(&store.ai_paths().data_root, "session-uid-ai", "uid-ai", None);
        let mut member = stored_member("m-ai", "uid-ai", MemberState::Active);
        member.variant = Some(WbVariant::Ai);
        store.seed(vec![group("g-ai", WbVariant::Ai, vec![member], Vec::new())]);

        // 行只在国际版数据根：按成员档位读 → 组可见（误读国内根会把它隐藏掉）。
        assert_eq!(visible(&store, SessionClient::Workbuddy), vec!["g-ai"]);
    }
