//! macOS 登录自启：LaunchAgent plist。

#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use tauri::Manager;

#[cfg(target_os = "macos")]
pub(crate) fn login_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .home_dir()
        .map_err(|e| e.to_string())?
        .join("Library/LaunchAgents")
        .join(format!("{}.rail-login.plist", app.config().identifier)))
}

#[cfg(target_os = "macos")]
pub(crate) fn bundle() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .ancestors()
        .find(|p| p.extension().is_some_and(|s| s == "app"))
        .map(|p| p.to_owned())
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_plist(label: &str, args: &[String], path: Option<&str>) -> String {
    let arguments = args
        .iter()
        .map(|a| format!("<string>{}</string>", xml(a)))
        .collect::<String>();
    let environment = path
        .map(|p| {
            format!(
                "<key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict>",
                xml(p)
            )
        })
        .unwrap_or_default();
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{arguments}</array>{environment}<key>RunAtLoad</key><true/></dict></plist>", xml(label))
}

#[cfg(target_os = "macos")]
pub(crate) fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
