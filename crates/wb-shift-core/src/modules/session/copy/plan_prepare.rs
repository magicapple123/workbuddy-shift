//! 复制前的准备与校验：目标解析、成员有效性、关联存储就绪，以及备份清理报告。
use super::plan::CopyContext;
use super::*;
/// 目标解析：组内目标账号是否已有可复用的有效副本。
pub(in crate::modules::session) struct TargetResolution {
    pub(in crate::modules::session) group_id: Option<String>,
    pub(in crate::modules::session) existing_link: Option<String>,
}

/// 目标账号在关联组内的 active 成员是否真实有效：正文可验证 + 会话行归属正确。
pub(in crate::modules::session) fn member_is_valid(
    paths: &SessionPaths,
    member: &LinkMember,
) -> bool {
    let Some(body) = find_project_jsonl(paths, &member.session_id) else {
        return false;
    };
    match session_link::read_content_snapshot(&body, &member.session_id) {
        ContentState::Ready(_) => {}
        ContentState::Missing | ContentState::Unavailable(_) => return false,
    }
    session_row_owner(paths, &member.session_id).is_some_and(|owner| owner == member.uid)
}

/// 解析（variant, 来源会话）所属组，以及目标账号是否已有有效副本。
pub(in crate::modules::session) fn resolve_target(
    context: &CopyContext,
    cid: &str,
) -> Result<TargetResolution, String> {
    let store = match session_link::load_store(context.paths) {
        StoreState::Missing => {
            return Ok(TargetResolution {
                group_id: None,
                existing_link: None,
            })
        }
        StoreState::Ready(store) => store,
        StoreState::Unavailable(reason) => {
            return Err(format!("{reason}；已阻止复制"));
        }
    };
    // 身份查找按**源档**（跨档组的源成员记源档位，组级 variant 不参与判定）。
    let Some(group) = session_link::find_group_for_identity(
        &store,
        context.source_variant,
        context.source_uid,
        cid,
    ) else {
        return Ok(TargetResolution {
            group_id: None,
            existing_link: None,
        });
    };
    let group_id = Some(group.id.clone());
    let Some(member) = session_link::active_member_for(group, context.target_uid) else {
        return Ok(TargetResolution {
            group_id,
            existing_link: None,
        });
    };
    if member_is_valid(context.paths, member) {
        return Ok(TargetResolution {
            group_id,
            existing_link: Some(member.session_id.clone()),
        });
    }
    // 失效成员保留记录、不自动复活；本次会重建一个新成员。
    Ok(TargetResolution {
        group_id,
        existing_link: None,
    })
}

/// 首次使用时先落地空的关联存储。
///
/// 保证「操作日志出现」一定晚于「主文件存在」，否则刚预分配的操作日志会被
/// `load_store` 的残留痕迹规则误判成「主文件缺失但残留未完成操作」而自锁。
/// 存储损坏/未知版本/权限失败时同样在这里拒绝，绝不降级成空表。
pub(in crate::modules::session) fn ensure_link_store_ready(
    paths: &SessionPaths,
) -> Result<(), String> {
    match session_link::load_store(paths) {
        StoreState::Missing => {
            session_link::with_link_store_write(paths, |_| Ok(()))?;
            Ok(())
        }
        StoreState::Ready(_) => Ok(()),
        StoreState::Unavailable(reason) => Err(format!("{reason}；已阻止复制")),
    }
}

/// 业务完成后的收尾：可靠转 cleanupPending 再回收；失败只报告，不改业务结果。
pub(in crate::modules::session) fn finish_backup_cleanup(
    paths: &SessionPaths,
    variant: WbVariant,
    lifecycle: &mut BackupLifecycle,
) -> CleanupOutcome {
    if let Err(error) = session_backup::mark_cleanup_pending(paths, lifecycle, None) {
        // 业务已完成、维护记录仍是 protected：下次维护按 Completed 补转后清理。
        return CleanupOutcome::Pending {
            reason: format!("清理状态推进失败（{error}）"),
        };
    }
    session_backup::cleanup_after_success(paths, variant, lifecycle)
}

/// 维护入口补清成功后，把仍写着 pending 路径的成功项改成 cleaned / null。
///
/// `symlink_metadata` 把符号链接也视为「还在」，避免把拒绝删除的链接目标标成已清理。
pub(in crate::modules::session) fn reconcile_reported_cleanup(items: &mut [Value]) {
    for item in items {
        if item.get("cleanupState").and_then(Value::as_str) != Some("pending") {
            continue;
        }
        let still_there = item
            .get("backup")
            .and_then(Value::as_str)
            .is_some_and(|path| std::fs::symlink_metadata(path).is_ok());
        if still_there {
            continue;
        }
        item["backup"] = json!(null);
        if item.get("backupManifest").is_some() {
            item["backupManifest"] = json!(null);
        }
        item["cleanupState"] = json!("cleaned");
        if let Some(object) = item.as_object_mut() {
            object.remove("cleanupError");
        }
    }
}

/// 清理结果到报告字段的投影：已清理不展示路径；待清理保留位置与原因。
pub(in crate::modules::session) fn report_cleanup(
    cleanup: &CleanupOutcome,
    dir: &Path,
) -> (Option<String>, &'static str, Option<String>) {
    match cleanup {
        CleanupOutcome::Cleaned => (None, "cleaned", None),
        CleanupOutcome::Pending { reason } => (
            Some(dir.to_string_lossy().to_string()),
            "pending",
            Some(reason.clone()),
        ),
        CleanupOutcome::Protected { reason } => (
            Some(dir.to_string_lossy().to_string()),
            "pending",
            Some(reason.clone()),
        ),
    }
}
