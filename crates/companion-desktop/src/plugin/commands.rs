//! Tauri command 与窗口/视图操作。

use super::*;

#[tauri::command]
pub(crate) fn monitor_state(state: State<'_, Arc<Service>>) -> Value {
    // 快照是采集器下一次轮询会整体覆盖的缓存：锁中毒时取回旧快照继续渲染，
    // 比让悬浮栏 IPC 直接 panic 更可控。
    json!({"snapshot":*crate::locks::recover(&state.snapshot),"connected":state.connected.load(Ordering::Relaxed)})
}

#[tauri::command]
pub(crate) async fn collector_request(
    state: State<'_, Arc<Service>>,
    command: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    if !matches!(
        command.as_str(),
        "settings_get"
            | "settings_set"
            | "settings_check"
            | "integrations_get"
            | "integrations_set"
            | "custom_integrations_get"
            | "custom_integrations_set"
            | "custom_preview"
            | "session_monitor_close"
    ) {
        return Err("不支持的命令".into());
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let client = crate::locks::guard(&state.client, "采集器连接")?;
        client
            .as_ref()
            .ok_or("采集器未连接")?
            .request(&command, payload.unwrap_or(Value::Null))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn set_hit_regions(
    _app: tauri::AppHandle,
    regions: Vec<hit_test::Region>,
    state: State<'_, hit_test::Regions>,
) -> Result<(), String> {
    if regions.len() > 512 || regions.iter().any(|r| !r.valid()) {
        return Err("无效的窗口交互区域".into());
    }
    *crate::locks::guard(state.inner(), "窗口交互区域")? = regions;
    #[cfg(target_os = "macos")]
    if let Some(w) = _app.get_webview_window(RAIL) {
        let pointer = w.ns_window().map_err(|e| e.to_string())? as usize;
        let regions = state.inner().clone();
        _app.run_on_main_thread(move || hit_test::refresh(pointer, &regions))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    hit_test::refresh();
    Ok(())
}

#[tauri::command]
pub(crate) async fn rail_reset_position(app: tauri::AppHandle) -> Result<(), String> {
    // One main-thread read plus a ~200 ms animated move would stall the UI if
    // run inline; hand both to the blocking pool instead.
    tauri::async_runtime::spawn_blocking(move || {
        let rail = app.get_webview_window(RAIL).ok_or("悬浮窗未开启")?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let read_rail = rail.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(rail_position::reset_plan(&read_rail));
        })
        .map_err(|e| e.to_string())?;
        let target = rx.recv().map_err(|e| e.to_string())??;
        rail_position::glide(&rail, target);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn close_settings(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn open_view(app: tauri::AppHandle, view: String) -> Result<(), String> {
    // Sync plugin handlers run while the plugin-store lock is held. Window
    // construction re-enters that store; defer it until dispatch has returned.
    tauri::async_runtime::spawn_blocking(move || open(&app, &view))
        .await
        .map_err(|e| e.to_string())?
}

pub fn open(app: &tauri::AppHandle, view: &str) -> Result<(), String> {
    let state = app.state::<Arc<Service>>();
    if !state.enabled.load(Ordering::Acquire) {
        return Err("内置会话悬浮窗已关闭".into());
    }
    let (label, file, title, width, height) = match view {
        "settings" => (
            SETTINGS,
            "desktop-settings.html",
            "WorkBuddy Shift · 悬浮窗设置",
            480.,
            700.,
        ),
        "rail" => {
            if let Some(w) = app.get_webview_window(RAIL) {
                w.show().map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        _ => return Err("未知视图".into()),
    };
    if let Some(w) = app.get_webview_window(label) {
        return focus_settings(&w);
    }
    let window = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App(format!("{}{file}", state.config.assets).into()),
    )
    .title(title)
    .inner_size(width, height)
    .min_inner_size(420., 520.)
    .center();
    #[cfg(target_os = "macos")]
    let window = window
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16., 18.))
        .allow_link_preview(false);
    let window = window.build().map_err(|e| e.to_string())?;
    focus_settings(&window)
}

pub(crate) fn focus_settings(window: &tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    // On macOS set_focus also activates the application. Window construction
    // alone can leave this accessory app behind the previously active app.
    window.set_focus().map_err(|e| e.to_string())
}

pub fn toggle_rail(app: &tauri::AppHandle) {
    if !is_enabled(app) {
        return;
    }
    if let Some(w) = app.get_webview_window(RAIL) {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
        }
    }
}
