//! 账号切换：目标校验、关闭后写 secret、失败回滚与结果文案。

#[allow(unused_imports)]
use super::detect::*;
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

    if jetbrains_config_root().is_none() {
        return Err("无法定位 JetBrains 配置目录".to_string());
    }
    let targets: Vec<Target> = list_targets()
        .into_iter()
        .filter(|t| t.plugin_installed)
        .collect();
    if targets.is_empty() {
        return Err(
            "未找到已安装 CodeBuddy 插件的 JetBrains IDE。请先在 IDEA / PyCharm 中安装「Tencent Cloud CodeBuddy」插件后重试。".to_string(),
        );
    }
    let targets = filter_targets_by_selection(targets, config_dirs)?;
    Ok((acc, targets))
}

/// 「编辑器已按需关闭后」的切换主体：写所有目标 → 记录当前账号 → 重开。
///
/// 各目标独立读取既有 secret 做 merge：已登录的目标走 upsert（保留扩展私有字段），
/// 未登录的目标走新建完整载荷，互不影响。`config_dirs` 限定写入范围（None = 全部）。
pub(super) fn switch_after_close_inner(
    account_id: &str,
    closed: &[RunningIde],
    config_dirs: Option<&[String]>,
) -> Result<Value, String> {
    let (acc, targets) = validate_switch_target(account_id, config_dirs)?;

    let mut written: Vec<String> = Vec::new();
    for target in &targets {
        let existing = read_secret_for(&target.config_dir).map_err(describe_secret_error)?;
        let payload = build_ext_session_json(&acc, existing.as_deref());
        write_secret_for(&target.config_dir, &payload).map_err(describe_inject_error)?;
        written.push(target.name.clone());
    }

    set_active_account_id(account_id)?;

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

/// 切换 JetBrains IDE（IDEA / PyCharm）CodeBuddy 插件账号。
///
/// `config_dirs`：目标配置目录名列表（如 `["PyCharm2026.2"]`）。`None` / 空列表
/// = 全部装了插件的 IDE；非空时只写所选目录、只关闭/重开这些目录的运行实例。
///
/// 时序：校验 → 采集运行快照 → 关闭决策（运行中且 `restart=false` 报错、
/// `restart=true` 优雅退出；映射不到配置目录的进程一律不动）→ 写入 → 重开。
pub fn switch_account(
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
    let matched: Vec<RunningIde> = running_ides()
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

    if !matched.is_empty() {
        close_ides(&matched, CLOSE_TIMEOUT_SECS)?;
    }

    match switch_after_close_inner(account_id, &matched, selected) {
        Ok(value) => Ok(value),
        Err(error) => Err(relaunch_closed_on_error(&matched, error)),
    }
}
