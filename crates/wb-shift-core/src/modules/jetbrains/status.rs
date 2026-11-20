//! 运行状态：关闭/重启 IDE 进程与 status 查询。

#[allow(unused_imports)]
use super::detect::*;
use super::diagnose::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

// ---------------------------------------------------------------------------
// 优雅关闭 / 重开（契约同 VS Code 端：绝不强杀，超时只报错）
// ---------------------------------------------------------------------------

/// 等待 IDE 退出的默认上限（秒）。
pub(super) const CLOSE_TIMEOUT_SECS: i64 = 60;

/// 运行中 + 手动模式的报错文案。
pub(super) const RUNNING_MANUAL_HINT: &str =
    "检测到 JetBrains 系 IDE（IDEA / PyCharm / GoLand / Android Studio 等）正在运行，请先完全退出后再切换，否则写入会被 IDE 覆盖。";

pub(super) fn close_timeout_error(alive: &[u32]) -> String {
    let pids: Vec<String> = alive.iter().map(|pid| pid.to_string()).collect();
    format!(
        "等待 JetBrains IDE 退出超时（仍有进程运行: {}）。请处理 IDE 内的保存提示后重试；若你在等待期间重新打开过 IDE，请退出后重试。",
        pids.join(", ")
    )
}

/// 一次关闭请求的结论：**确实退出了**的实例 + 超时仍在跑的 PID。
///
/// 只报「成功 / 失败」会丢掉关键事实：超时时往往「已经关掉一半」，
/// 若不把这半边记下来，调用方无法回滚（IDE 被关掉却没写盘、也没开回来）。
#[derive(Debug)]
pub(super) struct CloseReport {
    /// 已确认退出的实例（关闭前快照，重开时只用它）。
    pub(super) closed: Vec<RunningIde>,
    /// 超时仍存活的 PID（空 = 全部关闭成功）。
    pub(super) alive: Vec<u32>,
}

impl CloseReport {
    /// 按「仍存活的 PID」切分出已关闭实例，保持输入顺序（重开顺序稳定）。
    pub(super) fn from_alive(instances: &[RunningIde], alive: Vec<u32>) -> Self {
        let closed: Vec<RunningIde> = instances
            .iter()
            .filter(|inst| !alive.contains(&inst.pid))
            .cloned()
            .collect();
        Self { closed, alive }
    }

    /// 是否全部关闭（`false` = 有进程没退出，写入必须中止）。
    pub(super) fn is_complete(&self) -> bool {
        self.alive.is_empty()
    }
}

/// 优雅关闭指定 IDE 实例。
///
/// Windows 的 JetBrains IDE（AWT/Swing 窗口）**对 `WM_CLOSE` / `WM_ENDSESSION`
/// 静默忽略**（与 VS Code 等 Electron 应用不同，`taskkill` 不带 `/F` 等价消息
/// 发了也退不掉），可用的优雅路径是 `WM_SYSCOMMAND/SC_CLOSE`：它走 Swing 的
/// `windowClosing` 链，由 IDE 自己弹出「确认退出」对话框，再向该对话框发送
/// Enter 触发默认按钮（退出），IDE 即保存会话后干净退出。确认框的标题随 IDE
/// 语言变化，因此用「SC_CLOSE 后新出现的可见窗口」识别，不匹配标题。
pub(super) fn close_ides(instances: &[RunningIde], timeout_secs: i64) -> CloseReport {
    if instances.is_empty() {
        return CloseReport {
            closed: Vec::new(),
            alive: Vec::new(),
        };
    }
    let alive = close_ides_alive(instances, timeout_secs);
    CloseReport::from_alive(instances, alive)
}

