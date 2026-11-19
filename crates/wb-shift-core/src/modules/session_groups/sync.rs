//! 显式配对同步、编辑器生命周期窗口与统一同步的收尾报告。

use super::*;

/// Preview one exact source/target member pair using the existing per-client preview-token kernel.
pub fn preview_pair(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    target_member_id: &str,
) -> Result<Value, String> {
    let context = resolve_pair(client, scope, group_id, source_member_id, target_member_id)?;
    preview_resolved_pair(client, scope, &context)
}

/// Sync one explicit pair. The existing kernel revalidates the namespace/group/member/content token.
#[allow(clippy::too_many_arguments)] // 参数都是本次同步的显式输入（与 rotate.rs 同口径）
pub fn sync_pair(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    target_member_id: &str,
    preview_token: &str,
    mode: &str,
    restart: bool,
) -> Result<Value, String> {
    let context = resolve_pair(client, scope, group_id, source_member_id, target_member_id)?;
    if preview_token.trim().is_empty() {
        return Err("缺少会话同步预览凭据".to_string());
    }
    let selection = parse_selection(group_id, preview_token, mode)?;
    // 校验（resolve / 凭据 / 模式）全部通过后再关编辑器：为注定失败的请求打断用户是无谓的。
    let guard = prepare_editor_window(client, restart, false, &[context.target.uid.as_str()])?;
    let result = run_pair_sync(client, scope, &context, &[selection], guard.is_some());
    settle_editor_window(guard, result)
}

pub(super) fn running_target_needs_restart(variant: WbVariant, target_uid: &str) -> bool {
    should_restart_running_target(
        process::is_workbuddy_running(variant),
        session::current_user_uid(variant).as_deref(),
        target_uid,
    )
}

pub(super) fn should_restart_running_target(
    running: bool,
    current_uid: Option<&str>,
    target_uid: &str,
) -> bool {
    running && current_uid.is_none_or(|uid| uid == target_uid)
}

pub(super) fn workbuddy_variant_label(variant: WbVariant) -> &'static str {
    match variant {
        WbVariant::Cn => "WorkBuddy 国内版",
        WbVariant::Ai => "WorkBuddy 国际版",
    }
}

