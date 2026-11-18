//! 运行状态：关闭/重启 IDE 进程与 status 查询。

#[allow(unused_imports)]
use super::detect::*;
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
    "检测到 JetBrains IDE（IDEA / PyCharm）正在运行，请先完全退出后再切换，否则写入会被 IDE 覆盖。";

pub(super) fn close_timeout_error(alive: &[u32]) -> String {
    let pids: Vec<String> = alive.iter().map(|pid| pid.to_string()).collect();
    format!(
        "等待 JetBrains IDE 退出超时（仍有进程运行: {}）。请处理 IDE 内的保存提示后重试；若你在等待期间重新打开过 IDE，请退出后重试。",
        pids.join(", ")
    )
}

/// 优雅关闭指定 IDE 实例。
///
/// Windows 的 JetBrains IDE（AWT/Swing 窗口）**对 `WM_CLOSE` / `WM_ENDSESSION`
/// 静默忽略**（与 VS Code 等 Electron 应用不同，`taskkill` 不带 `/F` 等价消息
/// 发了也退不掉），可用的优雅路径是 `WM_SYSCOMMAND/SC_CLOSE`：它走 Swing 的
/// `windowClosing` 链，由 IDE 自己弹出「确认退出」对话框，再向该对话框发送
/// Enter 触发默认按钮（退出），IDE 即保存会话后干净退出。确认框的标题随 IDE
/// 语言变化，因此用「SC_CLOSE 后新出现的可见窗口」识别，不匹配标题。
pub(super) fn close_ides(instances: &[RunningIde], timeout_secs: i64) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let pids: Vec<u32> = instances.iter().map(|inst| inst.pid).collect();
        if pids.is_empty() {
            return Ok(());
        }
        let alive =
            win_close::close_gracefully(&pids, Duration::from_secs(timeout_secs.max(1) as u64));
        if alive.is_empty() {
            Ok(())
        } else {
            Err(close_timeout_error(&alive))
        }
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
            if alive.is_empty() {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err(close_timeout_error(&alive));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let pids: Vec<u32> = instances.iter().map(|inst| inst.pid).collect();
        if pids.is_empty() {
            return Ok(());
        }
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
            if alive.is_empty() {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err(close_timeout_error(&alive));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
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

/// 状态：是否安装、插件安装、运行、登录、当前账号（本地状态文件 + 账号库）。
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

    let mut active_account_id = active_account_id_from_state();
    let mut active_account_name: Option<String> = None;
    if let Some(id) = active_account_id.clone() {
        if let Some(acc) = account::find_account(&id) {
            active_account_name = Some(account::account_display_name(&acc));
        } else {
            active_account_id = None;
        }
    }

    json!({
        "installed": installed,
        "pluginInstalled": plugin_installed,
        "running": is_running,
        "loggedIn": logged_in,
        "configRoot": jetbrains_config_root().map(|p| p.to_string_lossy().to_string()),
        "targets": targets.iter().map(|t| t.to_status_json(&running_dirs)).collect::<Vec<_>>(),
        "activeAccountId": active_account_id,
        "activeAccountName": active_account_name,
        "detectedFrom": "secret-storage",
        "statePath": state_path().to_string_lossy(),
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
