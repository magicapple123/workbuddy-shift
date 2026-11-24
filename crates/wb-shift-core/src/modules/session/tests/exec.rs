// ---------------------------------------------------------------------------
// 备份、写入与中断恢复（S6）
// ---------------------------------------------------------------------------

/// 目标会话行的可观测状态（用户 id、标题、自定义标题、更新时间、删除时间）。
type RowView = (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

fn try_session_row(env: &Env, cid: &str) -> Option<RowView> {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.query_row(
        "SELECT user_id, title, custom_title, updated_at, deleted_at FROM sessions WHERE id = ?1",
        [cid],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )
    .ok()
}

fn session_row(env: &Env, cid: &str) -> RowView {
    try_session_row(env, cid).expect("会话记录必须存在")
}

/// 改标题与自定义标题（模拟用户在目标账号改名；标题不是内容身份，不使预览失效）。
fn set_row_meta(env: &Env, cid: &str, title: &str, custom_title: Option<&str>) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET title = ?1, custom_title = ?2 WHERE id = ?3",
        rusqlite::params![title, custom_title, cid],
    )
    .unwrap();
}

fn group_snapshot(env: &Env, group_id: &str) -> LinkGroup {
    env.store()
        .groups
        .into_iter()
        .find(|group| group.id == group_id)
        .expect("同步关系必须存在")
}

fn pair_ref_of(group: &LinkGroup, left_uid: &str, right_uid: &str) -> String {
    session_link::find_pair_base(
        group,
        &member_id_of(group, left_uid),
        &member_id_of(group, right_uid),
    )
    .expect("成员对基线必须存在")
    .baseline_ref
    .clone()
}

/// 同步后目标正文的预期内容：来源正文 + 目标 sessionId。
fn incoming_text(env: &Env, source_id: &str, target_id: &str) -> String {
    std::fs::read_to_string(env.body_path(source_id))
        .unwrap()
        .replace(source_id, target_id)
}

// ---------------------------------------------------------------------------
// 同步写入时的「末条 incomplete」归一（来源已结束才归一）
// ---------------------------------------------------------------------------

/// 造一个「来源末条 assistant 消息停在 incomplete」的配对场景。
///
/// `source_status`：来源行状态（决定门控是否放行归一）；
/// `diverged`：给目标追加一条来源没有的记录，把判定推向 `diverge`（覆盖模式），
/// 否则来源只是目标的有序延长（`fastForward`，追加模式）。
/// 返回 (groupId, previewToken, targetId)。
fn incomplete_tail_sync_scene(
    env: &Env,
    source_status: &str,
    diverged: bool,
) -> (String, String, String) {
    // 来源状态必须在复制前设好：副本行会按「已结束即保留」的规则继承它。
    env.set_status("sess-1", source_status);
    let report = copy(env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 复制之后才追加：复制会归一末条，这里要的正是「配对之后来源又留下残缺」。
    let mut source_body = std::fs::read_to_string(env.body_path("sess-1")).unwrap();
    source_body.push_str(
        &json!({
            "type": "message",
            "role": "assistant",
            "status": "incomplete",
            "sessionId": "sess-1",
            "content": [{"type": "output_text", "text": "Interrupted by user"}]
        })
        .to_string(),
    );
    source_body.push('\n');
    std::fs::write(env.body_path("sess-1"), &source_body).unwrap();
    if diverged {
        append_records(&env.body_path(&target_id), &target_id, 100, 1);
    }
    let report = preview(env, "uid-b");
    let group = &report["groups"][0];
    assert_eq!(
        group["verdict"],
        if diverged { "diverge" } else { "fastForward" },
        "{report}"
    );
    (
        group["groupId"].as_str().unwrap().to_string(),
        group["previewToken"].as_str().unwrap().to_string(),
        target_id,
    )
}

/// 断言目标正文 == 「来源正文替换 id 后再归一末条」，且配对基线已同步到该内容。
fn assert_synced_body_normalized(env: &Env, target_id: &str) {
    let source_text = std::fs::read_to_string(env.body_path("sess-1")).unwrap();
    let target_text = std::fs::read_to_string(env.body_path(target_id)).unwrap();
    let source_lines = body_lines(&source_text);
    let target_lines = body_lines(&target_text);
    assert_eq!(
        target_lines.len(),
        source_lines.len(),
        "行数必须不变：{target_text}"
    );
    for (source_line, target_line) in source_lines
        .iter()
        .zip(target_lines.iter())
        .take(source_lines.len() - 1)
    {
        assert_eq!(
            *target_line,
            source_line.replace("sess-1", target_id),
            "末行之前的每一行都必须逐字等于来源行（仅换 id）"
        );
    }
    let last: Value = serde_json::from_str(target_lines.last().unwrap()).unwrap();
    assert_eq!(last["status"], json!("completed"), "末条必须归一：{last}");
    assert_eq!(
        last["_meta"]["incompleteNormalized"]["reason"],
        json!("sync normalize"),
        "留痕必须标明是同步这条路径归一的"
    );
    assert_eq!(
        last["content"],
        json!([{"type": "output_text", "text": "Interrupted by user"}]),
        "正文内容不得被改写"
    );

    // 配对基线必须落在写入后的内容上，否则下一轮预览会判成漂移。
    let baseline_ref = pair_ref_of(&env.store().groups[0], "uid-a", "uid-b");
    let baseline = session_link::load_baseline(&env.paths(), &baseline_ref).unwrap();
    match session_link::read_content_snapshot(&env.body_path(target_id), target_id) {
        ContentState::Ready(snapshot) => assert_eq!(
            baseline.total_digest, snapshot.normalized.total_digest,
            "基线摘要必须等于目标正文摘要"
        ),
        other => panic!("目标正文应可读，实际 {other:?}"),
    }
}

/// 覆盖（diverge）：来源已结束 + 末条 incomplete → 写入后目标末条 completed，基线同步。
#[test]
fn sync_overwrite_normalizes_incomplete_tail_when_source_settled() {
    let env = ready_env_with_status("sync-normalize-overwrite");
    let (group_id, token, target_id) = incomplete_tail_sync_scene(&env, "completed", true);

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::Overwrite)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_synced_body_normalized(&env, &target_id);
    // 来源正文一字不动（归一只作用于目标副本）。
    assert!(std::fs::read_to_string(env.body_path("sess-1"))
        .unwrap()
        .contains("\"status\":\"incomplete\""));
}

