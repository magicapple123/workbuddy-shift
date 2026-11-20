//! 复制流程编排：公开入口、锁与恢复顺序、逐会话循环与结果报告。
use super::super::recovery::recover_pending_session_operations_at_with;
use super::*;
/// 云端映射登记结果：失败必须上报，不能静默降级成成功。
/// 把勾选的会话复制到目标账号（路径 B）。返回复制报告。
///
/// 档位以**目标账号**自身为准：数据根、数据库、备份目录、认证文件都取该档位。
/// 国际版能力不满足时直接返回明确错误，绝不写半成品（design D6）。
/// App 正在运行时拒绝写入：独立复制 API 与桌面端共用同一条生命周期保护。
pub fn copy_sessions_for_switch(
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
) -> Result<Value, String> {
    let variant = account::variant_of(target_acc);
    let paths = SessionPaths::for_variant(variant);
    copy_sessions_for_switch_at(
        &paths,
        variant,
        target_acc,
        session_ids,
        ignore_aux,
        process::is_workbuddy_running,
    )
}

/// 把勾选的会话从**显式源账号**复制到**显式目标账号**（跨档复制入口，design §2.1）。
///
/// 与 [`copy_sessions_for_switch`] 的差异只在来源判定：源 uid 取自 `source_acc`，
/// 不再从目标档登录态读取；源档与目标档可为不同档位。写入侧仍以目标账号为准
/// （数据根、数据库、映射库、备份目录），源档只读。
pub fn copy_sessions_cross(
    source_acc: &Value,
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
) -> Result<Value, String> {
    let source_variant = account::variant_of(source_acc);
    let target_variant = account::variant_of(target_acc);
    let source_paths = SessionPaths::for_variant(source_variant);
    let target_paths = SessionPaths::for_variant(target_variant);
    let source_uid = account_uid(source_acc);
    if source_uid.is_empty() {
        return Err("源账号缺少 uid，无法复制会话".to_string());
    }
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法复制会话".to_string());
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
    copy_sessions_cross_at(
        source,
        target,
        session_ids,
        ignore_aux,
        process::is_workbuddy_running,
        SessionPaths::for_variant,
    )
}

/// 可注入路径与「App 是否运行」探针的跨档复制入口（单测注入临时双档目录与假探针，
/// 不触碰真实路径、不探测真实进程）。
pub(in crate::modules::session) fn copy_sessions_cross_at(
    source: CopySide<'_>,
    target: CopySide<'_>,
    session_ids: &[String],
    ignore_aux: bool,
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    // 快拒只针对**目标档**（写侧）：源档只读，运行中的源客户端不影响读取安全；
    // 目标账号不是当前登录账号时免关直接写（实测，见门禁函数注释）。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    // 源档也要读（正文与源行），国际版数据根不同构时同样不支持（design D6）。
    ensure_session_copy_supported(source.paths, source.variant)?;
    ensure_session_copy_supported(target.paths, target.variant)?;
    if source.variant == target.variant && source.uid == target.uid {
        return Err("源账号与目标账号相同，无需复制会话".to_string());
    }
    run_copy(
        &source,
        &target,
        session_ids,
        ignore_aux,
        is_app_running,
        resolve_source_paths,
    )
}

