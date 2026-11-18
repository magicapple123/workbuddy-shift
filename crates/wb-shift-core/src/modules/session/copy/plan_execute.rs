//! 复制执行与恢复重放：逐会话写入、操作日志推进、关联组与基线提交。
use super::plan::{CopyContext, CopyOutcome};
use super::*;
/// 复制单个会话：预分配 UUID → 持久化操作 → 正文 → 数据库 → 映射 → 关联/基线。
pub(in crate::modules::session) fn copy_one_session(
    context: &CopyContext,
    cid: &str,
) -> Result<CopyOutcome, String> {
    let paths = context.paths;
    // 上一次未完成的同一请求：只复用，不新建第二个副本。
    if let Some(operation) = session_link::find_pending_operation(
        context.pending,
        context.source_uid,
        cid,
        context.target_uid,
    ) {
        return Err(format!(
            "上一次复制尚未完成（操作 {}）：{}，这次不会重复创建",
            operation.operation_id,
            operation
                .last_error
                .clone()
                .unwrap_or_else(|| "等待恢复".to_string())
        ));
    }

    // 源正文与源行都在源档（跨档时与目标档不同）。
    let Some(source_path) = find_project_jsonl(context.source_paths, cid) else {
        return Err("会话内容不存在，未复制".to_string());
    };
    let snapshot = match session_link::read_content_snapshot(&source_path, cid) {
        ContentState::Ready(snapshot) => snapshot,
        ContentState::Missing => return Err("会话内容不存在，未复制".to_string()),
        ContentState::Unavailable(reason) => {
            return Err(format!("会话内容无法验证（{reason}），未复制"));
        }
    };
    let source_owner = session_row_owner(context.source_paths, cid);
    match source_owner.as_deref() {
        Some(owner) if owner == context.source_uid => {}
        Some(_) => return Err("源会话不属于当前账号，未复制".to_string()),
        None => return Err("数据库中找不到源会话记录，未复制".to_string()),
    }

    let resolution = resolve_target(context, cid)?;
    if let Some(session_id) = resolution.existing_link {
        return Ok(CopyOutcome::AlreadyLinked {
            session_id,
            group_id: resolution.group_id.unwrap_or_default(),
        });
    }

    // 预分配身份：维护记录（allocating）先于任何备份与业务写入（design §3）。
    // 任何副本写入之前，生命周期身份必须已可靠落盘，恢复/补清理才有依据。
    let new_cid = uuid::Uuid::new_v4().to_string();
    let group_id = resolution
        .group_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut lifecycle = session_backup::begin_operation(
        paths,
        context.variant,
        OPERATION_KIND_COPY,
        Some(new_cid.clone()),
        None,
    )?;
    let backup_dir =
        session_backup::transaction_dir(paths, context.variant, &lifecycle.operation_id)?;
    let backup = match backup_workbuddy_db(paths, &backup_dir) {
        Ok(backup) => backup,
        Err(error) => {
            // 受控失败分支：契约保证 protected 之前不发生业务写入，可安全回收残留。
            session_backup::reclaim_unwritten(paths, context.variant, &mut lifecycle, &error);
            return Err(error);
        }
    };
    // 备份完备先转 protected：此步失败禁止任何业务写入，残留保守保留待下次维护。
    session_backup::mark_protected(paths, &mut lifecycle)?;
    ensure_link_store_ready(paths)?;

    let mut operation = Operation {
        version: OPERATION_VERSION,
        operation_id: lifecycle.operation_id.clone(),
        kind: OPERATION_KIND_COPY.to_string(),
        variant: context.variant,
        // 同档复制写 None（与旧记录逐字兼容）；跨档复制显式记录源档，恢复时据此读源。
        source_variant: (context.source_variant != context.variant)
            .then_some(context.source_variant),
        group_id: group_id.clone(),
        source: OperationMember {
            account_id: context.source_account_id.clone(),
            uid: context.source_uid.to_string(),
            session_id: cid.to_string(),
        },
        target: OperationMember {
            account_id: context.target_account_id.clone(),
            uid: context.target_uid.to_string(),
            session_id: new_cid.clone(),
        },
        // 预填源摘要：真正写正文前会被「待写入文本」的摘要覆盖（复制可能归一最后一条
        // assistant 消息的 `incomplete`，两者不一定相等），此后它就是目标正文的核验基准。
        expected_content_digest: snapshot.normalized.total_digest.clone(),
        expected_record_count: snapshot.normalized.record_count,
        phase: OpPhase::Prepared,
        backup: Some(backup.to_string_lossy().to_string()),
        lifecycle_version: Some(OPERATION_LIFECYCLE_VERSION),
        cleanup_state: None,
        last_error: None,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    session_link::save_operation(paths, &operation)?;

    if let Err(error) = finish_copy_from_body(
        paths,
        context.variant,
        context.source_paths,
        &mut operation,
        &snapshot,
        &source_path,
    ) {
        fail_operation(paths, &mut operation, &error);
        return Err(error);
    }
    let _ = session_link::prune_operations(
        paths,
        context.variant,
        session_link::KEEP_COMPLETED_OPERATIONS,
    );
    // 业务可靠完成之后才授权清理；清理失败不回滚业务、不报告复制失败。
    let cleanup = finish_backup_cleanup(paths, context.variant, &mut lifecycle);
    let (backup, cleanup_state, cleanup_error) = report_cleanup(&cleanup, &backup_dir);
    Ok(CopyOutcome::Copied {
        new_id: new_cid,
        group_id,
        backup,
        cleanup_state: cleanup_state.to_string(),
        cleanup_error,
    })
}

/// 从「源快照已确认」开始推进复制：写正文 → 写数据库行 → 登记映射 → 提交关联与基线。
///
/// 源行读取与目标行写入的档位经 `source_paths` 分离（跨档复制时与 `paths` 不同，
/// 同档时相同）；正文只从 `source_path` 读、只往目标档写。
pub(in crate::modules::session) fn finish_copy_from_body(
    paths: &SessionPaths,
    variant: WbVariant,
    source_paths: &SessionPaths,
    operation: &mut Operation,
    snapshot: &ContentSnapshot,
    source_path: &Path,
) -> Result<(), String> {
    // 复制可能归一最后一条 assistant 消息的 `incomplete`，所以「本次写入的正文摘要」
    // 不一定等于源快照摘要：
    // 校验基准与关联基线一律用待写入文本算出的那一份（见 `copy_body`）。
    let body = copy_body(
        snapshot,
        &operation.source.session_id,
        &operation.target.session_id,
    )?;
    // 期望摘要必须**先于正文落盘**：万一写成功而操作记录没存下来，恢复会拿源摘要去
    // 核验归一后的正文，把它误判成「被其它程序改动」而永久阻塞（不可重试的错误）。
    operation.expected_content_digest = body.normalized.total_digest.clone();
    operation.expected_record_count = body.normalized.record_count;
    session_link::save_operation(paths, operation)?;
    write_copy_body(
        &body,
        source_path,
        source_paths,
        paths,
        &operation.target.session_id,
    )?;
    advance_operation(paths, operation, OpPhase::BodyWritten)?;
    let normalized = body.normalized;

    match insert_session_copy(
        source_paths,
        paths,
        &operation.target.session_id,
        &operation.source.session_id,
        &operation.source.uid,
        &operation.target.uid,
    )? {
        DbCopyOutcome::Inserted => {}
        DbCopyOutcome::SourceRowMissing => {
            return Err("数据库中找不到源会话记录，未复制".to_string())
        }
        DbCopyOutcome::NoSessionsTable => return Err("会话数据缺少数据表，未复制".to_string()),
        DbCopyOutcome::NoDb => return Err("会话数据不存在，未复制".to_string()),
    }
    verify_session_row(paths, &operation.target.session_id, &operation.target.uid)?;
    advance_operation(paths, operation, OpPhase::DbWritten)?;

    // 云端登记交接给客户端：不预写 edge_sync_mapping——预写会让 edge-sync 扩展
    // 判定"已迁移"而跳过上传，导致云端没有会话、后续 RENAME/ACTIVITY 404。
    // 副本由客户端下次启动时自行迁移并写入映射行；阶段标记保留，语义为"已交接"。
    advance_operation(paths, operation, OpPhase::MappingWritten)?;

    let group_id = commit_links(paths, variant, operation, &normalized)?;
    operation.group_id = group_id;
    advance_operation(paths, operation, OpPhase::LinksCommitted)?;
    advance_operation(paths, operation, OpPhase::Completed)?;
    Ok(())
}

/// 推进操作阶段。阶段只能前进：已经走到更靠后的阶段时不回写（恢复路径不得把
/// `LinksCommitted`/`Completed` 写回 `DbWritten`）。
///
/// 先保存候选副本、成功后才替换内存状态：保存失败时内存阶段不变，随后的
/// `fail_operation` 不会把未落盘的阶段写回磁盘（design §3）。
pub(in crate::modules::session) fn advance_operation(
    paths: &SessionPaths,
    operation: &mut Operation,
    phase: OpPhase,
) -> Result<(), String> {
    if operation.phase >= phase {
        return Ok(());
    }
    let mut candidate = operation.clone();
    candidate.phase = phase;
    candidate.updated_at = now_ms();
    session_link::save_operation(paths, &candidate)?;
    *operation = candidate;
    Ok(())
}

pub(in crate::modules::session) fn fail_operation(
    paths: &SessionPaths,
    operation: &mut Operation,
    error: &str,
) {
    operation.last_error = Some(error.to_string());
    operation.updated_at = now_ms();
    let _ = session_link::save_operation(paths, operation);
}

/// 原子提交关联与配对基线（含失效成员替换与基线继承）。
///
/// 整个读改写都在关联存储锁内完成；只有全部成功才推进 revision。
pub(in crate::modules::session) fn commit_links(
    paths: &SessionPaths,
    variant: WbVariant,
    operation: &Operation,
    normalized: &NormalizedContent,
) -> Result<String, String> {
    let source = &operation.source;
    let target = &operation.target;
    // 成员级档位：源成员记源档（跨档组的身份基础），目标成员记目标档（参数 variant）。
    let source_variant = operation.source_variant.unwrap_or(variant);
    let group_id = operation.group_id.clone();
    let group_id_out = group_id.clone();
    session_link::with_link_store_write(paths, move |store| {
        let index = match store.groups.iter().position(|group| group.id == group_id) {
            Some(index) => index,
            None => {
                store.groups.push(LinkGroup {
                    id: group_id.clone(),
                    variant,
                    created_at: now_ms(),
                    members: Vec::new(),
                    pair_bases: Vec::new(),
                });
                store.groups.len() - 1
            }
        };
        let group = &mut store.groups[index];

        let source_member_id =
            match session_link::find_member(group, &source.uid, &source.session_id) {
                Some(member) => member.member_id.clone(),
                None => {
                    let member_id = uuid::Uuid::new_v4().to_string();
                    session_link::add_active_member(
                        group,
                        LinkMember {
                            member_id: member_id.clone(),
                            account_id: source.account_id.clone(),
                            uid: source.uid.clone(),
                            session_id: source.session_id.clone(),
                            variant: Some(source_variant),
                            state: MemberState::Active,
                            linked_at: now_ms(),
                            last_synced_at: None,
                        },
                    );
                    member_id
                }
            };

        let target_member_id =
            match session_link::find_member(group, &target.uid, &target.session_id) {
                Some(member) => {
                    let member_id = member.member_id.clone();
                    session_link::set_member_state(group, &member_id, MemberState::Active);
                    member_id
                }
                None => {
                    let member_id = uuid::Uuid::new_v4().to_string();
                    // 同账号的失效 active 成员在这里被显式 supersede，保留记录。
                    session_link::add_active_member(
                        group,
                        LinkMember {
                            member_id: member_id.clone(),
                            account_id: target.account_id.clone(),
                            uid: target.uid.clone(),
                            session_id: target.session_id.clone(),
                            variant: Some(variant),
                            state: MemberState::Active,
                            linked_at: now_ms(),
                            last_synced_at: None,
                        },
                    );
                    member_id
                }
            };

        // 本次复制的正文即来源与目标的共同基线（定向更新，不动其它配对）。
        let pair_baseline_ref = uuid::Uuid::new_v4().to_string();
        session_link::save_baseline(paths, &pair_baseline_ref, normalized)?;
        session_link::set_pair_base(
            group,
            &source_member_id,
            &target_member_id,
            &pair_baseline_ref,
            NORMALIZATION_VERSION,
        );

        // 继承：来源与组内其它成员已有的历史共同基线，只有在「来源内容有序前缀
        // 包含该基线」时才能建立到新成员的基线；已有配对基线不覆盖。
        let others: Vec<String> = group
            .members
            .iter()
            .filter(|member| {
                member.member_id != source_member_id && member.member_id != target_member_id
            })
            .map(|member| member.member_id.clone())
            .collect();
        for other_id in others {
            if session_link::find_pair_base(group, &other_id, &target_member_id).is_some() {
                continue;
            }
            let Some(pair) =
                session_link::find_pair_base(group, &source_member_id, &other_id).cloned()
            else {
                continue;
            };
            if let Some(record) =
                session_link::inheritable_baseline(paths, &pair, &normalized.line_digests)
            {
                session_link::set_pair_base(
                    group,
                    &other_id,
                    &target_member_id,
                    &record.baseline_ref,
                    pair.normalization_version,
                );
            }
        }
        Ok(group_id_out)
    })
}

/// 操作已标成 `LinksCommitted` 时，核验关联组与双方成员仍在（只读，不写存储）。
pub(in crate::modules::session) fn committed_links_present(
    paths: &SessionPaths,
    operation: &Operation,
) -> Result<(), String> {
    match session_link::load_store(paths) {
        StoreState::Ready(store) => {
            let Some(group) = store
                .groups
                .iter()
                .find(|group| group.id == operation.group_id)
            else {
                return Err("会话的关联关系缺失，已停止恢复".to_string());
            };
            let has_source = session_link::find_member(
                group,
                &operation.source.uid,
                &operation.source.session_id,
            )
            .is_some();
            let has_target = session_link::find_member(
                group,
                &operation.target.uid,
                &operation.target.session_id,
            )
            .is_some();
            if !has_source || !has_target {
                return Err("对应的会话缺失，已停止恢复".to_string());
            }
            Ok(())
        }
        StoreState::Missing => Err("同步记录主文件缺失，已停止恢复".to_string()),
        StoreState::Unavailable(reason) => Err(reason),
    }
}