/// 追加（fastForward）：同样归一——否则「覆盖能修、追加又写回去」等于没修。
#[test]
fn sync_fast_forward_normalizes_incomplete_tail_when_source_settled() {
    let env = ready_env_with_status("sync-normalize-fastforward");
    let (group_id, token, target_id) = incomplete_tail_sync_scene(&env, "completed", false);

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_synced_body_normalized(&env, &target_id);
}

/// 门控：来源仍在运行（`working`）→ 末条 incomplete 是真实状态，按原字节搬运。
#[test]
fn sync_overwrite_keeps_incomplete_tail_when_source_running() {
    let env = ready_env_with_status("sync-normalize-gate");
    let (group_id, token, target_id) = incomplete_tail_sync_scene(&env, "working", true);

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::Overwrite)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let target_text = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert_eq!(
        target_text,
        incoming_text(&env, "sess-1", &target_id),
        "来源还在运行时不得归一（那是真实状态，改了就是篡改）"
    );
    assert!(target_text.contains("\"status\":\"incomplete\""));
    assert!(!target_text.contains("_meta"));
}

/// 让 `sessions` 的 UPDATE 在事务内失败（模拟数据库写入阶段中断）。
fn install_block_update_trigger(env: &Env) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER block_sync_update BEFORE UPDATE ON sessions
         BEGIN SELECT RAISE(ABORT, 'sessions 更新被测试拦截'); END;",
    )
    .unwrap();
}

fn drop_block_update_trigger(env: &Env) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch("DROP TRIGGER block_sync_update;")
        .unwrap();
}

fn body_bytes(env: &Env, cid: &str) -> Vec<u8> {
    std::fs::read(env.body_path(cid)).unwrap()
}

fn sync_operations(env: &Env) -> Vec<Operation> {
    session_link::scan_operations(&env.paths)
        .operations
        .into_iter()
        .filter(|operation| operation.kind == OPERATION_KIND_SYNC)
        .collect()
}

/// 成功路径：正文替换为来源内容，SID/标题/custom_title/归属保留，updated_at 更新，
/// A/B 基线推进、目标成员 lastSyncedAt 更新、不触碰映射库、备份可查看。
#[test]
fn sync_fast_forward_replaces_body_and_advances_pair_baseline() {
    let env = ready_env("sync-ff-write");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let source_body = std::fs::read_to_string(env.body_path("sess-1")).unwrap();
    let source_digest_before = full_digest_of(source_body.as_bytes());
    let target_body_before = body_bytes(&env, &target_id);
    // 目标账号已改过名：同步必须原样保留标题与自定义标题。
    set_row_meta(&env, &target_id, "改名后的标题", Some("自定义名"));
    let row_before = session_row(&env, &target_id);
    let pair_ref_before = pair_ref_of(&group_snapshot(&env, &group_id), "uid-a", "uid-b");
    let baselines_before = env.baseline_files();
    let edge_db_before = std::fs::read(env.paths.edge_sync_db(WbVariant::Cn)).unwrap();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
    assert!(report.get("needsRecovery").is_none(), "{report}");
    let synced = report["synced"].as_array().unwrap();
    assert_eq!(synced.len(), 1, "{report}");
    assert_eq!(synced[0]["status"], "synced");
    assert_eq!(synced[0]["groupId"], group_id);
    assert_eq!(synced[0]["mode"], "fastForward");
    assert_eq!(synced[0]["verdict"], "fastForward");
    assert_eq!(synced[0]["sourceSessionId"], "sess-1");
    assert_eq!(synced[0]["targetSessionId"], target_id);
    assert_eq!(synced[0]["recordCount"]["source"], 5);
    assert_eq!(synced[0]["recordCount"]["targetBefore"], 2);
    assert_eq!(synced[0]["recordCount"]["target"], 5);

    // 成功清理：不展示可还原路径，本次临时目录与维护记录都已回收。
    assert!(synced[0]["backup"].is_null(), "{report}");
    assert!(synced[0]["backupManifest"].is_null(), "{report}");
    assert_eq!(synced[0]["cleanupState"], "cleaned", "{report}");
    assert_ne!(body_bytes(&env, &target_id), target_body_before);
    let operations = sync_operations(&env);
    assert_eq!(operations.len(), 1);
    let operation_id = operations[0].operation_id.clone();
    assert!(
        !session_backup::transaction_dir(&env.paths, WbVariant::Cn, &operation_id)
            .unwrap()
            .exists(),
        "成功路径必须回收本次操作专属目录"
    );
    assert!(
        session_backup::scan_lifecycle(&env.paths)
            .records
            .is_empty(),
        "成功路径不得残留维护记录"
    );
    assert_eq!(
        operations[0].cleanup_state.as_deref(),
        Some(session_backup::CLEANUP_STATE_CLEANED),
        "业务日志标注备份已清理"
    );
    assert!(
        operations[0].backup.is_none(),
        "已清理的操作不再展示备份位置"
    );

    // 目标正文被替换为来源内容（本副本 sessionId 换成目标 id）；来源正文不动。
    let expected = incoming_text(&env, "sess-1", &target_id);
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(
        full_digest_of(std::fs::read(env.body_path("sess-1")).unwrap().as_slice()),
        source_digest_before
    );

    // 数据库：只更新 updated_at，SID/归属/标题/custom_title 原样保留。
    let row_after = session_row(&env, &target_id);
    assert_eq!(row_after.0, row_before.0);
    assert_eq!(row_after.1, row_before.1);
    assert_eq!(row_after.2, row_before.2);
    assert_eq!(row_after.3, Some(synced[0]["updatedAt"].as_i64().unwrap()));
    assert!(row_after.3.unwrap() > row_before.3.unwrap());
    assert_eq!(row_after.4, None);

    // 组表：A/B 基线推进到本次写入内容，目标成员 lastSyncedAt 更新。
    let incoming_normalized =
        session_link::normalize_jsonl(&expected, &target_id).expect("内容可归一化");
    let group = group_snapshot(&env, &group_id);
    let pair_ref_after = pair_ref_of(&group, "uid-a", "uid-b");
    assert_ne!(pair_ref_after, pair_ref_before);
    assert_eq!(env.baseline_files(), baselines_before + 1);
    let baseline = session_link::load_baseline(&env.paths, &pair_ref_after).unwrap();
    assert_eq!(baseline.total_digest, incoming_normalized.total_digest);
    assert_eq!(baseline.record_count, 5);
    let last_synced = session_link::active_member_for(&group, "uid-b")
        .unwrap()
        .last_synced_at
        .expect("目标成员必须记录本次同步时间");
    assert!(last_synced >= row_after.3.unwrap());
    assert!(session_link::active_member_for(&group, "uid-a")
        .unwrap()
        .last_synced_at
        .is_none());

    // 不修改 edge-sync-mapping 库；不产生第二个副本；操作日志只有一条且已完成。
    assert_eq!(
        std::fs::read(env.paths.edge_sync_db(WbVariant::Cn)).unwrap(),
        edge_db_before
    );
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(operations[0].phase, OpPhase::Completed);
    assert_eq!(operations[0].target.session_id, target_id);
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
}

