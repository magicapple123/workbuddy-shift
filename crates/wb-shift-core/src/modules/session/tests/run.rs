// ---------------------------------------------------------------------------
// 跨档复制（WorkBuddy 国内版 ↔ 国际版）
// ---------------------------------------------------------------------------

/// 双档测试环境：共享工具存储根（关联表/锁/备份）+ 两个档位数据根（CN 与 AI）。
struct CrossEnv {
    root: PathBuf,
    store_root: PathBuf,
    cn_data: PathBuf,
    ai_data: PathBuf,
}

impl CrossEnv {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "wb_shift_cross_test_{}_{name}",
            uuid::Uuid::new_v4().simple()
        ));
        let store_root = root.join("store");
        let cn_data = root.join("cn-data");
        let ai_data = root.join("ai-data");
        for data in [&cn_data, &ai_data] {
            std::fs::create_dir_all(data.join("projects").join("ws-a")).unwrap();
        }
        CrossEnv {
            root,
            store_root,
            cn_data,
            ai_data,
        }
    }

    fn paths(&self, variant: WbVariant) -> SessionPaths {
        SessionPaths {
            store_root: self.store_root.clone(),
            data_root: match variant {
                WbVariant::Cn => self.cn_data.clone(),
                WbVariant::Ai => self.ai_data.clone(),
            },
            // 跨档入口不读登录态（源 uid 由调用方显式传入）；门禁用它判断
            // 目标账号是否为该档当前登录账号。
            auth_file: self
                .store_root
                .join(format!("auth-{}.info", variant.as_str())),
            link_namespace: LinkNamespace::WorkBuddy,
        }
    }

    /// 写入指定档位的登录态（门禁读 `account.uid` 判定目标是否当前登录账号）。
    fn set_login(&self, variant: WbVariant, uid: &str) {
        let path = self.paths(variant).auth_file;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, json!({"account": {"uid": uid}}).to_string()).unwrap();
    }

    /// 建 sessions 表；`extra` 追加一列，用于模拟两档列集合差异。
    fn create_db(&self, variant: WbVariant, extra: &str) {
        let conn = Connection::open(self.paths(variant).workbuddy_db()).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT,
                custom_title TEXT,
                cwd TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                deleted_at INTEGER,
                is_playground INTEGER{extra}
            );"
        ))
        .unwrap();
    }

    fn create_edge_db(&self, variant: WbVariant) {
        let conn = Connection::open(self.paths(variant).edge_sync_db(variant)).unwrap();
        conn.execute_batch(
            "CREATE TABLE edge_sync_mapping (
                session_id TEXT,
                conversation_id TEXT,
                msg_channel TEXT,
                created_at INTEGER
            );",
        )
        .unwrap();
    }

    fn add_session(&self, variant: WbVariant, id: &str, uid: &str, title: &str) {
        let conn = Connection::open(self.paths(variant).workbuddy_db()).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, deleted_at, is_playground)
             VALUES (?1, ?2, ?3, NULL, '/ws/a', 1000, 2000, NULL, 0)",
            rusqlite::params![id, uid, title],
        )
        .unwrap();
    }

    fn add_body(&self, variant: WbVariant, cid: &str, text: &str) {
        std::fs::write(
            self.paths(variant)
                .projects_dir()
                .join("ws-a")
                .join(format!("{cid}.jsonl")),
            text,
        )
        .unwrap();
    }

    fn body_path(&self, variant: WbVariant, cid: &str) -> PathBuf {
        self.paths(variant)
            .projects_dir()
            .join("ws-a")
            .join(format!("{cid}.jsonl"))
    }

    fn rows_for(&self, variant: WbVariant, uid: &str) -> Vec<String> {
        let conn = Connection::open(self.paths(variant).workbuddy_db()).unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM sessions WHERE user_id = ?1 AND deleted_at IS NULL")
            .unwrap();
        let mut rows: Vec<String> = stmt
            .query_map([uid], |row| row.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect();
        rows.sort();
        rows
    }

    fn mapping_rows(&self, variant: WbVariant) -> Vec<(String, String)> {
        let conn = Connection::open(self.paths(variant).edge_sync_db(variant)).unwrap();
        let mut stmt = conn
            .prepare("SELECT session_id, msg_channel FROM edge_sync_mapping ORDER BY session_id")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .flatten()
            .collect()
    }

    fn delete_session(&self, variant: WbVariant, id: &str) {
        let conn = Connection::open(self.paths(variant).workbuddy_db()).unwrap();
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])
            .unwrap();
    }
}

