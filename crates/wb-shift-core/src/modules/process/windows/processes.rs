//! 进程枚举与关闭：镜像名判定、tasklist/PowerShell 行查询、PID 等待与 close。

#[cfg(target_os = "windows")]
use super::super::{
    close_timeout_error_message, close_wait_targets, force_kill_targets,
    post_close_to_visible_windows, utf8_script, visible_window_pids, window_titles,
};
use super::parsing::*;
use super::registry::*;
use super::*;

/// 精确匹配本档位的 WorkBuddy 映像，禁止子串命中其它档位或 workbuddy-shift。
pub(super) fn is_workbuddy_image_name(name: &str, variant: WbVariant) -> bool {
    let stem = image_stem(image_name_from_path_str(name));
    variant
        .windows_image_names()
        .iter()
        .any(|candidate| stem.eq_ignore_ascii_case(candidate))
}

/// 本档位的 Windows 可执行文件名。
pub(crate) fn windows_exe_names(variant: WbVariant) -> Vec<String> {
    variant
        .windows_image_names()
        .iter()
        .map(|name| format!("{name}.exe"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsProcessRow {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) exe_path: Option<PathBuf>,
}

/// Windows：执行 PowerShell 并取 stdout。
///
/// 脚本统一经 [`utf8_script`] 前置 UTF-8 输出编码：否则 PowerShell 按控制台代码页
/// （简中 = GBK）写 stdout，含中文的路径会解成 `U+FFFD`，后续 spawn 直接 `os error 3`。
#[cfg(target_os = "windows")]
pub(crate) fn ps_output(script: &str, timeout_secs: u64) -> Option<String> {
    let encoded = utf8_script(script);
    let out = run_cmd_timeout(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &encoded],
        timeout_secs,
    )?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_tasklist_image_rows(image: &str) -> Vec<WindowsProcessRow> {
    let filter = format!("IMAGENAME eq {image}");
    let Some(out) = run_cmd_timeout("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"], 5) else {
        return Vec::new();
    };
    parse_tasklist_csv(&String::from_utf8_lossy(&out.stdout))
}

/// 精确映像名收集本档位 WorkBuddy PID，排除本工具、自身 PID 与 crashpad。
#[cfg(target_os = "windows")]
pub(crate) fn windows_workbuddy_process_rows(variant: WbVariant) -> Vec<WindowsProcessRow> {
    let self_pid = std::process::id();
    let mut rows = Vec::new();
    for exe in windows_exe_names(variant) {
        rows.extend(windows_tasklist_image_rows(&exe));
    }
    filter_windows_workbuddy_rows(&rows, variant)
        .into_iter()
        .filter(|r| r.pid != self_pid)
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn is_windows_pid_running(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    match run_cmd_timeout("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"], 5) {
        Some(out) => parse_tasklist_csv(&String::from_utf8_lossy(&out.stdout))
            .iter()
            .any(|r| r.pid == pid),
        None => true,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn wait_windows_pids_gone(pids: &[u32], timeout: Duration) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        let alive: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| is_windows_pid_running(*pid))
            .collect();
        if alive.is_empty() || Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn existing_windows_drives() -> Vec<char> {
    ('A'..='Z')
        .filter(|c| Path::new(&format!(r"{c}:\")).exists())
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn windows_running_workbuddy_exe(variant: WbVariant) -> Option<PathBuf> {
    let names = variant.windows_image_names().join(",");
    let script = format!(
        "Get-Process -Name {names} -ErrorAction SilentlyContinue | \
         ForEach-Object {{ $p = ''; try {{ $p = $_.Path }} catch {{}}; '{{0}}|{{1}}|{{2}}' -f $_.Id, $_.ProcessName, $p }}"
    );
    let stdout = ps_output(&script, 5)?;
    for row in filter_windows_workbuddy_rows(&parse_windows_process_rows(&stdout), variant) {
        if let Some(p) = row.exe_path {
            if is_existing_workbuddy_exe(&p, variant) {
                return Some(p);
            }
        }
    }
    None
}

/// 优雅关闭 WorkBuddy → 超时只强杀无窗口残留 → 报错点名残留 PID（+ 窗口标题）。
///
/// **为什么不是对每个 PID 直接 `taskkill /PID /T`**：不带 `/F` 时它只对「有窗口」的进程
/// 有效——对没有窗口的子进程只回一行「只能强制终止」，而这些子进程并不是真正要关的主
/// 窗口；主窗口本身则要靠目标进程自己走保存/退出流程。所以：
/// - 有可见窗口的 PID → 投 `SC_CLOSE`（等价用户点 ✕，保存提示由目标进程自己弹），
///   **不再**对它发 `taskkill /T`；
/// - 没有可见窗口的 PID → 保持旧行为，`taskkill /PID /T`（不带 `/F`）。
///
/// 超时后的 `/F` **只允许**打给「此刻仍没有可见窗口」的残留 PID（[`force_kill_targets`]）：
/// 对有窗口的进程强杀会丢用户未保存的内容，宁可超时报错让用户自己处理。
#[cfg(target_os = "windows")]
pub(crate) fn close_workbuddy_windows(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    let rows = windows_workbuddy_process_rows(variant);
    if rows.is_empty() {
        return Ok(());
    }
    let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();

    let window_pids = visible_window_pids(&pids);
    for pid in &window_pids {
        // 返回值只表示「有没有命中窗口」：集合已经算过，这里忽略即可。
        let _ = post_close_to_visible_windows(*pid);
    }
    // 没有可见窗口的 PID 走旧行为（不带 `/F` 的 `/T`）。集合与「允许强杀的 PID」同源，
    // 故复用 [`force_kill_targets`]，不另写一份「pids - window_pids」。
    for pid in force_kill_targets(&pids, &window_pids) {
        let pid_s = pid.to_string();
        let _ = run_cmd_timeout("taskkill", &["/PID", &pid_s, "/T"], 10);
    }

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let graceful_budget = Duration::from_secs(8).min(timeout);
    // 等待判据收窄到「仍持有可见窗口的 PID」：后台子进程不参与落盘，等它们只会把
    // 「已经关掉了」误判成「等待退出超时」。
    let wait_targets = close_wait_targets(&pids, &window_pids);
    let remaining = wait_windows_pids_gone(&wait_targets, graceful_budget);
    if remaining.is_empty() {
        return Ok(());
    }

    // 重新探测而不是复用上面的 `window_pids`：窗口可能是在上一轮探测之后才建出来的。
    let still_windowed = visible_window_pids(&remaining);
    for pid in force_kill_targets(&remaining, &still_windowed) {
        let pid_s = pid.to_string();
        let _ = run_cmd_timeout("taskkill", &["/PID", &pid_s, "/T", "/F"], 10);
    }
    let rest = timeout
        .saturating_sub(started.elapsed())
        .max(Duration::from_secs(1));
    let leftover = wait_windows_pids_gone(&remaining, rest);
    if leftover.is_empty() {
        return Ok(());
    }
    let product = variant
        .windows_image_names()
        .first()
        .copied()
        .unwrap_or("WorkBuddy");
    let titles = window_titles(&leftover);
    Err(close_timeout_error_message(product, &leftover, &titles))
}
