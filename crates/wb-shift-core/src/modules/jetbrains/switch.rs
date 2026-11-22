//! 账号切换：目标校验、关闭后写 secret、失败回滚与结果文案。
//!
//! 失败可诊断是本模块的硬要求：每个失败分支都写 `error.log`
//! （[`crate::modules::error_log`]），并在提示里附「已识别 N 个 IDE：A、B」，
//! 让用户能区分「我们没识别到」与「他自己操作错了」。

#[allow(unused_imports)]
use super::detect::*;
use super::diagnose::*;
#[allow(unused_imports)]
use super::status::*;
use super::*;

/// 切换前置校验：账号 / `access_token` / 按选择过滤后的目标目录。
pub(super) fn validate_switch_target(
    account_id: &str,
    config_dirs: Option<&[String]>,
) -> Result<(Value, Vec<Target>), String> {
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let token = get_str(&acc, "access_token")
        .ok_or_else(|| "账号缺少 access_token，无法注入 JetBrains IDE 插件".to_string())?;
    if token.is_empty() {
        return Err("账号 access_token 为空".to_string());
    }

    // 两个配置根都定位不到 → 无法切换；只有**两个根都不存在**时报错，单个根缺失
    // （如只装了 IDEA 没装 Android Studio）属于正常情况。
    let roots = jetbrains_config_roots();
    if roots.is_empty() {
        return Err("无法定位 JetBrains 配置目录（未取得用户配置目录）".to_string());
    }
    if roots.iter().all(|root| !root.exists()) {
        let paths: Vec<String> = roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect();
        return Err(format!(
            "JetBrains 配置根不存在（已检查：{}）。请先安装并至少启动一次 IDE 或 Android Studio。",
            paths.join("、")
        ));
    }
    let targets: Vec<Target> = list_targets()
        .into_iter()
        .filter(|t| t.plugin_installed)
        .collect();
    if targets.is_empty() {
        return Err(
            "未找到已安装 CodeBuddy 插件的 JetBrains IDE。请先在 JetBrains 系 IDE（IDEA / PyCharm / GoLand 等）或 Android Studio 中安装「Tencent Cloud CodeBuddy」插件后重试。".to_string(),
        );
    }
    let targets = filter_targets_by_selection(targets, config_dirs)?;
    Ok((acc, targets))
}

/// 「编辑器已按需关闭后」的切换主体：写所有目标 → 记录当前账号 → 重开。
///
/// 各目标独立读取既有 secret 做 merge：已登录的目标走 upsert（保留扩展私有字段），
/// 未登录的目标走新建完整载荷，互不影响。`config_dirs` 限定写入范围（None = 全部）。
///
/// 任一个目标写失败 → **把已写过的目标回滚到写入前的值**再报错：多 IDE 场景下
/// 只写一半会让用户看到「PyCharm 换了、IDEA 没换」，比整体失败更难诊断。
pub(super) fn switch_after_close_inner(
    account_id: &str,
    closed: &[RunningIde],
    config_dirs: Option<&[String]>,
) -> Result<Value, String> {
    let (acc, targets) = validate_switch_target(account_id, config_dirs)?;

    let mut written: Vec<String> = Vec::new();
    // (配置目录, 写入前的值)：回滚唯一依据，`None` = 写入前该目录没有登录态。
    let mut applied: Vec<(PathBuf, Option<String>)> = Vec::new();
    for target in &targets {
        let existing = read_secret_for(&target.config_dir).map_err(describe_secret_error)?;
        let payload = build_ext_session_json(&acc, existing.as_deref());
        if let Err(error) = write_secret_for(&target.config_dir, &payload) {
            let error = describe_inject_error(error);
            let rollback_errors = rollback_writes(&applied);
            return Err(join_with_rollback(error, applied.len(), rollback_errors));
        }
        applied.push((target.config_dir.clone(), existing));
        written.push(target.name.clone());
    }

    set_active_account_id(account_id)?;

    // 「最近切换」时间戳（issue #184）：全部目标已写入、当前账号已记录，切换已生效。
    // 失败只影响卡片徽标与排序，不得触发回滚、不得改成报错。
    let _ = account::touch_last_used(account_id, "jetbrains");

    let mut restarted = false;
    let mut relaunch_error: Option<String> = None;
    for inst in closed {
        match launch_ide(inst) {
            Ok(()) => restarted = true,
            Err(err) => relaunch_error = Some(err),
        }
    }

    let closed_by_us = !closed.is_empty();
    let name = account::account_display_name(&acc);
    let message = switch_message(&name, &written, restarted, relaunch_error.as_deref());

    Ok(json!({
        "ok": true,
        "account": name,
        "accountId": account_id,
        "written": written,
        "restarted": restarted,
        "closedByUs": closed_by_us,
        "message": message,
    }))
}

