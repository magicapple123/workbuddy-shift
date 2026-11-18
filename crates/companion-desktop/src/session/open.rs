//! 平台唤起：按解析出的目标调用系统打开服务。

use super::url::SessionTarget;
use std::process::Command;

#[cfg(target_os = "macos")]
use super::url::{app_for_scheme, VSCODE_BUNDLE};
#[cfg(target_os = "macos")]
use super::vscode_bucket::{resolve_vscode_folder, vscode_history_roots};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
const VSCODE_APP: &str = "Visual Studio Code.app";

/// The app bundle location when the app is installed, so `-a` can beat a
/// possibly missing URL scheme registration.
#[cfg(target_os = "macos")]
fn installed_app_path(app: &str) -> Option<PathBuf> {
    [
        Some(PathBuf::from(format!("/Applications/{app}"))),
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications").join(app)),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.join("Contents/Info.plist").is_file())
}

/// Runs an open command and maps its failure to the message the UI shows.
fn run_command(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|e| format!("无法调用系统打开服务：{e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    eprintln!("Agent session open failed: {}", detail.trim());
    Err(
        if detail.contains("-10814") || detail.contains("Unable to find application") {
            "系统未找到应用或链接处理程序，请确认应用位置后重试".into()
        } else {
            "未能唤起目标 Agent，请打开应用后重试".into()
        },
    )
}

/// Opens a folder in VS Code, best target first. `open -a <app> <folder>` makes
/// VS Code focus the window that already holds that folder (or open a new one),
/// which is the whole point. The bundled CLI behaves the same but briefly flashes
/// a second Dock icon, so it is only the fallback. `vscode://file/...` is never
/// used: the URL handler always targets the last active window.
#[cfg(target_os = "macos")]
fn open_vscode_folder(folder: &Path) -> Result<(), String> {
    if let Some(app) = installed_app_path(VSCODE_APP) {
        if run_command(
            Command::new("/usr/bin/open")
                .arg("-a")
                .arg(&app)
                .arg(folder),
        )
        .is_ok()
        {
            return Ok(());
        }
        let cli = app.join("Contents/Resources/app/bin/code");
        if cli.is_file() && run_command(Command::new(cli).arg(folder)).is_ok() {
            return Ok(());
        }
    }
    run_command(
        Command::new("/usr/bin/open")
            .args(["-b", VSCODE_BUNDLE])
            .arg(folder),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn open_session_target(target: SessionTarget) -> Result<(), String> {
    match target {
        SessionTarget::Link(target) => {
            let mut command = Command::new("/usr/bin/open");
            if let Some((app, bundle)) = app_for_scheme(target.scheme()) {
                // URL scheme registration can be missing even while the app
                // is installed/running. Deliver the deep link to the app itself.
                if let Some(path) = installed_app_path(app) {
                    command.arg("-a").arg(path);
                } else {
                    command.args(["-b", bundle]);
                }
            }
            command.arg(target.as_str());
            run_command(&mut command)
        }
        SessionTarget::App(bundle) => {
            run_command(Command::new("/usr/bin/open").args(["-b", bundle]))
        }
        SessionTarget::VSCode { session, cwd } => {
            match resolve_vscode_folder(&vscode_history_roots(), &session, cwd.as_deref()) {
                Some(folder) => open_vscode_folder(&folder),
                None => run_command(Command::new("/usr/bin/open").args(["-b", VSCODE_BUNDLE])),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn open_session_target(target: SessionTarget) -> Result<(), String> {
    let SessionTarget::Link(target) = target else {
        return Err("当前平台尚不支持仅唤起应用".into());
    };
    run_command(Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", target.as_str()]))
}

#[cfg(target_os = "linux")]
pub(super) fn open_session_target(target: SessionTarget) -> Result<(), String> {
    let SessionTarget::Link(target) = target else {
        return Err("当前平台尚不支持仅唤起应用".into());
    };
    run_command(Command::new("xdg-open").arg(target.as_str()))
}
