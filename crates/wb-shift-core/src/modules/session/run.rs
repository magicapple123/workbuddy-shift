//! 会话同步的公共入口与执行编排。
//!
//! 本模块把「读侧」的预览/计划（`sync`）与「写侧」的备份/阶段化执行（`exec`）串起来：
//! 先恢复未完成操作、再逐项校验并执行。恢复逻辑本身在 `recovery`，这里只负责调用顺序，
//! 避免 `sync` 与 `exec` 互相依赖形成环。

use super::copy::*;
use super::exec::execute_sync_item;
use super::recovery::{
    recover_pending_session_operations_at, recover_pending_session_operations_at_with,
};
use super::sync::*;
use super::*;

/// 重新校验勾选的同步项并返回 `sessionSync` 报告（design §5 / §6）。
///
/// 每项走「解析 → 重新加载身份/成员/基线/正文 → 核对预览凭据 → 重新判定 → 备份 →
/// 阶段化写入」：任一版本变化都跳过该项（原因码 [`REASON_PREVIEW_STALE`]），
/// 只在全部校验通过后才写目标；写入到 `completed` 才算 `synced`，
/// 中断的操作保留记录并置 `needsRecovery`，绝不报成成功。
///
/// 调用方需保证 App 已停止写入：本函数自行获取档位操作锁，并在加锁后复查一次。
pub fn sync_sessions_for_switch(
    target_acc: &Value,
    selections: &[SyncSelection],
) -> Result<Value, String> {
    let variant = account::variant_of(target_acc);
    let paths = SessionPaths::for_variant(variant);
    sync_sessions_for_switch_at(
        &paths,
        variant,
        target_acc,
        selections,
        process::is_workbuddy_running,
    )
}