pub(super) fn reopen_workbuddy_variants(variants: &[WbVariant]) -> Vec<String> {
    variants
        .iter()
        .filter_map(|variant| {
            process::launch_workbuddy(*variant, None)
                .err()
                .map(|error| format!("{}：{error}", workbuddy_variant_label(*variant)))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(super) struct PairContext {
    pub(super) group: LinkGroup,
    pub(super) source: LinkMember,
    pub(super) target: LinkMember,
    pub(super) source_account: Value,
    pub(super) target_account: Value,
}

pub(super) fn resolve_pair(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    target_member_id: &str,
) -> Result<PairContext, String> {
    client.validate_scope(scope)?;
    let paths = client.store_paths();
    let store = match session_link::load_store(&paths) {
        StoreState::Ready(store) => store,
        StoreState::Missing => return Err("会话关联组不存在".to_string()),
        // 与列表载荷的 `pruneBlocked`、批量清理的 `Err` 共用同一句（单一来源）。
        StoreState::Unavailable(reason) => {
            return Err(super::maintenance::store_unavailable_reason(&reason));
        }
    };
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id && group_is_in_scope(client, scope, group))
        .cloned()
        .ok_or_else(|| "会话关联组不存在或不属于当前客户端".to_string())?;
    let source = group
        .members
        .iter()
        .find(|member| member.member_id == source_member_id && member.state == MemberState::Active)
        .cloned()
        .ok_or_else(|| "来源成员已失效，请刷新会话组".to_string())?;
    let target = group
        .members
        .iter()
        .find(|member| member.member_id == target_member_id && member.state == MemberState::Active)
        .cloned()
        .ok_or_else(|| "目标成员已失效，请刷新会话组".to_string())?;
    if source.member_id == target.member_id || source.uid == target.uid {
        return Err("来源和目标必须是不同账号成员".to_string());
    }
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    let source_account = account_by_uid(&accounts, &source.uid)
        .ok_or_else(|| "来源账号已不在本机账号库中".to_string())?;
    let target_account = account_by_uid(&accounts, &target.uid)
        .ok_or_else(|| "目标账号已不在本机账号库中".to_string())?;
    validate_target_compatibility(client, scope, &target_account)?;
    Ok(PairContext {
        group,
        source,
        target,
        source_account,
        target_account,
    })
}

pub(super) fn preview_resolved_pair(
    client: SessionClient,
    scope: Option<WbVariant>,
    pair: &PairContext,
) -> Result<Value, String> {
    let paths = client.store_paths();
    let report = match client {
        SessionClient::Workbuddy => {
            session::session_links_preview_cross(&pair.source_account, &pair.target_account)?
        }
        SessionClient::CodebuddyIde => {
            let variant = scope.ok_or_else(|| "CodeBuddy IDE 缺少档位".to_string())?;
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 CodeBuddy IDE 会话数据目录".to_string())?;
            vscode_session_sync::links_preview_in_for_variant(
                CODEBUDDY_IDE_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
                variant,
            )?
        }
        SessionClient::VscodeExt => {
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 VS Code 插件会话数据目录".to_string())?;
            vscode_session_sync::links_preview_in(
                VSCODE_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
            )?
        }
        SessionClient::Jetbrains => {
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 JetBrains CodeBuddy 插件的会话数据目录".to_string())?;
            vscode_session_sync::links_preview_in(
                JETBRAINS_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
            )?
        }
    };
    let preview = report
        .get("groups")
        .and_then(Value::as_array)
        .and_then(|groups| {
            groups.iter().find(|item| {
                item.get("groupId").and_then(Value::as_str) == Some(pair.group.id.as_str())
            })
        })
        .cloned()
        .ok_or_else(|| "该组当前没有可预览的来源与目标副本".to_string())?;
    let mut modes = preview.get("availableModes").cloned().unwrap_or(json!([]));
    if preview.get("verdict").and_then(Value::as_str) == Some("ahead")
        && preview
            .get("previewToken")
            .and_then(Value::as_str)
            .is_some()
    {
        modes = json!(["unifyOverwrite"]);
    }
    Ok(json!({
        "client": client.as_str(),
        "variantScope": scope.map(WbVariant::as_str),
        "groupId": pair.group.id,
        "sourceMemberId": pair.source.member_id,
        "targetMemberId": pair.target.member_id,
        "verdict": preview.get("verdict").cloned().unwrap_or(Value::Null),
        "availableModes": modes,
        "previewToken": preview.get("previewToken").cloned().unwrap_or(Value::Null),
        "reason": preview.get("reason").cloned().unwrap_or(json!("无法确认")),
        "recordCount": preview.get("recordCount").cloned().unwrap_or(Value::Null),
        "extraTargetCount": preview.get("extraB").cloned().unwrap_or(json!(0)),
    }))
}

/// 关闭决策（纯函数）：`Ok(true)` = 需要关闭编辑器，`Ok(false)` = 未运行或不涉及、不关不拉。
///
/// 运行中的插件只会写「当前登录账号」的数据，因此仅当**本次写入目标包含当前登录账号**时才需要
/// 编辑器生命周期；`current_uid = None`（未登录/状态缺失）时保守按涉及处理——与 WorkBuddy 的
/// `current_uid.map_or(true, …)` 同口径。
pub(super) fn plan_editor_restart(
    running: bool,
    current_uid: Option<&str>,
    target_uids: &[&str],
    restart: bool,
    manual_hint: &str,
) -> Result<bool, String> {
    if !running {
        return Ok(false);
    }
    let involved = current_uid.is_none_or(|uid| target_uids.contains(&uid));
    if !involved {
        return Ok(false);
    }
    if !restart {
        return Err(manual_hint.to_string());
    }
    Ok(true)
}

/// 插件型客户端的「运行中」文案：按客户端与动作取词（用户可见文案的单一来源）。
pub(super) fn editor_running_hint(client: SessionClient, adding_member: bool) -> &'static str {
    match (client, adding_member) {
        (SessionClient::Jetbrains, false) => JETBRAINS_SYNC_RUNNING_HINT,
        (SessionClient::Jetbrains, true) => JETBRAINS_ADD_MEMBER_RUNNING_HINT,
        (_, false) => VSCODE_SYNC_RUNNING_HINT,
        (_, true) => VSCODE_ADD_MEMBER_RUNNING_HINT,
    }
}

/// 预检：当前是否需要关闭 VS Code 才能安全执行写操作（运行中 且 目标账号含当前登录账号）。
///
/// 供前端决定「是否先弹确认框」；判定与写入口共用 [`plan_editor_restart`]，最终把关仍在各写入口。
pub fn vscode_restart_precheck(target_account_ids: &[String]) -> Value {
    let paths = SessionClient::VscodeExt.store_paths();
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    let target_uids: Vec<String> = target_account_ids
        .iter()
        .filter_map(|id| {
            accounts
                .iter()
                .find(|item| account::get_str(item, "id").as_deref() == Some(id.as_str()))
        })
        .filter_map(|item| account::get_str(item, "uid"))
        .collect();
    let target_refs: Vec<&str> = target_uids.iter().map(String::as_str).collect();
    let running = vscode_ext::is_vscode_running();
    let current_uid = vscode_ext::active_ext_uid();
    // 以 `restart = true` 走同一决策（不会产生文案错误）：`Ok(true)` 即「需要关闭」。
    let required = plan_editor_restart(running, current_uid.as_deref(), &target_refs, true, "")
        .unwrap_or(false);
    json!({ "required": required, "running": running })
}

/// 预检：当前是否需要关闭 JetBrains IDE 才能安全执行写操作。
///
/// **与目标账号无关**：IDEA 与 PyCharm 共用同一份 `Data/<uid>/JetBrains`，只要还有装了插件
/// 且在运行的实例，写入就可能被它的内存态覆盖。参数只为与 [`vscode_restart_precheck`] 同形。
pub fn jetbrains_restart_precheck(_target_account_ids: &[String]) -> Value {
    jetbrains::jetbrains_restart_precheck()
}

/// 插件侧写操作的编辑器生命周期窗口（仅 `SessionClient::VscodeExt` 使用）。
///
/// - 未运行 → 不关闭、不主动拉起；
/// - 运行中 + `restart=false` → `Err(manual_hint)`（各入口保留自己的既有文案）；
/// - 运行中 + `restart=true` → 复用 [`vscode_ext::close_vscode_for_switch`] 优雅关闭，
///   窗口结束后 best-effort 重开。
///
/// **单窗口**：调用方在窗口内跑完全部目标才收尾，全程只关 / 开各一次；任何失败路径都必须
/// 经 [`fail`](Self::fail) 收尾，保证「关了的要开回来」，且不对未运行者主动拉起。
pub(super) struct VscodeEditorGuard {
    /// 本次由我们关闭的编辑器的重开动作；`None` = 本来就没运行（或关闭前已自行退出）。
    pub(super) relaunch: Option<Box<dyn FnOnce() -> Result<(), String>>>,
}

impl VscodeEditorGuard {
    /// 打开窗口：判定 →（必要时）关闭。校验类错误必须在此前完成，避免为注定失败的请求关编辑器。
    pub(super) fn prepare(
        restart: bool,
        manual_hint: &str,
        target_uids: &[&str],
    ) -> Result<Self, String> {
        let running = vscode_ext::is_vscode_running();
        let current_uid = vscode_ext::active_ext_uid();
        if !plan_editor_restart(
            running,
            current_uid.as_deref(),
            target_uids,
            restart,
            manual_hint,
        )? {
            return Ok(Self { relaunch: None });
        }
        match vscode_ext::close_vscode_for_switch(true)? {
            None => Ok(Self { relaunch: None }),
            Some(closed) => Ok(Self {
                relaunch: Some(Box::new(move || {
                    vscode_ext::relaunch_closed_editor(&closed)
                })),
            }),
        }
    }

    /// 本次窗口是否真的关闭了编辑器（报告字段只认「我们关的 + 重开成功」）。
    pub(super) fn closed_by_us(&self) -> bool {
        self.relaunch.is_some()
    }

    /// 成功收尾：重开（仅本次由我们关闭时）。`Err` = 重开失败，操作本身已成功。
    pub(super) fn finish(self) -> Result<(), String> {
        match self.relaunch {
            None => Ok(()),
            Some(relaunch) => relaunch(),
        }
    }

    /// 失败收尾：best-effort 重开，并把重开失败信息追加到原错误之后（不吞原错误）。
    pub(super) fn fail(self, error: String) -> String {
        match self.finish() {
            Ok(()) => error,
            Err(launch_error) => format!("{error}\n\n{launch_error}"),
        }
    }
}

/// 编辑器生命周期守卫：按客户端分派到 VS Code / JetBrains 两套实现。
///
/// 两者的语义完全一致（只重开自己关掉的、失败也尽力开回），差异只在「怎么关」——因此
/// 调用方只认这个枚举，收尾逻辑共用 [`settle_guard`]。
pub(super) enum EditorGuard {
    Vscode(VscodeEditorGuard),
    Jetbrains(JetbrainsIdeGuard),
}

impl EditorGuard {
    /// 报告 / 提示里的宿主名（重开失败的文案要指名是哪个宿主）。
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Vscode(_) => "VS Code",
            Self::Jetbrains(_) => "JetBrains IDE",
        }
    }

    /// 本次窗口是否真的关闭了宿主（报告字段只认「我们关的 + 重开成功」）。
    pub(super) fn closed_by_us(&self) -> bool {
        match self {
            Self::Vscode(guard) => guard.closed_by_us(),
            Self::Jetbrains(guard) => guard.closed_by_us(),
        }
    }

    /// 成功收尾：重开（仅本次由我们关闭时）。`Err` = 重开失败，操作本身已成功。
    pub(super) fn finish(self) -> Result<(), String> {
        match self {
            Self::Vscode(guard) => guard.finish(),
            Self::Jetbrains(guard) => guard.finish(),
        }
    }

    /// 失败收尾：best-effort 重开，并把重开失败信息追加到原错误之后（不吞原错误）。
    pub(super) fn fail(self, error: String) -> String {
        match self {
            Self::Vscode(guard) => guard.fail(error),
            Self::Jetbrains(guard) => guard.fail(error),
        }
    }
}

