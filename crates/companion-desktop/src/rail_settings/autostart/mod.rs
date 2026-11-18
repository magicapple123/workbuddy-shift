//! 平台登录自启：Windows Run 值、macOS LaunchAgent plist 与 Linux XDG autostart。

use std::path::{Path, PathBuf};
use tauri::Manager;

mod launch;
mod linux;
mod macos;
mod win;

pub(crate) use launch::*;
#[cfg(any(target_os = "linux", test))]
pub(crate) use linux::*;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;
#[cfg(any(target_os = "windows", test))]
pub(crate) use win::*;

/// The configured product name. The Windows Run value is named after it, so the
/// NSIS uninstall (which deletes `${PRODUCTNAME}`) and `/UPDATE` (which keeps
/// the value) stay in sync; the desktop entry shows it as `Name`.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
fn product_name(app: &tauri::AppHandle) -> String {
    app.config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.config().identifier.clone())
}

pub(super) fn managed(app: &tauri::AppHandle) -> bool {
    app.state::<std::sync::Arc<crate::Service>>()
        .config
        .manage_autostart
}

pub(super) fn supported(app: &tauri::AppHandle) -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    )) && managed(app)
        && launch_command(app).is_ok()
}

/// Whether a login item is registered. As with the plist before it, only the
/// presence of the item is reported; a stale target is repaired by `heal_login`.
pub(super) fn login_state(app: &tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        login_path(app).map(|path| path.exists())
    }
    #[cfg(target_os = "windows")]
    {
        win_login::read(&product_name(app)).map(|value| value.is_some())
    }
    #[cfg(target_os = "linux")]
    {
        let file = autostart_file(app)?;
        xdg_login::read(&file).map(|body| body.is_some_and(|body| !desktop_entry_hidden(&body)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = app;
        Ok(false)
    }
}

/// Adds or removes the login item, surfacing write failures such as a blocked
/// registry key or an unwritable autostart directory.
pub(super) fn set_login(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let login = login_path(app)?;
        if enabled {
            let launch = launch_command(app)?;
            let mut args = vec![launch.program.to_string_lossy().into_owned()];
            args.extend(launch.args);
            let plist = launch_plist(
                &format!("{}.rail-login", app.config().identifier),
                &args,
                launch.env_path.as_deref(),
            );
            std::fs::create_dir_all(login.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&login, plist).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(&login).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let name = product_name(app);
        if !enabled {
            return win_login::remove(&name);
        }
        win_login::write(&name, &run_value_data(&launch_command(app)?.program)?)
    }
    #[cfg(target_os = "linux")]
    {
        let file = autostart_file(app)?;
        if !enabled {
            return xdg_login::remove(&file);
        }
        let launch = launch_command(app)?;
        xdg_login::write(
            &file,
            &desktop_entry(&product_name(app), AUTOSTART_COMMENT, &launch.program)?,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (app, enabled);
        Err("当前运行环境不支持开机自启".into())
    }
}

/// Rewrites a login item whose target no longer matches this binary — the one
/// case a moved AppImage or a reinstall leaves behind. A Windows value that
/// still carries the verbatim `\\?\` prefix is rewritten even though it names
/// the same file. Failures are ignored so reading settings never fails because
/// of the repair. Embedded hosts do not get their login item edited.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(super) fn heal_login(app: &tauri::AppHandle) {
    if !managed(app) {
        return;
    }
    let Ok(launch) = launch_command(app) else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        let name = product_name(app);
        let (Ok(expected), Ok(Some(current))) =
            (run_value_data(&launch.program), win_login::read(&name))
        else {
            return;
        };
        if run_value_needs_rewrite(&current, &expected) {
            let _ = win_login::write(&name, &expected);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let (Ok(file), Ok(expected)) = (
            autostart_file(app),
            desktop_entry(&product_name(app), AUTOSTART_COMMENT, &launch.program),
        ) else {
            return;
        };
        let Ok(Some(body)) = xdg_login::read(&file) else {
            return;
        };
        if autostart_target_differs(&body, &expected) {
            let _ = xdg_login::write(&file, &expected);
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(super) fn heal_login(_app: &tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn login_plist_keeps_arguments_separate_and_escapes_paths() {
        let args = vec![
            "/opt/node tools/npm".into(),
            "--prefix".into(),
            "/Users/A & B/project".into(),
            "run".into(),
            "desktop:dev".into(),
        ];
        let plist = launch_plist(
            "com.test.rail-login",
            &args,
            Some("/opt/node tools:/usr/bin"),
        );
        assert!(plist.contains("<string>/Users/A &amp; B/project</string>"));
        assert!(plist.contains("<string>run</string><string>desktop:dev</string>"));
        assert!(plist.contains("<key>PATH</key><string>/opt/node tools:/usr/bin</string>"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
    }
    #[test]
    fn run_value_data_quotes_the_program_and_caps_the_command_line() {
        assert_eq!(
            run_value_data(Path::new(
                r"C:\Users\a b\AppData\Local\WorkBuddy Shift\agent-companion.exe"
            ))
            .unwrap(),
            r#""C:\Users\a b\AppData\Local\WorkBuddy Shift\agent-companion.exe""#
        );
        // `canonicalize` hands over the verbatim form on Windows; the Run value
        // must keep the ordinary spelling.
        assert_eq!(
            run_value_data(Path::new(
                r"\\?\C:\Users\a b\AppData\Local\WorkBuddy Shift\agent-companion.exe"
            ))
            .unwrap(),
            r#""C:\Users\a b\AppData\Local\WorkBuddy Shift\agent-companion.exe""#
        );
        // 258 characters plus the two quotes fill the 260 allowed. The verbatim
        // prefix is not stored, so it must not consume that budget.
        assert!(run_value_data(&PathBuf::from("x".repeat(258))).is_ok());
        assert!(run_value_data(&PathBuf::from("x".repeat(259))).is_err());
        assert!(run_value_data(&PathBuf::from(format!(r"\\?\{}", "x".repeat(258)))).is_ok());
    }
    #[test]
    fn plain_path_strips_the_verbatim_prefix_only() {
        assert_eq!(
            plain_path(r"\\?\C:\Users\a b\agent-companion.exe"),
            r"C:\Users\a b\agent-companion.exe"
        );
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\agent-companion.exe"),
            r"\\server\share\agent-companion.exe"
        );
        assert_eq!(
            plain_path(r"C:\Users\a b\agent-companion.exe"),
            r"C:\Users\a b\agent-companion.exe"
        );
        assert_eq!(
            plain_path("/usr/bin/agent-companion"),
            "/usr/bin/agent-companion"
        );
    }
    #[test]
    fn reg_sz_bytes_are_utf16_with_a_terminator() {
        assert_eq!(reg_sz_bytes("Aé"), vec![0x41, 0xe9, 0x00]);
    }
    #[test]
    fn canonical_run_value_rewrites_known_paths_only() {
        assert_eq!(
            canonical_run_value("\"Z:\\missing\\agent-companion.exe\""),
            "\"Z:\\missing\\agent-companion.exe\""
        );
        let exe = std::env::current_exe().unwrap();
        let rewritten = canonical_run_value(&exe.to_string_lossy());
        assert_eq!(
            rewritten,
            format!(
                "\"{}\"",
                plain_path(&std::fs::canonicalize(&exe).unwrap().to_string_lossy())
            )
        );
        assert!(!rewritten.contains(r"\\?\"));
    }
    #[test]
    fn verbatim_run_spelling_is_rewritten_even_when_the_target_matches() {
        assert!(stored_run_spelling_rejected(
            r#""\\?\C:\WorkBuddy Shift\agent-companion.exe""#
        ));
        assert!(stored_run_spelling_rejected(
            r#""\\?\UNC\server\share\agent-companion.exe""#
        ));
        assert!(!stored_run_spelling_rejected(
            r#""C:\WorkBuddy Shift\agent-companion.exe""#
        ));
        assert!(!stored_run_spelling_rejected(
            "\"/usr/bin/agent-companion\""
        ));
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let expected = run_value_data(&exe).unwrap();
        assert!(!expected.contains(r"\\?\"));
        assert!(!run_value_needs_rewrite(&expected, &expected));
        let verbatim = format!("\"{}{}\"", r"\\?\", expected.trim_matches('"'));
        assert!(run_value_needs_rewrite(&verbatim, &expected));
    }
    #[test]
    fn xdg_exec_quote_follows_the_desktop_entry_escaping_table() {
        assert_eq!(
            xdg_exec_quote("/usr/bin/agent-companion"),
            "/usr/bin/agent-companion"
        );
        assert_eq!(
            xdg_exec_quote("/opt/WorkBuddy Shift/app"),
            "\"/opt/WorkBuddy Shift/app\""
        );
        assert_eq!(xdg_exec_quote(r"a\b"), r#""a\\\\b""#);
        assert_eq!(xdg_exec_quote(r#"a"b"#), r#""a\\"b""#);
        assert_eq!(xdg_exec_quote("a`b"), r#""a\\`b""#);
        assert_eq!(xdg_exec_quote("a$b"), r#""a\\$b""#);
        assert_eq!(xdg_exec_quote("100%"), "100%%");
    }
    #[test]
    fn desktop_entry_has_the_required_keys_and_rejects_non_ascii() {
        let body = desktop_entry(
            "WorkBuddy Shift",
            "登录后自动显示会话悬浮窗",
            Path::new("/opt/WorkBuddy Shift/agent-companion"),
        )
        .unwrap();
        assert_eq!(body.lines().next(), Some("[Desktop Entry]"));
        for key in [
            "Type=Application",
            "Name=WorkBuddy Shift",
            "Exec=\"/opt/WorkBuddy Shift/agent-companion\"",
            "Terminal=false",
            "StartupNotify=false",
            "X-GNOME-Autostart-enabled=true",
        ] {
            assert!(body.contains(key), "missing {key}");
        }
        assert!(desktop_entry(
            "WorkBuddy Shift",
            "c",
            Path::new("/opt/工具/agent-companion")
        )
        .is_err());
    }
    #[test]
    fn desktop_entry_hidden_reads_the_key_and_ignores_comments() {
        assert!(desktop_entry_hidden("[Desktop Entry]\nHidden=true\n"));
        assert!(!desktop_entry_hidden("[Desktop Entry]\nHidden=false\n"));
        assert!(!desktop_entry_hidden(
            "[Desktop Entry]\nName=WorkBuddy Shift\n"
        ));
        assert!(!desktop_entry_hidden("# Hidden=true\n"));
    }
    #[test]
    fn autostart_heal_compares_the_exec_target_only() {
        let expected = desktop_entry(
            "WorkBuddy Shift",
            "登录后自动显示会话悬浮窗",
            Path::new("/usr/bin/agent-companion"),
        )
        .unwrap();
        assert_eq!(
            desktop_entry_exec(&expected),
            Some("/usr/bin/agent-companion")
        );
        assert!(!autostart_target_differs(&expected, &expected));
        let gnome_off = expected.replace(
            "X-GNOME-Autostart-enabled=true",
            "X-GNOME-Autostart-enabled=false",
        );
        assert!(!autostart_target_differs(&gnome_off, &expected));
        let hidden = expected.replace("StartupNotify=false", "Hidden=true");
        assert!(!autostart_target_differs(&hidden, &expected));
        let moved = expected.replace("/usr/bin/agent-companion", "/opt/moved/agent-companion");
        assert!(autostart_target_differs(&moved, &expected));
        let spaced = desktop_entry(
            "WorkBuddy Shift",
            "c",
            Path::new("/opt/WorkBuddy Shift/app"),
        )
        .unwrap();
        assert_eq!(
            desktop_entry_exec(&spaced),
            Some("\"/opt/WorkBuddy Shift/app\"")
        );
        assert_eq!(
            desktop_entry_exec(
                "[Desktop Action x]\nExec=/other\n[Desktop Entry]\nExec=/usr/bin/agent-companion\n"
            ),
            Some("/usr/bin/agent-companion")
        );
    }
    #[test]
    fn autostart_dir_joins_the_config_directory() {
        assert_eq!(
            autostart_dir(Path::new("/home/user/.config")),
            PathBuf::from("/home/user/.config/autostart")
        );
    }
}
