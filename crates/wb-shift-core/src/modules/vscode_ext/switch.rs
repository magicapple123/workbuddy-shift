//! 账号切换：注入错误文案、目标校验、关闭后切换与失败回滚。

#[allow(unused_imports)]
use super::detect::*;
#[allow(unused_imports)]
use super::status::*;
use super::*;

/// 注入失败的文案映射（D3）：钥匙串 / Safe Storage 缺失时补一条可操作提示。
pub(super) fn describe_inject_error(err: String) -> String {
    if err.contains("Safe Storage") || err.contains("Keychain") {
        format!(
            "注入登录状态失败：{err}\n\n请先手动打开 VS Code 并登录一次 CodeBuddy 插件，确保系统凭据存储中存在「Code Safe Storage」条目后再试。"
        )
    } else {
        err
    }
}

/// 切换成功文案（design §5）：区分「已重新打开」「重开失败」「本来没运行」。
pub(super) fn switch_message(name: &str, restarted: bool, relaunch_error: Option<&str>) -> String {
    match (restarted, relaunch_error) {
        (true, _) => {
            format!("已写入 VS Code CodeBuddy 插件凭证（{name}），VS Code 已重新打开。")
        }
        (false, Some(err)) => format!(
            "已写入 VS Code CodeBuddy 插件凭证（{name}），但自动重新打开 VS Code 失败：{err}；请手动打开 VS Code 生效。"
        ),
        (false, None) => {
            format!("已写入 VS Code CodeBuddy 插件凭证（{name}）；请打开 VS Code 生效。")
        }
    }
}

/// 切换前置校验（账号 / `access_token` / 数据目录 / `state.vscdb`）。
///
/// 必须在关闭编辑器**之前**执行：账号不存在、`access_token` 为空、目录或 DB 缺失这类
/// 「无论怎么关都注定失败」的目标，不该让用户的编辑器被关掉。返回目标账号条目与用户数据目录；
/// 幂等且廉价，[`switch_account_after_close`] 会再校验一次。
pub(crate) fn validate_switch_target(account_id: &str) -> Result<(Value, PathBuf), String> {
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let token = get_str(&acc, "access_token")
        .ok_or_else(|| "账号缺少 access_token，无法注入 VS Code CodeBuddy 插件".to_string())?;
    if token.is_empty() {
        return Err("账号 access_token 为空".to_string());
    }

    let data_dir = vscode_data_dir().ok_or_else(|| "无法定位 VS Code 数据目录".to_string())?;
    if !data_dir.exists() {
        return Err(format!(
            "未找到 VS Code 用户数据目录（{}）。请先手动打开 VS Code 并安装 CodeBuddy 插件后重试。",
            data_dir.display()
        ));
    }

    let db_path =
        vscode_ext_state_db_path().ok_or_else(|| "无法定位 VS Code 数据目录".to_string())?;
    if !db_path.exists() {
        return Err(format!(
            "未找到 VS Code 状态数据库（{}）。请先手动打开一次 VS Code（无需先登录插件）后重试。",
            db_path.display()
        ));
    }

    Ok((acc, data_dir))
}

/// 关闭前采集的实例快照（不透明句柄）：交给调用方持有，供关闭后的重开复用（D8）。
///
/// 关掉编辑器后再去探测进程已经拿不到启动路径，所以快照必须由本模块在关闭前采集。
#[derive(Debug)]
pub struct ClosedEditor(pub(super) RunningInstance);

/// 关闭决策 + 执行：未运行 → `Ok(None)`（不主动拉起）；运行中且 `restart=false` → `Err`
/// （手动模式保持旧行为，编辑器未被触碰）；运行中且 `restart=true` → 优雅退出后返回关闭前快照。
///
/// 关闭失败 / 超时一律 `Err`（D6）：不吞错、不返回 `Ok`。
pub fn close_vscode_for_switch(restart: bool) -> Result<Option<ClosedEditor>, String> {
    close_vscode_for_switch_with(running_instance(), restart)
}

/// [`close_vscode_for_switch`] 的可测实现：运行实例由调用方注入，便于单测关闭决策而不碰真实进程。
pub(super) fn close_vscode_for_switch_with(
    inst: RunningInstance,
    restart: bool,
) -> Result<Option<ClosedEditor>, String> {
    if !inst.running() {
        return Ok(None);
    }
    if !restart {
        return Err(RUNNING_MANUAL_HINT.to_string());
    }
    close_vscode(&inst.pids, VSCODE_CLOSE_TIMEOUT_SECS)?;
    Ok(Some(ClosedEditor(inst)))
}