/// 故障状态下的备份必须保持完整可核验：清单、一致性快照、覆盖前/待写入正文与摘要
/// 齐全，位于本版专属临时目录根，且维护记录仍能追踪（design §8：安全断言保留在
/// 准备/故障阶段验证，而不是在成功路径上）。
#[test]
fn pending_sync_backup_stays_verifiable_until_recovery() {
    let env = ready_env("sync-backup-verify-on-failure");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    set_row_meta(&env, &target_id, "改名后的标题", Some("自定义名"));
    let row_before = session_row(&env, &target_id);
    let target_body_before = body_bytes(&env, &target_id);

    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["needsRecovery"], true, "{report}");
    drop_block_update_trigger(&env);

    let operation = session_link::pending_operations(&env.paths, WbVariant::Cn)
        .into_iter()
        .next()
        .expect("故障后必须保留未完成操作");
    let manifest_file = PathBuf::from(operation.backup.clone().expect("必须记录备份位置"));
    let backup_dir = manifest_file.parent().unwrap().to_path_buf();
    assert_eq!(
        manifest_file,
        backup_dir.join("manifest.json"),
        "清单是恢复的唯一依据"
    );
    let manifest = load_sync_manifest(&backup_dir).expect("备份清单必须可读");
    assert_eq!(
        backup_dir.file_name().unwrap().to_string_lossy(),
        manifest.operation_id,
        "备份目录按 operationId 唯一"
    );
    assert_eq!(manifest.group_id, group_id);
    assert_eq!(manifest.variant, WbVariant::Cn);
    assert_eq!(manifest.mode, SyncMode::FastForward);
    assert_eq!(manifest.verdict, SyncVerdict::FastForward);
    assert_eq!(manifest.source.uid, "uid-a");
    assert_eq!(manifest.source.session_id, "sess-1");
    assert_eq!(manifest.source.body_file, None);
    assert_eq!(manifest.target.uid, "uid-b");
    assert_eq!(manifest.target.session_id, target_id);
    assert_eq!(
        manifest.target_body_path,
        env.body_path(&target_id).to_string_lossy()
    );
    assert!(backup_dir.join(&manifest.db.snapshot_file).is_file());
    assert_eq!(manifest.db.method, DB_SNAPSHOT_METHOD);
    let before_row = manifest.db.target_row.as_ref().expect("必须记录目标行");
    assert_eq!(before_row.session_id, target_id);
    assert_eq!(before_row.user_id, "uid-b");
    assert_eq!(before_row.title.as_deref(), Some("改名后的标题"));
    assert_eq!(before_row.custom_title.as_deref(), Some("自定义名"));
    assert_eq!(before_row.updated_at, row_before.3);
    let original_backup = backup_dir.join(manifest.target.body_file.clone().unwrap());
    assert_eq!(std::fs::read(&original_backup).unwrap(), target_body_before);
    assert_eq!(
        full_digest_of(&std::fs::read(&original_backup).unwrap()),
        manifest.target.body_raw_digest
    );
    assert_eq!(manifest.target.record_count, 2);
    assert_eq!(manifest.incoming.record_count, 5);
    assert!(manifest.restore_steps.len() >= 4);
    verify_sync_backup(&backup_dir, &manifest).expect("备份必须可核验");
    // 备份位于本版专属临时目录根，维护记录仍能追踪同一 operationId。
    assert!(backup_dir.starts_with(
        env.paths
            .backup_root()
            .join(session_backup::TRANSACTIONS_DIR_NAME)
    ));
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .iter()
        .any(|record| record.operation_id == manifest.operation_id));
}

