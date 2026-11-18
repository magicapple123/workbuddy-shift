//! helper.cjs 安装与校验：命令/路径解析、Windows 短路径、node 探测与输出校验。

use super::settings::*;
use super::state::*;
use super::*;

pub(super) fn helper_command() -> Option<String> {
    read_json_file(&settings_path())
        .and_then(|settings| {
            settings
                .get("apiKeyHelper")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

/// `apiKeyHelper` 在 CodeBuddy 2.138.0 中先作为文件路径解析；
/// 这里只识别单一路径，并保留旧 `.cmd` / `.sh` helper 的升级通道。
pub(super) fn command_path(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 只把单一路径视为项目 helper；不解析或接管用户的其他 shell 命令。
    // Windows 绝对路径可能含空格；CodeBuddy 2.138.0 会先将路径解析为
    // workdir 绝对路径，再交给 shell，所以不能在设置里额外包引号。
    let windows_absolute = trimmed.len() >= 3
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':'
        && matches!(trimmed.as_bytes()[2], b'\\' | b'/');
    if trimmed.starts_with('\'')
        || trimmed.starts_with('"')
        || (!windows_absolute && trimmed.chars().any(char::is_whitespace))
    {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

pub(super) fn helper_path() -> Option<PathBuf> {
    helper_command().and_then(|command| command_path(&command))
}

pub(super) fn helper_is_configured() -> bool {
    helper_path().map(|path| path.is_file()).unwrap_or(false)
}

pub(super) fn helper_migration_required() -> bool {
    let Some(path) = helper_path() else {
        return false;
    };
    is_legacy_helper_path(&path, &rotate_dir(), cfg!(windows))
}

pub(super) fn is_legacy_helper_path(path: &Path, directory: &Path, windows: bool) -> bool {
    same_path(path, &directory.join(LEGACY_WINDOWS_HELPER_FILE), windows)
        || same_path(path, &directory.join(LEGACY_HELPER_FILE), windows)
}

pub(super) fn helper_is_current() -> bool {
    let Some(path) = helper_path() else {
        return false;
    };
    path.is_file()
        && same_path(&path, &rotate_dir().join(HELPER_FILE), cfg!(windows))
        && helper_supports_account_ids()
}

pub(super) fn helper_supports_account_ids() -> bool {
    // 升级前 Windows 可能仍配置 `.cmd` 跳板，因此同时检查实际
    // helper.cjs；新安装会直接指向 helper.cjs。
    let configured = helper_path().and_then(|path| std::fs::read_to_string(path).ok());
    let logic = std::fs::read_to_string(rotate_dir().join(LOGIC_FILE)).ok();
    configured
        .into_iter()
        .chain(logic)
        .any(|source| source.contains("activeAccountId"))
}

pub(super) fn comparable_path(path: &Path, windows: bool) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if windows {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

pub(super) fn same_path(left: &Path, right: &Path, windows: bool) -> bool {
    if let (Ok(left), Ok(right)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        return comparable_path(&left, windows) == comparable_path(&right, windows);
    }
    comparable_path(left, windows) == comparable_path(right, windows)
}

#[cfg(any(windows, test))]
pub(super) fn select_windows_configured_path(
    path: &Path,
    short_path: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if path_is_posix_eval_safe(path) {
        return Ok(path.to_path_buf());
    }
    short_path
        .filter(|candidate| path_is_posix_eval_safe(candidate))
        .ok_or_else(|| {
            helper_validation_error(
                "配置阶段",
                "helper 路径含有 shell 不安全字符，且 Windows 无法生成可供 CodeBuddy 2.138.0 执行的短路径",
            )
        })
}

#[cfg(any(windows, test))]
pub(super) fn path_is_posix_eval_safe(path: &Path) -> bool {
    !path.to_string_lossy().chars().any(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '&' | '|'
                    | ';'
                    | '\''
                    | '"'
                    | '`'
                    | '$'
                    | '('
                    | ')'
                    | '<'
                    | '>'
                    | '!'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '{'
                    | '}'
            )
    })
}

#[cfg(windows)]
pub(super) fn windows_short_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long_path: *const u16, short_path: *mut u16, buffer_len: u32) -> u32;
    }

    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let required = unsafe { GetShortPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return None;
    }
    let mut output = vec![0_u16; required as usize + 1];
    let written =
        unsafe { GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if written == 0 || written as usize >= output.len() {
        return None;
    }
    output.truncate(written as usize);
    Some(PathBuf::from(std::ffi::OsString::from_wide(&output)))
}

#[cfg(windows)]
pub(super) fn configured_helper_path(path: &Path) -> Result<PathBuf, String> {
    select_windows_configured_path(path, windows_short_path(path))
}

#[cfg(not(windows))]
pub(super) fn configured_helper_path(path: &Path) -> Result<PathBuf, String> {
    Ok(path.to_path_buf())
}

pub(super) fn helper_validation_error(stage: &str, cause: &str) -> String {
    #[cfg(windows)]
    let hint = "请确认 Git Bash 和 Node.js 可用，然后重试；如仍失败，请查看 CodeBuddy CLI 日志";
    #[cfg(not(windows))]
    let hint = "请确认 Node.js 可用，然后重试；如仍失败，请查看 CodeBuddy CLI 日志";
    format!("CodeBuddy CLI helper 验证失败（{stage}）：{cause}。{hint}")
}

pub(super) fn auth_config_error(stage: &str, cause: &str) -> String {
    format!("CodeBuddy CLI 认证配置失败（{stage}）：{cause}")
}

pub(super) fn validate_helper_output(output: &Output, expected_token: &str) -> Result<(), String> {
    validate_helper_result(
        output.status.success(),
        output.status.code(),
        &output.stdout,
        expected_token,
    )
}

pub(super) fn validate_helper_result(
    success: bool,
    exit_code: Option<i32>,
    stdout: &[u8],
    expected_token: &str,
) -> Result<(), String> {
    if !success {
        let code = exit_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "未知".to_string());
        return Err(helper_validation_error(
            "执行阶段",
            &format!("helper 退出码为 {code}"),
        ));
    }
    let stdout = String::from_utf8_lossy(stdout);
    if stdout.trim().is_empty() {
        return Err(helper_validation_error("输出阶段", "helper 未返回认证结果"));
    }
    if stdout.trim() != format!("Bearer {expected_token}") {
        return Err(helper_validation_error(
            "输出阶段",
            "helper 返回的认证结果与所选账号不一致",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn node_path_from_shell_output(stdout: &[u8]) -> Option<PathBuf> {
    String::from_utf8_lossy(stdout)
        .lines()
        .rev()
        .find_map(|line| {
            let path = PathBuf::from(line.trim());
            (path.is_absolute() && path.file_name().is_some_and(|name| name == "node"))
                .then_some(path)
        })
}

#[cfg(target_os = "macos")]
pub(super) fn macos_node_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("node"),
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
    ];
    // Finder/LaunchServices 不会继承终端（尤其是 nvm）注入的 PATH。
    // 这里只让用户的登录 shell 定位 node，helper 本身仍由 Rust 直接执行，
    // 避免 .zshrc 的欢迎语等输出污染 helper 的 stdout。
    if let Ok(output) = Command::new("/bin/zsh")
        .args(["-lic", "command -v node"])
        .output()
    {
        if let Some(path) = node_path_from_shell_output(&output.stdout) {
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

#[cfg(windows)]
pub(super) fn windows_shell_candidates() -> Vec<PathBuf> {
    use std::os::windows::process::CommandExt;

    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("CODEBUDDY_CODE_GIT_BASH_PATH") {
        candidates.push(PathBuf::from(path));
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(&root).join("Git/bin/bash.exe"));
            candidates.push(PathBuf::from(root).join("Git/usr/bin/bash.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(root).join("Programs/Git/bin/bash.exe"));
    }
    let mut where_git = Command::new("where.exe");
    where_git.creation_flags(0x0800_0000);
    if let Ok(output) = where_git.arg("git.exe").output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let git = PathBuf::from(line.trim());
            if let Some(cmd_dir) = git.parent() {
                if let Some(git_root) = cmd_dir.parent() {
                    candidates.push(git_root.join("bin/bash.exe"));
                    candidates.push(git_root.join("usr/bin/bash.exe"));
                }
            }
        }
    }
    // 最后才使用 PATH 里的裸 bash.exe，避免优先命中 System32/WSL
    // launcher；前面的候选顺序与 CodeBuddy 2.138.0 的 Git Bash 发现逻辑一致。
    candidates.push(PathBuf::from("bash.exe"));
    candidates
}

pub(super) fn run_helper_command(command: &str) -> Result<Output, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        // 与 CodeBuddy 2.138.0 `normalizeWindowsCommandForPosixEval` 保持一致：
        // 在 Git Bash eval 前把 Windows 路径分隔符转为 `/`。
        let command = command.replace('\\', "/");
        for shell in windows_shell_candidates() {
            let mut process = Command::new(shell);
            process.creation_flags(0x0800_0000);
            match process.arg("-c").arg(&command).output() {
                Ok(output) => return Ok(output),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(helper_validation_error(
                        "启动阶段",
                        "无法启动 Git Bash shell",
                    ));
                }
            }
        }
        Err(helper_validation_error(
            "启动阶段",
            "未找到 Git Bash bash.exe",
        ))
    }
    #[cfg(target_os = "macos")]
    {
        for node in macos_node_candidates() {
            match Command::new(node).arg(command).output() {
                Ok(output) => return Ok(output),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(helper_validation_error(
                        "启动阶段",
                        "无法使用 Node.js 执行 helper",
                    ));
                }
            }
        }
        Err(helper_validation_error(
            "启动阶段",
            "未找到可用的 Node.js；如通过 nvm 安装，请确认登录 shell 能执行 node",
        ))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|_| helper_validation_error("启动阶段", "无法启动 /bin/sh"))
    }
}

pub(super) fn account_token(account: &Value) -> Result<&str, String> {
    account
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            auth_config_error(
                "账号阶段",
                "所选账号没有可用的认证信息，请先重新登录或刷新 Token",
            )
        })
}

pub(super) fn settings_account_token(account: &Value) -> Result<&str, String> {
    let token = clean_bearer_token(account_token(account)?);
    if token.is_empty() {
        return Err(auth_config_error(
            "账号阶段",
            "所选账号没有可用的认证信息，请先重新登录或刷新 Token",
        ));
    }
    Ok(token)
}

pub(super) fn validate_helper_for_account(command: &str, account: &Value) -> Result<(), String> {
    let token = account_token(account)?;
    let output = run_helper_command(command)?;
    validate_helper_output(&output, token)
}