/// best-effort 重新打开本次由我们关闭的编辑器（复用关闭前记录的路径，D8）。
pub fn relaunch_closed_editor(closed: &ClosedEditor) -> Result<(), String> {
    launch_vscode(&closed.0)
}

/// 「编辑器已按需关闭后」的切换主体：读既有会话 → 注入 → 记录当前账号 → 重开（仅本次我们关闭时）。
///
/// `closed = Some(..)` 表示编辑器已由 [`close_vscode_for_switch`] 优雅退出；`None` 表示本来没运行。
/// **任何失败路径**（含校验早退、读既有 session 失败、注入失败、写状态失败）在编辑器是我们关的
/// 时候都会先 best-effort 开回来再报错（D4）；重开失败只降级为警告文案，写入本身已生效，
/// 整体仍算成功。
pub fn switch_account_after_close(
    account_id: &str,
    closed: Option<ClosedEditor>,
) -> Result<Value, String> {
    match switch_account_after_close_inner(account_id, closed.as_ref()) {
        Ok(value) => Ok(value),
        Err(error) => Err(relaunch_closed_on_error(
            closed.as_ref(),
            relaunch_closed_editor,
            error,
        )),
    }
}

/// 失败兜底（D4）：`closed.is_some()` 时先尽力把编辑器开回来，再回报原错误。
///
/// `relaunch` 由调用方注入——真实实现会**真的启动编辑器**，因此单测必须在不碰进程的前提下
/// 验证「关了的要开回来 / 本来没关的不许动」这两条分支。
pub(super) fn relaunch_closed_on_error(
    closed: Option<&ClosedEditor>,
    relaunch: impl FnOnce(&ClosedEditor) -> Result<(), String>,
    error: String,
) -> String {
    match closed {
        None => error,
        Some(closed) => match relaunch(closed) {
            Ok(()) => error,
            // 两条信息都要给用户：否则他不知道编辑器还关着，也不知道为什么。
            Err(launch_error) => format!("{error}\n\n{launch_error}"),
        },
    }
}

/// [`switch_account_after_close`] 的主体：**不负责失败兜底**（由外层统一处理，才能覆盖
/// 「校验就失败」这类还没开始切换的早退）。
pub(super) fn switch_account_after_close_inner(
    account_id: &str,
    closed: Option<&ClosedEditor>,
) -> Result<Value, String> {
    let (acc, data_dir) = validate_switch_target(account_id)?;

    // 未登录（无既有会话行）→ 走新建完整载荷，不再要求用户先登录一次。
    let existing_secret =
        read_secret_for(&VSCODE_TARGET, Some(&data_dir)).map_err(describe_secret_error)?;
    let existing_session = existing_secret.is_some();
    let session = build_ext_session_json(&acc, existing_secret.as_deref());

    let db_path = inject_secret_for(&VSCODE_TARGET, &session, Some(&data_dir))
        .map_err(describe_inject_error)?;

    set_active_account_id(account_id)?;

    // 「最近切换」时间戳（issue #184）：secret 与当前账号都已落盘，切换已生效。
    // 失败只影响卡片徽标与排序，不得回滚写入、不得改成报错。
    let _ = account::touch_last_used(account_id, "vscode");

    let mut restarted = false;
    let mut relaunch_error: Option<String> = None;
    if let Some(closed) = closed {
        match relaunch_closed_editor(closed) {
            Ok(()) => restarted = true,
            Err(err) => relaunch_error = Some(err),
        }
    }

    let closed_by_us = closed.is_some();
    let name = account::account_display_name(&acc);
    let message = switch_message(&name, restarted, relaunch_error.as_deref());

    Ok(json!({
        "ok": true,
        "account": name,
        "accountId": account_id,
        "dbPath": db_path.to_string_lossy(),
        "restarted": restarted,
        "closedByUs": closed_by_us,
        "existingSession": existing_session,
        "message": message,
    }))
}

/// 切换 VS Code CodeBuddy 扩展账号。
///
/// 时序（design §3）：校验账号 / 目录 / db → 关闭决策（[`close_vscode_for_switch`]：
/// 运行中且 `restart=false` 报错、`restart=true` 先优雅退出，失败即报错且**不写入**）
/// → [`switch_account_after_close`]。切换前未运行时行为与改造前一致（只写入，不主动拉起编辑器）。
pub fn switch_account(account_id: &str, restart: bool) -> Result<Value, String> {
    // 先校验再关闭：账号不存在时不关编辑器（design §3.1）。
    validate_switch_target(account_id)?;
    let closed = close_vscode_for_switch(restart)?;
    switch_account_after_close(account_id, closed)
}