impl Drop for CrossEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn cross_side<'a>(paths: &'a SessionPaths, variant: WbVariant, uid: &str) -> CopySide<'a> {
    CopySide {
        paths,
        variant,
        uid: uid.to_string(),
        account_id: Some(format!("acc-{uid}")),
    }
}

/// 可注入双档临时目录的复制调用（解析器与两条 side 一致，不触碰真实路径）。
fn cross_copy(
    env: &CrossEnv,
    source_variant: WbVariant,
    source_uid: &str,
    target_variant: WbVariant,
    target_uid: &str,
    ids: &[&str],
) -> Value {
    let source_paths = env.paths(source_variant);
    let target_paths = env.paths(target_variant);
    let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    copy_sessions_cross_at(
        cross_side(&source_paths, source_variant, source_uid),
        cross_side(&target_paths, target_variant, target_uid),
        &ids,
        |_| false,
        |variant| env.paths(variant),
    )
    .unwrap()
}

/// 与 `cross_copy` 相同，但注入「App 是否运行」探针（门禁测试用）。
fn cross_copy_with_probe(
    env: &CrossEnv,
    source_variant: WbVariant,
    source_uid: &str,
    target_variant: WbVariant,
    target_uid: &str,
    ids: &[&str],
    is_app_running: impl Fn(WbVariant) -> bool,
) -> Result<Value, String> {
    let source_paths = env.paths(source_variant);
    let target_paths = env.paths(target_variant);
    let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    copy_sessions_cross_at(
        cross_side(&source_paths, source_variant, source_uid),
        cross_side(&target_paths, target_variant, target_uid),
        &ids,
        is_app_running,
        |variant| env.paths(variant),
    )
}