/// 非插件侧不涉及编辑器生命周期（`Ok(None)`）；插件侧按 `restart` 打开窗口。
///
/// `adding_member` 只用于选文案（同步 / 添加关联账号的既有措辞不同）。
pub(super) fn prepare_editor_window(
    client: SessionClient,
    restart: bool,
    adding_member: bool,
    target_uids: &[&str],
) -> Result<Option<EditorGuard>, String> {
    let hint = editor_running_hint(client, adding_member);
    match client {
        SessionClient::VscodeExt => VscodeEditorGuard::prepare(restart, hint, target_uids)
            .map(|guard| Some(EditorGuard::Vscode(guard))),
        SessionClient::Jetbrains => {
            // JetBrains 的判定与目标账号无关：IDE 内存里只有一个账号，且多个产品共用数据目录。
            let _ = target_uids;
            JetbrainsIdeGuard::prepare(restart, hint).map(|guard| Some(EditorGuard::Jetbrains(guard)))
        }
        SessionClient::Workbuddy | SessionClient::CodebuddyIde => Ok(None),
    }
}

/// 插件侧写操作的统一收尾：重开宿主并把结果并入报告。
///
/// - 重开成功且本次确实关过 → 报告补 `restartedEditor: true`；
/// - 重开失败不进整体 `Err`（操作本身已成功）：能进 `errors[]` 就进，否则挂 `editorError`。
pub(super) fn settle_editor_window(
    guard: Option<EditorGuard>,
    result: Result<Value, String>,
) -> Result<Value, String> {
    let Some(guard) = guard else {
        return result;
    };
    let label = guard.label();
    match result {
        Ok(mut report) => {
            let closed_by_us = guard.closed_by_us();
            match guard.finish() {
                Ok(()) => {
                    if closed_by_us {
                        report["restartedEditor"] = json!(true);
                    }
                }
                Err(error) => {
                    let message = format!("会话已处理，但 {label} 重新打开失败：{error}");
                    match report.get_mut("errors").and_then(Value::as_array_mut) {
                        Some(errors) => errors.push(json!({ "error": message })),
                        None => report["editorError"] = json!(message),
                    }
                }
            }
            Ok(report)
        }
        Err(error) => Err(guard.fail(error)),
    }
}

