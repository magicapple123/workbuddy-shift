//! JetBrains 安装/运行检测：配置根与插件目录、运行中 IDE 枚举、切换目标与当前账号识别。

#[allow(unused_imports)]
use super::status::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

/// JetBrains 配置根目录（应用级，各产品 `<产品><版本>` 子目录并列）。
///
/// - Windows: `%APPDATA%\JetBrains`
/// - macOS: `~/Library/Application Support/JetBrains`
/// - Linux: `$XDG_CONFIG_HOME/JetBrains`（缺省 `~/.config/JetBrains`）
pub fn jetbrains_config_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::modules::config::home_dir().join("Library/Application Support/JetBrains"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join("JetBrains"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs::config_dir().map(|d| d.join("JetBrains"))
    }
}

/// 配置目录名是否属于受支持产品：前缀 + 纯版本号后缀（`2026.2` 形态）。
///
/// 后缀严格限定数字与点，把 `PyCharm2026.2-backup` 这类迁移残留目录排除在写入范围外。
pub(super) fn is_supported_config_dir_name(name: &str) -> bool {
    PRODUCT_PREFIXES.iter().any(|prefix| {
        name.len() > prefix.len()
            && name.starts_with(prefix)
            && name[prefix.len()..]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.')
            && name[name.len() - 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
    })
}

/// `<配置目录>/plugins/` 下是否存在 CodeBuddy 插件安装目录。
pub(super) fn plugin_dir(config_dir: &Path) -> Option<PathBuf> {
    let plugins = config_dir.join("plugins");
    let entries = std::fs::read_dir(&plugins).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PLUGIN_DIR_PREFIX) && entry.path().is_dir() {
            return Some(entry.path());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 进程发现：运行中的 IDE 与其配置目录的精确映射
// ---------------------------------------------------------------------------

/// 运行中的 JetBrains IDE 实例快照（关闭前采集）。
#[derive(Debug, Clone)]
pub(super) struct RunningIde {
    pub(super) pid: u32,
    /// Windows: 主进程 exe；macOS: `.app` bundle；Linux: 安装根（best-effort）。
    pub(super) launch_target: Option<PathBuf>,
    /// 由安装目录 `product-info.json#dataDirectoryName` 解析出的配置目录名。
    pub(super) config_dir_name: Option<String>,
}

/// 从安装根读取 `product-info.json#dataDirectoryName`（JetBrains 全系自带）。
pub(super) fn data_dir_name_for_install_root(root: &Path) -> Option<String> {
    let info = std::fs::read_to_string(root.join("product-info.json")).ok()?;
    let value: Value = serde_json::from_str(&info).ok()?;
    value
        .get("dataDirectoryName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 从主进程可执行文件路径反推安装根：`<root>/bin/<x>.exe` → `<root>`。
#[cfg(target_os = "windows")]
pub(super) fn install_root_from_exe(exe: &Path) -> Option<PathBuf> {
    exe.parent().and_then(Path::parent).map(Path::to_path_buf)
}

/// 从运行进程行构造快照；映射不上的实例 `config_dir_name = None`。
#[cfg(target_os = "windows")]
pub(super) fn running_ide_from_row(row: &process::WindowsProcessRow) -> RunningIde {
    let config_dir_name = row
        .exe_path
        .as_deref()
        .and_then(install_root_from_exe)
        .and_then(|root| data_dir_name_for_install_root(&root));
    RunningIde {
        pid: row.pid,
        launch_target: row.exe_path.clone(),
        config_dir_name,
    }
}

#[cfg(target_os = "windows")]
pub(super) fn windows_ide_process_rows() -> Vec<process::WindowsProcessRow> {
    let self_pid = std::process::id();
    let script = "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | \
                  Where-Object { $_.Name -in 'idea64.exe','pycharm64.exe','idea.exe','pycharm.exe' } | \
                  ForEach-Object { '{0}|{1}|{2}' -f $_.ProcessId, $_.Name, $_.ExecutablePath }";
    let mut rows: Vec<_> = process::ps_output(script, 8)
        .map(|stdout| {
            process::parse_windows_process_rows(&stdout)
                .into_iter()
                .filter(|row| row.pid != self_pid)
                .collect()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        // CIM 不可用时退回 tasklist（拿到 PID 关闭，但丢失 exe 路径 → 无法重开）。
        for image in ["idea64.exe", "pycharm64.exe", "idea.exe", "pycharm.exe"] {
            rows.extend(process::windows_tasklist_image_rows(image));
        }
    }
    rows.retain(|row| {
        let name = row.name.trim();
        WINDOWS_IMAGE_NAMES
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n))
    });
    rows
}

/// 枚举当前运行中的受支持 IDE 实例（同配置目录多实例按 PID 全保留）。
pub(super) fn running_ides() -> Vec<RunningIde> {
    #[cfg(target_os = "windows")]
    {
        windows_ide_process_rows()
            .iter()
            .map(running_ide_from_row)
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        // 进程 cmdline 形如 `/Applications/IntelliJ IDEA.app/Contents/MacOS/idea`；
        // bundle 路径回溯后读 `Contents/Resources/product-info.json`。
        // 产品名集合（.app 名）：IntelliJ IDEA、IntelliJ IDEA CE、PyCharm、PyCharm CE。
        let patterns: Vec<String> = [
            "IntelliJ IDEA.app/Contents/MacOS",
            "IntelliJ IDEA CE.app/Contents/MacOS",
            "PyCharm.app/Contents/MacOS",
            "PyCharm CE.app/Contents/MacOS",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut rows = Vec::new();
        for (pid, args) in process::macos_rows_by_patterns(&patterns) {
            let bundle = process::extract_app_bundle_from_args(&args);
            let config_dir_name = bundle
                .as_deref()
                .and_then(|b| data_dir_name_for_install_root(&b.join("Contents/Resources")));
            rows.push(RunningIde {
                pid,
                launch_target: bundle,
                config_dir_name,
            });
        }
        rows
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux：脚本启动时 argv[0] 是 `<root>/bin/<x>`；java 直启时 cmdline 含
        // `-Didea.home.path=<root>`。两者都取不到就放弃映射（只报告、不关闭）。
        let self_pid = std::process::id();
        let mut rows = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let pid: u32 = match entry.file_name().to_string_lossy().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if pid == self_pid {
                    continue;
                }
                let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
                    Ok(bytes) if !bytes.is_empty() => {
                        String::from_utf8_lossy(&bytes).replace('\0', " ")
                    }
                    _ => continue,
                };
                let lower = cmdline.to_ascii_lowercase();
                if !["idea", "pycharm"].iter().any(|name| {
                    lower.contains(&format!("/bin/{name} "))
                        || lower.contains(&format!("/bin/{name}\""))
                        || lower.ends_with(&format!("/bin/{name}"))
                }) && !lower.contains("-didea.home.path=")
                {
                    continue;
                }
                let root = cmdline.split_whitespace().find_map(|arg| {
                    let path = Path::new(arg);
                    if path.is_dir() && path.join("product-info.json").exists() {
                        return Some(path.to_path_buf());
                    }
                    // `<root>/bin/<x>` → `<root>`
                    let mut ancestors = path.ancestors();
                    ancestors.next()?; // 自身
                    let bin = ancestors.next()?;
                    let root = bin.parent()?;
                    if root.join("product-info.json").exists() && bin.ends_with("bin") {
                        return Some(root.to_path_buf());
                    }
                    None
                });
                let config_dir_name = root.as_deref().and_then(data_dir_name_for_install_root);
                rows.push(RunningIde {
                    pid,
                    launch_target: root,
                    config_dir_name,
                });
            }
        }
        rows
    }
}

// ---------------------------------------------------------------------------
// 状态与切换
// ---------------------------------------------------------------------------

/// 切换目标：一个装有（或可装）CodeBuddy 插件的产品配置目录。
#[derive(Debug, Clone)]
pub(super) struct Target {
    /// 产品配置目录（如 `%APPDATA%\JetBrains\PyCharm2026.2`）。
    pub(super) config_dir: PathBuf,
    /// 配置目录名（如 `PyCharm2026.2`）。
    pub(super) name: String,
    /// 是否安装了 CodeBuddy 插件。
    pub(super) plugin_installed: bool,
}

impl Target {
    pub(super) fn secret_path(&self) -> PathBuf {
        secret_path_for(&self.config_dir)
    }

    pub(super) fn to_status_json(&self, running_dirs: &[String]) -> Value {
        let logged_in = read_secret_for(&self.config_dir).ok().flatten().is_some();
        json!({
            "configDir": self.name,
            "pluginInstalled": self.plugin_installed,
            "running": running_dirs.iter().any(|n| n == &self.name),
            "loggedIn": logged_in,
            "secretPath": self.secret_path().to_string_lossy(),
        })
    }
}

/// 扫描配置根，列出受支持产品的配置目录（存在即列出，无论是否装插件）。
pub(super) fn list_targets() -> Vec<Target> {
    let Some(root) = jetbrains_config_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut targets: Vec<Target> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (entry.path(), name)
        })
        .filter(|(_, name)| is_supported_config_dir_name(name))
        .map(|(path, name)| Target {
            plugin_installed: plugin_dir(&path).is_some(),
            config_dir: path,
            name,
        })
        .collect();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    targets
}

