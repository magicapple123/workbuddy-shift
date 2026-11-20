// ---------------------------------------------------------------------------
// 幂等复制与关联组（R1）
// ---------------------------------------------------------------------------

#[test]
fn copy_writes_body_row_and_link_group() {
    let env = ready_env("happy");
    let report = copy(&env, "uid-b", &["sess-1"]);

    assert_eq!(report["sourceUid"], "uid-a");
    assert_eq!(report["targetUid"], "uid-b");
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert!(report.get("errors").is_none());
    assert!(report.get("needsRecovery").is_none());

    let new_id = env.first_copy_id(&report);
    assert_ne!(new_id, "sess-1");
    assert_eq!(report["copied"][0]["id"], "sess-1");

    // 正文：新 id 文件存在、旧 id 引用已替换，源文件不动。
    let copied_body = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert!(copied_body.contains(&new_id));
    assert!(!copied_body.contains("sess-1"));
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        body_text("sess-1")
    );

    // 数据库行归属目标账号。
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    assert_eq!(env.rows_for("uid-a"), vec!["sess-1".to_string()]);

    // 不预写云端映射：登记交接给客户端（edge-sync 扩展迁移时自行写入）。
    assert_eq!(env.mapping_rows(), 0, "复制不得预写映射行");

    // 关联组：同一逻辑会话、两个账号各一个 active 成员、一对基线。
    let store = env.store();
    assert!(
        store.revision >= 1,
        "首次落地空存储 + 关联提交都会推进 revision"
    );
    assert_eq!(store.groups.len(), 1);
    let group = &store.groups[0];
    assert_eq!(group.variant, WbVariant::Cn);
    assert_eq!(group.members.len(), 2);
    assert!(group
        .members
        .iter()
        .all(|member| member.state == MemberState::Active));
    assert_eq!(group.pair_bases.len(), 1);
    assert!(env
        .paths
        .baselines_dir()
        .join(format!("{}.json", group.pair_bases[0].baseline_ref))
        .exists());
    // 来源账号的成员带上了 accountId（账号库缺失时为 None，不影响身份判定）。
    assert!(group
        .members
        .iter()
        .any(|member| member.uid == "uid-a" && member.session_id == "sess-1"));

    // 成功清理：临时备份已回收，报告不展示可还原路径，也没有维护记录残留。
    assert!(report["copied"][0]["backup"].is_null(), "{report}");
    assert_eq!(report["copied"][0]["cleanupState"], "cleaned", "{report}");
    assert_eq!(report["temporaryFiles"], json!([]), "{report}");
    let operation_id = session_link::scan_operations(&env.paths)
        .operations
        .first()
        .expect("操作日志必须保留")
        .operation_id
        .clone();
    assert!(
        !session_backup::transaction_dir(&env.paths, WbVariant::Cn, &operation_id)
            .unwrap()
            .exists(),
        "成功路径必须回收本次操作专属目录"
    );
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
}

/// 批量复制逐项清理：进入下一项之前，上一成功项的临时目录已经消失（不累积备份）。
#[test]
fn batch_copy_cleans_each_backup_before_next_item() {
    let env = ready_env("batch-cleanup");
    env.add_session("sess-2", "uid-a", "标题二");
    env.add_body("sess-2", &body_text("sess-2"));

    let report = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 2, "{report}");
    for item in report["copied"].as_array().unwrap() {
        assert!(item["backup"].is_null(), "{report}");
        assert_eq!(item["cleanupState"], "cleaned", "{report}");
    }
    // 两个成功项都不留目录与维护记录：连续操作不累积成功备份。
    let transactions = env
        .paths
        .backup_root()
        .join(session_backup::TRANSACTIONS_DIR_NAME)
        .join(WbVariant::Cn.as_str());
    let leftovers = std::fs::read_dir(&transactions)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "成功批次不得累积操作专属目录");
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
    assert_eq!(
        env.body_files().len(),
        4,
        "两条来源内容与两个复制后的内容都在"
    );
}