/// 显式覆盖：目标全文被替换为来源内容（目标独有记录不再保留），仍然保留 SID/标题。
#[test]
fn sync_overwrite_replaces_target_full_text() {
    let env = ready_env("sync-overwrite");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 双方都变化：来源追加 2 条，目标也追加 3 条 → diverge。
    append_records(&env.body_path("sess-1"), "sess-1", 0, 2);
    append_records(&env.body_path(&target_id), &target_id, 10, 3);
    set_row_meta(&env, &target_id, "目标标题", Some("目标自定义名"));
    let row_before = session_row(&env, &target_id);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "diverge", "{preview}");
    assert_eq!(group["defaultChecked"], false);
    assert_eq!(group["availableModes"], json!(["overwrite"]));
    assert_eq!(group["extraB"], 3);
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::Overwrite)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    let expected = incoming_text(&env, "sess-1", &target_id);
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected,
        "覆盖必须替换目标全文"
    );
    let row_after = session_row(&env, &target_id);
    assert_eq!(row_after.1.as_deref(), Some("目标标题"));
    assert_eq!(row_after.2.as_deref(), Some("目标自定义名"));
    assert_ne!(row_after.3, row_before.3);
}

/// 双方一致（identical）不写正文：既不发凭据，强行执行也会被拒绝。
#[test]
fn sync_identical_verdict_never_writes_body() {
    let env = ready_env("sync-identical");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let target_before = body_bytes(&env, &target_id);
    let revision_before = env.store().revision;

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview}");
    assert_eq!(group["defaultChecked"], false);
    assert_eq!(group["availableModes"], json!([]));
    assert!(group.get("previewToken").is_none(), "不可执行的组不发凭据");
    let group_id = group["groupId"].as_str().unwrap().to_string();

    // 伪造一份 identical 的服务端凭据强行覆盖：判定不允许该模式 → 拒绝，不写正文。
    let paths = env.paths();
    let store = env.store();
    let g = store
        .groups
        .iter()
        .find(|candidate| candidate.id == group_id)
        .unwrap();
    let source_member = session_link::active_member_for(g, "uid-a").unwrap();
    let target_member = session_link::active_member_for(g, "uid-b").unwrap();
    let source_content = member_content_state(&paths, &source_member.session_id);
    let target_content = member_content_state(&paths, &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        &paths,
        g,
        &source_member.member_id,
        &target_member.member_id,
    );
    assert_eq!(
        session_link::decide_sync(&source_content, &target_content, &baseline).verdict,
        SyncVerdict::Identical
    );
    let forged = session_link::save_preview_token(
        &paths,
        live_preview_binding(
            g,
            source_member,
            target_member,
            &source_content,
            &target_content,
            &baseline,
            SyncVerdict::Identical,
            &PreviewArchiveState::default(),
        ),
    )
    .unwrap();
    for mode in [SyncMode::Overwrite, SyncMode::FastForward] {
        let report = sync(&env, "uid-b", &[selection(&group_id, &forged, mode)]);
        assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
        assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
        assert!(
            report["errors"][0]["error"]
                .as_str()
                .unwrap()
                .contains("不允许以"),
            "{report}"
        );
    }
    assert_eq!(body_bytes(&env, &target_id), target_before);
    assert_eq!(env.store().revision, revision_before);
    assert_eq!(pair_snapshot(&env).len(), 1, "基线没有被推进");
}

/// 备份失败 → 零覆盖：正文、数据库、基线、关联版本都没有变化，也不留未完成操作。
#[test]
fn sync_backup_failure_leaves_target_and_store_untouched() {
    let env = ready_env("sync-backup-fail");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let target_before = body_bytes(&env, &target_id);
    let row_before = session_row(&env, &target_id);
    let revision_before = env.store().revision;
    let baselines_before = env.baseline_files();
    let pairs_before = pair_snapshot(&env);

    // 临时目录根被占成普通文件 → 操作专属目录创建失败。
    std::fs::create_dir_all(env.paths.backup_root()).unwrap();
    let transactions_root = env
        .paths
        .backup_root()
        .join(session_backup::TRANSACTIONS_DIR_NAME);
    let _ = std::fs::remove_dir_all(&transactions_root);
    std::fs::write(&transactions_root, b"occupied").unwrap();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("临时目录创建失败"), "{error}");

    // 零写入：正文、数据库行、基线、关联版本原样保留。
    assert_eq!(body_bytes(&env, &target_id), target_before);
    assert_eq!(session_row(&env, &target_id), row_before);
    assert_eq!(env.store().revision, revision_before);
    assert_eq!(env.baseline_files(), baselines_before);
    assert_eq!(pair_snapshot(&env), pairs_before);
    // 备份没成功就什么都不能留下：没有未完成操作，也不需要恢复。
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
    assert!(sync_operations(&env).is_empty());
    assert!(report.get("needsRecovery").is_none(), "{report}");
}