/// 跨档门禁只拦「目标账号 = 目标档当前登录账号」：App 运行中
/// 目标≠当前登录 → 放行；目标=当前登录 → 拒绝；读不到登录态 → 保守拒绝。
#[test]
fn cross_copy_gate_only_blocks_current_login_target() {
    let env = CrossEnv::new("cross-gate");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let run = |uid: &str| {
        cross_copy_with_probe(
            &env,
            WbVariant::Cn,
            "uid-a",
            WbVariant::Ai,
            uid,
            &["sess-1"],
            |_| true,
        )
    };

    // 目标档登录的是别人（uid-x）：运行中放行，副本落库。
    env.set_login(WbVariant::Ai, "uid-x");
    let report = run("uid-b").expect("目标≠当前登录：运行中必须放行");
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();
    assert_eq!(env.rows_for(WbVariant::Ai, "uid-b"), vec![new_id]);

    // 目标账号正是目标档当前登录账号：运行中拒绝，且不再写入。
    env.set_login(WbVariant::Ai, "uid-b");
    let err = run("uid-b").unwrap_err();
    assert_eq!(err, SESSION_COPY_APP_RUNNING);
    assert_eq!(
        env.rows_for(WbVariant::Ai, "uid-b").len(),
        1,
        "未新增副本行"
    );

    // 读不到登录态（认证文件缺失）：保守拒绝。
    std::fs::remove_file(env.paths(WbVariant::Ai).auth_file).unwrap();
    let err = run("uid-b").unwrap_err();
    assert_eq!(err, SESSION_COPY_APP_RUNNING);

    // App 未运行：任何目标都放行（登录态缺失也不再拦截）；此时源会话已有
    // 有效副本，返回 alreadyLinked 而非再次复制。
    let report = cross_copy_with_probe(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
        |_| false,
    )
    .unwrap();
    assert!(
        !report["alreadyLinked"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(report.get("errors").is_none(), "{report}");
}

/// 基础跨档复制（CN → AI）：正文按源工作区同名目录落到目标档，行写目标档、
/// 映射行交给客户端，关联组两个成员各记自己的档位。
#[test]
fn cross_copy_lands_body_row_and_member_variants() {
    let env = CrossEnv::new("cn-to-ai");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let report = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    assert_eq!(report["sourceVariant"], "cn");
    assert_eq!(report["targetVariant"], "ai");
    assert_eq!(report["sourceUid"], "uid-a");
    assert_eq!(report["targetUid"], "uid-b");
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();
    assert_ne!(new_id, "sess-1");

    // 正文：目标档同名工作区目录，id 全量替换；源档不新增文件。
    let target_body = env.body_path(WbVariant::Ai, &new_id);
    assert!(target_body.is_file());
    let text = std::fs::read_to_string(&target_body).unwrap();
    assert!(text.contains(&new_id));
    assert!(!text.contains("sess-1"));
    assert!(!env.body_path(WbVariant::Cn, &new_id).exists());

    assert_eq!(env.rows_for(WbVariant::Ai, "uid-b"), vec![new_id.clone()]);
    assert_eq!(
        env.rows_for(WbVariant::Cn, "uid-a"),
        vec!["sess-1".to_string()]
    );

    // 不预写映射行：云端登记交接给客户端，两档映射表都不应出现工具写入。
    assert!(env.mapping_rows(WbVariant::Ai).is_empty());
    assert!(env.mapping_rows(WbVariant::Cn).is_empty());

    // 关联组：跨档组，两成员各记自己的档位。
    let store = match session_link::load_store(&env.paths(WbVariant::Ai)) {
        StoreState::Ready(store) => store,
        other => panic!("同步记录应为 Ready，实际 {other:?}"),
    };
    assert_eq!(store.groups.len(), 1);
    let group = &store.groups[0];
    let source_member = session_link::find_member(group, "uid-a", "sess-1").unwrap();
    assert_eq!(source_member.variant, Some(WbVariant::Cn));
    let target_member = session_link::find_member(group, "uid-b", &new_id).unwrap();
    assert_eq!(target_member.variant, Some(WbVariant::Ai));
}

/// 目标档列集合与源档不同（双向缺列）时按目标列取交集：不写 NULL、不报错。
#[test]
fn cross_copy_uses_target_columns_intersection() {
    let env = CrossEnv::new("columns");
    // 源多一列 transport；目标多一列 unread。
    env.create_db(WbVariant::Cn, ", transport TEXT");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, ", unread INTEGER");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let report = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();

    let conn = Connection::open(env.paths(WbVariant::Ai).workbuddy_db()).unwrap();
    let (title, cwd, unread): (String, String, Option<i64>) = conn
        .query_row(
            "SELECT title, cwd, unread FROM sessions WHERE id = ?1",
            [&new_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(title, "标题一");
    assert_eq!(cwd, "/ws/a");
    // 目标独有列由库默认值补齐（无默认值时 NULL），不因源列结构报错。
    assert_eq!(unread, None);
}

/// 反向（AI → CN）同路径可用：正文/行/映射落 CN，成员档位正确。
#[test]
fn cross_copy_ai_to_cn_uses_same_kernel() {
    let env = CrossEnv::new("ai-to-cn");
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.add_session(WbVariant::Ai, "sess-ai", "uid-ai", "AI 标题");
    env.add_body(WbVariant::Ai, "sess-ai", &body_text("sess-ai"));

    let report = cross_copy(
        &env,
        WbVariant::Ai,
        "uid-ai",
        WbVariant::Cn,
        "uid-cn",
        &["sess-ai"],
    );
    assert_eq!(report["sourceVariant"], "ai");
    assert_eq!(report["targetVariant"], "cn");
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();
    assert!(env.body_path(WbVariant::Cn, &new_id).is_file());
    assert_eq!(env.rows_for(WbVariant::Cn, "uid-cn"), vec![new_id.clone()]);
    assert!(
        env.mapping_rows(WbVariant::Cn).is_empty(),
        "复制不得预写映射行"
    );

    let store = match session_link::load_store(&env.paths(WbVariant::Cn)) {
        StoreState::Ready(store) => store,
        other => panic!("同步记录应为 Ready，实际 {other:?}"),
    };
    let group = &store.groups[0];
    assert_eq!(
        session_link::find_member(group, "uid-ai", "sess-ai")
            .unwrap()
            .variant,
        Some(WbVariant::Ai)
    );
    assert_eq!(
        session_link::find_member(group, "uid-cn", &new_id)
            .unwrap()
            .variant,
        Some(WbVariant::Cn)
    );
}

/// 同一跨档请求重复执行：第二次复用已有有效副本（alreadyLinked），不写第二个副本。
#[test]
fn cross_copy_reuses_existing_target_link() {
    let env = CrossEnv::new("already");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let first = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    let new_id = first["copied"][0]["newId"].as_str().unwrap().to_string();
    let second = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    assert!(second["copied"].as_array().unwrap().is_empty(), "{second}");
    assert_eq!(
        second["alreadyLinked"][0]["sessionId"],
        new_id.as_str(),
        "{second}"
    );
    assert_eq!(env.rows_for(WbVariant::Ai, "uid-b"), vec![new_id]);
}

/// 跨档操作中断后恢复：源正文与源行从源档（sourceVariant）读取，产物落目标档。
#[test]
fn cross_recovery_reads_source_from_operation_variant() {
    let env = CrossEnv::new("recovery");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let ai_paths = env.paths(WbVariant::Ai);
    // 与生产一致：操作产生于一次复制，关联存储主文件必然已存在（先建空表）。
    session_link::with_link_store_write(&ai_paths, |_| Ok(())).unwrap();
    // 手工留下一条 Prepared 阶段的跨档操作（CN → AI），模拟正文写入前中断。
    session_link::save_operation(
        &ai_paths,
        &Operation {
            version: crate::modules::session_link::OPERATION_VERSION,
            operation_id: "op-cross".to_string(),
            kind: "copy".to_string(),
            variant: WbVariant::Ai,
            source_variant: Some(WbVariant::Cn),
            group_id: "g-cross".to_string(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-1".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "new-cross".to_string(),
            },
            expected_content_digest: "将在恢复时按源内容刷新".to_string(),
            expected_record_count: 0,
            phase: crate::modules::session_link::OpPhase::Prepared,
            backup: None,
            lifecycle_version: None,
            cleanup_state: None,
            last_error: None,
            created_at: 1,
            updated_at: 1,
        },
    )
    .unwrap();

    let report = recover_pending_session_operations_at_with(&ai_paths, WbVariant::Ai, |variant| {
        env.paths(variant)
    });
    assert!(report.needs_recovery.is_empty(), "{report:?}");
    assert_eq!(report.recovered, vec!["op-cross".to_string()]);

    // 产物全部落到 AI（目标档）。
    assert!(env.body_path(WbVariant::Ai, "new-cross").is_file());
    assert_eq!(
        env.rows_for(WbVariant::Ai, "uid-b"),
        vec!["new-cross".to_string()]
    );
    assert!(
        env.mapping_rows(WbVariant::Ai).is_empty(),
        "恢复不得补写映射行"
    );
    let store = match session_link::load_store(&ai_paths) {
        StoreState::Ready(store) => store,
        other => panic!("同步记录应为 Ready，实际 {other:?}"),
    };
    let group = store
        .groups
        .iter()
        .find(|group| group.id == "g-cross")
        .expect("恢复后应提交关联组");
    assert_eq!(
        session_link::find_member(group, "uid-a", "sess-1")
            .unwrap()
            .variant,
        Some(WbVariant::Cn)
    );
}

/// 跨档预览：复制建组后判 identical；源追加 → fastForward；两边分别追加 → diverge；
/// 源行删除 → unknown。内容与标题按成员档位读取（源读 CN、目标读 AI）。
#[test]
fn cross_preview_verdicts_follow_member_content() {
    let env = CrossEnv::new("preview");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    let source_paths = env.paths(WbVariant::Cn);
    let target_paths = env.paths(WbVariant::Ai);
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"});
    let preview = || {
        session_links_preview_cross_at(
            &source_paths,
            WbVariant::Cn,
            &source_acc,
            &target_paths,
            WbVariant::Ai,
            &target_acc,
            |variant| env.paths(variant),
        )
        .unwrap()
    };

    // 复制建组：CN → AI（正文落在目标档同名工作区目录）。
    let report = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();

    // 复制完成即一致；报告带两侧档位，标题取自源档。
    let first = preview();
    assert_eq!(first["sourceVariant"], "cn");
    assert_eq!(first["targetVariant"], "ai");
    assert_eq!(first["storeStatus"], "ready");
    assert_eq!(first["groups"][0]["verdict"], "identical");
    assert_eq!(first["groups"][0]["title"], "标题一");
    assert_eq!(first["groups"][0]["source"]["sessionId"], "sess-1");
    assert_eq!(first["groups"][0]["target"]["sessionId"], new_id.as_str());

    // 源追加 3 条 → fastForward（extraA=3），给出可执行的 previewToken。
    append_records(&env.body_path(WbVariant::Cn, "sess-1"), "sess-1", 2, 3);
    let second = preview();
    let item = &second["groups"][0];
    assert_eq!(item["verdict"], "fastForward");
    assert_eq!(item["extraA"], 3);
    assert_eq!(item["extraB"], 0);
    assert_eq!(item["defaultChecked"], true);
    assert_eq!(item["recordCount"]["source"], 5);
    assert_eq!(item["recordCount"]["target"], 2);
    assert!(item["previewToken"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));

    // 目标追加自己独有的 2 条 → diverge（两边各有独有内容，只能显式覆盖）。
    append_records(&env.body_path(WbVariant::Ai, &new_id), &new_id, 100, 2);
    let third = preview();
    let item = &third["groups"][0];
    assert_eq!(item["verdict"], "diverge");
    assert_eq!(item["extraA"], 3);
    assert_eq!(item["extraB"], 2);
    assert_eq!(item["availableModes"][0], "overwrite");

    // 源行删除 → 成员失效，整组从预览剔除（不可能同步，判定必然 unknown）。
    env.delete_session(WbVariant::Cn, "sess-1");
    let fourth = preview();
    assert!(
        fourth["groups"].as_array().unwrap().is_empty(),
        "源行缺失的组不得再列出：{fourth}"
    );
}

/// 跨档预览（CN 源 → AI 目标）的便捷调用：解析器注入临时双档目录。
fn cross_preview_at(
    env: &CrossEnv,
    source_paths: &SessionPaths,
    target_paths: &SessionPaths,
    source_acc: &Value,
    target_acc: &Value,
) -> Value {
    session_links_preview_cross_at(
        source_paths,
        WbVariant::Cn,
        source_acc,
        target_paths,
        WbVariant::Ai,
        target_acc,
        |variant| env.paths(variant),
    )
    .unwrap()
}

/// 跨档预览按**成员自身档位**判定会话行存活：源（CN）或目标（AI）行被清理 → 整组不列出。
#[test]
fn cross_preview_omits_group_when_member_row_is_gone() {
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"});

    // 目标（AI）行缺失：读的是目标档的库，不能误用源档数据根。
    let (env, cn_paths, ai_paths, new_id) = cross_sync_env("preview-gone-target");
    let before = cross_preview_at(&env, &cn_paths, &ai_paths, &source_acc, &target_acc);
    assert_eq!(before["groups"].as_array().unwrap().len(), 1, "{before}");
    env.delete_session(WbVariant::Ai, &new_id);
    let after = cross_preview_at(&env, &cn_paths, &ai_paths, &source_acc, &target_acc);
    assert!(
        after["groups"].as_array().unwrap().is_empty(),
        "目标行缺失的组不得再列出：{after}"
    );

    // 源（CN）行缺失。
    let (env, cn_paths, ai_paths, _new_id) = cross_sync_env("preview-gone-source");
    env.delete_session(WbVariant::Cn, "sess-1");
    let after = cross_preview_at(&env, &cn_paths, &ai_paths, &source_acc, &target_acc);
    assert!(
        after["groups"].as_array().unwrap().is_empty(),
        "源行缺失的组不得再列出：{after}"
    );
}

/// 跨档预览入口也覆盖同档组（两个国内版账号）：判定路径一致，报告不带档位字段。
#[test]
fn cross_preview_supports_same_variant_groups() {
    let env = CrossEnv::new("preview-same");
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));

    cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Cn,
        "uid-b",
        &["sess-1"],
    );

    let paths = env.paths(WbVariant::Cn);
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "cn"});
    let report = session_links_preview_cross_at(
        &paths,
        WbVariant::Cn,
        &source_acc,
        &paths,
        WbVariant::Cn,
        &target_acc,
        |variant| env.paths(variant),
    )
    .unwrap();
    assert!(report.get("sourceVariant").is_none());
    assert_eq!(report["groups"][0]["verdict"], "identical");
}