/// 归属不可验证（临时目录路径被替换为符号链接）：业务成功保持不变、材料保留并上报，
/// 解除异常后由维护入口补清理。
#[cfg(unix)]
#[test]
fn cleanup_protects_business_success_when_transaction_path_is_symlinked() {
    let env = ready_env("cleanup-symlink");
    let transactions = env
        .paths
        .backup_root()
        .join(session_backup::TRANSACTIONS_DIR_NAME);
    std::fs::create_dir_all(&transactions).unwrap();
    let target = env.root.join("redirected-transactions");
    std::fs::create_dir_all(&target).unwrap();
    // 档位目录被替换为符号链接：删除前校验必须拒绝，且不得跟随链接删除。
    std::os::unix::fs::symlink(&target, transactions.join(WbVariant::Cn.as_str())).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    // 业务成功不变：正文与数据库行都在，报告仍报成功，只是临时文件待处理。
    assert!(env.body_path(&new_id).exists());
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    assert_eq!(report["copied"][0]["cleanupState"], "pending", "{report}");
    assert!(
        report["copied"][0]["backup"].as_str().is_some(),
        "待清理必须保留位置：{report}"
    );
    assert!(
        report["copied"][0]["cleanupError"]
            .as_str()
            .unwrap()
            .contains("符号链接"),
        "{report}"
    );
    let temporary_files = report["temporaryFiles"].as_array().expect("temporaryFiles");
    assert!(
        !temporary_files.is_empty(),
        "本轮清理受阻必须出现在报告级 temporaryFiles：{report}"
    );
    assert!(
        temporary_files.iter().any(|item| item["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("符号链接"))),
        "{report}"
    );
    assert_eq!(session_backup::scan_lifecycle(&env.paths).records.len(), 1);

    // 解除异常后：维护入口补清理，业务结果不变；链接目标不被当作本操作材料删除。
    std::fs::remove_file(transactions.join(WbVariant::Cn.as_str())).unwrap();
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
    assert!(env.body_path(&new_id).exists());
    assert_eq!(
        session_link::scan_operations(&env.paths).operations[0]
            .cleanup_state
            .as_deref(),
        Some(session_backup::CLEANUP_STATE_CLEANED)
    );
}

#[test]
fn copy_retry_reuses_member_without_second_copy() {
    let env = ready_env("retry");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let revision_before_retry = env.store().revision;

    let second = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(second["copied"].as_array().unwrap().len(), 0);
    assert_eq!(second["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(second["alreadyLinked"][0]["sessionId"], new_id);
    assert!(second.get("errors").is_none());

    assert_eq!(env.body_files().len(), 2, "重试不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b").len(), 1);
    assert_eq!(
        env.store().revision,
        revision_before_retry,
        "幂等复用不写同步记录"
    );
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn copy_back_reuses_original_session() {
    let env = ready_env("back");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);

    // 目标账号成为当前账号，把副本复制回原账号：必须复用原件。
    env.set_login("uid-b");
    let back = copy(&env, "uid-a", &[&new_id]);
    assert_eq!(back["copied"].as_array().unwrap().len(), 0);
    assert_eq!(back["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(back["alreadyLinked"][0]["sessionId"], "sess-1");

    assert_eq!(env.body_files().len(), 2, "B→A 不得新建副本");
    assert_eq!(env.rows_for("uid-a"), vec!["sess-1".to_string()]);
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn chain_a_to_b_then_a_to_c_then_b_to_c_reuses_existing_copy() {
    let env = ready_env("chain");
    let to_b = copy(&env, "uid-b", &["sess-1"]);
    let b_id = env.first_copy_id(&to_b);

    // 同一来源再复制给 C：同组内新增一个成员，不是新组。
    let to_c = copy(&env, "uid-c", &["sess-1"]);
    let c_id = env.first_copy_id(&to_c);
    assert_eq!(env.store().groups.len(), 1);
    assert_eq!(env.store().groups[0].members.len(), 3);

    // B→C：组内已有 C 的有效副本，复用而不重复复制。
    env.set_login("uid-b");
    let b_to_c = copy(&env, "uid-c", &[&b_id]);
    assert_eq!(b_to_c["copied"].as_array().unwrap().len(), 0);
    assert_eq!(b_to_c["alreadyLinked"][0]["sessionId"], c_id);
    assert_eq!(env.body_files().len(), 3);

    // 配对基线按成员对保存：A/B、A/C 各有基线；C 加入时按 A/B 基线继承出 B/C。
    let group = &env.store().groups[0];
    assert_eq!(group.pair_bases.len(), 3);
    let member_id = |uid: &str| {
        group
            .members
            .iter()
            .find(|member| member.uid == uid)
            .unwrap()
            .member_id
            .clone()
    };
    let (a, b, c) = (member_id("uid-a"), member_id("uid-b"), member_id("uid-c"));
    let pair = |left: &str, right: &str| {
        session_link::find_pair_base(group, left, right)
            .unwrap_or_else(|| panic!("缺少成员对基线 {left}/{right}"))
            .baseline_ref
            .clone()
    };
    assert_eq!(pair(&a, &b), pair(&b, &c), "B/C 继承自 A/B 的共同基线");
    assert_ne!(pair(&a, &c), pair(&a, &b), "A/C 是各自新建的基线");
}

#[test]
fn rename_does_not_break_link() {
    let env = ready_env("rename");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);

    // 用户在目标账号改名：关联不依赖标题。
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET title = '改过的标题', custom_title = '自定义名' WHERE id = ?1",
        [&new_id],
    )
    .unwrap();
    drop(conn);

    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn same_title_independent_sessions_stay_separate() {
    let env = ready_env("same-title");
    env.add_session("sess-2", "uid-a", "标题一");
    env.add_body("sess-2", &body_text("sess-2"));

    let report = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 2);
    let ids: Vec<String> = report["copied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["newId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);

    // 同标题不建立关联：第二个会话各成一个组，且各自独立幂等。
    assert_eq!(env.store().groups.len(), 2);
    assert_eq!(env.rows_for("uid-b").len(), 2);
    assert_eq!(env.body_files().len(), 4);

    let again = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(again["alreadyLinked"].as_array().unwrap().len(), 2);
    assert_eq!(env.body_files().len(), 4);
}

#[test]
fn invalid_target_member_is_superseded_and_rebuilt_without_resurrection() {
    let env = ready_env("rebuild");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let old_id = env.first_copy_id(&first);

    // 目标副本的正文丢失 → 旧成员失效，重建新成员。
    std::fs::remove_file(env.body_path(&old_id)).unwrap();
    let rebuilt = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(rebuilt["copied"].as_array().unwrap().len(), 1);
    let new_id = env.first_copy_id(&rebuilt);
    assert_ne!(new_id, old_id);
    assert_eq!(
        env.body_files().len(),
        2,
        "旧复制后的内容已删，只剩源与新建副本"
    );

    let group = &env.store().groups[0];
    assert_eq!(group.members.len(), 3);
    let actives: Vec<&str> = group
        .members
        .iter()
        .filter(|member| member.uid == "uid-b" && member.state == MemberState::Active)
        .map(|member| member.session_id.as_str())
        .collect();
    assert_eq!(actives, vec![new_id.as_str()], "每账号唯一 active");
    assert_eq!(
        group
            .members
            .iter()
            .find(|member| member.session_id == old_id)
            .unwrap()
            .state,
        MemberState::Superseded
    );

    // 旧正文恢复后不得自动争夺有效位置：仍是重建后的成员有效。
    env.add_body(&old_id, &body_text(&old_id));
    let after_restore = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(after_restore["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.store().groups[0].members.len(), 3, "不新增成员");
}

#[test]
fn identity_uses_uid_and_never_rebinds_by_account_id() {
    let env = ready_env("identity");

    // 手工写入一个组：目标成员 uid=uid-b，accountId 是旧账号 id（重新导入前的 id）。
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        store.groups.push(LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![
                LinkMember {
                    member_id: "m-a".to_string(),
                    account_id: Some("acc-uid-a".to_string()),
                    uid: "uid-a".to_string(),
                    session_id: "sess-1".to_string(),
                    variant: None,
                    state: MemberState::Active,
                    linked_at: 1,
                    last_synced_at: None,
                },
                LinkMember {
                    member_id: "m-b".to_string(),
                    account_id: Some("old-account-id".to_string()),
                    uid: "uid-b".to_string(),
                    session_id: "sess-b".to_string(),
                    variant: None,
                    state: MemberState::Active,
                    linked_at: 1,
                    last_synced_at: None,
                },
            ],
            pair_bases: Vec::new(),
        });
        Ok(())
    })
    .unwrap();
    env.add_body("sess-b", &body_text("sess-b"));
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('sess-b', 'uid-b', '旧副本', '/ws/a', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    // uid 相同、accountId 变了 → 仍复用（身份以 uid 为准）。
    let reused = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(reused["alreadyLinked"][0]["sessionId"], "sess-b");
    assert_eq!(env.body_files().len(), 2);

    // accountId 相同但 uid 不同 → 不得错误绑定，必须新建。
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET user_id = 'uid-x' WHERE id = 'sess-b'",
        [],
    )
    .unwrap();
    drop(conn);
    let fresh = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(fresh["copied"].as_array().unwrap().len(), 1);
    assert_eq!(env.store().groups[0].members.len(), 3);
}

#[test]
fn variant_isolation_keeps_groups_separate() {
    let env = ready_env("variants");
    let cn = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(cn["copied"].as_str(), None);
    assert_eq!(cn["copied"].as_array().unwrap().len(), 1);

    // 国际版数据根：单独一套数据（同 store 根），身份字符串相同但档位不同。
    let mut ai_paths = env.paths();
    ai_paths.data_root = env.root.join("data-ai");
    std::fs::create_dir_all(ai_paths.projects_dir().join("ws-a")).unwrap();
    let conn = Connection::open(ai_paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT, custom_title TEXT, cwd TEXT, created_at INTEGER, updated_at INTEGER, deleted_at INTEGER, is_playground INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('ai-sess-1', 'uid-a', 'AI 会话', '/ws/a', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    std::fs::write(
        ai_paths.projects_dir().join("ws-a").join("ai-sess-1.jsonl"),
        body_text("ai-sess-1"),
    )
    .unwrap();
    let conn = Connection::open(ai_paths.edge_sync_db(WbVariant::Ai)).unwrap();
    conn.execute_batch(
        "CREATE TABLE edge_sync_mapping (session_id TEXT, conversation_id TEXT, msg_channel TEXT, created_at INTEGER);",
    )
    .unwrap();
    drop(conn);

    let ai_report = copy_sessions_for_switch_at(
        &ai_paths,
        WbVariant::Ai,
        &json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"}),
        &["ai-sess-1".to_string()],
        |_| false,
    )
    .unwrap();
    assert_eq!(
        ai_report["copied"].as_array().unwrap().len(),
        1,
        "同档位身份不同，必须新复制"
    );

    let store = env.store();
    assert_eq!(store.groups.len(), 2);
    let variants: Vec<&str> = store
        .groups
        .iter()
        .map(|group| group.variant.as_str())
        .collect();
    assert!(
        variants.contains(&"cn") && variants.contains(&"ai"),
        "{variants:?}"
    );
    // 两档位不串数据：成员会话 id 不相交，国际版组里带着国际版来源会话。
    let mut seen = std::collections::HashSet::new();
    for group in &store.groups {
        for member in &group.members {
            assert!(
                seen.insert(member.session_id.clone()),
                "会话 {} 同时出现在两个档位的组里",
                member.session_id
            );
        }
    }
    let ai_group = store
        .groups
        .iter()
        .find(|group| group.variant == WbVariant::Ai)
        .unwrap();
    assert!(ai_group
        .members
        .iter()
        .any(|member| member.session_id == "ai-sess-1"));
}

/// 报告契约：copied / alreadyLinked / errors 同时出现时字段完整（桌面与 webui 同形）。
#[test]
fn copy_report_carries_copied_already_linked_and_errors_together() {
    let env = ready_env("contract");
    env.add_session("sess-2", "uid-a", "标题二");
    env.add_body("sess-2", &body_text("sess-2"));

    let first = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(first["copied"].as_array().unwrap().len(), 2);

    // 第二个会话的正文丢失：本次一个复用、一个失败。
    std::fs::remove_file(env.body_path("sess-2")).unwrap();
    let mixed = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(mixed["sourceUid"], "uid-a");
    assert_eq!(mixed["targetUid"], "uid-b");
    assert_eq!(mixed["copied"].as_array().unwrap().len(), 0);
    assert_eq!(mixed["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(mixed["alreadyLinked"][0]["id"], "sess-1");
    assert_eq!(mixed["errors"].as_array().unwrap().len(), 1);
    assert_eq!(mixed["errors"][0]["id"], "sess-2");
    assert_eq!(mixed["errors"][0]["error"], "会话内容不存在，未复制");
    // 复用/失败都不算未完成写入。
    assert!(mixed.get("needsRecovery").is_none());
}

// ---------------------------------------------------------------------------
// 失败必须可见、可恢复（R1 / R5）
// ---------------------------------------------------------------------------

#[test]
fn missing_body_is_not_reported_as_success() {
    let env = ready_env("no-body");
    std::fs::remove_file(env.body_path("sess-1")).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["errors"][0]["error"], "会话内容不存在，未复制");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出半成品会话记录");
    assert_eq!(env.body_files().len(), 0);
}

#[test]
fn truncated_body_is_not_reported_as_success() {
    let env = ready_env("truncated");
    env.add_body("sess-1", "{\"sessionId\":\"sess-1\"\n");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

#[test]
fn missing_source_row_is_not_reported_as_success() {
    let env = ready_env("no-row");
    env.delete_row("sess-1");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(
        report["errors"][0]["error"],
        "数据库中找不到源会话记录，未复制"
    );
    assert_eq!(env.body_files().len(), 1, "不得写出无数据库行的内容");
}

#[test]
fn source_row_of_another_account_is_rejected() {
    let env = ready_env("other-owner");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET user_id = 'uid-x' WHERE id = 'sess-1'",
        [],
    )
    .unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["errors"][0]["error"], "源会话不属于当前账号，未复制");
}

/// 复制不再依赖云端映射库：库缺失也照常成功，不重建、不写行。
#[test]
fn copy_succeeds_without_edge_sync_db() {
    let env = ready_env("mapping-absent");
    std::fs::remove_file(env.paths.edge_sync_db(WbVariant::Cn)).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert!(report.get("errors").is_none(), "{report}");
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.rows_for("uid-b").len(), 1);
    assert!(
        !env.paths().edge_sync_db(WbVariant::Cn).exists(),
        "复制不得创建或写入映射库"
    );
    assert!(session_link::pending_operations(&env.paths(), WbVariant::Cn).is_empty());
}

/// 关联提交失败（存储目录不可写）→ 不报成功；恢复后同一 UUID 完成。
#[cfg(unix)]
#[test]
fn link_commit_failure_keeps_pending_then_recovers_with_same_uuid() {
    use std::os::unix::fs::PermissionsExt;

    let env = ready_env("link-commit");
    // 先跑一次成功，建立锁文件与存储文件，避免把「无法加锁」当成关联提交失败。
    copy(&env, "uid-c", &["sess-1"]);
    if std::fs::write(env.paths.store_root.join(".probe"), b"x").is_err() {
        return;
    }

    std::fs::set_permissions(
        &env.paths.store_root,
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    if std::fs::write(env.paths.store_root.join(".probe"), b"x").is_ok() {
        // root / 特殊 ACL 环境写保护无效：跳过（不误报）。
        std::fs::set_permissions(
            &env.paths.store_root,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        return;
    }

    let failed = copy(&env, "uid-b", &["sess-1"]);
    std::fs::set_permissions(
        &env.paths.store_root,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(failed["copied"].as_array().unwrap().len(), 0);
    let error = failed["errors"][0]["error"].as_str().unwrap();
    assert!(
        error.contains("保存失败") || error.contains("同步记录"),
        "{error}"
    );
    assert_eq!(failed["needsRecovery"], true);

    let pending = session_link::pending_operations(&env.paths(), WbVariant::Cn);
    let target = pending
        .iter()
        .find(|operation| operation.target.uid == "uid-b")
        .expect("应保留未完成操作");
    let new_id = target.target.session_id.clone();

    let retry = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(retry["copied"].as_array().unwrap().len(), 0);
    assert_eq!(retry["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(
        env.body_files().len(),
        4,
        "两个目标各一份副本，重试不得新增"
    );
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
}

/// 正文写入失败：不得写出数据库行，也不报成功。
#[cfg(unix)]
#[test]
fn body_write_failure_reports_error_without_db_row() {
    use std::os::unix::fs::PermissionsExt;

    let env = ready_env("body-write");
    let ws = env.paths.projects_dir().join("ws-a");
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::write(ws.join(".probe"), b"x").is_ok();
    let report = copy(&env, "uid-b", &["sess-1"]);
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        // root / 特殊 ACL 环境：写保护无效，跳过断言。
        return;
    }
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("复制后的内容保存失败"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
    assert_eq!(env.body_files().len(), 1);
}

#[test]
fn corrupt_store_blocks_copy_and_preserves_original() {
    let env = ready_env("corrupt-store");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(env.paths.session_links_file(), "not-json").unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(
        error.contains("同步记录") && error.contains("已阻止复制"),
        "{error}"
    );
    assert_eq!(env.rows_for("uid-b").len(), 0);
    assert_eq!(env.body_files().len(), 1);
    assert_eq!(
        std::fs::read_to_string(env.paths.session_links_file()).unwrap(),
        "not-json",
        "必须保留现场，不得当空表覆盖"
    );
}

#[test]
fn unknown_store_version_blocks_copy() {
    let env = ready_env("unknown-version");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(
        env.paths.session_links_file(),
        json!({"version": 99, "revision": 1, "groups": []}).to_string(),
    )
    .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("版本"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

/// 主文件被删但基线文件仍在 → 检测到痕迹，不得当首次使用重建空表，复制被阻止。
#[test]
fn copy_blocked_when_store_file_missing_but_baselines_remain() {
    let env = ready_env("baseline-trace");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    assert!(env.baseline_files() > 0, "首次复制应留下基线文件");

    std::fs::remove_file(env.paths.session_links_file()).unwrap();
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("保留现场"), "{error}");
    assert!(
        !env.paths.session_links_file().exists(),
        "不得重建空表覆盖现场"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
}

/// 并发请求：档位操作锁被占用时直接拒绝，不产生第二个副本。
#[test]
fn concurrent_request_is_rejected_without_second_copy() {
    let env = ready_env("concurrent");
    let held = session_link::try_acquire_variant_ops_lock(&env.paths(), WbVariant::Cn).unwrap();

    let err = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        |_| false,
    )
    .expect_err("持锁期间必须拒绝");
    assert!(err.contains("会话操作"), "{err}");
    assert_eq!(env.body_files().len(), 1);

    drop(held);
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
}

/// 目标账号不是当前登录账号（auth=uid-a）时，App 运行中不再拦截写入（S5 实测口径）；
/// 快拒与锁后复查仍各执行一次，且复查必须发生在持锁之后、写入之前。
#[test]
fn copy_runs_while_app_running_for_non_current_account() {
    let env = ready_env("app-running-non-current");
    let probes = std::cell::Cell::new(0usize);
    let lock_held_on_recheck = std::cell::Cell::new(false);
    let lock_path = env.paths.variant_ops_lock_file(WbVariant::Cn);
    let probe = |_: WbVariant| {
        let n = probes.get() + 1;
        probes.set(n);
        if n == 1 {
            true
        } else {
            // 第二次必须发生在持锁之后：此时再抢同一把锁应为 Busy。
            match session_link::try_lock_file(&lock_path) {
                Err(session_link::LockError::Busy) => lock_held_on_recheck.set(true),
                Err(session_link::LockError::Unavailable(reason)) => {
                    panic!("第二次探针时期望档位锁已被持有，实际 Unavailable: {reason}")
                }
                Ok(_) => panic!("第二次探针时期望档位锁已被持有，实际拿到了锁"),
            }
            true
        }
    };
    let report = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        probe,
    )
    .expect("目标非当前登录账号：运行中必须放行");
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert_eq!(probes.get(), 2, "快拒与锁后复查各检查一次");
    assert!(
        lock_held_on_recheck.get(),
        "复查必须发生在已经拿到档位锁之后、任何写入之前"
    );
    assert_eq!(env.rows_for("uid-b").len(), 1, "副本行已写入");
    assert!(env.paths.session_links_file().exists(), "关联记录已建立");
}

// ---------------------------------------------------------------------------
// 恢复
// ---------------------------------------------------------------------------

#[test]
fn recovery_abandons_prepared_operation_when_source_is_gone() {
    let env = ready_env("abandon");
    let paths = env.paths();
    session_link::save_operation(
        &paths,
        &Operation {
            version: crate::modules::session_link::OPERATION_VERSION,
            operation_id: "op-gone".to_string(),
            kind: "copy".to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id: "g-gone".to_string(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-gone".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "new-gone".to_string(),
            },
            expected_content_digest: "d".to_string(),
            expected_record_count: 1,
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

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert_eq!(report.abandoned, vec!["op-gone".to_string()]);
    assert!(report.is_clean());
    assert!(session_link::pending_operations(&paths, WbVariant::Cn).is_empty());
    assert_eq!(env.body_files().len(), 1);
}

#[test]
fn recovery_stops_when_intermediate_body_was_modified() {
    let env = ready_env("recovery-stop");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    // 模拟"正文已写、后续阶段中断"：把已完成的操作日志回退到 BodyWritten。
    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::BodyWritten;
    session_link::save_operation(&paths, &operation).unwrap();

    // 中间产物被其它程序改动 → 停止恢复，不覆盖。
    let tampered = format!(
        "{}\n",
        json!({"type": "user", "sessionId": new_id, "text": "别人改的"})
    );
    std::fs::write(env.body_path(&new_id), &tampered).unwrap();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(!report.is_clean());
    let issue = &report.needs_recovery[0];
    assert!(!issue.retryable);
    assert!(
        issue.reason.contains("目标内容与操作记录不一致"),
        "{}",
        issue.reason
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&new_id)).unwrap(),
        tampered
    );

    // 该会话再次请求时不新建副本，而是报告未完成。
    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["copied"].as_array().unwrap().len(), 0);
    assert_eq!(again["errors"].as_array().unwrap().len(), 1);
    assert!(again["errors"][0]["error"]
        .as_str()
        .unwrap()
        .contains("上一次复制尚未完成"));
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
}

/// 复制完成后再次请求同一会话：报告 alreadyLinked，不新建副本。
#[test]
fn retry_after_completed_copy_does_not_duplicate() {
    let env = ready_env("partial-resume");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let body_count = env.body_files().len();
    assert_eq!(body_count, 2);

    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["copied"].as_array().unwrap().len(), 0);
    assert_eq!(again["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.body_files().len(), body_count, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b").len(), 1);
}

/// 恢复不重放已完成的阶段：产物全部就位、只剩 Completed 未写时，只补写阶段标记，
/// 不重写关联存储与基线、不重复登记映射（design §5）。
#[test]
fn recovery_completes_without_replaying_finished_stages() {
    let env = ready_env("recover-no-replay");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    // 模拟「关联已提交、Completed 写入失败」：把已完成的操作日志回退到 LinksCommitted。
    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::LinksCommitted;
    session_link::save_operation(&paths, &operation).unwrap();
    let operation_id = operation.operation_id.clone();

    let before = env.store();
    let revision_before = before.revision;
    let pair_bases_before = before.groups[0].pair_bases.clone();
    let baselines_before = env.baseline_files();
    let members_before = before.groups[0].members.len();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert_eq!(report.recovered, vec![operation_id]);
    assert!(report.is_clean(), "{:?}", report.needs_recovery);

    let after = env.store();
    assert_eq!(after.revision, revision_before, "恢复不得重写同步记录");
    assert_eq!(after.groups[0].pair_bases.len(), pair_bases_before.len());
    assert_eq!(
        after.groups[0].pair_bases[0].baseline_ref, pair_bases_before[0].baseline_ref,
        "不得重写配对基线"
    );
    assert_eq!(after.groups[0].members.len(), members_before);
    assert_eq!(env.baseline_files(), baselines_before, "不得新增基线文件");
    assert_eq!(
        env.mapping_rows(),
        0,
        "复制不再预写云端映射，恢复也不得补写"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
    assert!(session_link::pending_operations(&paths, WbVariant::Cn).is_empty());
}

/// phase 已是 LinksCommitted，但关联主文件被删：不得跳过核验后标 Completed。
#[test]
fn recovery_stops_when_committed_links_are_missing() {
    let env = ready_env("recover-links-gone");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::LinksCommitted;
    session_link::save_operation(&paths, &operation).unwrap();
    std::fs::remove_file(paths.session_links_file()).unwrap();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(report.recovered.is_empty(), "{:?}", report.recovered);
    assert_eq!(report.needs_recovery.len(), 1);
    assert!(!report.needs_recovery[0].retryable);
    assert!(
        report.needs_recovery[0].reason.contains("同步记录"),
        "{}",
        report.needs_recovery[0].reason
    );
    assert!(
        !paths.session_links_file().exists(),
        "不得把缺失的主文件当成空表重建"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(
        session_link::pending_operations(&paths, WbVariant::Cn).len(),
        1,
        "必须保留未完成操作，不能标 Completed"
    );
}

// ---------------------------------------------------------------------------
// 账户/环境辅助
// ---------------------------------------------------------------------------

#[test]
fn unparseable_operation_log_blocks_copy_without_second_replica() {
    let env = ready_env("bad-op-json");
    std::fs::create_dir_all(env.paths.operations_dir()).unwrap();
    std::fs::write(env.paths.operations_dir().join("broken.json"), "not-json").unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths(), WbVariant::Cn);
    assert!(!recovery.is_clean());
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(!recovery.needs_recovery[0].retryable);
    assert!(
        recovery.needs_recovery[0]
            .reason
            .contains(UNPARSEABLE_OPERATION_REASON),
        "{}",
        recovery.needs_recovery[0].reason
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert_eq!(report["needsRecovery"], true);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains(UNPARSEABLE_OPERATION_REASON), "{error}");
    assert_eq!(
        env.body_files().len(),
        1,
        "不得绕过损坏的操作记录写出第二个副本"
    );
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

#[test]
fn recovery_reports_store_unavailable_instead_of_writing() {
    let env = ready_env("store-broken-recovery");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(env.paths.session_links_file(), "not-json").unwrap();

    let report = recover_pending_session_operations_at(&env.paths(), WbVariant::Cn);
    assert!(report.is_clean(), "没有未完成操作时不做任何事");
    assert_eq!(
        std::fs::read_to_string(env.paths.session_links_file()).unwrap(),
        "not-json"
    );
}

#[test]
fn session_display_title_prefers_custom_title() {
    assert_eq!(
        session_display_title(Some("自动标题".into()), Some("美团每日自动领券".into())),
        "美团每日自动领券"
    );
    assert_eq!(
        session_display_title(None, Some("美团每日自动领券".into())),
        "美团每日自动领券"
    );
    assert_eq!(
        session_display_title(Some("汉字详情页".into()), None),
        "汉字详情页"
    );
    assert_eq!(session_display_title(None, None), "(无标题)");
    assert_eq!(
        session_display_title(Some("  ".into()), Some("".into())),
        "(无标题)"
    );
}

#[test]
fn claw_workspace_detected_by_folder_name() {
    assert!(is_claw_workspace("/Users/apple/WorkBuddy/Claw"));
    assert!(is_claw_workspace("/Users/apple/WorkBuddy/claw/"));
    assert!(is_claw_workspace(r"C:\Users\me\WorkBuddy\Claw"));
    assert!(!is_claw_workspace("/Users/apple/WorkBuddy/ClawBot"));
    assert!(!is_claw_workspace(
        "/Users/apple/Documents/AI-PROJECT/LetterTotTown"
    ));
}

#[test]
fn list_sessions_marks_has_history() {
    let env = ready_env("list-sessions");
    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], "sess-1");
    assert_eq!(sessions[0]["hasHistory"], true);

    std::fs::remove_file(env.body_path("sess-1")).unwrap();
    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions[0]["hasHistory"], false);
}

#[test]
fn insert_session_copy_duplicates_row_with_target_uid() {
    let env = ready_env("insert");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '旧标题', '/ws', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-uuid-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    assert_eq!(outcome, DbCopyOutcome::Inserted);

    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    let (id, user_id, title, deleted_at, is_playground): (String, String, String, Option<i64>, i64) =
        conn.query_row(
            "SELECT id, user_id, title, deleted_at, is_playground FROM sessions WHERE id = 'new-uuid-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(id, "new-uuid-1");
    assert_eq!(user_id, "uid-b");
    assert_eq!(title, "旧标题"); // 普通列原样保留
    assert_eq!(deleted_at, None);
    assert_eq!(is_playground, 0);
}

#[test]
fn insert_session_copy_reports_missing_source_and_db() {
    let env = ready_env("insert-missing");
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-1",
            "missing",
            "uid-a",
            "uid-b"
        )
        .unwrap(),
        DbCopyOutcome::SourceRowMissing,
        "源行缺失必须显式上报，不能当成功（旧实现的假成功）"
    );

    std::fs::remove_file(env.paths.workbuddy_db()).unwrap();
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-1",
            "sess-1",
            "uid-a",
            "uid-b"
        )
        .unwrap(),
        DbCopyOutcome::NoDb
    );
}

// ---------------------------------------------------------------------------
// 复制时的「末条 incomplete」归一
// ---------------------------------------------------------------------------

/// 末条停在 `status = "incomplete"` 的正文：前两行正常，第三行是「运行中」假象
/// （真实数据里的「Interrupted by user / 429 限流 / 500 报错」都是这个形态）。
fn incomplete_body(cid: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        json!({"type": "user", "sessionId": cid, "text": "你好"}),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "sessionId": cid,
            "content": [{"type": "output_text", "text": "hi"}]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "incomplete",
            "content": [{"type": "output_text", "text": "Interrupted by user"}]
        })
    )
}

