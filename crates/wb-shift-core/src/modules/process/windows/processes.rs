//! 进程枚举与关闭：镜像名判定、tasklist/PowerShell 行查询、PID 等待与 close。

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
#[cfg(target_os = "windows")]
pub(crate) fn ps_output(script: &str, timeout_secs: u64) -> Option<String> {
    let out = run_cmd_timeout(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
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

/// 先对目标 PID `taskkill /PID /T`（无 /F），超时再 `/F`；按 PID 等待，不按名称子串。
#[cfg(target_os = "windows")]
pub(crate) fn close_workbuddy_windows(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    let rows = windows_workbuddy_process_rows(variant);
    if rows.is_empty() {
        return Ok(());
    }
    let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();
    for pid in &pids {
        let pid_s = pid.to_string();
        let _ = run_cmd_timeout("taskkill", &["/PID", &pid_s, "/T"], 10);
    }

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let graceful_budget = Duration::from_secs(8).min(timeout);
    let remaining = wait_windows_pids_gone(&pids, graceful_budget);
    if remaining.is_empty() {
        return Ok(());
    }

    for pid in &remaining {
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
    Err("WorkBuddy 进程无法关闭，请手动结束 WorkBuddy/CodeBuddy 进程".to_string())
}