/// 可注入路径与「App 是否运行」探针的同档复制入口（单测注入临时目录与假探针，不触碰
/// 真实路径、不探测真实进程）。
///
/// App 运行检查做两次：拿档位操作锁之前先快速拒绝；拿锁之后再复查——锁前到拿锁之间
/// App 可能被启动，只有锁后复查才能保证会话写入发生在 App 停止写入之后（design §4.1）。
pub(in crate::modules::session) fn copy_sessions_for_switch_at(
    paths: &SessionPaths,
    variant: WbVariant,
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
    is_app_running: impl Fn(WbVariant) -> bool,
) -> Result<Value, String> {
    // 探测只对国际版生效（design D6 针对的是国际版数据根不同构）。国内版数据根与
    // 改造前同构，保留改造前的路径与返回结构，不让国内版看到「暂不支持」类新文案。
    ensure_session_copy_supported(paths, variant)?;
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法复制会话".to_string());
    }
    let source_uid = current_user_uid_at(&paths.auth_file)
        .ok_or_else(|| "未读取到本机登录态，无法确定来源账号".to_string())?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需复制会话".to_string());
    }
    // 同档路径的源恒为当前登录账号、且上面已拦同账号，因此这里对"目标=当前登录"
    // 恒为放行；保留统一门禁只为与其他写入入口共用同一条规则。
    if app_running_blocks_target_write(paths, variant, &target_uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    let source = CopySide {
        paths,
        variant,
        account_id: account_id_for_uid(paths, &source_uid),
        uid: source_uid,
    };
    let target = CopySide {
        paths,
        variant,
        account_id: nonempty_text(
            target_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
        uid: target_uid,
    };
    run_copy(
        &source,
        &target,
        session_ids,
        ignore_aux,
        is_app_running,
        SessionPaths::for_variant,
    )
}

/// 国际版支持性探测的统一封装：只有国际版需要数据根同构检查（design D6）。
pub(in crate::modules::session) fn ensure_session_copy_supported(
    paths: &SessionPaths,
    variant: WbVariant,
) -> Result<(), String> {
    if variant == WbVariant::Ai && !session_copy_supported_at(&paths.data_root) {
        return Err(format!(
            "{SESSION_COPY_UNSUPPORTED}（档位 {}）",
            variant.as_str()
        ));
    }
    Ok(())
}

/// 复制的一方：档位数据根、档位名与账号身份（源侧只读、目标侧写入）。
pub(in crate::modules::session) struct CopySide<'a> {
    pub(in crate::modules::session) paths: &'a SessionPaths,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) uid: String,
    /// 账号库中的账号 id（仅作展示，身份判定以 uid 为准）。
    pub(in crate::modules::session) account_id: Option<String>,
}