/// 正文行（去掉末尾换行后的逐行切片）。
fn body_lines(text: &str) -> Vec<&str> {
    text.trim_end_matches('\n').split('\n').collect()
}

/// 末条为 incomplete → 副本末条改为 completed：行数不变、其它行逐字不变，
/// 源正文一字不动，配对基线摘要 = 归一化后的正文摘要（后续同步不误判漂移）。
#[test]
fn copy_normalizes_incomplete_last_turn() {
    let env = ready_env("copy-normalize-incomplete");
    let source_text = incomplete_body("sess-1");
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    let source_lines = body_lines(&source_text);
    let target_lines = body_lines(&target_text);
    assert_eq!(
        target_lines.len(),
        source_lines.len(),
        "归一不得增删行：{target_text}"
    );
    // 其它行只做 session id 替换，逐字不变（摘要按行算，任何多余改动都是「内容变了」）。
    for (source_line, target_line) in source_lines.iter().zip(target_lines.iter()).take(2) {
        assert_eq!(*target_line, source_line.replace("sess-1", &new_id));
    }
    let last: Value = serde_json::from_str(target_lines[2]).unwrap();
    assert_eq!(
        last["status"],
        json!("completed"),
        "末条必须归一到 completed"
    );
    assert_eq!(
        last["content"],
        json!([{"type": "output_text", "text": "Interrupted by user"}]),
        "正文内容不得被改写"
    );
    assert_eq!(last["role"], json!("assistant"));
    assert_eq!(
        last["_meta"]["incompleteNormalized"]["reason"],
        json!("copy normalize")
    );
    assert!(
        last["_meta"]["incompleteNormalized"]["at"].is_string(),
        "留痕必须带时间戳：{last}"
    );
    // 源会话（原会话）必须保持原样：归一只作用于副本。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source_text
    );

    // 基线摘要必须是归一化后的值，否则下一次同步会把副本判成「内容漂移」。
    let baseline_ref = env.store().groups[0].pair_bases[0].baseline_ref.clone();
    let baseline = session_link::load_baseline(&env.paths(), &baseline_ref).unwrap();
    let target_snapshot =
        match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
            ContentState::Ready(snapshot) => snapshot,
            other => panic!("副本正文应可读，实际 {other:?}"),
        };
    assert_eq!(
        baseline.total_digest, target_snapshot.normalized.total_digest,
        "基线必须与落盘的归一化正文一致"
    );
    assert_eq!(baseline.record_count, 3, "基线记录数不变");
}