/// 正文写入阶段中断：不报成功、保留未完成操作、恢复后补完且不产生第二份。
#[cfg(unix)]
#[test]
fn sync_body_write_failure_keeps_pending_then_recovers() {
    use std::os::unix::fs::PermissionsExt;

    let env = ready_env("sync-body-fail");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let row_before = session_row(&env, &target_id);
    let target_before = body_bytes(&env, &target_id);

    let ws = env.paths.projects_dir().join("ws-a");
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::write(ws.join(".probe"), b"x").is_ok();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        // root / 特殊 ACL 环境：写保护无效，跳过断言。
        return;
    }
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("同步内容保存失败"), "{error}");
    assert_eq!(report["needsRecovery"], true);

    // 正文与数据库都没变；备份已生成并留下未完成操作（阶段停在 Prepared）。
    assert_eq!(body_bytes(&env, &target_id), target_before);
    assert_eq!(session_row(&env, &target_id), row_before);
    let pending = session_link::pending_operations(&env.paths, WbVariant::Cn);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].phase, OpPhase::Prepared);
    let operation_id = pending[0].operation_id.clone();
    let backup_dir = PathBuf::from(pending[0].backup.clone().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = load_sync_manifest(&backup_dir).expect("备份清单必须可读");

    // 恢复：按同一份清单补完，复用同一个目标 UUID 与新基线引用。
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.recovered, vec![operation_id.clone()]);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    let expected = incoming_text(&env, "sess-1", &target_id);
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_ne!(session_row(&env, &target_id).3, row_before.3);
    assert_eq!(env.body_files().len(), 2, "恢复不得产生第二份副本");
    assert_eq!(
        Some(pair_ref_of(
            &group_snapshot(&env, &group_id),
            "uid-a",
            "uid-b"
        )),
        manifest.new_baseline_ref,
        "恢复复用同一个新基线引用"
    );
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
    let operations = sync_operations(&env);
    assert_eq!(operations.len(), 1, "不得新增第二条操作记录");
    assert_eq!(operations[0].operation_id, operation_id);
    assert_eq!(operations[0].phase, OpPhase::Completed);
    assert_eq!(operations[0].target.session_id, target_id);
}

/// 数据库更新阶段中断：不报成功；解除故障后恢复补完，不产生第二份写入。
#[test]
fn sync_db_update_failure_is_not_reported_as_success_then_recovers() {
    let env = ready_env("sync-db-fail");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let row_before = session_row(&env, &target_id);
    let expected = incoming_text(&env, "sess-1", &target_id);
    let baselines_before = env.baseline_files();
    let pairs_before = pair_snapshot(&env);

    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("目标会话记录更新失败"), "{error}");
    assert_eq!(report["needsRecovery"], true);

    // 正文已写入，但数据库未更新：绝不报成功；组表也未提交。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(session_row(&env, &target_id), row_before);
    assert_eq!(env.baseline_files(), baselines_before);
    assert_eq!(pair_snapshot(&env), pairs_before);
    let pending = session_link::pending_operations(&env.paths, WbVariant::Cn);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].phase, OpPhase::BodyWritten);
    let manifest_dir = PathBuf::from(pending[0].backup.clone().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = load_sync_manifest(&manifest_dir).unwrap();

    // 同一组再次同步（故障未解除）：预览已过期，不得写入，也不得新建第二份。
    let again = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(again["synced"].as_array().unwrap().is_empty(), "{again}");
    assert!(again.get("needsRecovery").is_some(), "{again}");
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(env.body_files().len(), 2);

    // 历史同步恢复仍失败时也必须阻断启动，不能仅依赖本轮新增 pending 差集。
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(recovery.recovered.is_empty());
    assert!(recovery.abandoned.is_empty());
    assert!(crate::modules::switch::recovery_blocks_startup(&recovery));
    assert_eq!(sync_operations(&env)[0].phase, OpPhase::BodyWritten);
    assert_eq!(session_row(&env, &target_id), row_before);
    assert_eq!(pair_snapshot(&env), pairs_before);

    // 解除故障后恢复：补完数据库与组表，仍复用同一份清单与同一条操作记录。
    drop_block_update_trigger(&env);
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery.needs_recovery);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(
        session_row(&env, &target_id).3,
        Some(manifest.db.new_updated_at)
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.baseline_files(), baselines_before + 1);
    let group = group_snapshot(&env, &group_id);
    assert_eq!(
        Some(pair_ref_of(&group, "uid-a", "uid-b")),
        manifest.new_baseline_ref
    );
    assert_eq!(
        session_link::active_member_for(&group, "uid-b")
            .unwrap()
            .last_synced_at,
        Some(manifest.last_synced_at)
    );
    let operations = sync_operations(&env);
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].phase, OpPhase::Completed);
    assert_eq!(operations[0].target.session_id, target_id);
}

/// 组表提交阶段中断：正文与数据库已写、不报成功；恢复只补组表，不重放已完成阶段。
#[test]
fn sync_link_commit_failure_keeps_pending_then_recovers() {
    let env = ready_env("sync-link-fail");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let row_before = session_row(&env, &target_id);
    let expected = incoming_text(&env, "sess-1", &target_id);
    let baselines_before = env.baseline_files();
    let revision_before = env.store().revision;

    // 关联存储锁被占用：只有组表提交这一步会失败。
    let held = session_link::try_lock_file(&env.paths.link_store_lock_file()).unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    drop(held);

    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["needsRecovery"], true);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("同步记录"), "{error}");

    // 正文与数据库都已写入，但组表未提交 → 阶段停在 DbWritten。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_ne!(session_row(&env, &target_id).3, row_before.3);
    assert_eq!(
        env.baseline_files(),
        baselines_before,
        "组表未提交，基线文件也未落盘"
    );
    assert_eq!(env.store().revision, revision_before, "组表未写");
    let pending = session_link::pending_operations(&env.paths, WbVariant::Cn);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].phase, OpPhase::DbWritten);
    let manifest_dir = PathBuf::from(pending[0].backup.clone().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = load_sync_manifest(&manifest_dir).unwrap();

    // 恢复：只补组表（复用清单里的新基线引用），不重写正文、不重复登记第二份。
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery.needs_recovery);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(env.store().revision, revision_before + 1, "组表只提交一次");
    assert_eq!(env.baseline_files(), baselines_before + 1, "不新增重复基线");
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(
        session_row(&env, &target_id).3,
        Some(manifest.db.new_updated_at)
    );
    let group = group_snapshot(&env, &group_id);
    assert_eq!(
        Some(pair_ref_of(&group, "uid-a", "uid-b")),
        manifest.new_baseline_ref
    );
    assert_eq!(
        session_link::active_member_for(&group, "uid-b")
            .unwrap()
            .last_synced_at,
        Some(manifest.last_synced_at)
    );
    let operations = sync_operations(&env);
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].phase, OpPhase::Completed);
    assert_eq!(env.body_files().len(), 2);
}