/// 跨档同步的公共准备：CN 源 sess-1 → AI 目标复制建组，返回（环境、源/目标路径、newId）。
fn cross_sync_env(name: &str) -> (CrossEnv, SessionPaths, SessionPaths, String) {
    let env = CrossEnv::new(name);
    env.create_db(WbVariant::Cn, "");
    env.create_edge_db(WbVariant::Cn);
    env.create_db(WbVariant::Ai, "");
    env.create_edge_db(WbVariant::Ai);
    env.add_session(WbVariant::Cn, "sess-1", "uid-a", "标题一");
    env.add_body(WbVariant::Cn, "sess-1", &body_text("sess-1"));
    let report = cross_copy(
        &env,
        WbVariant::Cn,
        "uid-a",
        WbVariant::Ai,
        "uid-b",
        &["sess-1"],
    );
    let new_id = report["copied"][0]["newId"].as_str().unwrap().to_string();
    let cn_paths = env.paths(WbVariant::Cn);
    let ai_paths = env.paths(WbVariant::Ai);
    (env, cn_paths, ai_paths, new_id)
}

/// 跨档同步（fastForward）：源追加后把新增同步到目标，判定回到 identical。
#[test]
fn cross_sync_fast_forward_catches_target_up() {
    let (env, cn_paths, ai_paths, new_id) = cross_sync_env("sync-ff");
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"});

    // 源产生 3 条新增 → 预览给 fastForward + 可执行凭据。
    append_records(&env.body_path(WbVariant::Cn, "sess-1"), "sess-1", 2, 3);
    let before = session_links_preview_cross_at(
        &cn_paths,
        WbVariant::Cn,
        &source_acc,
        &ai_paths,
        WbVariant::Ai,
        &target_acc,
        |variant| env.paths(variant),
    )
    .unwrap();
    let item = &before["groups"][0];
    assert_eq!(item["verdict"], "fastForward");
    let selection = SyncSelection {
        group_id: item["groupId"].as_str().unwrap().to_string(),
        preview_token: item["previewToken"].as_str().unwrap().to_string(),
        mode: SyncMode::FastForward,
    };

    let report = sync_sessions_cross_at(
        &cross_side(&cn_paths, WbVariant::Cn, "uid-a"),
        &cross_side(&ai_paths, WbVariant::Ai, "uid-b"),
        &[selection],
        |_| false,
        |variant| env.paths(variant),
    )
    .unwrap();
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["synced"][0]["status"], "synced");
    assert_eq!(report["synced"][0]["mode"], "fastForward");

    // 目标正文追平源（保留自己的 sessionId）；再判定为 identical。
    let source_text = std::fs::read_to_string(env.body_path(WbVariant::Cn, "sess-1")).unwrap();
    let target_text = std::fs::read_to_string(env.body_path(WbVariant::Ai, &new_id)).unwrap();
    assert_eq!(target_text, source_text.replace("sess-1", &new_id));

    let after = session_links_preview_cross_at(
        &cn_paths,
        WbVariant::Cn,
        &source_acc,
        &ai_paths,
        WbVariant::Ai,
        &target_acc,
        |variant| env.paths(variant),
    )
    .unwrap();
    assert_eq!(after["groups"][0]["verdict"], "identical");
}

