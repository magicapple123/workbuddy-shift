//! 运行状态与窗口生命周期：is_vscode_running、status、RunningInstance、close/launch。

#[allow(unused_imports)]
use super::detect::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

/// VS Code 是否在运行（footer 语义 = GUI 主进程）。
pub fn is_vscode_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        let patterns = macos_code_main_patterns();
        !process::macos_pids_by_patterns(&patterns).is_empty()
    }
    #[cfg(target_os = "windows")]
    {
        !windows_code_process_rows().is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        !linux_code_pids().is_empty()
    }
}

/// 状态：是否安装（数据目录存在）、是否运行、是否登录（只读）、当前账号（本地状态文件 + 账号库）。
pub fn status() -> Value {
    let data_dir = vscode_data_dir();
    let db_path = vscode_ext_state_db_path();
    let installed = data_dir.as_ref().map(|p| p.exists()).unwrap_or(false);
    // 扩展是否已安装：VS Code 数据目录下 globalStorage/<extensionId> 存在。
    let extension_installed = data_dir
        .as_ref()
        .map(|d| {
            d.join("User")
                .join("globalStorage")
                .join(EXTENSION_ID)
                .exists()
        })
        .unwrap_or(false);
    let db_exists = db_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let running = is_vscode_running();
    // 登录态 = `state.vscdb` 里是否存在会话 secret 行：只查 key、不解密（macOS 解密会弹
    // 钥匙串授权，绝不能进这条轮询路径）。查询失败/文件不存在一律 false，仅用于文案。
    let logged_in = has_secret_row_for(&VSCODE_TARGET, data_dir.as_deref()).unwrap_or(false);

    let mut active_account_id = active_account_id_from_state();
    let mut active_account_name: Option<String> = None;

    if let Some(id) = active_account_id.clone() {
        if let Some(acc) = account::find_account(&id) {
            active_account_name = Some(account::account_display_name(&acc));
        } else {
            // 状态文件有记录但账号库已无此账号：视为未检测到，不回退读取 secret。
            active_account_id = None;
        }
    }

    json!({
        "installed": installed,
        "extensionInstalled": extension_installed,
        "running": running,
        "loggedIn": logged_in,
        "dataDir": data_dir.map(|p| p.to_string_lossy().to_string()),
        "dbPath": db_path.map(|p| p.to_string_lossy().to_string()),
        "dbExists": db_exists,
        "activeAccountId": active_account_id,
        "activeAccountName": active_account_name,
        "detectedFrom": "state",
        "statePath": state_path().to_string_lossy(),
    })
}

/// 等待 VS Code 退出的默认上限（秒）：超时只报错，不强杀（D2/D9）。
pub(super) const VSCODE_CLOSE_TIMEOUT_SECS: i64 = 60;

/// 运行中 + 手动模式（`restart=false`）的报错文案：与改造前逐字一致。
pub(super) const RUNNING_MANUAL_HINT: &str =
    "检测到 VS Code 正在运行，请先完全退出后再切换，否则写入会被 VS Code 覆盖。";

/// 关闭前采集的 VS Code 运行实例快照。
///
/// **必须在关闭前采集**：编辑器退出后进程表里已无可回溯的启动路径，重开就没依据了。
#[derive(Debug)]
pub(super) struct RunningInstance {
    /// 命中的相关进程 PID（macOS 主进程层 / Windows 全部 `Code.exe` / Linux Code 进程）。
    pub(super) pids: Vec<u32>,
    /// 主进程可执行文件路径（Windows 来自 CIM `ExecutablePath`，Linux 读 `/proc/<pid>/exe`）。
    pub(super) main_exe: Option<PathBuf>,
    /// macOS 由主进程 args 回溯出的 `.app` bundle 路径。
    pub(super) macos_bundle: Option<PathBuf>,
}

impl RunningInstance {
    pub(super) fn running(&self) -> bool {
        !self.pids.is_empty()
    }

    /// 重开目标：macOS 优先 `.app` bundle，Windows / Linux 用可执行文件路径。
    pub(super) fn launch_target(&self) -> Option<&Path> {
        self.macos_bundle.as_deref().or(self.main_exe.as_deref())
    }
}

#[cfg(target_os = "macos")]
pub(super) fn running_instance() -> RunningInstance {
    let rows = process::macos_rows_by_patterns(&macos_code_main_patterns());
    let pids = rows.iter().map(|(pid, _)| *pid).collect();
    let macos_bundle = rows
        .iter()
        .find_map(|(_, args)| process::extract_app_bundle_from_args(args));
    RunningInstance {
        pids,
        main_exe: None,
        macos_bundle,
    }
}

