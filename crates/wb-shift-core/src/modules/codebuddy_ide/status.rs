//! 运行状态：Linux 进程探测、应用路径与 status 查询。

#[allow(unused_imports)]
use super::detect::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_cmdline_is_codebuddy_ide(cmdline: &str) -> bool {
    let lower = cmdline.to_ascii_lowercase();
    // 自排除：命令行含本工具产物名（`workbuddy-shift` 子串已涵盖
    // `workbuddy-shift-desktop`）即不是 CodeBuddy IDE 进程。
    if lower.contains("workbuddy-shift") {
        return false;
    }
    if lower.contains("crashpad") || lower.contains("--type=") {
        return false;
    }
    if cmdline.contains("CodeBuddy CN")
        || lower.contains("codebuddy-cn")
        || lower.contains("codebuddycn")
        || lower.contains("workbuddy")
    {
        return false;
    }
    cmdline.contains("CodeBuddy") || lower.contains("codebuddy")
}

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_exe_is_codebuddy_ide(exe: &Path) -> bool {
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim();
    let lower = name.to_ascii_lowercase();
    if lower.contains("codebuddy-cn")
        || lower.contains("codebuddycn")
        || lower.contains("workbuddy")
    {
        return false;
    }
    name.eq_ignore_ascii_case("codebuddy") || is_codebuddy_ide_image_name(name)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_codebuddy_ide_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pid == self_pid {
            continue;
        }
        if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            if linux_exe_is_codebuddy_ide(&exe) {
                pids.push(pid);
                continue;
            }
        }
        let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).replace('\0', " "),
            _ => continue,
        };
        if linux_cmdline_is_codebuddy_ide(&cmdline) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn wait_linux_pids_gone(pids: &[u32], timeout: Duration) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        let alive: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
            .collect();
        if alive.is_empty() || Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn kill_linux_pids(pids: &[u32], signal: &str) {
    if pids.is_empty() {
        return;
    }
    let owned: Vec<String> = std::iter::once(signal.to_string())
        .chain(pids.iter().map(|pid| pid.to_string()))
        .collect();
    let args: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let _ = run_cmd("kill", &args, 10);
}

/// 解析 CodeBuddy 应用路径（macOS: .app bundle；Windows: exe）。
pub fn codebuddy_ide_app_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_ide_app_path_resolved()
    }
    #[cfg(target_os = "windows")]
    {
        windows_ide_exe_path_resolved()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(cached) = load_codebuddy_ide_app_cache() {
            if cached.is_file() && linux_exe_is_codebuddy_ide(&cached) {
                return Some(cached);
            }
            clear_codebuddy_ide_app_cache();
        }
        let candidates = [
            "/usr/bin/codebuddy",
            "/usr/local/bin/codebuddy",
            "/opt/codebuddy/codebuddy",
        ];
        for p in candidates {
            let path = PathBuf::from(p);
            if path.is_file() {
                persist_ide_app_cache(&path);
                return Some(path);
            }
        }
        None
    }
}

/// CodeBuddy 是否在运行（footer 语义 = GUI 主进程）。
pub fn is_codebuddy_ide_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        let resolved = macos_ide_app_path_resolved();
        let patterns = macos_ide_main_patterns(resolved.as_deref());
        !process::macos_pids_by_patterns(&patterns).is_empty()
    }
    #[cfg(target_os = "windows")]
    {
        !windows_ide_process_rows().is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        !linux_codebuddy_ide_pids().is_empty()
    }
}

/// 状态：是否安装、是否运行、当前账号（仅来自本地状态文件 + 账号库，不读取钥匙串）。
pub fn status() -> Value {
    let data_dir = intl_data_dir();
    let db_path = intl_state_db_path();
    // `installed` 只表示"存在可运行的客户端"。数据目录存在不能算已安装——
    // 只读探测不再建目录（issue #91），残留空目录也不应误报「已接入」；
    // 是否有残留数据由 dataDir / dbExists 表达。
    let installed = codebuddy_ide_app_path().is_some();
    let db_exists = db_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let running = is_codebuddy_ide_running();
    // 登录态 = `state.vscdb` 里是否存在会话 secret 行：只查 key、不解密（macOS 解密会弹
    // 钥匙串授权，绝不能进这条轮询路径）。查询失败/文件不存在一律 false，仅用于文案与入口判定。
    let logged_in =
        has_secret_row_for(&CODEBUDDY_INTL_TARGET, data_dir.as_deref()).unwrap_or(false);

    let mut active_account_id = active_account_id_from_state();
    let mut active_account_name: Option<String> = None;

    if let Some(id) = active_account_id.clone() {
        if let Some(acc) = account::find_account(&id) {
            active_account_name = Some(account::account_display_name(&acc));
        } else {
            // 状态文件有记录但账号库已无此账号：视为未检测到，不回退读取钥匙串
            active_account_id = None;
        }
    }

    json!({
        "installed": installed,
        "running": running,
        "loggedIn": logged_in,
        "dataDir": data_dir.map(|p| p.to_string_lossy().to_string()),
        "dbPath": db_path.map(|p| p.to_string_lossy().to_string()),
        "dbExists": db_exists,
        "appPath": codebuddy_ide_app_path().map(|p| p.to_string_lossy().to_string()),
        "activeAccountId": active_account_id,
        "activeAccountName": active_account_name,
        "detectedFrom": "state",
        "statePath": state_path().to_string_lossy(),
    })
}
