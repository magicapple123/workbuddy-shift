//! 清理：清理决策与执行、失败记录、成功收尾与未写回回收。

#[allow(unused_imports)]
use super::backup::*;
#[allow(unused_imports)]
use super::lifecycle::*;
use super::*;

// ---------------------------------------------------------------------------
// 回收
// ---------------------------------------------------------------------------

/// 回收一次操作的临时目录与维护记录（幂等；调用方必须已持有档位操作锁）。
///
/// 顺序：目录 → 业务日志（backup=null / cleaned）→ 维护记录；任一步失败都保留
/// 可重试依据。目录已经不存在时视为已删，不要求备份清单仍然完整。
pub fn cleanup_operation(
    paths: &SessionPaths,
    variant: WbVariant,
    record: &BackupLifecycle,
) -> CleanupOutcome {
    let dir = match transaction_dir(paths, variant, &record.operation_id) {
        Ok(dir) => dir,
        Err(reason) => return CleanupOutcome::Protected { reason },
    };
    let root = transactions_root(paths, variant);
    // 可信根是工具存储根：从 store_root 往下的每一级（含 backups/ 祖先）都不能是符号链接。
    if let Err(reason) = ensure_no_symlink_components(&paths.store_root, &dir) {
        return CleanupOutcome::Protected { reason };
    }
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                return CleanupOutcome::Pending {
                    reason: format!("临时目录删除失败：{error}"),
                };
            }
            if let Err(error) = sync_dir(&root) {
                return CleanupOutcome::Pending {
                    reason: format!("目录项持久化失败：{error}"),
                };
            }
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            return CleanupOutcome::Protected {
                reason: "临时目录被替换为符号链接，已保留现场".to_string(),
            };
        }
        Ok(_) => {
            return CleanupOutcome::Protected {
                reason: "临时目录位置存在同名非目录文件，已保留现场".to_string(),
            };
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return CleanupOutcome::Pending {
                reason: format!("临时目录状态无法确认：{error}"),
            };
        }
    }

    if let Err(reason) = mark_operation_cleaned(paths, variant, record) {
        return CleanupOutcome::Pending { reason };
    }
    let file = match lifecycle_file(paths, variant, &record.operation_id) {
        Ok(file) => file,
        Err(reason) => return CleanupOutcome::Protected { reason },
    };
    match std::fs::remove_file(&file) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return CleanupOutcome::Pending {
                reason: format!("维护记录删除失败：{error}"),
            };
        }
    }
    if let Err(error) = sync_parent_of(&file) {
        return CleanupOutcome::Pending {
            reason: format!("维护记录目录项持久化失败：{error}"),
        };
    }
    CleanupOutcome::Cleaned
}

/// 业务日志标注备份已清理（`backup=null` + `cleanupState=cleaned`）。
///
/// 日志缺失不算失败（例如准备阶段失败的残留）；日志仍在且仍未完成时拒绝清理，
/// 避免把未完成业务的日志改成「已清理」。
pub(super) fn mark_operation_cleaned(
    paths: &SessionPaths,
    variant: WbVariant,
    record: &BackupLifecycle,
) -> Result<(), String> {
    let scan = session_link::scan_operations(paths);
    if !scan.complete {
        return Err("操作记录扫描不完整，本轮不更新业务日志".to_string());
    }
    let Some(mut operation) = scan.operations.into_iter().find(|operation| {
        operation.operation_id == record.operation_id && operation.variant == variant
    }) else {
        return Ok(());
    };
    if operation.phase.is_unfinished() {
        return Err("业务尚未完成，拒绝把操作日志标记为已清理".to_string());
    }
    operation.backup = None;
    operation.cleanup_state = Some(CLEANUP_STATE_CLEANED.to_string());
    operation.updated_at = now_ms();
    session_link::save_operation(paths, &operation)
}

// ---------------------------------------------------------------------------
// 维护入口：补清理与保护上报
// ---------------------------------------------------------------------------

/// 单条维护记录的处理判定。
pub(super) enum Decision {
    Clean,
    Protect(String),
}

/// 按操作日志阶段判断能否回收（只有可靠终态才授权删除）。
pub(super) fn decide_by_operation(operation: Option<&Operation>) -> Decision {
    let Some(operation) = operation else {
        return Decision::Protect("缺少操作日志，无法证明业务已完成，已保留材料".to_string());
    };
    if operation.lifecycle_version != Some(OPERATION_LIFECYCLE_VERSION) {
        return Decision::Protect(
            "操作日志缺少本版生命周期标记，无法作为删除授权，已保留材料".to_string(),
        );
    }
    if operation.phase == OpPhase::Completed {
        return Decision::Clean;
    }
    if operation.phase == OpPhase::Abandoned {
        if operation.cleanup_state.as_deref() == Some(CLEANUP_STATE_SAFE_TERMINATED) {
            return Decision::Clean;
        }
        return Decision::Protect(
            "操作已放弃但缺少「已验证安全终止」标记，已保留材料待人工确认".to_string(),
        );
    }
    Decision::Protect(
        operation
            .last_error
            .clone()
            .unwrap_or_else(|| "业务尚未完成，待恢复后清理".to_string()),
    )
}