#[cfg(target_os = "windows")]
pub(super) fn running_instance() -> RunningInstance {
    let rows = windows_code_process_rows();
    let pids = rows.iter().map(|row| row.pid).collect();
    let main_exe = rows.iter().find_map(|row| row.exe_path.clone());
    RunningInstance {
        pids,
        main_exe,
        macos_bundle: None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn running_instance() -> RunningInstance {
    let rows = linux_code_processes();
    let pids = rows.iter().map(|(pid, _)| *pid).collect();
    let main_exe = rows.iter().find_map(|(_, exe)| exe.clone());
    RunningInstance {
        pids,
        main_exe,
        macos_bundle: None,
    }
}

/// 关闭超时文案：列出剩余 PID 与可操作提示（**不做强杀**）。
pub(super) fn close_timeout_error(alive: &[u32]) -> String {
    let pids: Vec<String> = alive.iter().map(|pid| pid.to_string()).collect();
    format!(
        "等待 VS Code 退出超时（仍有进程运行: {}）。请在 VS Code 中处理保存提示；若你在等待期间重新打开过 VS Code，请退出后重试。",
        pids.join(", ")
    )
}

/// 关闭超时文案（带窗口标题版）：在只报 PID 的基础上点名「卡住的是哪个窗口」。
///
/// 为什么需要标题：PID 对用户没有意义，而 `main.ts - 项目 - Visual Studio Code`
/// 能直接告诉他是哪个窗口没关掉（例如弹了保存提示、或等待期间又打开了一个窗口）。
/// 标题可能拿不到（窗口随进程一起消失、权限不足），此时必须退化到只报 PID。
///
/// 标题的归一化（去空白 / 截断到 80 字符 / 最多 3 个 / 去重）由
/// [`process::window_titles_clause`] 统一提供：WorkBuddy / CodeBuddy / CodeBuddy CN /
/// CodeBuddy CLI 四条关闭路径共用同一份规则，这里只负责 VS Code 自己的话术。
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(super) fn close_timeout_error_with_titles(alive: &[u32], titles: &[String]) -> String {
    let base = close_timeout_error(alive);
    let Some(clause) = process::window_titles_clause(titles) else {
        return base;
    };
    // 在括号内追加窗口名，其余文案（含排查提示）逐字保留，避免两条路径话术漂移。
    base.replacen(
        "）。请在 VS Code 中处理保存提示",
        &format!("；{clause}）。请在 VS Code 中处理保存提示"),
        1,
    )
}

/// 关闭超时写入 `error.log`：超时是用户可感知的失败，现场（PID + 窗口标题）只在
/// 这一刻拿得到——VS Code 彻底退出后就再也无法回溯是哪个窗口没关掉。
#[cfg(target_os = "windows")]
fn record_close_timeout(message: &str) {
    crate::modules::error_log::record("backend", "关闭 VS Code 超时", message);
}

/// 优雅关闭 VS Code → 等待进程消失；超时返回可读错误，**绝不强杀**。
///
/// `pids` 取自**关闭前**采集的 [`RunningInstance`] 快照：Windows / Linux 按这些 PID 精确
/// 关闭，不再二次探测（二次探测会多花一次 CIM/PowerShell 探测，且两次探测之间编辑器自己
/// 退出时会拿到空集合、被误判成「我们关掉了它」而在写入后又把它拉起来）。
///
/// 契约（对齐 `.trellis/spec/wb-shift-core/backend/codebuddy-cn-process.md`）：
/// 禁止 `pkill -f` / `pgrep -f` / `taskkill /IM`，一律精确 PID；等待退出不得早退
/// （确认的是「按主进程模式命中的进程集合为空」，不是「主进程不在」）。
/// Windows 的优雅退出走 `SC_CLOSE`（`process::win_close`），同样**不加强制终止**；
/// 等待判据收窄为「仍持有可见窗口的 PID 全部退出」，理由见函数内 Windows 分支注释。
pub(super) fn close_vscode(pids: &[u32], timeout_secs: i64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // macOS 无需 PID 列表：`osascript quit app id` 走原生退出，等待用的是「主进程模式
        // 命中的全部进程」（含 Helper 辅助进程，见 close_vscode 的调用方契约）。
        let _ = pids;
        let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
        let started = Instant::now();
        let patterns = macos_code_main_patterns();
        // 走原生退出流程：保存提示 / 热退出保护由 VS Code 自己负责。
        let quit_script = r#"quit app id "com.microsoft.VSCode""#;
        match process::run_cmd_timeout("osascript", &["-e", quit_script], 10) {
            Some(out) if !out.status.success() => eprintln!(
                "[vscode] osascript quit failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
            None => eprintln!("[vscode] osascript quit timed out"),
            _ => {}
        }
        let remaining = timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100));
        let alive = if process::wait_macos_main_gone(&patterns, remaining) {
            Vec::new()
        } else {
            // 超时：把仍存活的 PID 报给用户，不做清杀。
            process::macos_pids_by_patterns(&patterns)
        };
        if alive.is_empty() {
            Ok(())
        } else {
            Err(close_timeout_error(&alive))
        }
    }
    #[cfg(target_os = "windows")]
    {
        if pids.is_empty() {
            return Ok(());
        }
        // 关闭手段：优先 `SC_CLOSE`（= 点窗口关闭按钮），让主进程自己走保存/退出流程。
        //
        // 为什么不是 `taskkill /PID /T`：不带 `/F` 的 taskkill 只对「有窗口」的进程有效，
        // 对语言服务 / GPU / 扩展宿主这类无窗口进程只回一行「只能强制终止」、进程原样存活；
        // 带上 `/F` 又是强杀，会丢用户未保存的编辑。原实现两种作用都没落到真正要关的主
        // 窗口上，于是必然等到超时（用户看到的「等待 VS Code 退出超时」）。
        //
        // 等待判据同步收窄：只等「仍持有可见窗口」的 PID——主进程负责把账号写回
        // `state.vscdb`，主进程退出即写入安全；剩下的语言服务再等下去只会误判超时。
        // 一个 pid 都没有可见窗口时（后台启动 / 窗口还没建出来）退回旧行为：对每个 PID
        // 发不带 `/F` 的 taskkill，并按全部 PID 等待。
        let window_pids = process::visible_window_pids(pids);
        if window_pids.is_empty() {
            for pid in pids {
                let pid_arg = pid.to_string();
                let _ = process::run_cmd_timeout("taskkill", &["/PID", &pid_arg, "/T"], 10);
            }
        } else {
            for pid in &window_pids {
                // 返回值只用于判断「有没有窗口」：已经算过 window_pids，这里忽略即可。
                let _ = process::post_close_to_visible_windows(*pid);
            }
        }
        let wait_targets = process::close_wait_targets(pids, &window_pids);
        let alive = process::wait_windows_pids_gone(
            &wait_targets,
            Duration::from_secs(timeout_secs.max(1) as u64),
        );
        if alive.is_empty() {
            Ok(())
        } else {
            let titles = process::window_titles(&alive);
            let message = close_timeout_error_with_titles(&alive, &titles);
            record_close_timeout(&message);
            Err(message)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if pids.is_empty() {
            Ok(())
        } else {
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
}

/// `open` 失败原因：优先 stderr，为空时回退出码。
#[cfg(target_os = "macos")]
pub(super) fn open_error_reason(out: &std::process::Output) -> String {
    let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if reason.is_empty() {
        format!("open 退出码 {}", out.status.code().unwrap_or(-1))
    } else {
        reason
    }
}

/// 重新打开 VS Code：只用关闭前记录的路径（D8），不依赖 `code` CLI / PATH。
pub(super) fn launch_vscode(inst: &RunningInstance) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let bundle = inst
            .launch_target()
            .filter(|path| process::is_app_bundle(path))
            .map(Path::to_path_buf);
        match bundle {
            Some(bundle) => {
                let app = bundle.to_string_lossy().into_owned();
                match process::run_cmd_timeout("open", &[app.as_str()], 10) {
                    Some(out) if out.status.success() => Ok(()),
                    Some(out) => Err(format!(
                        "重新打开 VS Code 失败: {}（路径: {}）",
                        open_error_reason(&out),
                        bundle.display()
                    )),
                    None => Err(format!(
                        "重新打开 VS Code 失败: open 超时（路径: {}）",
                        bundle.display()
                    )),
                }
            }
            // 退化：记录不到 bundle 路径时按 bundle id 打开。
            None => match process::run_cmd_timeout("open", &["-b", "com.microsoft.VSCode"], 10) {
                Some(out) if out.status.success() => Ok(()),
                Some(out) => Err(format!(
                    "重新打开 VS Code 失败: {}",
                    open_error_reason(&out)
                )),
                None => Err("重新打开 VS Code 失败: open 超时".to_string()),
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let exe = inst
            .launch_target()
            .ok_or_else(|| "未能记录 VS Code 可执行文件路径，请手动打开 VS Code".to_string())?
            .to_path_buf();
        process::cmd_builder(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("重新打开 VS Code 失败: {e}（路径: {}）", exe.display()))?;
        Ok(())
    }
}
