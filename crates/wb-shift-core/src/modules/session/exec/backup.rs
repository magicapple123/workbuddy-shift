//! 同步备份：SQLite 一致性快照、目标正文逐文件备份、恢复清单与核验。
use super::*;

/// 同步备份清单格式版本；读到其它版本一律视为不可用。
pub const SYNC_BACKUP_VERSION: u32 = 1;

/// 数据库快照方法（清单里如实记录，恢复方据此选择恢复方法）。
pub(in crate::modules::session) const DB_SNAPSHOT_METHOD: &str = "sqliteBackupApi";

/// 数据库快照的分页步长与超时：App 已关闭，超时说明库被其它进程占用。
pub(in crate::modules::session) const DB_BACKUP_PAGES_PER_STEP: i32 = 1024;

pub(in crate::modules::session) const DB_BACKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次同步的备份目录：`backups/session-transactions/{variant}/{operationId}`。
///
/// 身份由调用方（生命周期记录）预分配，目录用 `create_dir` 拒绝复用。
pub(in crate::modules::session) fn sync_backup_dir(
    paths: &SessionPaths,
    variant: WbVariant,
    operation_id: &str,
) -> Result<PathBuf, String> {
    session_backup::transaction_dir(paths, variant, operation_id)
}

pub(in crate::modules::session) fn sync_manifest_file(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

/// 清单里记录的单个成员：身份、覆盖前正文摘要与备份位置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::modules::session) struct SyncBackupMember {
    pub(in crate::modules::session) member_id: String,
    pub(in crate::modules::session) uid: String,
    pub(in crate::modules::session) session_id: String,
    /// 覆盖前正文的备份位置（相对备份目录）；来源侧不写目标，为 None。
    pub(in crate::modules::session) body_file: Option<String>,
    pub(in crate::modules::session) body_raw_digest: String,
    pub(in crate::modules::session) body_normalized_digest: String,
    pub(in crate::modules::session) record_count: usize,
}

/// 本次待写入目标的新正文：恢复补写按这份内容重放，不回读可能已变化的来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::modules::session) struct SyncBackupPayload {
    pub(in crate::modules::session) body_file: String,
    pub(in crate::modules::session) body_raw_digest: String,
    pub(in crate::modules::session) body_normalized_digest: String,
    pub(in crate::modules::session) record_count: usize,
    /// 本次待写入正文里最新一条记录的毫秒时间戳（维护目标行 `last_activity_at` 用）。
    ///
    /// 取的是**这份字节**的时间戳（与 `body_raw_digest` 出自同一次计算），恢复重放时
    /// 不需要重新读目标正文：那会引入 TOCTOU，也与「备份不可信时零写入」的口径冲突。
    ///
    /// 旧清单没有这个字段（`#[serde(default)]` 读成 None）：升级后重放上一版本留下的
    /// 清单时**跳过**写这一列、保持目标现值，不得因为缺字段失败。
    #[serde(default)]
    pub(in crate::modules::session) latest_activity_at: Option<i64>,
}

/// 目标会话行的覆盖前快照（恢复「目标行」用，design §5.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::modules::session) struct SyncBackupRow {
    pub(in crate::modules::session) session_id: String,
    pub(in crate::modules::session) user_id: String,
    pub(in crate::modules::session) title: Option<String>,
    pub(in crate::modules::session) custom_title: Option<String>,
    /// 覆盖前的 `status`；旧清单或旧库缺列时缺省为 None（恢复不得据此改状态）。
    #[serde(default)]
    pub(in crate::modules::session) status: Option<String>,
    pub(in crate::modules::session) updated_at: Option<i64>,
    pub(in crate::modules::session) deleted_at: Option<i64>,
}

/// 数据库备份：一致性快照位置、目标行覆盖前快照与本次写入的时间戳。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::modules::session) struct SyncBackupDb {
    pub(in crate::modules::session) snapshot_file: String,
    pub(in crate::modules::session) method: String,
    pub(in crate::modules::session) target_row: Option<SyncBackupRow>,
    /// 本次写入目标行的 updated_at（恢复时据此判断这一步是否已应用）。
    pub(in crate::modules::session) new_updated_at: i64,
    /// 本次要写入的 `status`；旧清单与不做状态同步的操作缺省为 None。
    #[serde(default)]
    pub(in crate::modules::session) new_status: Option<String>,
}