/// 同档与跨档复制的统一流程：双档锁 → 恢复 → 逐会话复制 → 报告。
///
/// `resolve_source_paths` 用于恢复未完成的**跨档**操作时解析其源档数据根；
/// 同档入口传 [`SessionPaths::for_variant`]，跨档入口可注入与两条 side 一致的映射，
/// 单测由此避免触碰真实目录。
pub(in crate::modules::session) fn run_copy(
    source: &CopySide<'_>,
    target: &CopySide<'_>,
    session_ids: &[String],
    ignore_aux: bool,
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    // 档位操作锁覆盖「恢复 → 查询关联 → 写副本 → 提交关联」全过程，
    // 并发请求与中断重试因此不会各自写出第二个副本。
    // 跨档操作同时持有两档锁，获取顺序固定为枚举序（Cn → Ai），任何代码路径不得
    // 反序，避免两个方向的跨档操作互相等待（design §2.5）。
    let mut _ops_locks = Vec::new();
    for variant in WbVariant::ALL {
        if variant == source.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                source.paths,
                variant,
            )?);
        } else if variant == target.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                target.paths,
                variant,
            )?);
        }
    }
    // 复查：锁前未运行、拿锁后目标账号被登录进 App 的情况在这里被拦住，不写入任何产物。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    // 「仍然复制（不带用量）」的重试：上一次失败时操作停在 BodyWritten，本次请求要把
    // 忽略标记**先落到操作记录**，随后的恢复重放才会跳过账本并补完同一条会话。
    if ignore_aux {
        mark_aux_skipped_for_requests(target, source, session_ids);
    }
    let recovery = recover_pending_session_operations_at_with(
        target.paths,
        target.variant,
        resolve_source_paths,
    );
    let pending = session_link::pending_operations(target.paths, target.variant);

    let context = CopyContext {
        paths: target.paths,
        variant: target.variant,
        source_paths: source.paths,
        source_variant: source.variant,
        source_uid: &source.uid,
        source_account_id: source.account_id.clone(),
        target_uid: &target.uid,
        target_account_id: target.account_id.clone(),
        pending: &pending,
        ignore_aux,
    };

    let mut copied: Vec<Value> = Vec::new();
    let mut already_linked: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    // 可「显式忽略」的失败（账本写入）：前端据此给「仍然复制」按钮。
    let mut aux_failed: Vec<Value> = Vec::new();
    // 解析失败的操作日志无法对应到具体会话：继续复制会绕过 pending 去重，
    // 可能对同一请求再写出第二个副本。
    if let Some(issue) = recovery
        .needs_recovery
        .iter()
        .find(|issue| !issue.retryable && issue.reason.contains(UNPARSEABLE_OPERATION_REASON))
    {
        for cid in session_ids {
            errors.push(json!({"id": cid, "error": issue.reason.clone()}));
        }
    } else {
        for cid in session_ids {
            match copy_one_session(&context, cid) {
                Ok(CopyOutcome::Copied {
                    new_id,
                    group_id,
                    backup,
                    cleanup_state,
                    cleanup_error,
                    source_repaired,
                    aux_note,
                    space_note,
                }) => {
                    let mut item = json!({
                        "id": cid,
                        "newId": new_id,
                        "groupId": group_id,
                        "backup": backup,
                        "cleanupState": cleanup_state,
                    });
                    if let Some(error) = cleanup_error {
                        item["cleanupError"] = json!(error);
                    }
                    // 源正文有坏行、复制时做了容错处理：必须让用户知道副本与原会话不逐字相同。
                    if let Some(note) = source_repaired {
                        item["sourceRepaired"] = json!(note);
                    }
                    // 目标库缺账本表/列：不算失败，但要让用户知道「共消耗」为什么还是少。
                    if let Some(note) = aux_note {
                        item["auxNote"] = json!(note);
                    }
                    // 同一目录命中多个目标空间：本次按目录新建了分组，别让用户以为串组了。
                    if let Some(note) = space_note {
                        item["spaceNote"] = json!(note);
                    }
                    copied.push(item);
                }
                Ok(CopyOutcome::AlreadyLinked {
                    session_id,
                    group_id,
                }) => already_linked.push(json!({
                    "id": cid,
                    "sessionId": session_id,
                    "groupId": group_id,
                })),
                Err(error) => {
                    // 账本写入失败是唯一带「显式忽略」出口的失败：报告里点明可忽略，
                    // 前端据此给「仍然复制（不带用量与中断记录）」按钮。
                    if is_aux_write_failure(&error) {
                        aux_failed.push(json!(cid));
                    }
                    errors.push(json!({"id": cid, "error": error}));
                }
            }
        }
    }

    // 本次请求之后仍存在未完成操作（含本次刚留下的）→ 必须提示恢复需求。
    let unfinished_after = session_link::pending_operations(target.paths, target.variant);
    let unusable = !recovery.is_clean() || !unfinished_after.is_empty();
    let mut report = json!({
        "sourceUid": &source.uid,
        "targetUid": &target.uid,
        "copied": copied,
        "alreadyLinked": already_linked,
    });
    // 跨档复制在报告里带两侧档位；同档报告保持原字段（逐字不变）。
    if source.variant != target.variant {
        report["sourceVariant"] = json!(source.variant.as_str());
        report["targetVariant"] = json!(target.variant.as_str());
    }
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    // 只有账本写入失败才可忽略：给出「仍然复制（不带用量与中断记录）」入口所需的
    // 会话 id；其它失败一律没有这个出口（不该用忽略掩盖数据缺失）。
    if !aux_failed.is_empty() {
        report["canIgnoreAux"] = json!(true);
        report["auxFailed"] = json!(aux_failed);
    }
    // 本次请求本身带了忽略标记：让前端能把「已按忽略处理」如实提示出来。
    if ignore_aux {
        report["auxSkipped"] = json!(true);
    }
    // 恢复重放补完的会话：重试「仍然复制」时它们不会出现在 copied 里（已是既有副本），
    // 不说明的话用户会以为没生效。
    if !recovery.recovered.is_empty() {
        report["recovered"] = json!(recovery.recovered);
    }
    if unusable {
        report["needsRecovery"] = json!(true);
    }
    // 本轮复制之后再扫一遍：当前项的清理失败/保护残留必须出现在报告里，
    // 不能只用请求开始时的维护快照（否则成功项 pending 只在 copied[] 上）。
    // 维护可能补清成功：成功项上的 pending 路径必须改写成 null，避免虚假可还原位置。
    report["temporaryFiles"] = json!(session_backup::maintain(target.paths, target.variant));
    if let Some(items) = report.get_mut("copied").and_then(Value::as_array_mut) {
        reconcile_reported_cleanup(items);
    }
    Ok(report)
}