/// 维护一个档位的临时备份：回收可安全回收的残留，保护其余并返回上报项。
///
/// 调用方必须已持有档位操作锁。扫描不完整（目录不可读/枚举失败/记录损坏）时
/// 本轮不做任何删除——无法限定影响范围时宁可保留。
pub fn maintain(paths: &SessionPaths, variant: WbVariant) -> Vec<TemporaryFileIssue> {
    let mut issues: Vec<TemporaryFileIssue> = Vec::new();
    let operation_scan = session_link::scan_operations(paths);
    let lifecycle_scan = scan_lifecycle(paths);

    for problem in &operation_scan.problems {
        issues.push(TemporaryFileIssue::needs_recovery(
            String::new(),
            None,
            None,
            format!("{OP_SCAN_PROBLEM_PREFIX}{problem}"),
        ));
    }
    for problem in &lifecycle_scan.problems {
        issues.push(TemporaryFileIssue::needs_recovery(
            String::new(),
            None,
            None,
            problem.clone(),
        ));
    }
    // 任何一处读不到/解析不了（操作日志或维护记录）都无法限定影响范围：本轮不删除。
    let deletions_allowed = operation_scan.complete
        && operation_scan.problems.is_empty()
        && lifecycle_scan.problems.is_empty();

    for mut record in lifecycle_scan
        .records
        .into_iter()
        .filter(|record| record.variant == variant)
    {
        let operation = operation_scan.operations.iter().find(|operation| {
            operation.operation_id == record.operation_id && operation.variant == variant
        });
        let decision = match record.state {
            BackupState::CleanupPending => Decision::Clean,
            BackupState::Allocating => match operation {
                // 本版契约：protected 之前禁止业务写入；扫描完整且无操作日志 ⇒ 尚未写业务。
                None if deletions_allowed => Decision::Clean,
                None => Decision::Protect(
                    "无法确认是否已写业务（扫描不完整），已保留准备残留".to_string(),
                ),
                Some(operation) => decide_by_operation(Some(operation)),
            },
            BackupState::Protected => decide_by_operation(operation),
        };
        match decision {
            Decision::Clean => {
                if !deletions_allowed {
                    issues.push(TemporaryFileIssue::needs_recovery(
                        record.operation_id.clone(),
                        record.session_id.clone(),
                        record.title.clone(),
                        "本轮扫描不完整，已跳过清理".to_string(),
                    ));
                    continue;
                }
                // 补转 cleanupPending 失败也继续尝试清理：目录归属由记录推导，
                // 不需要状态推进成功；失败时记录仍在，下次继续。
                let _ = mark_cleanup_pending(paths, &mut record, None);
                match cleanup_operation(paths, variant, &record) {
                    CleanupOutcome::Cleaned => {}
                    CleanupOutcome::Pending { reason } => {
                        issues.push(TemporaryFileIssue::cleanup_pending(
                            record.operation_id.clone(),
                            record.session_id.clone(),
                            record.title.clone(),
                            reason,
                        ));
                    }
                    CleanupOutcome::Protected { reason } => {
                        issues.push(TemporaryFileIssue::needs_recovery(
                            record.operation_id.clone(),
                            record.session_id.clone(),
                            record.title.clone(),
                            reason,
                        ));
                    }
                }
            }
            Decision::Protect(reason) => {
                issues.push(TemporaryFileIssue::needs_recovery(
                    record.operation_id.clone(),
                    record.session_id.clone(),
                    record.title.clone(),
                    reason,
                ));
            }
        }
    }
    issues
}

/// 清理失败后的即时落盘：把失败原因写回维护记录，作为下次补清理的依据。
pub fn record_cleanup_failure(
    paths: &SessionPaths,
    record: &BackupLifecycle,
    reason: &str,
) -> Result<(), String> {
    let mut candidate = record.clone();
    candidate.state = BackupState::CleanupPending;
    candidate.updated_at = now_ms();
    candidate.last_error = Some(reason.to_string());
    save_lifecycle(paths, &candidate)
}

/// 回收一个操作并处理失败：失败原因写回维护记录，业务结果不受影响。
pub fn cleanup_after_success(
    paths: &SessionPaths,
    variant: WbVariant,
    record: &BackupLifecycle,
) -> CleanupOutcome {
    let outcome = cleanup_operation(paths, variant, record);
    if let CleanupOutcome::Pending { reason } = &outcome {
        let _ = record_cleanup_failure(paths, record, reason);
    }
    outcome
}

/// 受控失败分支：尚未写业务（contract：protected 之前禁止业务写入）时回收自己的残留。
///
/// 先把「已验证安全终止」可靠写回维护记录再回收；写不进去就保留 allocating 记录，
/// 由下次维护入口按「无操作日志」判定回收。
pub fn reclaim_unwritten(
    paths: &SessionPaths,
    variant: WbVariant,
    record: &mut BackupLifecycle,
    reason: &str,
) -> CleanupOutcome {
    let mut candidate = record.clone();
    candidate.state = BackupState::CleanupPending;
    candidate.updated_at = now_ms();
    candidate.last_error = Some(reason.to_string());
    match save_lifecycle(paths, &candidate) {
        Ok(()) => {
            *record = candidate;
            cleanup_operation(paths, variant, record)
        }
        Err(error) => CleanupOutcome::Pending {
            reason: format!("安全终止标记写入失败（{error}），已保留准备残留待下次维护"),
        },
    }
}