/// 末条已是 completed → 副本逐字等于「源正文替换 session id」，幂等不误改。
#[test]
fn copy_keeps_completed_last_turn_untouched() {
    let env = ready_env("copy-normalize-completed");
    let source_text = format!(
        "{}\n{}\n",
        json!({"type": "user", "sessionId": "sess-1", "text": "你好"}),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "sessionId": "sess-1",
            "content": [{"type": "output_text", "text": "hi"}]
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    assert_eq!(
        target_text,
        source_text.replace("sess-1", &new_id),
        "已完成的会话不得被改写（幂等）"
    );
    assert!(
        !target_text.contains("_meta"),
        "没有归一动作就不该留痕：{target_text}"
    );
}

/// 最后一条 assistant 消息后面跟着 tool 行 → 仍归一那一行，tool 行逐字不变。
///
/// 定位口径是「最后一条 assistant 消息」（与存量修复脚本一致）：尾部跟
/// `function_call_result` 的会话，客户端会显示「正在执行工具」，同样要修。
#[test]
fn copy_normalizes_assistant_before_trailing_tool_rows() {
    let env = ready_env("copy-normalize-tool-tail");
    let source_text = format!(
        "{}\n{}\n{}\n",
        json!({
            "type": "message",
            "role": "user",
            "sessionId": "sess-1",
            "content": [{"type": "input_text", "text": "跑一下"}]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "incomplete",
            "sessionId": "sess-1",
            "content": [{"type": "output_text", "text": "Interrupted by user"}]
        }),
        json!({
            "type": "function_call_result",
            "sessionId": "sess-1",
            "name": "read_file",
            "output": "ok"
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    let source_lines = body_lines(&source_text);
    let target_lines = body_lines(&target_text);
    assert_eq!(target_lines.len(), 3, "行数必须不变：{target_text}");
    // 其它行（用户消息、tool 结果）逐字等于「源行替换 session id」。
    assert_eq!(target_lines[0], source_lines[0].replace("sess-1", &new_id));
    assert_eq!(
        target_lines[2],
        source_lines[2].replace("sess-1", &new_id),
        "尾部 tool 行必须逐字不变"
    );
    let assistant: Value = serde_json::from_str(target_lines[1]).unwrap();
    assert_eq!(assistant["status"], json!("completed"));
    assert_eq!(
        assistant["_meta"]["incompleteNormalized"]["reason"],
        json!("copy normalize")
    );
    // 源正文不受影响。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source_text
    );
}

/// 正文里根本没有 assistant 记录 → 不动（哪怕别的记录带同名 status 文本）。
#[test]
fn copy_keeps_body_without_assistant_untouched() {
    let env = ready_env("copy-normalize-no-assistant");
    let source_text = format!(
        "{}\n{}\n",
        json!({"type": "user", "sessionId": "sess-1", "text": "只有用户消息"}),
        json!({
            "type": "function_call_result",
            "sessionId": "sess-1",
            "output": "ok"
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    assert_eq!(
        target_text,
        source_text.replace("sess-1", &new_id),
        "没有 assistant 记录时一律不动"
    );
}

/// 复制归一 DB 行状态：`working` / `active` → `completed`，`error` / `completed` 原样。
#[test]
fn insert_session_copy_normalizes_running_row_status() {
    let env = Env::new("insert-status");
    env.create_db_with_status();
    env.set_login("uid-a");
    let insert = |id: &str, status: &str| {
        let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, cwd, status, created_at, updated_at, deleted_at, is_playground)
             VALUES (?1, 'uid-a', '标题', '/ws/a', ?2, 1000, 2000, NULL, 0)",
            rusqlite::params![id, status],
        )
        .unwrap();
    };
    insert("src-working", "working");
    insert("src-active", "active");
    insert("src-error", "error");
    insert("src-completed", "completed");
    // 终态常量之外的一切状态都算「可能被当成运行中」：合法但语义未定的默认值
    // `Pending`、未知新状态都在内。
    insert("src-pending", "Pending");
    insert("src-unknown", "queued");
    // 终态 `terminated` 不在真实库里常见，但必须证明保留集合没误伤它。
    insert("src-terminated", "terminated");
    // 归档态：用户显式做过的整理动作，复制不得抹掉（它不在归档动作的目标终态里）。
    insert("src-archived", "archived");

    for (source_id, new_id) in [
        ("src-working", "new-working"),
        ("src-active", "new-active"),
        ("src-error", "new-error"),
        ("src-completed", "new-completed"),
        ("src-pending", "new-pending"),
        ("src-unknown", "new-unknown"),
        ("src-terminated", "new-terminated"),
        ("src-archived", "new-archived"),
    ] {
        assert_eq!(
            insert_session_copy(
                &env.paths(),
                &env.paths(),
                new_id,
                source_id,
                "uid-a",
                "uid-b"
            )
            .unwrap(),
            DbCopyOutcome::Inserted
        );
    }

    assert_eq!(
        env.status_of("new-working"),
        Some("completed".to_string()),
        "运行中的源状态必须归一为 completed（否则客户端仍显示思考中）"
    );
    assert_eq!(env.status_of("new-active"), Some("completed".to_string()));
    assert_eq!(
        env.status_of("new-pending"),
        Some("completed".to_string()),
        "Pending 不在终态常量里，必须被归一（白名单口径会漏掉它）"
    );
    assert_eq!(
        env.status_of("new-unknown"),
        Some("completed".to_string()),
        "未知状态同样归一：客户端可能把它当运行中"
    );
    assert_eq!(
        env.status_of("new-error"),
        Some("error".to_string()),
        "错误状态必须原样保留"
    );
    assert_eq!(
        env.status_of("new-completed"),
        Some("completed".to_string())
    );
    assert_eq!(
        env.status_of("new-terminated"),
        Some("terminated".to_string()),
        "terminated 是终态，保留集合不得误伤"
    );
    assert_eq!(
        env.status_of("new-archived"),
        Some("archived".to_string()),
        "归档态必须原样保留：复制不该让已归档会话变成未归档"
    );
    // 源行本身的 status 不受影响（只归一副本）。
    assert_eq!(env.status_of("src-working"), Some("working".to_string()));
    assert_eq!(env.status_of("src-pending"), Some("Pending".to_string()));
    assert_eq!(env.status_of("src-archived"), Some("archived".to_string()));
}

/// 回归：复制一个**已归档**会话，副本行必须仍是 `archived`（而不是被抹成
/// `completed`），因此预览也不会额外下发 `archiveAction: statusOnly`。
///
/// 抹成 `completed` 会让副本在目标账号里显示成未归档，还会让同步预览多出一个
/// 用户没请求的归档动作。
#[test]
fn copy_preserves_archived_row_status_without_archive_action() {
    let env = ready_env_with_status("copy-archived-row");
    env.set_status("sess-1", "archived");

    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("archived"),
        "归档态必须原样继承"
    );
    let preview_report = preview(&env, "uid-b");
    let group = &preview_report["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview_report}");
    assert!(
        group["archiveAction"].is_null(),
        "副本已归档，不该再下发归档动作：{group}"
    );
}

/// 目标表没有 `status` 列（老库）：照常复制，不得因此报错。
#[test]
fn insert_session_copy_without_status_column_still_works() {
    let env = ready_env("insert-no-status");
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-no-status",
            "sess-1",
            "uid-a",
            "uid-b"
        )
        .unwrap(),
        DbCopyOutcome::Inserted
    );
    assert_eq!(env.status_of("new-no-status"), None);
}

/// 归一后的正文必须能通过 `write_copy_body` 的完整性校验。
///
/// 这是本次最容易踩的坑：`status` 一变，那一行的摘要与 `total_digest` 就变，
/// 期望值只能取「待写入文本」的摘要；拿源快照摘要去比必然不等。
#[test]
fn normalized_copy_body_passes_integrity_check() {
    let env = ready_env("copy-normalize-integrity");
    let source_text = incomplete_body("sess-1");
    env.add_body("sess-1", &source_text);
    let paths = env.paths();
    let source_path = env.body_path("sess-1");
    let snapshot = match session_link::read_content_snapshot(&source_path, "sess-1") {
        ContentState::Ready(snapshot) => snapshot,
        other => panic!("源正文应可读，实际 {other:?}"),
    };

    let body = copy_body(&snapshot, "sess-1", "new-cid-fixed").unwrap();
    // 期望摘要 == 对同一份待写入文本重新归一化算出的摘要（口径完全一致）。
    let recomputed = session_link::normalize_jsonl(&body.text, "new-cid-fixed").unwrap();
    assert_eq!(recomputed.total_digest, body.normalized.total_digest);
    // 源摘要与归一化摘要必须不同：否则这条用例证明不了「先写后改会判失败」这个坑。
    assert_ne!(
        snapshot.normalized.total_digest, body.normalized.total_digest,
        "归一必须真的改变了摘要，用例才有意义"
    );

    // 真正落盘 + 回读校验（校验逻辑本身没有放宽）：必须成功。
    let written = write_copy_body(&body, &source_path, &paths, &paths, "new-cid-fixed")
        .expect("归一后的正文必须通过写后完整性校验");
    assert_eq!(written, env.body_path("new-cid-fixed"));
    match session_link::read_content_snapshot(&written, "new-cid-fixed") {
        ContentState::Ready(read_back) => assert_eq!(
            read_back.normalized.total_digest, body.normalized.total_digest,
            "落盘正文与期望摘要必须一致"
        ),
        other => panic!("落盘正文应可读，实际 {other:?}"),
    }
}

/// 建一套「带项目归属」的库：sessions 有 `space_id` / `project_id`，另有 spaces 表。
///
/// 表结构与本机实测的真实库一致（`deleted_at` 默认 -1、`pinned`/`status`/`transport`
/// 都有默认值），这样才能验证复制的行是否真能被客户端当成项目会话。
fn create_db_with_spaces(env: &Env) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            custom_title TEXT,
            cwd TEXT,
            space_id TEXT,
            project_id TEXT,
            created_at INTEGER,
            updated_at INTEGER,
            deleted_at INTEGER,
            is_playground INTEGER
        );
        CREATE TABLE spaces (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            name TEXT NOT NULL,
            primary_folder_path TEXT,
            create_folder_path TEXT,
            pinned INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER,
            status TEXT NOT NULL DEFAULT 'active',
            transport TEXT NOT NULL DEFAULT 'local',
            conversation_origin TEXT,
            deleted_at INTEGER NOT NULL DEFAULT -1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            source_kind TEXT
        );",
    )
    .unwrap();
}

