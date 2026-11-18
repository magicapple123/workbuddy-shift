//! 同步写入应用的原子操作：正文替换 / 还原、数据库行与基线提交、归档状态动作。
use super::*;
/// 目标正文相对本次操作的状态。
pub(in crate::modules::session) enum SyncBodyState {
    /// 已经是本次写入的内容。
    Ours,
    /// 仍是覆盖前的内容（尚未写入，或已被还原）。
    PreSync,
    /// 正文不存在。
    Gone,
    /// 两者都不是：可能被其它程序改动，停止恢复，不覆盖未知内容。
    Unknown(String),
}

pub(in crate::modules::session) fn classify_sync_body(
    target_body_path: &Path,
    target_session_id: &str,
    pre_sync_raw_digest: &str,
    expected_digest: &str,
) -> SyncBodyState {
    match session_link::read_content_snapshot(target_body_path, target_session_id) {
        ContentState::Ready(content) if content.normalized.total_digest == expected_digest => {
            SyncBodyState::Ours
        }
        ContentState::Ready(content) if content.full_digest == pre_sync_raw_digest => {
            SyncBodyState::PreSync
        }
        ContentState::Ready(_) => SyncBodyState::Unknown(
            "目标内容与本次保存及覆盖前版本都不一致（可能被其它程序改动），已停止保存，不覆盖未知内容"
                .to_string(),
        ),
        ContentState::Missing => SyncBodyState::Gone,
        ContentState::Unavailable(reason) => SyncBodyState::Unknown(format!(
            "目标内容无法验证（{reason}），已停止保存"
        )),
    }
}

/// 原子替换目标正文（同目录临时文件 + rename），写后复算摘要核验。
pub(in crate::modules::session) fn write_sync_body(
    target_body_path: &Path,
    target_session_id: &str,
    text: &str,
    expected_digest: &str,
) -> Result<NormalizedContent, String> {
    // 正文属于业务完成门禁：会话专用持久化写（sync_all + 父目录持久化）。
    session_backup::durable_write_str(target_body_path, text)
        .map_err(|error| format!("同步内容保存失败：{error}"))?;
    match session_link::read_content_snapshot(target_body_path, target_session_id) {
        ContentState::Ready(read_back) if read_back.normalized.total_digest == expected_digest => {
            Ok(read_back.normalized)
        }
        ContentState::Ready(_) => Err("同步内容保存后校验不一致，未按成功处理".to_string()),
        ContentState::Missing => Err("同步内容保存后不存在，未按成功处理".to_string()),
        ContentState::Unavailable(reason) => Err(format!("同步内容保存后无法确认：{reason}")),
    }
}

/// 从备份目录取待写入正文并原子替换目标正文：写入的字节等于备份的字节。
pub(in crate::modules::session) fn apply_sync_body(
    paths: &SessionPaths,
    backup_dir: &Path,
    manifest: &SyncBackupManifest,
    expected_digest: &str,
) -> Result<NormalizedContent, String> {
    let target_body_path = validated_target_body_path(paths, manifest)?;
    let bytes = std::fs::read(backup_dir.join(&manifest.incoming.body_file))
        .map_err(|error| format!("待保存内容备份读取失败：{error}"))?;
    if full_digest_of(&bytes) != manifest.incoming.body_raw_digest {
        return Err("待保存内容备份与清单不一致，已停止保存".to_string());
    }
    let text =
        String::from_utf8(bytes).map_err(|_| "待保存内容不是合法 UTF-8，未保存".to_string())?;
    write_sync_body(
        &target_body_path,
        &manifest.target.session_id,
        &text,
        expected_digest,
    )
}

/// 事务内更新目标行 updated_at：命中归属（owner = 目标 uid）且未删除。
///
/// 目标行状态的条件写入要求：事务内按前置状态等值匹配，避免覆盖并发改动。
pub(in crate::modules::session) struct StatusChange<'a> {
    /// 覆盖前的 `status`（事务内 WHERE 条件）。
    pub(in crate::modules::session) before: String,
    /// 本次要写入的状态。
    pub(in crate::modules::session) after: &'a str,
}