/// 把已经写过的目标恢复成写入前的值（逆序：与写入顺序相反，先撤最后一步）。
///
/// 返回逐条失败明细；空 = 全部回滚成功。
pub(super) fn rollback_writes(applied: &[(PathBuf, Option<String>)]) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    for (config_dir, previous) in applied.iter().rev() {
        let name = config_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| config_dir.to_string_lossy().into_owned());
        if let Err(error) = restore_secret_for(config_dir, previous.as_deref()) {
            errors.push(format!("{name}: {error}"));
        }
    }
    errors
}

/// 回滚后的提示拼装：成功时明确告知「账号保持原样」，失败时列出需要人工处理的目录。
pub(super) fn join_with_rollback(
    error: String,
    applied: usize,
    rollback_errors: Vec<String>,
) -> String {
    if applied == 0 && rollback_errors.is_empty() {
        return error;
    }
    if rollback_errors.is_empty() {
        return format!("{error}\n\n已回滚本次写入的 {applied} 个目标，IDE 内账号保持原样。");
    }
    format!(
        "{error}\n\n回滚失败，请手动重新登录以下 IDE 的 CodeBuddy 插件：\n{}",
        rollback_errors.join("\n")
    )
}

pub(super) fn describe_secret_error(err: String) -> String {
    err
}

/// 注入失败的文案映射。
pub(super) fn describe_inject_error(err: String) -> String {
    format!("注入 JetBrains IDE 插件登录状态失败：{err}")
}

/// 切换成功文案：区分「已重新打开」「重开失败」「本来没运行」。
pub(super) fn switch_message(
    name: &str,
    written: &[String],
    restarted: bool,
    relaunch_error: Option<&str>,
) -> String {
    let scope = if written.is_empty() {
        String::new()
    } else {
        format!("，目标: {}", written.join("、"))
    };
    match (restarted, relaunch_error) {
        (true, _) => format!("已写入 JetBrains IDE CodeBuddy 插件凭证（{name}{scope}），IDE 已重新打开。"),
        (false, Some(err)) => format!(
            "已写入 JetBrains IDE CodeBuddy 插件凭证（{name}{scope}），但自动重新打开 IDE 失败：{err}；请手动打开 IDE 生效。"
        ),
        (false, None) => format!("已写入 JetBrains IDE CodeBuddy 插件凭证（{name}{scope}）；请打开 IDE 生效。"),
    }
}

/// 失败兜底：被我们关掉的 IDE 尽力逐个开回来；重开失败只追加提示，不吞原错误。
pub(super) fn relaunch_closed_on_error(closed: &[RunningIde], error: String) -> String {
    let mut messages = vec![error];
    for inst in closed {
        if let Err(launch_error) = launch_ide(inst) {
            messages.push(launch_error);
        }
    }
    messages.join("\n\n")
}

// ---------------------------------------------------------------------------
// 失败诊断：每个失败分支写 error.log，并附「已识别 N 个 IDE」
// ---------------------------------------------------------------------------