// ---------------------------------------------------------------------------
// 同步时的「最后活跃时间」（`sessions.last_activity_at`）
// ---------------------------------------------------------------------------

/// 复制与同步共用的口径 `advance_activity_at`：取较大值，`None` 表示「没有候选」。
///
/// 四个分支里有两个（`(None, Some)` / `(None, None)`）只有「目标行是 NULL」的现场才
/// 走得到，而那种现场在端到端用例里只能间接观察；这里把口径本身钉住，避免有人把
/// `None` 当成 0（1970）参与比较——那会把 NULL 写成 0，侧栏显示成 1970 年。
#[test]
fn advance_activity_at_takes_the_larger_candidate() {
    // 两边都有：取较大值（只前进不回退），与大小顺序无关。
    assert_eq!(advance_activity_at(Some(10), Some(20)), Some(20));
    assert_eq!(advance_activity_at(Some(20), Some(10)), Some(20));
    assert_eq!(advance_activity_at(Some(20), Some(20)), Some(20));
    // 只有一侧有值：取那一侧，缺失的一侧不参与比较（None ≠ 0）。
    assert_eq!(advance_activity_at(None, Some(20)), Some(20));
    assert_eq!(advance_activity_at(Some(20), None), Some(20));
    assert_eq!(
        advance_activity_at(None, Some(0)),
        Some(0),
        "0 是合法毫秒值，不能因为「假值」被当成缺失"
    );
    // 两边都没有：仍是 None（调用方据此不写这一列，而不是写 0）。
    assert_eq!(advance_activity_at(None, None), None);
}

/// 往正文末尾追加一条带毫秒时间戳的记录（模拟来源账号继续对话）。
fn append_record_with_timestamp(path: &Path, cid: &str, timestamp: i64) {
    let mut text = std::fs::read_to_string(path).unwrap();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(
        &json!({
            "type": "message",
            "role": "assistant",
            "sessionId": cid,
            "timestamp": timestamp
        })
        .to_string(),
    );
    text.push('\n');
    std::fs::write(path, &text).unwrap();
}

/// 目标行比正文旧 → 同步把活跃时间推进到本次正文的最新记录。
#[test]
fn sync_advances_activity_to_incoming_body_timestamp() {
    let env = ready_env_with_status("sync-activity-forward");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 目标行停在很早以前（客户端一直没维护这一列的真实形态）。
    env.set_activity(&target_id, 1_000);
    let newest = 1_700_000_000_000;
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", newest);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&target_id),
        Some(newest),
        "同步必须把活跃时间推进到本次正文的最新记录"
    );
}

/// 目标行比正文新（目标侧刚用过）→ 同步不得把它改小：活跃时间只前进不回退。
#[test]
fn sync_never_lowers_existing_target_activity() {
    let env = ready_env_with_status("sync-activity-keep");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let target_newer = 1_700_000_900_000;
    env.set_activity(&target_id, target_newer);
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", 1_700_000_000_000);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&target_id),
        Some(target_newer),
        "目标侧更新时必须保留原值，不得回退"
    );
}

/// 老库没有 `last_activity_at` 列 → 同步照常写 `updated_at`，不因缺列报错。
#[test]
fn sync_without_activity_column_still_updates_row() {
    let env = ready_env_with_status("sync-activity-legacy");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let updated_before = session_row(&env, &target_id).3;

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let updated_after = session_row(&env, &target_id).3;
    assert_eq!(
        updated_after,
        Some(report["synced"][0]["updatedAt"].as_i64().unwrap()),
        "缺列时 updated_at 必须照常写入"
    );
    assert_ne!(updated_after, updated_before, "本次保存必须推进 updated_at");
}

/// 仅同步归档（正文零写入）→ 活跃时间保持现值。
///
/// 目标行的活跃时间刻意设得比正文里的时间戳更早：只要这条路径去「按正文重算」，
/// 值就会被推大，本用例就会失败。
#[test]
fn status_only_sync_keeps_activity_untouched() {
    let env = ready_env_with_status("status-activity-untouched");
    env.add_activity_column();
    // 正文在复制前就带时间戳：副本与配对基线都落在它上面，判定仍是 identical。
    env.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_NEWEST]),
    );
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    env.set_activity(&target_id, ACTIVITY_SOURCE_STALE);

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_SOURCE_STALE),
        "只归档不写正文，活跃时间必须保持现值（不得按正文重算）"
    );
}