/// 复制后目标会话行的 (space_id, project_id)。
fn space_of(env: &Env, cid: &str) -> (Option<String>, Option<String>) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.query_row(
        "SELECT space_id, project_id FROM sessions WHERE id = ?1",
        [cid],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

/// a) 目标账号已有同名 space → 复制后 `space_id` 指向**目标账号**的那个 id。
///
/// 这是用户报告的现象的最小复现：同名不同 id，原样搬运会让目标行悬空。
#[test]
fn copy_remaps_space_id_to_target_account_space() {
    let env = Env::new("space-reuse");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2),
                ('space-b', 'uid-b', 'dev-1', 'ResumeForge', 'seed', 3, 4);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, project_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 'proj-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    assert_eq!(outcome, DbCopyOutcome::Inserted);

    let (space_id, project_id) = space_of(&env, "new-1");
    assert_eq!(
        space_id.as_deref(),
        Some("space-b"),
        "必须落到目标账号自己的同名 space，不能留源账号的 space-a"
    );
    assert_eq!(
        project_id, None,
        "跨账号的 project_id 一定悬空，必须写 NULL"
    );

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 2, "命中已有 space 时不得新建");
}

/// b) 目标账号没有同名 space → 自动新建一条 spaces 行，会话指向它。
#[test]
fn copy_creates_space_for_target_when_missing() {
    let env = Env::new("space-create");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-src', 'ResumeForge', 'seed', 1, 2),
                ('space-other', 'uid-b', 'dev-dst', '其它项目', 'local', 3, 4);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    assert_eq!(outcome, DbCopyOutcome::Inserted);

    let (space_id, _) = space_of(&env, "new-1");
    let space_id = space_id.expect("目标账号没有同名 space 时应当新建并回填");
    assert_ne!(space_id, "space-a", "不得复用源账号的 space");
    assert_ne!(space_id, "space-other", "不得挂到不同名的 space 上");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let (user_id, device_id, name, status, transport, pinned, deleted_at, source_kind): (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT user_id, device_id, name, status, transport, pinned, deleted_at, source_kind
             FROM spaces WHERE id = ?1",
            [&space_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(user_id, "uid-b", "新 space 必须属于目标账号");
    assert_eq!(device_id, "dev-src", "设备标识沿用源 space");
    assert_eq!(name, "ResumeForge");
    assert_eq!(source_kind.as_deref(), Some("seed"), "来源标记沿用源 space");
    assert_eq!(status, "active");
    assert_eq!(transport, "local");
    assert_eq!(pinned, 0);
    assert_eq!(deleted_at, -1);
    let created_at: i64 = conn
        .query_row(
            "SELECT created_at FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(created_at > 0, "时间戳必须写入当前时间，不能是 0");
}

/// c) 源 `space_id` 为 NULL → 目标仍为 NULL，且不新建任何 spaces 行。
#[test]
fn copy_keeps_null_space_id_without_creating_space() {
    let env = Env::new("space-null");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-b', 'uid-b', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '无归属会话', 'D:/ResumeForge', NULL, 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    assert_eq!(outcome, DbCopyOutcome::Inserted);

    let (space_id, _) = space_of(&env, "new-1");
    assert_eq!(space_id, None, "源无项目归属时不得凭空塞进同名 space");
    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "不得新建 spaces 行");
}

/// QA-E1 回归：复制被拒时不得残留本次新建的 space 行。
///
/// 错误路径是「Claw 工作区」：校验必须早于建 space，否则目标账号会多出一条
/// 0 会话的空项目。这里刻意让目标账号**没有**同名 space（即会走新建分支），
/// 才能证明失败路径上确实什么都没落地。
#[test]
fn copy_rejection_leaves_no_orphan_space() {
    let env = Env::new("space-orphan");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, create_folder_path, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 'D:/ResumeForge', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', 'Claw 会话', 'D:/proj/claw', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let error = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap_err();
    assert!(error.contains("Claw"), "{error}");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "错误路径不得残留孤儿 spaces 行");
    let sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 1, "错误路径不得写入会话行");
}