/// 更新目标会话行：`updated_at` 恒写，状态只在本次获得授权时按前置值条件写入。
///
/// 只改 updated_at（与本次状态）；sessionId、标题与 custom_title 必须与覆盖前一致（R4）。
pub(in crate::modules::session) fn update_target_session_row(
    paths: &SessionPaths,
    target: &OperationMember,
    new_updated_at: i64,
    before: Option<&SyncBackupRow>,
    status_change: Option<&StatusChange<'_>>,
) -> Result<(), String> {
    let mut conn = open_db(&paths.workbuddy_db(), false)
        .ok_or_else(|| "会话数据无法打开，未同步".to_string())?;
    if !table_exists(&conn, "sessions") {
        return Err("会话数据缺少数据表，未同步".to_string());
    }
    // 写事务的提交必须可靠持久：在本次实际写连接上确认 synchronous ≥ FULL。
    session_backup::ensure_full_synchronous(&conn)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("会话数据事务开启失败：{error}"))?;
    // 状态写入用条件 UPDATE：目标在事务内被改为活跃态/被取消归档时 affected 为 0，
    // 事务外的一次读取不能替代事务内的条件。
    let affected = match status_change {
        // `status = after` 这一支是幂等重放：数据库已提交但阶段标记未推进时，恢复会
        // 再跑一次本函数。前置值不匹配就不写——活跃态、未知值均被排除在外。
        Some(change) => tx.execute(
            "UPDATE sessions SET updated_at = ?1, status = ?2 \
             WHERE id = ?3 AND user_id = ?4 AND deleted_at IS NULL \
             AND (status = ?5 OR status = ?6)",
            rusqlite::params![
                new_updated_at,
                change.after,
                target.session_id,
                target.uid,
                change.before,
                change.after
            ],
        ),
        None => tx.execute(
            "UPDATE sessions SET updated_at = ?1 \
             WHERE id = ?2 AND user_id = ?3 AND deleted_at IS NULL",
            rusqlite::params![new_updated_at, target.session_id, target.uid],
        ),
    }
    .map_err(|error| format!("目标会话记录更新失败：{error}"))?;
    if affected != 1 {
        return Err(
            "目标会话记录归属校验失败：会话不存在、不属于目标账号或已被删除，未按成功处理"
                .to_string(),
        );
    }
    match (read_session_row(&tx, &target.session_id)?, before) {
        (None, _) => return Err("目标会话记录保存后不可见，未按成功处理".to_string()),
        (Some(after), Some(before)) => {
            if after.session_id != before.session_id
                || after.user_id != before.user_id
                || after.title != before.title
                || after.custom_title != before.custom_title
            {
                return Err(
                    "目标会话记录的归属或标题在保存期间发生变化，已回滚本次更新".to_string()
                );
            }
            if after.updated_at != Some(new_updated_at) {
                return Err("目标会话记录更新时间未按本次保存生效，未按成功处理".to_string());
            }
            // 只在本次确实写状态时才校验状态。不写状态的正文同步里，目标被第三方
            // 从 completed 改成 active 属于与本次无关的変化，保留当前状态继续补完
            // 正文，不能把一次正常的正文恢复升级成人工处理。
            // （旧清单同样记不到 status：此处若拿 None 去比对，真实库上恒不相等。）
            if let Some(change) = status_change {
                if after.status.as_deref() != Some(change.after) {
                    return Err("目标会话记录的状态在保存期间发生变化，已回滚本次更新".to_string());
                }
            }
        }
        (Some(_), None) => {}
    }
    tx.commit()
        .map_err(|error| format!("会话数据提交失败：{error}"))?;
    Ok(())
}

/// 提交 A/B 新基线与目标成员 lastSyncedAt（定向更新，不触碰其它配对）。
///
/// 新基线用新的 ref，避免覆盖被其它成员对继承的历史基线（A/B 同步不代表 C 也同步）。
/// 组查找只按 id（跨档组的组级 variant 是创建档位，不作过滤）；成员身份由下面的
/// member_id 校验兜底。
pub(in crate::modules::session) fn commit_sync_baseline(
    paths: &SessionPaths,
    manifest: &SyncBackupManifest,
    normalized: &NormalizedContent,
) -> Result<(), String> {
    // 仅同步归档不产生新基线：缺引用即拒绝提交，不能用空引用或假引用蒙混。
    let Some(new_baseline_ref) = manifest.new_baseline_ref.as_deref() else {
        return Err("同步备份清单缺少新基线引用，未提交同步结果".to_string());
    };
    let source_member_id = manifest.source.member_id.as_str();
    let target_member_id = manifest.target.member_id.as_str();
    session_link::with_link_store_write(paths, |store| {
        let Some(group) = store
            .groups
            .iter_mut()
            .find(|group| group.id == manifest.group_id)
        else {
            return Err("会话的关联关系已不存在，未提交同步结果".to_string());
        };
        if !group
            .members
            .iter()
            .any(|member| member.member_id == source_member_id)
        {
            return Err("当前账号的会话已不存在，未提交同步结果".to_string());
        }
        {
            let Some(target) = group
                .members
                .iter_mut()
                .find(|member| member.member_id == target_member_id)
            else {
                return Err("目标账号的会话已不存在，未提交同步结果".to_string());
            };
            target.last_synced_at = Some(manifest.last_synced_at);
        }
        session_link::save_baseline(paths, new_baseline_ref, normalized)?;
        session_link::set_pair_base(
            group,
            source_member_id,
            target_member_id,
            new_baseline_ref,
            NORMALIZATION_VERSION,
        );
        Ok(())
    })
}