/// 跨档同步（overwrite）：冲突经显式覆盖后目标 = 源（目标独有内容被替换）。
#[test]
fn cross_sync_overwrite_resolves_diverge() {
    let (env, cn_paths, ai_paths, new_id) = cross_sync_env("sync-overwrite");
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"});

    append_records(&env.body_path(WbVariant::Cn, "sess-1"), "sess-1", 2, 3);
    append_records(&env.body_path(WbVariant::Ai, &new_id), &new_id, 100, 2);

    let before = session_links_preview_cross_at(
        &cn_paths,
        WbVariant::Cn,
        &source_acc,
        &ai_paths,
        WbVariant::Ai,
        &target_acc,
        |variant| env.paths(variant),
    )
    .unwrap();
    let item = &before["groups"][0];
    assert_eq!(item["verdict"], "diverge");
    assert_eq!(item["availableModes"][0], "overwrite");
    let selection = SyncSelection {
        group_id: item["groupId"].as_str().unwrap().to_string(),
        preview_token: item["previewToken"].as_str().unwrap().to_string(),
        mode: SyncMode::Overwrite,
    };

    let report = sync_sessions_cross_at(
        &cross_side(&cn_paths, WbVariant::Cn, "uid-a"),
        &cross_side(&ai_paths, WbVariant::Ai, "uid-b"),
        &[selection],
        |_| false,
        |variant| env.paths(variant),
    )
    .unwrap();
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["synced"][0]["mode"], "overwrite");

    let source_text = std::fs::read_to_string(env.body_path(WbVariant::Cn, "sess-1")).unwrap();
    let target_text = std::fs::read_to_string(env.body_path(WbVariant::Ai, &new_id)).unwrap();
    assert_eq!(target_text, source_text.replace("sess-1", &new_id));
    assert!(
        !target_text.contains("\"index\":100"),
        "目标独有内容应被覆盖"
    );
}