/// 同一不变量的第二条路径：会话行插入失败（UUID 撞库）时，新建的 space 必须回滚。
///
/// 这是事务存在的意义——只把校验提前挡不住 INSERT 失败，必须整体回滚。
#[test]
fn copy_insert_failure_rolls_back_new_space() {
    let env = Env::new("space-rollback");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    // 目标账号先占住 'new-1' 这个 id：普通 INSERT 必然撞主键失败。
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('new-1', 'uid-b', '占位', 'D:/ResumeForge', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let error = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap_err();
    assert!(error.contains("会话记录保存失败"), "{error}");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "会话行写入失败时，新建的 space 必须随事务回滚");
    let sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 2, "原有两行会话不得被改动");
}

/// 新建 space 必须继承源 space 的 `create_folder_path`；源为 NULL 时保持 NULL。
///
/// 实测真实行：`primary_folder_path` 是 NULL、`create_folder_path` 有值
/// （如 `D:\ResumeForge`）。客户端靠后者展示/打开项目，漏写会让新项目显示异常。
#[test]
fn copy_new_space_inherits_create_folder_path() {
    // 源有值 → 新行沿用。
    let env = Env::new("space-folder");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, create_folder_path, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 'D:/ResumeForge', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    let (space_id, _) = space_of(&env, "new-1");
    let space_id = space_id.unwrap();
    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let folder: Option<String> = conn
        .query_row(
            "SELECT create_folder_path FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        folder.as_deref(),
        Some("D:/ResumeForge"),
        "新建 space 必须沿用源 space 的 create_folder_path"
    );

    // 源为 NULL → 新行也是 NULL（不得拿 cwd 硬猜）。
    let env2 = Env::new("space-folder-null");
    create_db_with_spaces(&env2);
    let conn = Connection::open(env2.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge/sub', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    insert_session_copy(
        &env2.paths(),
        &env2.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
    )
    .unwrap();
    let (space_id, _) = space_of(&env2, "new-1");
    let space_id = space_id.unwrap();
    let conn = Connection::open(env2.paths().workbuddy_db()).unwrap();
    let folder: Option<String> = conn
        .query_row(
            "SELECT create_folder_path FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(folder, None, "源没有文件夹时不得用会话 cwd 硬猜");
}

#[test]
fn backup_failure_is_propagated_instead_of_claimed_success() {
    let env = ready_env("backup-fail");
    std::fs::remove_file(env.paths.workbuddy_db()).unwrap();
    let err = backup_workbuddy_db(&env.paths(), &env.paths.backup_root()).unwrap_err();
    assert!(err.contains("会话数据不存在"), "{err}");

    // 正常备份返回主库路径且大小一致。
    env.create_db();
    env.add_session("sess-1", "uid-a", "标题一");
    let backup = backup_workbuddy_db(&env.paths(), &env.paths.backup_root()).unwrap();
    assert!(backup.ends_with("workbuddy.db"));
    assert_eq!(
        std::fs::metadata(&backup).unwrap().len(),
        std::fs::metadata(env.paths.workbuddy_db()).unwrap().len()
    );
}

#[test]
fn session_row_owner_reads_target_uid() {
    let env = ready_env("row-owner");
    assert_eq!(
        session_row_owner(&env.paths(), "sess-1").as_deref(),
        Some("uid-a")
    );
    assert_eq!(session_row_owner(&env.paths(), "missing"), None);
    env.delete_row("sess-1");
    assert_eq!(session_row_owner(&env.paths(), "sess-1"), None);
}

/// 遗留库（`sessions` 没有 `deleted_at` 列）的归属判定：
///
/// 硬套 `deleted_at IS NULL` 会让 `prepare` 失败 → 所有行都被判成「行缺失」，与存活索引
/// 「缺列按全部存活读」的结论分叉（组会列出但内容永远 unknown）。缺列时按未删除处理。
#[test]
fn session_row_owner_tolerates_a_legacy_table_without_deleted_at() {
    let env = Env::new("row-owner-legacy");
    std::fs::create_dir_all(env.paths.data_root.clone()).unwrap();
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title) VALUES ('sess-1', 'uid-a', '标题一')",
        [],
    )
    .unwrap();
    drop(conn);

    // 缺列 + 行存在：仍然读得到归属（而不是一律判缺失）。
    assert_eq!(
        session_row_owner(&env.paths(), "sess-1").as_deref(),
        Some("uid-a")
    );
    // 缺列 + 行不存在：仍然返回 None。
    assert_eq!(session_row_owner(&env.paths(), "missing"), None);
}