/// 核验 A/B 配对基线已提交为本次的新引用（已越过该阶段时不重放，只核验）。
pub(in crate::modules::session) fn verify_sync_baseline_committed(
    paths: &SessionPaths,
    manifest: &SyncBackupManifest,
) -> Result<(), String> {
    // 仅同步归档没有新基线：这类操作不会走到本函数，缺引用即为清单异常。
    let Some(new_baseline_ref) = manifest.new_baseline_ref.as_deref() else {
        return Err("同步备份清单缺少新基线引用，已停止恢复".to_string());
    };
    match session_link::load_store(paths) {
        StoreState::Ready(store) => {
            let Some(group) = store
                .groups
                .iter()
                .find(|group| group.id == manifest.group_id)
            else {
                return Err("会话的关联关系缺失，已停止恢复".to_string());
            };
            let Some(pair) = session_link::find_pair_base(
                group,
                &manifest.source.member_id,
                &manifest.target.member_id,
            ) else {
                return Err("同步记录缺失，已停止恢复".to_string());
            };
            if Some(pair.baseline_ref.as_str()) != manifest.new_baseline_ref.as_deref() {
                return Err("同步记录与本次保存不一致，已停止恢复".to_string());
            }
            if session_link::load_baseline(paths, new_baseline_ref).is_none() {
                return Err("同步记录缺失或内容不一致，已停止恢复".to_string());
            }
            Ok(())
        }
        StoreState::Missing => Err("同步记录主文件缺失，已停止恢复".to_string()),
        StoreState::Unavailable(reason) => Err(reason),
    }
}

/// 用备份里的覆盖前正文回滚目标正文（只在当前内容确为本次写入时调用）。
pub(in crate::modules::session) fn restore_sync_backup_body(
    paths: &SessionPaths,
    dir: &Path,
    manifest: &SyncBackupManifest,
) -> Result<(), String> {
    let Some(relative) = manifest.target.body_file.as_deref() else {
        return Err("备份清单缺少目标内容备份位置".to_string());
    };
    let bytes =
        std::fs::read(dir.join(relative)).map_err(|error| format!("备份内容读取失败：{error}"))?;
    if full_digest_of(&bytes) != manifest.target.body_raw_digest {
        return Err("备份内容摘要不一致，已停止回滚".to_string());
    }
    let text =
        String::from_utf8(bytes).map_err(|_| "备份内容不是合法 UTF-8，未回滚".to_string())?;
    let target_body_path = validated_target_body_path(paths, manifest)?;
    session_backup::durable_write_str(&target_body_path, &text)
        .map_err(|error| format!("目标内容回滚失败：{error}"))
}

/// 清单里的状态写入要求（新状态 + 覆盖前状态）；缺覆盖前状态即不可核验。
pub(in crate::modules::session) fn status_change_of(
    manifest: &SyncBackupManifest,
) -> Result<Option<StatusChange<'_>>, String> {
    let Some(after) = manifest.db.new_status.as_deref() else {
        return Ok(None);
    };
    let Some(before) = manifest
        .db
        .target_row
        .as_ref()
        .and_then(|row| row.status.clone())
    else {
        return Err("备份清单缺少目标会话的覆盖前状态，已停止保存".to_string());
    };
    Ok(Some(StatusChange { before, after }))
}

/// 正文模式下本次实际执行的状态写入。
///
/// 正文模式的归档是副作用：目标已不在可归档的状态（例如被打开后变成 active）时，
/// 不能为了归档把正文恢复卡成人工处理——跳过状态写入，正文照常补完。
/// 仅同步归档里状态就是全部动作，必须保留要求，由事务内条件 UPDATE 兜底。
pub(in crate::modules::session) fn planned_status_change<'a>(
    paths: &SessionPaths,
    operation: &Operation,
    manifest: &'a SyncBackupManifest,
) -> Result<Option<StatusChange<'a>>, String> {
    let Some(change) = status_change_of(manifest)? else {
        return Ok(None);
    };
    if manifest.mode == SyncMode::StatusOnly {
        return Ok(Some(change));
    }
    let current =
        session_row_snapshot(paths, &operation.target.session_id)?.and_then(|row| row.status);
    let still_archivable = current.as_deref() == Some(change.after)
        || current.as_deref() == Some(change.before.as_str());
    Ok(still_archivable.then_some(change))
}
