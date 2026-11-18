//! Linux 登录自启：XDG autostart desktop entry。

use super::*;

/// `Exec` escaping per the Desktop Entry Spec. The quoting layer escapes `\`,
/// `"`, backtick and `$`; the file's string layer then doubles every backslash
/// again, so a literal backslash is written as `\\\\`. `%` is a field code and
/// is always doubled. Arguments with reserved characters are double-quoted.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn xdg_exec_quote(arg: &str) -> String {
    let escaped = arg
        .replace('\\', r"\\\\")
        .replace('"', r#"\\""#)
        .replace('`', r"\\`")
        .replace('$', r"\\$")
        .replace('%', "%%");
    if arg.chars().any(|c| {
        matches!(
            c,
            ' ' | '\t'
                | '\n'
                | '"'
                | '\''
                | '\\'
                | '>'
                | '<'
                | '~'
                | '|'
                | '&'
                | ';'
                | '$'
                | '*'
                | '?'
                | '#'
                | '('
                | ')'
                | '`'
        )
    }) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// XDG autostart entry. `Type` and `Name` are required by the spec; the file
/// never sets `Hidden`, so the entry stays active.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn desktop_entry(name: &str, comment: &str, program: &Path) -> Result<String, String> {
    let program = program.to_str().ok_or("程序路径不是有效的 UTF-8")?;
    // The spec limits Exec's string value to ASCII, and a file some desktops
    // cannot parse is worse than a clear error at the switch.
    if !program.is_ascii() {
        return Err("程序路径包含非 ASCII 字符，无法写入自启配置".into());
    }
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\nExec={}\nTerminal=false\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n",
        xdg_exec_quote(program)
    ))
}

/// `Hidden=true` tells the desktop environment to ignore the file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn desktop_entry_hidden(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(key, value)| key.trim() == "Hidden" && value.trim() == "true")
    })
}

/// `Exec` value from the main `[Desktop Entry]` group, not from actions.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn desktop_entry_exec(body: &str) -> Option<&str> {
    let mut in_group = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_group = header == "Desktop Entry";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "Exec" {
                return Some(value.trim());
            }
        }
    }
    None
}

/// True only when an active entry points somewhere else. Other keys
/// (`X-GNOME-Autostart-enabled`, comments) are not the launch target.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn autostart_target_differs(body: &str, expected: &str) -> bool {
    !desktop_entry_hidden(body) && desktop_entry_exec(body) != desktop_entry_exec(expected)
}

/// `$XDG_CONFIG_HOME/autostart`, which Tauri's `config_dir` already resolves.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn autostart_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("autostart")
}

#[cfg(target_os = "linux")]
pub(crate) const AUTOSTART_COMMENT: &str = "登录后自动显示会话悬浮窗";

/// Linux login item: an XDG autostart desktop entry, replaced atomically.
#[cfg(target_os = "linux")]
pub(crate) mod xdg_login {
    use std::path::{Path, PathBuf};

    /// A fixed slug: renaming the product must not leave orphan entries behind.
    pub(crate) const FILE: &str = "agent-companion.desktop";

    pub fn path(config_dir: &Path) -> PathBuf {
        super::autostart_dir(config_dir).join(FILE)
    }

    pub fn read(file: &Path) -> Result<Option<String>, String> {
        match std::fs::read_to_string(file) {
            Ok(body) => Ok(Some(body)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("无法读取自启配置（{}）：{e}", file.display())),
        }
    }

    pub fn write(file: &Path, body: &str) -> Result<(), String> {
        std::fs::create_dir_all(file.parent().unwrap())
            .map_err(|e| format!("无法写入自启配置（{}）：{e}", file.display()))?;
        let temporary = file.with_extension("tmp");
        std::fs::write(&temporary, body)
            .map_err(|e| format!("无法写入自启配置（{}）：{e}", file.display()))?;
        std::fs::rename(&temporary, file)
            .map_err(|e| format!("无法写入自启配置（{}）：{e}", file.display()))
    }

    pub fn remove(file: &Path) -> Result<(), String> {
        std::fs::remove_file(file)
            .map_err(|e| format!("无法删除自启配置（{}）：{e}", file.display()))
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn autostart_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(xdg_login::path(
        &app.path().config_dir().map_err(|e| e.to_string())?,
    ))
}