// ---------------------------------------------------------------------------
// 源正文容错：末行截断可修复，且修复动作必须报告给用户（R1 / issue #1）
// ---------------------------------------------------------------------------

/// 源会话末行被截断（进程被杀留下的半条记录）：复制路径容错成功，并把修复动作报告出来。
///
/// 严格校验（`read_content_snapshot` → `normalize_jsonl`）仍然拒绝截断内容——这一条
/// 与 `truncated_body_is_not_reported_as_success` 是同一个不变量的两面：判据不变，
/// 只是复制路径多了一步「末行截断且前面有完整记录」的可解释修复。
#[test]
fn truncated_tail_body_is_repaired_and_reported() {
    let env = ready_env("repair-tail");
    // 两条完整记录 + 一条半截记录（第 3 行）。
    let source = format!(
        "{}{}",
        body_text("sess-1"),
        "{\"type\":\"user\",\"sessionId\":\"sess-1\",\"text\":\"半"
    );
    env.add_body("sess-1", &source);
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path("sess-1"), "sess-1"),
            session_link::ContentState::Unavailable(_)
        ),
        "严格校验必须仍然判为不可验证"
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 3 行"), "修复说明必须指出行号：{note}");
    assert!(note.contains("已跳过"), "{note}");

    // 副本 = 修复后的正文：两条记录、新 id 引用、能通过严格读取。
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
    assert!(copied.contains(&new_id));
    assert!(!copied.contains("sess-1"), "{copied}");
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path(&new_id), &new_id),
            session_link::ContentState::Ready(snapshot) if snapshot.normalized.record_count == 2
        ),
        "副本必须能通过严格校验"
    );

    // 源文件一字不改（修复只在内存里做）。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source
    );
    assert_eq!(env.rows_for("uid-b").len(), 1);
}