/// 复制上下文中不变的输入（避免逐会话重复解析）。
///
/// 源 / 目标解耦：`paths` 恒为目标档（写侧），`source_paths` 恒为源档（读侧）；
/// 同档复制时两者相同，跨档复制时指向不同数据根。
pub(in crate::modules::session) struct CopyContext<'a> {
    pub(in crate::modules::session) paths: &'a SessionPaths,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) source_paths: &'a SessionPaths,
    pub(in crate::modules::session) source_variant: WbVariant,
    pub(in crate::modules::session) source_uid: &'a str,
    pub(in crate::modules::session) source_account_id: Option<String>,
    pub(in crate::modules::session) target_uid: &'a str,
    pub(in crate::modules::session) target_account_id: Option<String>,
    pub(in crate::modules::session) pending: &'a [Operation],
    /// 用户显式忽略账本（用量 + 中断记录）：本次新建的操作记录带 `auxSkipped`。
    pub(in crate::modules::session) ignore_aux: bool,
}

/// 把「忽略账本」标记落到**未完成**操作的记录上（「仍然复制」的重试路径）。
///
/// 只改与本次请求一一对应的操作（同源会话、同目标账号、未完成），不做全量清理；
/// 保存失败不阻断本次流程——后面同一处会再失败一次，错误照样报给用户。
fn mark_aux_skipped_for_requests(
    target: &CopySide<'_>,
    source: &CopySide<'_>,
    session_ids: &[String],
) {
    for operation in session_link::pending_operations(target.paths, target.variant) {
        if operation.aux_skipped
            || operation.source.uid != source.uid
            || operation.target.uid != target.uid
            || !session_ids
                .iter()
                .any(|cid| *cid == operation.source.session_id)
        {
            continue;
        }
        let mut skipped = operation;
        skipped.aux_skipped = true;
        skipped.updated_at = now_ms();
        let _ = session_link::save_operation(target.paths, &skipped);
    }
}

/// 单个会话的一次复制结果。
pub(in crate::modules::session) enum CopyOutcome {
    Copied {
        new_id: String,
        group_id: String,
        /// 待清理位置（已清理为 None）；仅表示待清理，不是可撤销备份。
        backup: Option<String>,
        cleanup_state: String,
        cleanup_error: Option<String>,
        /// 源正文有坏行、复制时做了容错修复时的人话说明（报告里为 `sourceRepaired`）。
        /// `None` 表示源正文严格校验一次通过，副本与原会话逐字同源。
        source_repaired: Option<String>,
        /// 账本搬运的诊断说明（报告里为 `auxNote`）：目标客户端不支持时为 `Some`。
        aux_note: Option<&'static str>,
        /// 空间归属的诊断说明（报告里为 `spaceNote`）：同目录多分组时按目录新建。
        space_note: Option<&'static str>,
    },
    AlreadyLinked {
        session_id: String,
        group_id: String,
    },
}