/// [`sync_sessions_for_switch`] 的可注入版本：路径、档位与「App 是否运行」探针
/// 均由调用方传入（单测注入临时目录与假探针，不探测真实进程）。
pub(super) fn sync_sessions_for_switch_at(
    paths: &SessionPaths,
    variant: WbVariant,
    target_acc: &Value,
    selections: &[SyncSelection],
    is_app_running: impl Fn(WbVariant) -> bool,
) -> Result<Value, String> {
    let mut synced: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    if selections.is_empty() {
        return Ok(json!({ "synced": synced, "skipped": skipped, "errors": errors }));
    }
    if variant == WbVariant::Ai && !session_copy_supported_at(&paths.data_root) {
        return Err(format!(
            "{SESSION_SYNC_UNSUPPORTED}（档位 {}）",
            variant.as_str()
        ));
    }
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法同步会话".to_string());
    }
    let source_uid = current_user_uid_at(&paths.auth_file)
        .ok_or_else(|| "未读取到本机登录态，无法确定来源账号".to_string())?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需同步会话".to_string());
    }
    // 与复制同一条门禁：同档路径的源恒为当前登录账号，这里恒为放行（统一规则）。
    if app_running_blocks_target_write(paths, variant, &target_uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }

    // 档位操作锁与复制共用：预览后的校验与写入都不得与并发复制交错。
    let _ops_lock = session_link::try_acquire_variant_ops_lock(paths, variant)?;
    // 复查：锁前未运行、拿锁后目标账号被登录进 App 的情况在这里被拦住。
    if app_running_blocks_target_write(paths, variant, &target_uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    let recovery = recover_pending_session_operations_at(paths, variant);
    let pending = session_link::pending_operations(paths, variant);
    let (store, store_unavailable) = match session_link::load_store(paths) {
        StoreState::Ready(store) => (Some(store), None),
        StoreState::Missing => (None, None),
        StoreState::Unavailable(reason) => (None, Some(reason)),
    };
    // 关系表损坏/未知版本，或存在解析不出来的操作日志：不能当成没有关联继续校验。
    let store_broken = store_unavailable.is_some();
    let blocked = store_unavailable.or_else(|| {
        recovery
            .needs_recovery
            .iter()
            .find(|issue| !issue.retryable && issue.reason.contains(UNPARSEABLE_OPERATION_REASON))
            .map(|issue| issue.reason.clone())
    });
    let context = SyncContext {
        paths,
        variant,
        source_paths: paths,
        source_variant: variant,
        source_uid: &source_uid,
        source_account_id: account_id_for_uid(paths, &source_uid),
        target_uid: &target_uid,
        target_account_id: nonempty_text(
            target_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
        pending: &pending,
    };
    match blocked {
        Some(reason) => {
            for selection in selections {
                errors.push(json!({ "groupId": selection.group_id, "error": reason }));
            }
        }
        None => {
            for selection in selections {
                match plan_sync_selection(&context, store.as_ref(), selection) {
                    SyncItemOutcome::Validated {
                        mode,
                        verdict,
                        plan,
                    } => match execute_sync_item(&context, &plan, mode, verdict) {
                        Ok(item) => synced.push(item),
                        Err(error) => {
                            errors.push(json!({ "groupId": selection.group_id, "error": error }))
                        }
                    },
                    SyncItemOutcome::Skipped {
                        reason,
                        message,
                        verdict,
                    } => skipped.push(json!({
                        "groupId": selection.group_id,
                        "status": "skipped",
                        "reasonCode": reason,
                        "message": message,
                        "verdict": verdict.map(SyncVerdict::as_str),
                    })),
                    SyncItemOutcome::Rejected { message } => {
                        errors.push(json!({ "groupId": selection.group_id, "error": message }))
                    }
                }
            }
        }
    }

    let unfinished_after = session_link::pending_operations(paths, variant);
    let needs_recovery = store_broken || !recovery.is_clean() || !unfinished_after.is_empty();
    let mut report = json!({ "synced": synced, "skipped": skipped, "errors": errors });
    if needs_recovery {
        report["needsRecovery"] = json!(true);
    }
    // 本轮同步之后再扫一遍：当前项的清理失败/保护残留必须出现在报告里。
    report["temporaryFiles"] = json!(session_backup::maintain(paths, variant));
    if let Some(items) = report.get_mut("synced").and_then(Value::as_array_mut) {
        reconcile_reported_cleanup(items);
    }
    // 置顶传导：组内源成员被置顶时，其它成员的副本也应置顶（见 `copy/pinned.rs`）。
    // 只对本次真正同步成功的组做；失败只落报告字段，不影响同步结果。
    if let Some(store) = store.as_ref() {
        let group_ids: Vec<String> = report
            .get("synced")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("groupId")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !group_ids.is_empty() {
            report["pinned"] = sync_pinned_for_groups(store, &source_uid, &group_ids).to_json();
        }
    }
    Ok(report)
}

/// 把**显式来源账号**的新增内容同步到**显式目标账号**的关联会话（会话管理页；跨档支持）。
///
/// 与 [`sync_sessions_for_switch`] 的差异只在来源判定：源 uid 取自 `source_acc`，
/// 不再从登录态读取；源正文与源行按源档读取，写入侧仍以目标账号为准
/// （目标档数据根 + 备份 + 基线提交）。现有切号入口不受影响（同档语义逐字保留）。
pub fn sync_sessions_cross(
    source_acc: &Value,
    target_acc: &Value,
    selections: &[SyncSelection],
) -> Result<Value, String> {
    let source_variant = account::variant_of(source_acc);
    let target_variant = account::variant_of(target_acc);
    let source_paths = SessionPaths::for_variant(source_variant);
    let target_paths = SessionPaths::for_variant(target_variant);
    let source_uid = account_uid(source_acc);
    if source_uid.is_empty() {
        return Err("源账号缺少 uid，无法同步会话".to_string());
    }
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法同步会话".to_string());
    }
    let source = CopySide {
        paths: &source_paths,
        variant: source_variant,
        uid: source_uid,
        account_id: nonempty_text(
            source_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
    };
    let target = CopySide {
        paths: &target_paths,
        variant: target_variant,
        uid: target_uid,
        account_id: nonempty_text(
            target_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
    };
    sync_sessions_cross_at(
        &source,
        &target,
        selections,
        process::is_workbuddy_running,
        SessionPaths::for_variant,
    )
}

/// 可注入路径与「App 是否运行」探针的跨档同步入口（单测注入临时双档目录与假探针）。
pub(super) fn sync_sessions_cross_at(
    source: &CopySide<'_>,
    target: &CopySide<'_>,
    selections: &[SyncSelection],
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    let variant = target.variant;
    let mut synced: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    if selections.is_empty() {
        return Ok(json!({ "synced": synced, "skipped": skipped, "errors": errors }));
    }
    // 与复制同一条门禁：仅当目标账号是目标档当前登录账号时才要求关闭客户端
    // （源档只读，运行中的源客户端不影响读取安全）。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    // 源档也要读（源正文与源行），国际版数据根不同构时同样不支持（与复制同口径）。
    if source.variant == WbVariant::Ai && !session_copy_supported_at(&source.paths.data_root) {
        return Err(format!(
            "{SESSION_SYNC_UNSUPPORTED}（档位 {}）",
            source.variant.as_str()
        ));
    }
    if target.variant == WbVariant::Ai && !session_copy_supported_at(&target.paths.data_root) {
        return Err(format!(
            "{SESSION_SYNC_UNSUPPORTED}（档位 {}）",
            target.variant.as_str()
        ));
    }
    if source.variant == target.variant && source.uid == target.uid {
        return Err("源账号与目标账号相同，无需同步会话".to_string());
    }

    // 双档操作锁：获取顺序固定为枚举序（Cn → Ai），任何代码路径不得反序（design §2.5）。
    let mut _ops_locks = Vec::new();
    for lock_variant in WbVariant::ALL {
        if lock_variant == source.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                source.paths,
                lock_variant,
            )?);
        } else if lock_variant == target.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                target.paths,
                lock_variant,
            )?);
        }
    }
    // 复查：锁前未运行、拿锁后目标账号被登录进 App 的情况在这里被拦住，不写入任何产物。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    let recovery =
        recover_pending_session_operations_at_with(target.paths, variant, resolve_source_paths);
    let pending = session_link::pending_operations(target.paths, variant);
    let (store, store_unavailable) = match session_link::load_store(target.paths) {
        StoreState::Ready(store) => (Some(store), None),
        StoreState::Missing => (None, None),
        StoreState::Unavailable(reason) => (None, Some(reason)),
    };
    // 关系表损坏/未知版本，或存在解析不出来的操作日志：不能当成没有关联继续校验。
    let store_broken = store_unavailable.is_some();
    let blocked = store_unavailable.or_else(|| {
        recovery
            .needs_recovery
            .iter()
            .find(|issue| !issue.retryable && issue.reason.contains(UNPARSEABLE_OPERATION_REASON))
            .map(|issue| issue.reason.clone())
    });
    let context = SyncContext {
        paths: target.paths,
        variant: target.variant,
        source_paths: source.paths,
        source_variant: source.variant,
        source_uid: &source.uid,
        source_account_id: source.account_id.clone(),
        target_uid: &target.uid,
        target_account_id: target.account_id.clone(),
        pending: &pending,
    };
    match blocked {
        Some(reason) => {
            for selection in selections {
                errors.push(json!({ "groupId": selection.group_id, "error": reason }));
            }
        }
        None => {
            for selection in selections {
                match plan_sync_selection(&context, store.as_ref(), selection) {
                    SyncItemOutcome::Validated {
                        mode,
                        verdict,
                        plan,
                    } => match execute_sync_item(&context, &plan, mode, verdict) {
                        Ok(item) => synced.push(item),
                        Err(error) => {
                            errors.push(json!({ "groupId": selection.group_id, "error": error }))
                        }
                    },
                    SyncItemOutcome::Skipped {
                        reason,
                        message,
                        verdict,
                    } => skipped.push(json!({
                        "groupId": selection.group_id,
                        "status": "skipped",
                        "reasonCode": reason,
                        "message": message,
                        "verdict": verdict.map(SyncVerdict::as_str),
                    })),
                    SyncItemOutcome::Rejected { message } => {
                        errors.push(json!({ "groupId": selection.group_id, "error": message }))
                    }
                }
            }
        }
    }

    let unfinished_after = session_link::pending_operations(target.paths, variant);
    let needs_recovery = store_broken || !recovery.is_clean() || !unfinished_after.is_empty();
    let mut report = json!({ "synced": synced, "skipped": skipped, "errors": errors });
    if needs_recovery {
        report["needsRecovery"] = json!(true);
    }
    report["temporaryFiles"] = json!(session_backup::maintain(target.paths, variant));
    if let Some(items) = report.get_mut("synced").and_then(Value::as_array_mut) {
        reconcile_reported_cleanup(items);
    }
    // 与 `sync_sessions_for_switch_at` 同口径：显式来源账号的置顶跟到组内其它成员。
    if let Some(store) = store.as_ref() {
        let group_ids: Vec<String> = report
            .get("synced")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("groupId")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !group_ids.is_empty() {
            report["pinned"] = sync_pinned_for_groups(store, &source.uid, &group_ids).to_json();
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