/// 两个对象粘在同一行（上次退出没写换行）：无损拆分后复制成功，报告里说明「内容未丢失」。
#[test]
fn glued_records_are_split_and_reported() {
    let env = ready_env("repair-glued");
    let glued = format!(
        "{}{}",
        json!({"type": "user", "sessionId": "sess-1", "text": "你好"}),
        json!({"type": "assistant", "sessionId": "sess-1", "text": "hi"})
    );
    env.add_body("sess-1", &format!("{glued}\n"));

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 1 行"), "{note}");
    assert!(note.contains("拆分"), "{note}");
    assert!(!note.contains("已跳过"), "无损拆分不得报告丢记录：{note}");

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
}

/// 末行「完整记录 + 半截记录」：同行的完整记录必须保留（只丢末尾半截），
/// 报告要把「丢了半截」和「完整记录还在」两件事都说清楚，不能让用户以为丢了一条。
#[test]
fn truncated_tail_with_a_complete_record_keeps_that_record() {
    let env = ready_env("repair-tail-keep");
    let source = format!(
        "{}{}{}",
        body_text("sess-1"),
        json!({"type": "user", "sessionId": "sess-1", "text": "第三条"}),
        "{\"type\":\"assistant\",\"sessionId\":\"sess-1\",\"text\":\"半"
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 3 行"), "必须指出坏行行号：{note}");
    assert!(note.contains("已跳过"), "{note}");
    assert!(
        note.contains("同行的 1 条完整记录已保留"),
        "保留了记录就必须说明，不能只说丢了一条：{note}"
    );

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 3, "{copied}");
    assert!(
        copied.contains("第三条"),
        "同行已写完整的记录不得被丢掉：{copied}"
    );
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 3)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
    // 源文件一字不改。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source
    );
}

/// 没有做修复时报告里不出现 `sourceRepaired`（其它字段逐字不变）。
#[test]
fn clean_source_copy_has_no_source_repaired_field() {
    let env = ready_env("repair-clean");
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert!(
        report["copied"][0].get("sourceRepaired").is_none(),
        "{report}"
    );
}