/// 同步备份清单：路径、目标行、备份位置与恢复方法（design §5.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::modules::session) struct SyncBackupManifest {
    pub(in crate::modules::session) version: u32,
    pub(in crate::modules::session) operation_id: String,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) group_id: String,
    pub(in crate::modules::session) mode: SyncMode,
    pub(in crate::modules::session) verdict: SyncVerdict,
    pub(in crate::modules::session) created_at: i64,
    /// 目标正文的绝对路径（恢复补写用；使用时必须校验落在本项目录内）。
    pub(in crate::modules::session) target_body_path: String,
    pub(in crate::modules::session) source: SyncBackupMember,
    pub(in crate::modules::session) target: SyncBackupMember,
    pub(in crate::modules::session) incoming: SyncBackupPayload,
    pub(in crate::modules::session) db: SyncBackupDb,
    /// 本次要提交的新配对基线引用（恢复补完复用同一个引用，不重复新建）。
    /// 仅同步归档不产生新基线，为 None；旧清单的字符串仍读为 Some。
    #[serde(default)]
    pub(in crate::modules::session) new_baseline_ref: Option<String>,
    pub(in crate::modules::session) old_baseline_ref: Option<String>,
    /// 提交成功后目标成员的 lastSyncedAt。
    pub(in crate::modules::session) last_synced_at: i64,
    /// 恢复方法（人类可读；清单只在未完成/待清理期间保留，成功清理后随目录删除）。
    pub(in crate::modules::session) restore_steps: Vec<String>,
}

/// 一次同步的备份结果：目录、清单位置与清单本体。
pub(in crate::modules::session) struct SyncBackup {
    pub(in crate::modules::session) dir: PathBuf,
    pub(in crate::modules::session) manifest_file: PathBuf,
    pub(in crate::modules::session) manifest: SyncBackupManifest,
}

/// 备份单个文件并按摘要核验（不一致即报错，不宣称备份成功）。
pub(in crate::modules::session) fn backup_file_with_digest(
    source: &Path,
    dest: &Path,
    expected_raw_digest: &str,
) -> Result<(), String> {
    let bytes = std::fs::read(source).map_err(|error| format!("备份读取失败：{error}"))?;
    if full_digest_of(&bytes) != expected_raw_digest {
        return Err("备份内容与读取时不一致（内容在读取后被改动），已停止保存".to_string());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("备份目录创建失败：{error}"))?;
    }
    session_backup::durable_write(dest, &bytes)
        .map_err(|error| format!("备份保存失败：{error}"))?;
    let read_back = std::fs::read(dest).map_err(|error| format!("备份回读失败：{error}"))?;
    if full_digest_of(&read_back) != expected_raw_digest {
        return Err("备份保存后核验不一致，未按备份成功处理".to_string());
    }
    Ok(())
}