/// 失败现场：配置根（全部候选根）、配置目录清单与插件命中、运行实例映射结果（不含 secret）。
pub(super) fn failure_context(detected: &[Target], running: &[RunningIde]) -> String {
    let roots = jetbrains_config_roots();
    let root_text = if roots.is_empty() {
        "（无法定位）".to_string()
    } else {
        roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("、")
    };
    let mut lines = vec![format!("configRoots={root_text}")];
    for root in &roots {
        lines.push(format!("root={} exists={}", root.display(), root.exists()));
    }
    if detected.is_empty() {
        lines.push("targets=(空)".to_string());
    } else {
        for target in detected {
            lines.push(format!(
                "target={} pluginInstalled={} secretFileExists={}",
                target.name,
                target.plugin_installed,
                target.secret_path().exists()
            ));
        }
    }
    if running.is_empty() {
        lines.push("running=(无)".to_string());
    } else {
        for inst in running {
            lines.push(format!(
                "running pid={} mappingSource={} configDir={} note={} target={}",
                inst.pid,
                inst.mapping_source.as_str(),
                inst.config_dir_name.as_deref().unwrap_or("(未映射)"),
                inst.mapping_note.as_deref().unwrap_or("-"),
                inst.launch_target
                    .as_deref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
    }
    lines.join("\n")
}

/// 记录一次切换失败（写入失败**不影响**返回给用户的原始错误）。
pub(super) fn record_failure(summary: &str, message: &str, detail: &str) {
    crate::modules::error_log::record(
        "backend",
        FAILURE_LOG_TITLE,
        &failure_log_detail(summary, message, detail),
    );
}

/// `error.log` 里这条记录的标题（与第 2 条的日志出口复用同一个 `error_log` 模块）。
pub(super) const FAILURE_LOG_TITLE: &str = "JetBrains IDE 切换失败";

/// 失败日志正文：摘要（已识别几个 IDE）+ 错误原文 + 现场（配置根 / 目标 / 进程映射）。
///
/// 拆成纯函数是为了能在单测里断言「日志里到底写了什么」——`error_log::record`
/// 固定写到真实 `~/.workbuddy-shift/error.log`，测试不能触碰（AGENTS.md 第 8 节）。
pub(super) fn failure_log_detail(summary: &str, message: &str, detail: &str) -> String {
    format!("{summary}\n错误：{message}\n{detail}")
}

/// 切换 JetBrains 系 IDE（IDEA / PyCharm / GoLand / Android Studio 等）CodeBuddy 插件账号。
///
/// `config_dirs`：目标配置目录名列表（如 `["PyCharm2026.2"]`）。`None` / 空列表
/// = 全部装了插件的 IDE；非空时只写所选目录、只关闭/重开这些目录的运行实例。
///
/// 时序：校验 → **拒绝写入检查（运行中但映射不到配置目录的实例）** → 采集运行快照
/// → 关闭决策（运行中且 `restart=false` 报错、`restart=true` 优雅退出）→ 写入 → 重开。
///
/// 拒绝写入的依据：映射不到配置目录的进程关不掉，写盘后会被它的内存态覆盖，
/// 用户只会看到「切了没变」；这种情况下继续写还销毁了失败现场。
pub fn switch_account(
    account_id: &str,
    restart: bool,
    config_dirs: Option<&[String]>,
) -> Result<Value, String> {
    match switch_account_inner(account_id, restart, config_dirs) {
        Ok(value) => Ok(value),
        Err(error) => {
            let detected = list_targets();
            let running = running_ides();
            let summary = describe_detected(&detected);
            let detail = failure_context(&detected, &running);
            record_failure(&summary, &error, &detail);
            Err(format!("{error}\n\n{summary}"))
        }
    }
}

fn switch_account_inner(
    account_id: &str,
    restart: bool,
    config_dirs: Option<&[String]>,
) -> Result<Value, String> {
    let target_names: Vec<String> = validate_switch_target(account_id, config_dirs)?
        .1
        .into_iter()
        .map(|t| t.name)
        .collect();
    let selected: Option<&[String]> = Some(&target_names);
    let running = running_ides();

    // 有未映射的运行实例 → 拒绝写入（用户已决策：不接受「写入后只给警告」）。
    let unmapped = unmapped_running(&running);
    if !unmapped.is_empty() {
        return Err(unmapped_running_hint(&unmapped));
    }

    let matched: Vec<RunningIde> = running
        .into_iter()
        .filter(|inst| {
            inst.config_dir_name
                .as_deref()
                .is_some_and(|name| target_names.iter().any(|t| t == name))
        })
        .collect();

    if !matched.is_empty() && !restart {
        return Err(RUNNING_MANUAL_HINT.to_string());
    }

    // 关闭不完整 → 先把已关掉的开回来再报错（不留下「IDE 被关了、盘没写」）。
    let mut closed: Vec<RunningIde> = Vec::new();
    if !matched.is_empty() {
        let report = close_ides(&matched, CLOSE_TIMEOUT_SECS);
        if !report.is_complete() {
            return Err(partial_close_error(&report));
        }
        closed = report.closed;
    }

    match switch_after_close_inner(account_id, &closed, selected) {
        Ok(value) => Ok(value),
        Err(error) => Err(relaunch_closed_on_error(&closed, error)),
    }
}