/// 旧清单（没有 `latestActivityAt` 字段）→ 恢复照旧成功，只是不写活跃时间。
///
/// 升级后重放上一版本留下的未完成操作必须能补完；缺字段被当成错误就等于把用户
/// 卡在「每次启动都提示恢复失败」。
#[test]
fn recovery_with_old_manifest_without_activity_field_still_succeeds() {
    let env = ready_env_with_status("sync-activity-old-manifest");
    env.add_activity_column();
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let kept = 1_700_000_900_000;
    env.set_activity(&target_id, kept);

    // 数据库更新阶段中断：正文已写、清单已落盘、行未写 → 留下未完成操作。
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["needsRecovery"], true, "{report}");
    drop_block_update_trigger(&env);

    // 抹掉清单里的新字段，模拟「上一版本写下的清单」。
    let pending = session_link::pending_operations(&env.paths, WbVariant::Cn);
    let manifest_file = PathBuf::from(pending[0].backup.clone().expect("必须记录备份位置"));
    let mut manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_file).expect("清单必须可读"),
    )
    .expect("清单必须是合法 JSON");
    let removed = manifest["incoming"]
        .as_object_mut()
        .expect("incoming 必须是对象")
        .remove("latestActivityAt");
    assert!(removed.is_some(), "本用例前提：清单里确实有该字段");
    std::fs::write(
        &manifest_file,
        serde_json::to_string(&manifest).expect("清单必须可序列化"),
    )
    .unwrap();
    let new_updated_at = manifest["db"]["newUpdatedAt"].as_i64().unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery.needs_recovery);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(
        session_row(&env, &target_id).3,
        Some(new_updated_at),
        "缺字段只影响活跃时间，updated_at 照常写入"
    );
    assert_eq!(
        env.activity_of(&target_id),
        Some(kept),
        "旧清单缺字段时必须跳过写该列、保持现值"
    );
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
}

/// 动态 SET 子句的组合矩阵：`last_activity_at` 列存在与否 × 本次是否写状态 × 本次
/// 是否带活跃时间，四种写入组合都要「参数与占位符逐一对应」，未参与本次写入的列
/// 不得被改成 NULL。
///
/// 直接调用写库函数而不是走整条同步链路：这条 SQL 是按列存在性拼出来的，绑定顺序
/// 错了不会报错，只会静默串位置（状态写成时间戳、时间写成状态文本），必须逐组合
/// 用「两个可选列同时参与」的这一档把它钉住。
#[test]
fn update_target_session_row_covers_all_column_combinations() {
    let incoming = 1_700_000_009_000;
    let stale = 1_600_000_000_000;
    let new_updated_at = 1_800_000_000_000;
    for with_column in [false, true] {
        for with_status in [false, true] {
            for with_activity in [false, true] {
                let case =
                    format!("列={with_column} 状态={with_status} 活跃时间={with_activity}");
                let env = Env::new(&format!(
                    "write-matrix-{with_column}-{with_status}-{with_activity}"
                ));
                env.create_db_with_status();
                env.set_login("uid-b");
                env.add_session("sess-9", "uid-b", "标题九");
                env.set_status("sess-9", "completed");
                if with_column {
                    env.add_activity_column();
                    env.set_activity("sess-9", stale);
                }
                let target = session_link::OperationMember {
                    account_id: None,
                    uid: "uid-b".to_string(),
                    session_id: "sess-9".to_string(),
                };
                let change = with_status.then_some(StatusChange {
                    before: "completed".to_string(),
                    after: "archived",
                });

                let result = update_target_session_row(
                    &env.paths(),
                    &target,
                    new_updated_at,
                    None,
                    change.as_ref(),
                    with_activity.then_some(incoming),
                );

                assert!(result.is_ok(), "{case}：写入失败 {result:?}");
                assert_eq!(
                    session_row(&env, "sess-9").3,
                    Some(new_updated_at),
                    "{case}：updated_at 必须恒写"
                );
                assert_eq!(
                    env.status_of("sess-9").as_deref(),
                    Some(if with_status { "archived" } else { "completed" }),
                    "{case}：status 只应在本次要求写时才变（未写的列不得被清空）"
                );
                let expected_activity = if with_column {
                    // 给了时间就取 max（只前进不回退），没给就保持现值。
                    Some(if with_activity {
                        incoming.max(stale)
                    } else {
                        stale
                    })
                } else {
                    // 老库没有这一列：不写、不报错，列本身也不该被创建。
                    None
                };
                assert_eq!(
                    env.activity_of("sess-9"),
                    expected_activity,
                    "{case}：活跃时间取值与写入条件不符"
                );
            }
        }
    }
}

/// 正文模式的归档副作用：`status` 与 `last_activity_at` 在同一条 UPDATE 里写，
/// 两个可选列同时参与时不得串位置。
#[test]
fn sync_writes_activity_and_status_in_one_update() {
    let env = ready_env_with_status("sync-activity-with-status");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 来源追加一条带时间戳的记录（判定变快进），来源已归档、目标仍在终态白名单。
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", ACTIVITY_BODY_NEWEST);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, "completed");
    env.set_activity(&target_id, ACTIVITY_SOURCE_STALE);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    assert_eq!(
        group["archiveAction"], "statusOnly",
        "本用例前提：本次要随正文归档（两个可选列同时参与 SET）{preview}"
    );
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("archived"),
        "状态照旧写入"
    );
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST),
        "活跃时间必须单独落到时间列，不与状态串位置"
    );
}

/// 目标行的活跃时间是 NULL（客户端从未写过这一列）→ 首次写入本次正文的最新时间。
///
/// 反面断言同样重要：不得为了「有个值」而写 0（1970）或干脆留 NULL。
#[test]
fn sync_first_writes_activity_when_target_is_null() {
    let env = ready_env_with_status("sync-activity-null");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.clear_activity(&target_id);
    assert_eq!(env.activity_of(&target_id), None, "本用例前提：目标行是 NULL");
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", ACTIVITY_BODY_NEWEST);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST),
        "NULL 的目标行必须写入正文时间，不能写 0、也不能继续留 NULL"
    );
}

