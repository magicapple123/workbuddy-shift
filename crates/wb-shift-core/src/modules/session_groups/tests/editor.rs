    #[test]
    fn restart_only_when_running_client_may_be_using_write_target() {
        assert!(should_restart_running_target(
            true,
            Some("target"),
            "target"
        ));
        assert!(!should_restart_running_target(
            true,
            Some("other"),
            "target"
        ));
        assert!(should_restart_running_target(true, None, "target"));
        assert!(!should_restart_running_target(
            false,
            Some("target"),
            "target"
        ));
    }

    #[test]
    fn plan_editor_restart_only_closes_when_target_involves_current_login() {
        let targets = ["uid-a", "uid-b"];
        // 未运行：无论是否授权、是否涉及，都不关（也不主动拉起）。
        assert!(!plan_editor_restart(false, Some("uid-a"), &targets, true, "hint").unwrap());
        assert!(!plan_editor_restart(false, Some("uid-a"), &targets, false, "hint").unwrap());
        // 运行中但不涉及当前登录账号：不关、不报错（即使未授权）。
        assert!(!plan_editor_restart(true, Some("uid-other"), &targets, false, "hint").unwrap());
        assert!(!plan_editor_restart(true, Some("uid-other"), &targets, true, "hint").unwrap());
        // 空目标集合 + 读得到当前账号：同样不涉及。
        assert!(!plan_editor_restart(true, Some("uid-a"), &[], false, "hint").unwrap());
        // 运行中且涉及：未授权 → 原文案报错（逐字对齐既有入口文案）。
        assert_eq!(
            plan_editor_restart(
                true,
                Some("uid-a"),
                &targets,
                false,
                VSCODE_SYNC_RUNNING_HINT
            )
            .unwrap_err(),
            "检测到 VS Code 正在运行，请先完全退出后再同步会话。"
        );
        assert_eq!(
            plan_editor_restart(
                true,
                Some("uid-b"),
                &targets,
                false,
                VSCODE_ADD_MEMBER_RUNNING_HINT
            )
            .unwrap_err(),
            "检测到 VS Code 正在运行，请先完全退出后再添加关联账号。"
        );
        // 运行中且涉及 + 已授权：关闭。
        assert!(plan_editor_restart(true, Some("uid-a"), &targets, true, "hint").unwrap());
        // 读不到当前登录账号：保守按涉及处理（未授权 → 报错；已授权 → 关闭）。
        assert_eq!(
            plan_editor_restart(true, None, &targets, false, VSCODE_SYNC_RUNNING_HINT).unwrap_err(),
            "检测到 VS Code 正在运行，请先完全退出后再同步会话。"
        );
        assert!(plan_editor_restart(true, None, &targets, true, "hint").unwrap());
    }

    #[test]
    fn settle_editor_window_reports_restart_only_when_closed_here() {
        // 未关过（本来没运行或关闭前已退出）：不加字段、不调用重开。
        let report = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard { relaunch: None })),
            Ok(json!({ "errors": [] })),
        )
        .unwrap();
        assert!(report.get("restartedEditor").is_none());
        assert!(report["errors"].as_array().unwrap().is_empty());

        // 本次关过且重开成功：restartedEditor = true。
        let report = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard {
                relaunch: Some(Box::new(|| Ok(()))),
            })),
            Ok(json!({ "errors": [] })),
        )
        .unwrap();
        assert_eq!(report["restartedEditor"], json!(true));
    }

    #[test]
    fn settle_editor_window_degrades_restart_failure_without_failing_operation() {
        // 重开失败：操作仍返回 Ok，失败信息进 errors[]（不能只报成功）。
        let report = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard {
                relaunch: Some(Box::new(|| Err("open 超时".to_string()))),
            })),
            Ok(json!({ "errors": [] })),
        )
        .unwrap();
        assert!(report.get("restartedEditor").is_none());
        let errors = report["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        let message = errors[0]["error"].as_str().unwrap();
        assert!(message.contains("VS Code 重新打开失败"), "{message}");
        assert!(message.contains("open 超时"), "{message}");

        // 没有 errors 数组（add_member 形态）：失败信息挂顶层 editorError。
        let report = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard {
                relaunch: Some(Box::new(|| Err("open 超时".to_string()))),
            })),
            Ok(json!({ "status": "linked" })),
        )
        .unwrap();
        assert_eq!(report["status"], json!("linked"));
        assert!(report["editorError"]
            .as_str()
            .unwrap()
            .contains("VS Code 重新打开失败"));
    }

    #[test]
    fn settle_editor_window_relaunches_on_failure_and_keeps_original_error() {
        // 操作失败：必须 best-effort 重开，原错误保留。
        let error = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard {
                relaunch: Some(Box::new(|| Ok(()))),
            })),
            Err("同步失败".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "同步失败");

        // 重开也失败：两条信息都要给用户（他不知道编辑器还关着，也不知道为什么）。
        let error = settle_editor_window(
            Some(EditorGuard::Vscode(VscodeEditorGuard {
                relaunch: Some(Box::new(|| Err("open 超时".to_string()))),
            })),
            Err("同步失败".to_string()),
        )
        .unwrap_err();
        assert!(error.starts_with("同步失败"), "{error}");
        assert!(error.contains("open 超时"), "{error}");
    }

    #[test]
    fn settle_editor_window_is_noop_for_non_plugin_clients() {
        let report = settle_editor_window(None, Ok(json!({ "synced": [] }))).unwrap();
        assert!(report.get("restartedEditor").is_none());
        assert!(report.get("editorError").is_none());
        assert_eq!(
            settle_editor_window(None, Err("原错误".to_string())).unwrap_err(),
            "原错误"
        );
    }

    #[test]
    fn copy_linked_sessions_rejects_invalid_requests_before_touching_editor() {
        // WorkBuddy：走 `copy_sessions_cross`，本入口一律拒绝（不关任何宿主）。
        let error = copy_linked_sessions(
            SessionClient::Workbuddy,
            None,
            "source",
            "target",
            &["conv".to_string()],
            false,
        )
        .unwrap_err();
        assert!(error.contains("暂不支持新增关联会话"), "{error}");

        // CodeBuddy IDE 缺档位：明确要求 `cn` / `ai`，不能在无档位时猜。
        let error = copy_linked_sessions(
            SessionClient::CodebuddyIde,
            None,
            "source",
            "target",
            &["conv".to_string()],
            false,
        )
        .unwrap_err();
        assert!(error.contains("必须指定 cn 或 ai 档位"), "{error}");

        // 空勾选：直接拒绝。
        let error =
            copy_linked_sessions(SessionClient::VscodeExt, None, "source", "target", &[], false)
                .unwrap_err();
        assert!(error.contains("缺少要复制的会话"), "{error}");

        // 账号不存在：直接拒绝（不会走到编辑器生命周期窗口）。
        let error = copy_linked_sessions(
            SessionClient::VscodeExt,
            None,
            "no-such-source",
            "no-such-target",
            &["conv".to_string()],
            true,
        )
        .unwrap_err();
        assert!(error.contains("来源账号不存在"), "{error}");
    }

    #[test]
    fn explicit_group_unify_mode_reaches_sync_kernel() {
        let selection = parse_selection("group", "token", "unifyOverwrite").unwrap();
        assert_eq!(selection.mode, SyncMode::UnifyOverwrite);
        assert!(parse_selection("group", "token", "unsupported").is_err());
        // 会话组统一入口只提交正文模式：只同步归档由切号预览的 archiveAction 表达，
        // 不在统一入口的模式白名单里，也不影响它对 ahead 的 unifyOverwrite 改写。
        assert!(parse_selection("group", "token", "statusOnly").is_err());
    }

    fn member_view(
        member_id: &str,
        account_key: &str,
        state: MemberState,
        content: ContentState,
    ) -> MemberView {
        MemberView {
            member: LinkMember {
                member_id: member_id.to_string(),
                account_id: Some(account_key.to_string()),
                uid: format!("uid-{member_id}"),
                session_id: format!("session-{member_id}"),
                variant: Some(WbVariant::Cn),
                state,
                linked_at: 0,
                last_synced_at: None,
            },
            account_key: account_key.to_string(),
            account_name: account_key.to_string(),
            title: member_id.to_string(),
            project_label: String::new(),
            updated_at: 0,
            content,
            reason: String::new(),
            version_status: "unknown",
        }
    }

    fn ready_content(member_id: &str) -> ContentState {
        let text = format!(r#"{{"id":"{member_id}"}}"#);
        ContentState::Ready(ContentSnapshot {
            full_digest: full_digest_of(text.as_bytes()),
            normalized: normalize_jsonl(&text, &format!("session-{member_id}")).unwrap(),
            text,
        })
    }

    fn ready_lines(ids: &[&str]) -> ContentState {
        let text = ids
            .iter()
            .map(|id| format!(r#"{{"id":"{id}"}}"#))
            .collect::<Vec<_>>()
            .join("\n");
        ContentState::Ready(ContentSnapshot {
            full_digest: full_digest_of(text.as_bytes()),
            normalized: normalize_jsonl(&text, "unrelated-session-id").unwrap(),
            text,
        })
    }

    fn active_indexes(views: &[MemberView]) -> Vec<usize> {
        views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.member.state == MemberState::Active)
            .map(|(index, _)| index)
            .collect()
    }

    fn pair(
        matrix: &mut HashMap<(usize, usize), (SyncVerdict, String)>,
        source: usize,
        target: usize,
        verdict: SyncVerdict,
    ) {
        matrix.insert((source, target), (verdict, format!("{verdict:?}")));
    }

    #[test]
    fn client_namespaces_and_scope_are_explicit() {
        assert_eq!(
            SessionClient::parse("workbuddy")
                .unwrap()
                .store_paths()
                .link_namespace,
            crate::modules::session::LinkNamespace::WorkBuddy
        );
        assert!(SessionClient::parse("CodeBuddy").is_err());
        assert!(SessionClient::Workbuddy
            .validate_scope(Some(WbVariant::Ai))
            .is_err());
        assert!(SessionClient::CodebuddyIde.validate_scope(None).is_err());
        assert!(SessionClient::CodebuddyIde
            .validate_scope(Some(WbVariant::Ai))
            .is_ok());
    }

    #[test]
    fn plugin_accepts_both_regions_while_ide_requires_matching_region() {
        for variant in [WbVariant::Cn, WbVariant::Ai] {
            let target = json!({ "uid": "target", "variant": variant.as_str() });
            assert!(validate_target_compatibility(SessionClient::VscodeExt, None, &target).is_ok());
            for scope in [WbVariant::Cn, WbVariant::Ai] {
                assert_eq!(
                    validate_target_compatibility(
                        SessionClient::CodebuddyIde,
                        Some(scope),
                        &target
                    )
                    .is_ok(),
                    scope == variant
                );
            }
        }
    }

    #[test]
    fn workspace_label_never_exposes_or_guesses_a_path() {
        assert_eq!(basename("C:\\users\\private\\project"), "project");
        assert_eq!(basename("/users/private/project/"), "project");
    }

    /// TempDir + injected store root: link-store tests must never touch the real `~/.workbuddy-shift`.
    struct TempStore {
        dir: PathBuf,
        paths: SessionPaths,
    }

    impl TempStore {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wb_shift_groups_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let paths = SessionPaths {
                store_root: dir.join("store"),
                data_root: dir.join("data"),
                auth_file: dir.join("auth.info"),
                link_namespace: crate::modules::session::LinkNamespace::WorkBuddy,
            };
            std::fs::create_dir_all(&paths.store_root).unwrap();
            Self { dir, paths }
        }

        fn seed(&self, groups: Vec<LinkGroup>) {
            self.seed_at(&self.paths, groups);
        }

        /// 按命名空间把组写进指定关联主表（插件 / IDE 各自的文件名）。
        fn seed_at(&self, paths: &SessionPaths, groups: Vec<LinkGroup>) {
            let store = crate::modules::session_link::LinkStore {
                version: crate::modules::session_link::LINK_STORE_VERSION,
                revision: 1,
                groups,
            };
            std::fs::write(
                paths.session_links_file(),
                serde_json::to_string_pretty(&store).unwrap(),
            )
            .unwrap();
        }

        fn store(&self) -> crate::modules::session_link::LinkStore {
            self.store_at(&self.paths)
        }

        fn store_at(&self, paths: &SessionPaths) -> crate::modules::session_link::LinkStore {
            match session_link::load_store(paths) {
                StoreState::Ready(store) => store,
                other => panic!("存储应为可读状态：{other:?}"),
            }
        }

        /// VS Code 插件的关联存储路径（独立文件名，落在同一个临时工具存储根下）。
        fn ext_paths(&self) -> SessionPaths {
            SessionPaths::for_vscode_ext_at(self.paths.store_root.clone())
        }

        /// CodeBuddy IDE 的关联存储路径（两档共用一份表）。
        fn ide_paths(&self) -> SessionPaths {
            SessionPaths::for_codebuddy_ide_at(self.paths.store_root.clone())
        }

        /// 插件 / IDE 的会话数据根（临时目录，按真实布局 `…/<uid>/<client>/<uid>/history` 构造）。
        fn index_root(&self) -> PathBuf {
            self.dir.join("ext-data")
        }

        /// 建某账号的历史根（目录存在但里面什么都没有）。
        fn create_history_dir(&self, spec: SessionStoreSpec, uid: &str) {
            let history = vscode_session::history_root_in(spec, &self.index_root(), uid);
            std::fs::create_dir_all(history).unwrap();
        }

        /// 写一个工作区索引：`conversations` 为 `(会话 id, 标题)`。
        fn write_workspace_index(
            &self,
            spec: SessionStoreSpec,
            uid: &str,
            workspace: &str,
            conversations: &[(&str, &str)],
        ) {
            let ws_dir = vscode_session::history_root_in(spec, &self.index_root(), uid)
                .join(workspace);
            std::fs::create_dir_all(&ws_dir).unwrap();
            let items: Vec<serde_json::Value> = conversations
                .iter()
                .map(|(id, name)| json!({ "id": id, "type": "craft", "name": name }))
                .collect();
            std::fs::write(
                ws_dir.join("index.json"),
                json!({ "conversations": items }).to_string(),
            )
            .unwrap();
        }

        /// 写一个**损坏**的工作区索引（目录在、`index.json` 不是合法 JSON）。
        fn write_broken_workspace_index(
            &self,
            spec: SessionStoreSpec,
            uid: &str,
            workspace: &str,
        ) {
            let ws_dir = vscode_session::history_root_in(spec, &self.index_root(), uid)
                .join(workspace);
            std::fs::create_dir_all(&ws_dir).unwrap();
            std::fs::write(ws_dir.join("index.json"), "{ not json").unwrap();
        }

        /// 造一个**会话正文目录**：`history/<workspace>/<会话 id>/`（存活的最强证据）。
        ///
        /// 索引条目缺失但目录还在的现场就是靠它复现的：不要用「写一条 index 条目」代替，
        /// 那测的不是目录证据。
        fn create_session_dir(
            &self,
            spec: SessionStoreSpec,
            uid: &str,
            workspace: &str,
            conversation_id: &str,
        ) {
            let conv_dir = vscode_session::history_root_in(spec, &self.index_root(), uid)
                .join(workspace)
                .join(conversation_id);
            std::fs::create_dir_all(conv_dir).unwrap();
        }

        /// 造一个**只有目录、没有 `index.json`** 的工作区（客户端空工作区 / 尚未写入）。
        ///
        /// 这种目录不得参与索引四态的分母：否则「有一个空工作区」就会让该客户端永远
        /// `Partial`，清理被永久拒绝。
        fn create_workspace_dir(&self, spec: SessionStoreSpec, uid: &str, workspace: &str) {
            let ws_dir =
                vscode_session::history_root_in(spec, &self.index_root(), uid).join(workspace);
            std::fs::create_dir_all(ws_dir).unwrap();
        }

        /// 关联主表文件的 sha 与备份数量（`dry_run` 只读、写盘前留备份的判据）。
        fn links_file_digest(&self) -> String {
            self.links_digest_at(&self.paths)
        }

        fn links_digest_at(&self, paths: &SessionPaths) -> String {
            let bytes = std::fs::read(paths.session_links_file()).unwrap();
            format!("{}:{}", bytes.len(), simple_checksum(&bytes))
        }

        /// 第二档（国际版）数据根：跨档组的成员按自身档位读库，测试要能注入两份。
        fn ai_paths(&self) -> SessionPaths {
            SessionPaths {
                store_root: self.paths.store_root.clone(),
                data_root: self.dir.join("ai-data"),
                auth_file: self.dir.join("ai-auth.info"),
                link_namespace: self.paths.link_namespace,
            }
        }

        /// 建 `sessions` 表（结构与客户端实测库一致的最小列集）。
        fn create_sessions_table(&self, data_root: &Path) {
            std::fs::create_dir_all(data_root).unwrap();
            let conn = rusqlite::Connection::open(data_root.join("workbuddy.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    title TEXT,
                    custom_title TEXT,
                    cwd TEXT,
                    created_at INTEGER,
                    updated_at INTEGER,
                    deleted_at INTEGER,
                    is_playground INTEGER
                );",
            )
            .unwrap();
        }

        /// 插入一条会话行；`deleted_at = Some(ts)` 表示已被软删除。
        fn add_session_row(&self, data_root: &Path, id: &str, uid: &str, deleted_at: Option<i64>) {
            let conn = rusqlite::Connection::open(data_root.join("workbuddy.db")).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, deleted_at, is_playground)
                 VALUES (?1, ?2, ?3, NULL, '/ws/a', 1000, 2000, ?4, 0)",
                rusqlite::params![id, uid, format!("标题 {id}"), deleted_at],
            )
            .unwrap();
        }

        fn backup_files(&self) -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(&self.paths.store_root) else {
                return Vec::new();
            };
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.contains(".json.bak-"))
                .collect();
            names.sort();
            names
        }
    }

    /// 内容指纹（避免引入 hash 依赖：求和无碰撞顾虑，用例只比较「变没变」）。
    fn simple_checksum(bytes: &[u8]) -> u64 {
        bytes
            .iter()
            .enumerate()
            .fold(0_u64, |acc, (index, byte)| {
                acc.wrapping_mul(31).wrapping_add((*byte as u64) ^ index as u64)
            })
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn stored_member(member_id: &str, uid: &str, state: MemberState) -> LinkMember {
        LinkMember {
            member_id: member_id.to_string(),
            account_id: Some(format!("account-{uid}")),
            uid: uid.to_string(),
            session_id: format!("session-{uid}"),
            variant: Some(WbVariant::Cn),
            state,
            linked_at: 1,
            last_synced_at: None,
        }
    }

    fn remove_member_at(paths: &SessionPaths, group_id: &str, member_id: &str) -> Value {
        let (group_removed, remaining) = session_link::with_link_store_write(paths, |store| {
            super::remove_member_in(store, SessionClient::Workbuddy, None, group_id, member_id)
        })
        .unwrap();
        json!({
            "status": if group_removed { "groupRemoved" } else { "removed" },
            "remaining": remaining,
        })
    }

    fn delete_group_at(paths: &SessionPaths, group_id: &str) -> Result<usize, String> {
        session_link::with_link_store_write(paths, |store| {
            super::delete_group_in(store, SessionClient::Workbuddy, None, group_id)
        })
    }