pub(super) fn run_pair_sync(
    client: SessionClient,
    scope: Option<WbVariant>,
    pair: &PairContext,
    selections: &[SyncSelection],
    editor_prepared: bool,
) -> Result<Value, String> {
    let paths = client.store_paths();
    let report = match client {
        SessionClient::Workbuddy => {
            session::sync_sessions_cross(&pair.source_account, &pair.target_account, selections)?
        }
        SessionClient::CodebuddyIde => {
            let variant = scope.ok_or_else(|| "CodeBuddy IDE 缺少档位".to_string())?;
            let _operation_lock = session_link::try_acquire_client_ops_lock(&paths)?;
            if ide_flavor(variant).is_running() {
                return Err("检测到 CodeBuddy IDE 正在运行，请先完全退出后再同步会话。".to_string());
            }
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 CodeBuddy IDE 会话数据目录".to_string())?;
            vscode_session_sync::sync_selected_in_for_variant(
                CODEBUDDY_IDE_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
                selections,
                variant,
            )?
        }
        SessionClient::VscodeExt => {
            let _operation_lock = session_link::try_acquire_client_ops_lock(&paths)?;
            // `editor_prepared` = 调用方已在生命周期窗口内处理过编辑器（已关闭或本来未运行）。
            if !editor_prepared && vscode_ext::is_vscode_running() {
                return Err(VSCODE_SYNC_RUNNING_HINT.to_string());
            }
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 VS Code 插件会话数据目录".to_string())?;
            vscode_session_sync::sync_selected_in(
                VSCODE_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
                selections,
            )?
        }
        SessionClient::Jetbrains => {
            let _operation_lock = session_link::try_acquire_client_ops_lock(&paths)?;
            // `editor_prepared` 语义与 VS Code 侧相同：窗口内已关闭，或本来没运行。
            if !editor_prepared && !jetbrains::running_plugin_ide_labels().is_empty() {
                return Err(JETBRAINS_SYNC_RUNNING_HINT.to_string());
            }
            let root = client
                .data_root()
                .ok_or_else(|| "未找到 JetBrains CodeBuddy 插件的会话数据目录".to_string())?;
            vscode_session_sync::sync_selected_in(
                JETBRAINS_STORE,
                &root,
                &paths,
                &pair.source.uid,
                &pair.target_account,
                selections,
            )?
        }
    };
    Ok(json!({
        "client": client.as_str(),
        "groupId": pair.group.id,
        "sourceMemberId": pair.source.member_id,
        "targetMemberId": pair.target.member_id,
        "synced": report.get("synced").cloned().unwrap_or(json!([])),
        "skipped": report.get("skipped").cloned().unwrap_or(json!([])),
        "errors": report.get("errors").cloned().unwrap_or(json!([])),
        "needsRecovery": report.get("needsRecovery").cloned().unwrap_or(json!(false)),
        "temporaryFiles": report.get("temporaryFiles").cloned().unwrap_or(json!([])),
    }))
}