/// 跨档同步：预览后源又变化 → 跳过（previewStale），目标零写入。
#[test]
fn cross_sync_skips_stale_preview() {
    let (env, cn_paths, ai_paths, new_id) = cross_sync_env("sync-stale");
    let source_acc = json!({"id": "acc-uid-a", "uid": "uid-a", "variant": "cn"});
    let target_acc = json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"});

    append_records(&env.body_path(WbVariant::Cn, "sess-1"), "sess-1", 2, 1);
    let before = session_links_preview_cross_at(
        &cn_paths,
        WbVariant::Cn,
        &source_acc,
        &ai_paths,
        WbVariant::Ai,
        &target_acc,
        |variant| env.paths(variant),
    )
    .unwrap();
    let item = &before["groups"][0];
    assert_eq!(item["verdict"], "fastForward");
    let selection = SyncSelection {
        group_id: item["groupId"].as_str().unwrap().to_string(),
        preview_token: item["previewToken"].as_str().unwrap().to_string(),
        mode: SyncMode::FastForward,
    };

    // 预览之后源再变化：凭据失效，执行必须跳过并保持目标不变。
    let target_before = std::fs::read_to_string(env.body_path(WbVariant::Ai, &new_id)).unwrap();
    append_records(&env.body_path(WbVariant::Cn, "sess-1"), "sess-1", 3, 1);
    let report = sync_sessions_cross_at(
        &cross_side(&cn_paths, WbVariant::Cn, "uid-a"),
        &cross_side(&ai_paths, WbVariant::Ai, "uid-b"),
        &[selection],
        |_| false,
        |variant| env.paths(variant),
    )
    .unwrap();
    assert!(report["synced"].as_array().unwrap().is_empty());
    assert_eq!(report["skipped"][0]["reasonCode"], "previewStale");
    let target_after = std::fs::read_to_string(env.body_path(WbVariant::Ai, &new_id)).unwrap();
    assert_eq!(target_after, target_before);
}
