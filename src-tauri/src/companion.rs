//! 内置会话悬浮窗的宿主边界。workbuddy-shift 负责托盘与启用开关；
//! companion-desktop 插件负责自己的窗口与监控连接。

use tauri::AppHandle;

fn runtime_path() -> std::path::PathBuf {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let bundled = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|dir| dir.join(format!("companion-runtime{suffix}")))
    });
    if let Some(path) = bundled.filter(|path| path.is_file()) {
        return path;
    }
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!(
            "companion-runtime-{}{suffix}",
            env!("AGENT_COMPANION_TARGET")
        ))
}

pub fn config() -> companion_desktop::Config {
    companion_desktop::Config {
        assets: "companion/".into(),
        runtime: runtime_path(),
        manage_autostart: false,
        default_enabled: false,
        show_rail: true,
    }
}

pub fn enabled(app: &AppHandle) -> bool {
    #[cfg(not(desktop))]
    {
        let _ = app;
        return false;
    }
    #[cfg(desktop)]
    {
        !crate::is_screenshot_demo() && companion_desktop::is_enabled(app)
    }
}

#[tauri::command]
pub fn get_companion_enabled(app: AppHandle) -> bool {
    enabled(&app)
}

#[tauri::command]
pub async fn set_companion_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    #[cfg(not(desktop))]
    {
        let _ = (app, enabled);
        return Err("此平台不支持悬浮窗".into());
    }
    #[cfg(desktop)]
    {
        if crate::is_screenshot_demo() {
            return Err("演示模式不启用悬浮窗".into());
        }
        let host = app.clone();
        let confirmed = tauri::async_runtime::spawn_blocking(move || {
            companion_desktop::set_enabled(&host, enabled)
        })
        .await
        .map_err(|error| error.to_string())??;
        crate::tray::refresh_tray_menu(&app);
        Ok(confirmed)
    }
}

#[tauri::command]
pub async fn open_companion_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(not(desktop))]
    {
        let _ = app;
        return Err("此平台不支持悬浮窗".into());
    }
    #[cfg(desktop)]
    {
        if !enabled(&app) {
            return Err("请先启用悬浮窗".into());
        }
        tauri::async_runtime::spawn_blocking(move || companion_desktop::open(&app, "settings"))
            .await
            .map_err(|error| error.to_string())?
    }
}

#[cfg(desktop)]
pub fn toggle_rail(app: &AppHandle) {
    if enabled(app) {
        companion_desktop::toggle_rail(app);
    }
}

#[cfg(desktop)]
pub fn open_settings_from_tray(app: &AppHandle) {
    if !enabled(app) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = companion_desktop::open(&app, "settings");
    });
}