pub(super) fn parse_selection(
    group_id: &str,
    preview_token: &str,
    mode: &str,
) -> Result<SyncSelection, String> {
    if !matches!(mode, "fastForward" | "overwrite" | "unifyOverwrite") {
        return Err("不支持的会话同步模式".to_string());
    }
    session::parse_sync_selections(Some(&json!([{
        "groupId": group_id,
        "previewToken": preview_token,
        "mode": mode,
    }])))?
    .into_iter()
    .next()
    .ok_or_else(|| "会话同步选择项为空".to_string())
}

pub(super) fn merge_sync_report(into: &mut Value, item: Value) {
    let Value::Object(combined) = into else {
        return;
    };
    for key in ["synced", "skipped", "errors", "temporaryFiles"] {
        let values = item
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // 汇总对象的初始化 schema 可能与合并键列表漂移：缺键时 IndexMut 只会补成 Null，
        // 直接 as_array_mut().unwrap() 会 panic。这里把「缺失或不是数组」的槽位补成
        // 数组再合并 values —— 不 panic，也不丢数据。
        match combined.entry(key.to_owned()).or_insert_with(|| json!([])) {
            Value::Array(target) => target.extend(values),
            slot => *slot = json!(values),
        }
    }
    if item.get("needsRecovery").and_then(Value::as_bool) == Some(true) {
        combined.insert("needsRecovery".to_owned(), json!(true));
    }
}

pub(super) fn validate_target_compatibility(
    client: SessionClient,
    scope: Option<WbVariant>,
    target: &Value,
) -> Result<(), String> {
    match client {
        SessionClient::Workbuddy => Ok(()),
        SessionClient::CodebuddyIde => {
            let expected = scope.ok_or_else(|| "CodeBuddy IDE 缺少档位".to_string())?;
            if account::variant_of(target) != expected {
                return Err("目标账号与当前 CodeBuddy IDE 档位不兼容".to_string());
            }
            Ok(())
        }
        // The plugin uses one account-scoped store across account regions.
        // JetBrains 与 VS Code 插件同一口径：存储与地区无关，不做档位兼容校验。
        SessionClient::VscodeExt | SessionClient::Jetbrains => Ok(()),
    }
}