/// 关闭请求的底层实现：返回超时后仍存活的 PID（空 = 全部退出）。
fn close_ides_alive(instances: &[RunningIde], timeout_secs: i64) -> Vec<u32> {
    #[cfg(target_os = "windows")]
    {
        let pids: Vec<u32> = instances.iter().map(|inst| inst.pid).collect();
        win_close::close_gracefully(&pids, Duration::from_secs(timeout_secs.max(1) as u64))
    }
    #[cfg(target_os = "macos")]
    {
        // 原生退出流程（保存提示 / 热退出保护由 IDE 自己负责），等待退出。
        // macOS 无法按单 PID 优雅退出 GUI 应用：按 bundle 名 `quit app`。
        let started = Instant::now();
        for inst in instances {
            if let Some(app_name) = inst
                .launch_target
                .as_deref()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .map(|n| n.trim_end_matches(".app").to_string())
            {
                let script = format!("quit app \"{}\"", app_name.replace('"', "\\\""));
                let _ = process::run_cmd_timeout("osascript", &["-e", &script], 10);
            }
        }
        let deadline = started + Duration::from_secs(timeout_secs.max(1) as u64);
        loop {
            let alive: Vec<u32> = instances
                .iter()
                .map(|inst| inst.pid)
                .filter(|pid| process_alive(*pid))
                .collect();
            if alive.is_empty() || Instant::now() >= deadline {
                break alive;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let pids: Vec<u32> = instances.iter().map(|inst| inst.pid).collect();
        let args: Vec<String> = std::iter::once("-TERM".to_string())
            .chain(pids.iter().map(|pid| pid.to_string()))
            .collect();
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = process::run_cmd_timeout("kill", &arg_refs, 10);
        let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1) as u64);
        loop {
            let alive: Vec<u32> = pids
                .iter()
                .copied()
                .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
                .collect();
            if alive.is_empty() || Instant::now() >= deadline {
                break alive;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// 关闭不完整时的收尾：把**已经关掉**的实例尽力开回来，再组装错误文案。
///
/// 决策依据（清单 10.3.B）：关闭失败要回滚已关闭的实例并报原文——不能留下
/// 「IDE 被我们关了、盘没写、也没开回来」这种用户无法自行恢复的状态。
pub(super) fn partial_close_error(report: &CloseReport) -> String {
    let mut messages = vec![close_timeout_error(&report.alive)];
    if report.closed.is_empty() {
        return messages.join("\n\n");
    }
    let mut relaunch_errors: Vec<String> = Vec::new();
    for inst in &report.closed {
        if let Err(error) = launch_ide(inst) {
            relaunch_errors.push(error);
        }
    }
    if relaunch_errors.is_empty() {
        messages.push(format!(
            "已重新打开本次已关闭的 {} 个 IDE 实例（未写入任何账号）。",
            report.closed.len()
        ));
    } else {
        messages.push(format!(
            "本次已关闭的 IDE 重新打开失败：{}；请手动打开 IDE。",
            relaunch_errors.join("；")
        ));
    }
    messages.join("\n\n")
}

/// macOS 进程存活探测（`kill -0`）：`quit app` 后按 PID 等待退出。
#[cfg(target_os = "macos")]
pub(super) fn process_alive(pid: u32) -> bool {
    process::run_cmd_timeout("kill", &["-0", &pid.to_string()], 5)
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// 重新打开被我们关闭的 IDE（只用关闭前记录的路径）。
pub(super) fn launch_ide(inst: &RunningIde) -> Result<(), String> {
    let target = inst
        .launch_target
        .as_ref()
        .ok_or_else(|| "未能记录 IDE 可执行文件路径，请手动打开".to_string())?;
    #[cfg(target_os = "macos")]
    {
        let app = target.to_string_lossy().into_owned();
        match process::run_cmd_timeout("open", &[app.as_str()], 10) {
            Some(out) if out.status.success() => Ok(()),
            Some(out) => Err(format!(
                "重新打开 IDE 失败: {}（路径: {}）",
                String::from_utf8_lossy(&out.stderr).trim(),
                target.display()
            )),
            None => Err(format!(
                "重新打开 IDE 失败: open 超时（路径: {}）",
                target.display()
            )),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        process::cmd_builder(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("重新打开 IDE 失败: {e}（路径: {}）", target.display()))?;
        Ok(())
    }
}

/// 本地状态文件里记录的账号（id + 显示名）。
///
/// 只有「账号库里确实存在该 id」时才算数（历史行为：状态文件里指向已删除账号的 id
/// 一律当作没有当前账号）。
pub(super) type StateAccount = (String, String);

/// 解析「当前账号」：`secret-storage.xml` 优先，读不到 / 匹配不上回退本地状态文件。
///
/// 抽成纯函数是为了让两条分支都能被单测钉住（都不用碰真实账号库 / 真实配置目录）。
/// 返回值第三项是 `detectedFrom`，**如实**告诉前端这个结论是从哪儿来的：
/// - `secret-storage`：这次真的从 `secret-storage.xml` 读到了能匹配账号库的登录态；
/// - `state-file`：secret 缺失 / 不匹配 / 账号库换过账号，结论来自本地状态文件
///   （也可能是两边都没有，此时前两项为 `None`）。
pub(super) fn resolve_active_account(
    scan: &SecretScan,
    state: Option<StateAccount>,
) -> (Option<String>, Option<String>, &'static str) {
    if let SecretScan::Matched { account, .. } = scan {
        let id = get_str(account, "id").unwrap_or_default();
        if !id.trim().is_empty() {
            return (
                Some(id),
                Some(account::account_display_name(account)),
                "secret-storage",
            );
        }
    }
    match state {
        Some((id, name)) => (Some(id), Some(name), "state-file"),
        None => (None, None, "state-file"),
    }
}

/// 本地状态文件里的当前账号（账号库里查不到对应账号时视为没有）。
///
/// 该文件只在「切换成功」与「检测账号」时回写，因此它天然**滞后**于用户在 IDE 内的
/// 手动登录——这正是 `status()` 要内联一次轻量识别的原因。
pub(super) fn state_account() -> Option<StateAccount> {
    let id = active_account_id_from_state()?;
    account::find_account(&id).map(|acc| (id, account::account_display_name(&acc)))
}

/// 状态：是否安装、插件安装、运行、登录、当前账号（secret-storage 优先 + 本地状态文件兜底）。
///
/// 除既有开关字段外，额外给出两段**纯诊断**信息：
/// - `diagnostics`：配置根 / 子目录全量清单（含被拒原因）/ 运行实例映射结果；
/// - `unmappedRunning`：运行中但映射不到配置目录的实例（前端据此在切换前就拦住）。
///
/// `activeAccountId` / `activeAccountName` **不再只读本地状态文件**：用户在 PyCharm 里手动
/// 登录后，那份文件要等到下一次切换 / 点「检测账号」才更新，页面会长期显示上一次的账号。
/// 这里内联一次轻量识别（只读 `secret-storage.xml`，绝不写入），`detectedFrom` 如实回报
/// 结论来源（`secret-storage` / `state-file`）。
pub fn status() -> Value {
    let targets = list_targets();
    let running = running_ides();
    let running_dirs: Vec<String> = running
        .iter()
        .filter_map(|inst| inst.config_dir_name.clone())
        .collect();

    let installed = !targets.is_empty();
    let plugin_installed = targets.iter().any(|t| t.plugin_installed);
    let is_running = running_dirs
        .iter()
        .any(|dir| targets.iter().any(|t| t.plugin_installed && &t.name == dir));
    let logged_in = targets
        .iter()
        .filter(|t| t.plugin_installed)
        .any(|t| read_secret_for(&t.config_dir).ok().flatten().is_some());

    let (active_account_id, active_account_name, detected_from) =
        resolve_active_account(&scan_secret_account(), state_account());

    let unmapped = unmapped_running(&running);
    json!({
        "installed": installed,
        "pluginInstalled": plugin_installed,
        "running": is_running,
        "loggedIn": logged_in,
        "configRoots": jetbrains_config_roots()
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        "targets": targets.iter().map(|t| t.to_status_json(&running_dirs)).collect::<Vec<_>>(),
        "activeAccountId": active_account_id,
        "activeAccountName": active_account_name,
        "detectedFrom": detected_from,
        "statePath": state_path().to_string_lossy(),
        "unmappedRunning": unmapped_running_json(&running),
        "unmappedRunningHint": if unmapped.is_empty() {
            Value::Null
        } else {
            json!(unmapped_running_hint(&unmapped))
        },
        "diagnostics": diagnostics_json(&targets, &running),
    })
}

/// 按前端选择（配置目录名列表）过滤切换目标。
///
/// `None` / 空列表 = 全部装了插件的目录（缺省行为）；给了列表但一个都没匹配上
/// 属于前端状态过期（如选择期间插件被卸载），显式报错而不是静默扩成全部。
pub(super) fn filter_targets_by_selection(
    targets: Vec<Target>,
    config_dirs: Option<&[String]>,
) -> Result<Vec<Target>, String> {
    let Some(selected) = config_dirs else {
        return Ok(targets);
    };
    if selected.is_empty() {
        return Ok(targets);
    }
    let filtered: Vec<Target> = targets
        .into_iter()
        .filter(|t| selected.iter().any(|s| s == &t.name))
        .collect();
    if filtered.is_empty() {
        return Err(format!(
            "所选 JetBrains IDE（{}）均未安装 CodeBuddy 插件，请刷新状态后重试。",
            selected.join("、")
        ));
    }
    Ok(filtered)
}

// ---------------------------------------------------------------------------
// 会话关联用的公开生命周期 API（关闭 → 写入 → 重开）
// ---------------------------------------------------------------------------

/// 装了 CodeBuddy 插件的切换目标（会话关联只关心这些目录）。
pub(super) fn plugin_targets() -> Vec<Target> {
    list_targets()
        .into_iter()
        .filter(|t| t.plugin_installed)
        .collect()
}

/// 本次由我们关闭的 IDE 实例（不透明句柄）：只能交给 [`relaunch_closed_ides`] 重开。
///
/// 对外不暴露 [`RunningIde`]：会话关联只需要「关了什么、要不要开回来」，
/// 不需要（也不应该）知道配置目录映射细节。
#[derive(Debug)]
pub struct ClosedIdes {
    instances: Vec<RunningIde>,
}

impl ClosedIdes {
    /// 本次是否真的关了东西（`false` = 本来没运行，报告里不该说「已重开」）。
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// 本次关闭的实例数（报告 / 文案用）。
    pub fn count(&self) -> usize {
        self.instances.len()
    }
}

/// 装了插件且正在运行的 IDE 实例（映射不到配置目录的进程不算，见下）。
pub(super) fn running_plugin_ides() -> Result<Vec<RunningIde>, String> {
    let targets = plugin_targets();
    let running = running_ides();
    // 映射不到配置目录的同类进程关不掉，写入会被它的内存态覆盖 → 与切换同口径直接拒绝。
    let unmapped = unmapped_running(&running);
    if !unmapped.is_empty() {
        return Err(unmapped_running_hint(&unmapped));
    }
    Ok(running
        .into_iter()
        .filter(|inst| {
            inst.config_dir_name
                .as_deref()
                .is_some_and(|name| targets.iter().any(|t| t.name == name))
        })
        .collect())
}

/// 装了插件且正在运行的 IDE 产品名（去重排序，供确认框文案使用）。
pub fn running_plugin_ide_labels() -> Vec<String> {
    let Ok(running) = running_plugin_ides() else {
        return Vec::new();
    };
    let mut labels: Vec<String> = running
        .iter()
        .map(|inst| inst.describe())
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

/// 关闭所有「装了插件且在运行」的 IDE（各产品共用同一份账号数据，必须全关）。
///
/// 未运行时不主动拉起：返回空句柄。关闭不完整时**先把已关掉的开回来**再报错
/// （见 [`partial_close_error`]），不留「IDE 被关了但什么也没做」的状态。
pub fn close_running_plugin_ides() -> Result<ClosedIdes, String> {
    let matched = running_plugin_ides()?;
    if matched.is_empty() {
        return Ok(ClosedIdes {
            instances: Vec::new(),
        });
    }
    let report = close_ides(&matched, CLOSE_TIMEOUT_SECS);
    if !report.is_complete() {
        return Err(partial_close_error(&report));
    }
    Ok(ClosedIdes {
        instances: report.closed,
    })
}

/// 重新打开本次由我们关闭的 IDE；失败聚合成一条错误（不吞掉第一个原因）。
pub fn relaunch_closed_ides(closed: ClosedIdes) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();
    for inst in &closed.instances {
        if let Err(error) = launch_ide(inst) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// 会话关联的运行中预检：`{ required, running, ides }`。
///
/// `required` 与 `running` 同值（运行中就必须先关闭才能安全写盘）：保留两个字段是为了
/// 与 `vscode_restart_precheck` 的载荷形状一致，前端可以共用一套判定。
pub fn jetbrains_restart_precheck() -> Value {
    let ides = running_plugin_ide_labels();
    let running = !ides.is_empty();
    json!({ "required": running, "running": running, "ides": ides })
}