/// 从本机 JetBrains IDE 读取当前 token；若能匹配账号库则返回匹配信息。
pub fn detect_current_account() -> Result<Value, String> {
    let targets: Vec<Target> = list_targets()
        .into_iter()
        .filter(|t| t.plugin_installed)
        .collect();
    for target in &targets {
        // 单个目标的 secret 读取失败按未登录处理，不阻断其它目标。
        let Some(secret) = read_secret_for(&target.config_dir).ok().flatten() else {
            continue;
        };
        let Some((uid, token)) = parse_token_from_secret(&secret) else {
            continue;
        };
        if let Some(acc) = match_account_for_token(uid.as_deref(), &token) {
            let id = get_str(&acc, "id").unwrap_or_default();
            let _ = set_active_account_id(&id);
            return Ok(json!({
                "ok": true,
                "found": true,
                "matched": true,
                "configDir": target.name,
                "accountId": id,
                "account": account::account_meta(&acc),
                "uid": uid,
            }));
        }
        return Ok(json!({
            "ok": true,
            "found": true,
            "matched": false,
            "configDir": target.name,
            "uid": uid,
            "message": "本机 JetBrains IDE 已登录 CodeBuddy 插件，但账号库中无匹配账号；可先用「从本机导入」或扫码登录同步账号后再切换。",
        }));
    }
    Ok(json!({
        "ok": true,
        "found": false,
        "message": "本机 JetBrains IDE 未找到 CodeBuddy 插件登录 secret",
    }))
}