/// 用 SQLite 在线备份 API 生成一致性快照（design §5.4：不直接 cp 活动库）。
///
/// 活动库可能带 WAL/SHM，直接复制会拿到半写状态；backup API 产出的是自洽的独立
/// 数据库文件（同时自动带上未 checkpoint 的 WAL 内容），并做写后核验。
pub(in crate::modules::session) fn snapshot_workbuddy_db(
    source: &Path,
    dest: &Path,
) -> Result<(), String> {
    if !source.is_file() {
        return Err("会话数据不存在，无法备份".to_string());
    }
    if dest.exists() {
        return Err("备份数据库已存在同名文件，未覆盖".to_string());
    }
    // 源连接用读写打开：WAL 库在缺 -shm 时无法只读打开，而备份是写入门禁，
    // 不能因此失败。App 已关闭且持有档位锁，读写打开不会改动会话内容。
    let src = open_db(source, false).ok_or_else(|| "会话数据无法打开，未同步".to_string())?;
    {
        let mut dst =
            Connection::open(dest).map_err(|error| format!("备份数据库创建失败：{error}"))?;
        let backup = Backup::new(&src, &mut dst)
            .map_err(|error| format!("数据库快照初始化失败：{error}"))?;
        let deadline = Instant::now() + DB_BACKUP_TIMEOUT;
        loop {
            match backup.step(DB_BACKUP_PAGES_PER_STEP) {
                Ok(StepResult::Done) => break,
                Ok(StepResult::Busy) | Ok(StepResult::Locked) => {
                    if Instant::now() >= deadline {
                        return Err("数据库快照超时（数据库被占用），未同步".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(_) => {}
                Err(error) => return Err(format!("数据库快照失败：{error}")),
            }
        }
    }
    verify_db_snapshot(dest)
}

/// 快照核验：可回读、完整性检查通过、会话表存在。
pub(in crate::modules::session) fn verify_db_snapshot(path: &Path) -> Result<(), String> {
    let conn =
        open_db(path, true).ok_or_else(|| "备份数据库无法回读，未按备份成功处理".to_string())?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("备份数据库完整性校验失败：{error}"))?;
    if check != "ok" {
        return Err(format!("备份数据库完整性校验未通过：{check}"));
    }
    if !table_exists(&conn, "sessions") {
        return Err("备份数据库缺少数据表，未按备份成功处理".to_string());
    }
    Ok(())
}

/// 读取一行会话的覆盖前快照（含标题类列）。
pub(in crate::modules::session) fn read_session_row(
    conn: &Connection,
    cid: &str,
) -> Result<Option<SyncBackupRow>, String> {
    // Schema errors are not evidence that the target was deleted. Keep custom_title optional
    // for older databases, but propagate every failure while inspecting the schema.
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|error| format!("会话表结构读取失败：{error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("会话表结构读取失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("会话表结构读取失败：{error}"))?;
    if columns.is_empty() {
        return Err("会话数据缺少数据表，无法读取目标会话记录".to_string());
    }
    // `custom_title` 与 `status` 都可能缺失：缺列时在 SELECT 里用 NULL 占位保持列数，
    // 避免旧库（含旧测试用例）抛「列名不存在」，也避免 row.get 索引错位。
    let has_custom_title = columns.iter().any(|column| column == "custom_title");
    let has_status = columns.iter().any(|column| column == "status");
    let sql = match (has_custom_title, has_status) {
        (true, true) => {
            "SELECT id, user_id, title, custom_title, status, updated_at, deleted_at \
             FROM sessions WHERE id = ?1"
        }
        (true, false) => {
            "SELECT id, user_id, title, custom_title, NULL, updated_at, deleted_at \
             FROM sessions WHERE id = ?1"
        }
        (false, true) => {
            "SELECT id, user_id, title, NULL, status, updated_at, deleted_at \
             FROM sessions WHERE id = ?1"
        }
        (false, false) => {
            "SELECT id, user_id, title, NULL, NULL, updated_at, deleted_at \
             FROM sessions WHERE id = ?1"
        }
    };
    conn.query_row(sql, [cid], |row| {
        Ok(SyncBackupRow {
            session_id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            user_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            title: row.get(2)?,
            custom_title: row.get(3)?,
            status: row.get(4)?,
            updated_at: row.get(5)?,
            deleted_at: row.get(6)?,
        })
    })
    .optional()
    .map_err(|error| format!("目标会话记录读取失败：{error}"))
}

/// 会话行的覆盖前快照；只有成功查询且没有命中时才返回 None。
pub(in crate::modules::session) fn session_row_snapshot(
    paths: &SessionPaths,
    cid: &str,
) -> Result<Option<SyncBackupRow>, String> {
    let conn = open_db(&paths.workbuddy_db(), true)
        .ok_or_else(|| "会话数据无法打开，无法读取目标会话记录".to_string())?;
    read_session_row(&conn, cid)
}

/// 读取备份清单；缺失/损坏/版本不符一律返回 None（视为没有恢复依据）。
pub(in crate::modules::session) fn load_sync_manifest(dir: &Path) -> Option<SyncBackupManifest> {
    let text = std::fs::read_to_string(sync_manifest_file(dir)).ok()?;
    let manifest: SyncBackupManifest = serde_json::from_str(&text).ok()?;
    (manifest.version == SYNC_BACKUP_VERSION).then_some(manifest)
}

/// 备份完整性核验：覆盖前正文与待写入正文都能按清单摘要读回，数据库快照可回读。
pub(in crate::modules::session) fn verify_sync_backup(
    dir: &Path,
    manifest: &SyncBackupManifest,
) -> Result<(), String> {
    let Some(target_body_file) = manifest.target.body_file.as_deref() else {
        return Err("备份清单缺少目标内容备份位置".to_string());
    };
    for (relative, digest) in [
        (target_body_file, manifest.target.body_raw_digest.as_str()),
        (
            manifest.incoming.body_file.as_str(),
            manifest.incoming.body_raw_digest.as_str(),
        ),
    ] {
        let bytes = std::fs::read(dir.join(relative))
            .map_err(|error| format!("备份文件缺失或不可读（{relative}）：{error}"))?;
        if full_digest_of(&bytes) != digest {
            return Err(format!("备份文件摘要不一致（{relative}），已停止恢复"));
        }
    }
    let snapshot = dir.join(&manifest.db.snapshot_file);
    if !snapshot.is_file() {
        return Err("数据库快照缺失，已停止恢复".to_string());
    }
    let conn =
        open_db(&snapshot, true).ok_or_else(|| "数据库快照无法打开，已停止恢复".to_string())?;
    if !table_exists(&conn, "sessions") {
        return Err("数据库快照缺少数据表，已停止恢复".to_string());
    }
    Ok(())
}

/// 目标正文路径：必须是档位 `projects/` 目录下 `{目标 sessionId}.jsonl`。
///
/// 清单可能被外部改动，恢复写入前必须重新确认路径落在本项目录内，不能按清单原样写。
pub(in crate::modules::session) fn validated_target_body_path(
    paths: &SessionPaths,
    manifest: &SyncBackupManifest,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(&manifest.target_body_path);
    let expected_name = format!("{}.jsonl", manifest.target.session_id);
    let name_matches = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == expected_name);
    if !path.starts_with(paths.projects_dir()) || !name_matches {
        return Err("备份清单记录的目标内容路径不合法，已停止保存".to_string());
    }
    Ok(path)
}

/// 创建本次同步的备份：唯一目录 + 覆盖前正文 + 待写入正文 + 数据库一致性快照 + 清单。
///
/// 全过程只读目标、只写备份目录；任何一步失败都返回 Err，调用方必须零写入
/// （不碰目标正文、不改数据库、不提交基线）。`operation_id` 由生命周期记录预分配。
#[allow(clippy::too_many_arguments)] // 与 rotate.rs 同口径：参数都是本次备份的显式输入
pub(in crate::modules::session) fn create_sync_backup(
    paths: &SessionPaths,
    variant: WbVariant,
    operation_id: &str,
    plan: &SyncWritePlan,
    target_body_path: &Path,
    new_updated_at: i64,
    new_baseline_ref: Option<&str>,
    last_synced_at: i64,
) -> Result<SyncBackup, String> {
    let dir = sync_backup_dir(paths, variant, operation_id)?;
    if !dir.is_dir() {
        return Err("操作专属目录不存在，未创建备份".to_string());
    }
    std::fs::create_dir_all(dir.join("bodies"))
        .map_err(|error| format!("同步备份目录创建失败：{error}"))?;

    // 1) 覆盖前正文备份 + 摘要核验（恢复/回滚的唯一依据）。
    let original_rel = format!("bodies/original-{}.jsonl", plan.target_member.session_id);
    backup_file_with_digest(
        target_body_path,
        &dir.join(&original_rel),
        &plan.target_snapshot.full_digest,
    )?;
    // 2) 待写入正文备份 + 摘要核验（写入与恢复都只写这份字节）。
    let incoming_rel = format!("bodies/incoming-{}.jsonl", plan.target_member.session_id);
    let incoming_raw = plan.incoming_text.as_bytes();
    std::fs::write(dir.join(&incoming_rel), incoming_raw)
        .map_err(|error| format!("待保存内容备份失败：{error}"))?;
    let incoming_raw_digest = full_digest_of(incoming_raw);
    if full_digest_of(
        &std::fs::read(dir.join(&incoming_rel))
            .map_err(|error| format!("待保存内容回读失败：{error}"))?,
    ) != incoming_raw_digest
    {
        return Err("待保存内容备份保存后核验不一致，未按备份成功处理".to_string());
    }
    // 3) 数据库一致性快照。
    let db_rel = "workbuddy.db".to_string();
    snapshot_workbuddy_db(&paths.workbuddy_db(), &dir.join(&db_rel))?;
    let target_row = session_row_snapshot(paths, &plan.target_member.session_id)?;
    // 规划与备份之间状态被改动就不按「可归档」继续：条件 UPDATE 的前置值必须来自
    // 与规划时一致的现场，不能拿一份已经过期的前置值去写库。
    if plan.archive_target
        && target_row.as_ref().and_then(|row| row.status.clone()) != plan.target_status_before
    {
        return Err("目标会话的状态在校验后发生变化，已停止保存".to_string());
    }

    let manifest = SyncBackupManifest {
        version: SYNC_BACKUP_VERSION,
        operation_id: operation_id.to_string(),
        variant,
        group_id: plan.group_id.clone(),
        mode: plan.mode,
        verdict: plan.verdict,
        created_at: now_ms(),
        target_body_path: target_body_path.to_string_lossy().to_string(),
        source: SyncBackupMember {
            member_id: plan.source_member.member_id.clone(),
            uid: plan.source_member.uid.clone(),
            session_id: plan.source_member.session_id.clone(),
            body_file: None,
            body_raw_digest: plan.source_snapshot.full_digest.clone(),
            body_normalized_digest: plan.source_snapshot.normalized.total_digest.clone(),
            record_count: plan.source_records(),
        },
        target: SyncBackupMember {
            member_id: plan.target_member.member_id.clone(),
            uid: plan.target_member.uid.clone(),
            session_id: plan.target_member.session_id.clone(),
            body_file: Some(original_rel),
            body_raw_digest: plan.target_snapshot.full_digest.clone(),
            body_normalized_digest: plan.target_snapshot.normalized.total_digest.clone(),
            record_count: plan.target_snapshot.normalized.record_count,
        },
        incoming: SyncBackupPayload {
            body_file: incoming_rel,
            body_raw_digest: incoming_raw_digest,
            body_normalized_digest: plan.incoming.total_digest.clone(),
            record_count: plan.incoming.record_count,
            // 与上面写入备份的字节同源（同一个 `incoming_text`），恢复重放直接用它，
            // 不必（也不得）回读目标正文。
            latest_activity_at: session_link::latest_record_timestamp(&plan.incoming_text),
        },
        db: SyncBackupDb {
            snapshot_file: db_rel,
            method: DB_SNAPSHOT_METHOD.to_string(),
            target_row,
            new_updated_at,
            new_status: plan
                .archive_target
                .then(|| SESSION_STATUS_ARCHIVED.to_string()),
        },
        new_baseline_ref: new_baseline_ref.map(str::to_string),
        old_baseline_ref: plan.old_baseline_ref.clone(),
        last_synced_at,
        restore_steps: vec![
            "目标内容：从 bodies/ 下 original-*.jsonl 写回目标路径（先校验当前内容是否为本次保存的内容）"
                .to_string(),
            "目标会话记录：把 sessions.updated_at 还原为清单 db.targetRow.updatedAt（先校验归属与当前值）"
                .to_string(),
            "数据库：workbuddy.db 为本次保存前的一致性快照，可用 SQLite 打开核对；不要整库覆盖当前库"
                .to_string(),
            "同步记录：把 manifest.newBaselineRef 对应的成员间同步记录还原为 oldBaselineRef（若未改动则无需处理）"
                .to_string(),
        ],
    };
    // 4) 清单落盘并回读核验：备份必须可验证恢复，读不回来的清单不算备份成功。
    let content = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    let manifest_file = sync_manifest_file(&dir);
    session_backup::durable_write_str(&manifest_file, &content)
        .map_err(|error| format!("同步备份清单保存失败：{error}"))?;
    match load_sync_manifest(&dir) {
        Some(read_back) if read_back == manifest => {}
        _ => return Err("同步备份清单保存后核验不一致，未按备份成功处理".to_string()),
    }
    verify_sync_backup(&dir, &manifest)?;
    Ok(SyncBackup {
        dir,
        manifest_file,
        manifest,
    })
}