/// 恢复重放：本次时间戳来自清单里那份已核验的待写入正文，不重新读目标文件。
///
/// 覆盖「数据库阶段中断 → 恢复补完」：断点处目标正文已是本次内容、行还未写库，
/// 恢复必须把活跃时间按清单补上；再跑一次恢复不得重复推进、不得产生第二份副本。
#[test]
fn sync_recovery_advances_activity_from_manifest() {
    let env = ready_env_with_status("sync-activity-replay");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_activity(&target_id, ACTIVITY_SOURCE_STALE);
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", ACTIVITY_BODY_NEWEST);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["needsRecovery"], true, "{report}");
    drop_block_update_trigger(&env);
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_SOURCE_STALE),
        "写库阶段失败时活跃时间不得被改"
    );

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery.needs_recovery);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST),
        "恢复必须按清单里的待写入正文时间补写活跃时间"
    );

    let again = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(again.is_clean(), "{:?}", again.needs_recovery);
    assert!(again.recovered.is_empty(), "已完成的操不得被再重放一次");
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST),
        "重放幂等：值不变、不报错"
    );
    assert_eq!(env.body_files().len(), 2, "重放不得产生第二份副本");
}

/// 正文带来的时间戳远在未来（来源机时钟超前，或正文里混进一条未来时间的记录）→
/// 写入值必须夹到本次写入时间戳，不能停在未来。
///
/// 为什么必须夹：`max(现值, 候选)` 语义会让未来值**永久粘住**——之后每次同步的候选都
/// 比不过它，这条会话会一直顶在列表最前面，用户既改不回来也看不见原因。
///
/// 为什么夹的是候选而不是最终结果：目标侧**已有**的更大值必须原样保留（只前进不回退），
/// 否则「上一次夹出来的值 / 第三方写进去的值」会在下一次同步时被改小。
#[test]
fn sync_clamps_future_incoming_activity_to_this_write() {
    let env = ready_env_with_status("sync-activity-future");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_activity(&target_id, ACTIVITY_SOURCE_STALE);
    // 「一天以后」的记录：本次同步取的 now_ms() 一定早于它，夹取才有可观测的效果。
    let future = now_ms() + 86_400_000;
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", future);

    let first_preview = preview(&env, "uid-b");
    let group = &first_preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{first_preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let new_updated_at = report["synced"][0]["updatedAt"]
        .as_i64()
        .expect("同步结果必须带本次写入时间戳");
    assert!(
        new_updated_at < future,
        "本用例前提：本次写入时间戳必须早于正文里的未来时间戳"
    );
    assert_eq!(
        env.activity_of(&target_id),
        Some(new_updated_at),
        "超出本次写入时间的候选必须被夹到本次写入时间戳，不能落到未来"
    );

    // 再同步一次：候选仍是未来值，现值已是上一轮夹出来的值。既不得报错，也不得回退。
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", future + 1_000);
    let second_preview = preview(&env, "uid-b");
    let group = &second_preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{second_preview}");
    let token = group["previewToken"].as_str().unwrap().to_string();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(
        report.get("needsRecovery").is_none(),
        "第二轮不得留下未完成操作：{report}"
    );
    assert_eq!(report["errors"].as_array().unwrap().len(), 0, "{report}");
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let second_updated_at = report["synced"][0]["updatedAt"].as_i64().unwrap();
    let after = env
        .activity_of(&target_id)
        .expect("第二轮同步后活跃时间不得变回 NULL");
    assert!(
        after >= new_updated_at,
        "第二轮同步不得把现值改小：{after} < {new_updated_at}"
    );
    assert!(
        after <= second_updated_at,
        "第二轮同步同样不得把活跃时间写到未来：{after} > {second_updated_at}"
    );
}

/// 目标侧**已有**的活跃时间就在未来 → 本次同步必须原样保留，不得顺手夹回本次写入时间。
///
/// 夹取的对象只能是「本次候选」：`advance_activity_at(现值, clamp(候选))`。若图省事改成
/// 夹最终结果（`现值.max(候选).min(new_updated_at)`），现值一旦大于本次写入时间（上一轮
/// 同步夹出来的值 / 第三方写入 / 用户机器时钟回拨）就会被**改小**，用户看到「最近的对
/// 话没发生过」——那正是「只前进不回退」要防的回退。上一条用例的第二轮里现值恰好小于本
/// 次写入时间，钉不住这条分支，故单列一例。
#[test]
fn sync_keeps_existing_future_target_activity_untouched() {
    let env = ready_env_with_status("sync-activity-existing-future");
    env.add_activity_column();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 现值本身就在未来，且必然大于本次同步取到的 now_ms()。
    let existing_future = now_ms() + 86_400_000;
    env.set_activity(&target_id, existing_future);
    // 候选落在正常区间（早于本次写入时间）：夹取对它是恒等变换，本用例只考察现值。
    append_record_with_timestamp(&env.body_path("sess-1"), "sess-1", ACTIVITY_BODY_NEWEST);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let new_updated_at = report["synced"][0]["updatedAt"]
        .as_i64()
        .expect("同步结果必须带本次写入时间戳");
    // 反假绿：先证明本次真的写了目标行，否则「值没变」可能只是根本没同步。
    assert_eq!(
        session_row(&env, &target_id).3,
        Some(new_updated_at),
        "本次同步必须真的写过目标行：{report}"
    );
    assert!(
        existing_future > new_updated_at,
        "本用例前提：目标侧现值必须大于本次写入时间"
    );
    assert_eq!(
        env.activity_of(&target_id),
        Some(existing_future),
        "目标侧已有的更大值必须原样保留：夹的是本次候选，不是最终结果"
    );
}
