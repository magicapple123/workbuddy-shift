//! 更新状态到托盘菜单/提示的桥接。

use super::*;

/// 统一状态出口：写好的快照经 emit + 托盘刷新对外可见。
pub(crate) fn set_phase<R: Runtime>(
    app: &AppHandle<R>,
    snapshot: UpdateSnapshot,
) -> UpdateSnapshot {
    let _ = app.emit("update-state", &snapshot);
    refresh_tray(app);
    snapshot
}

pub(crate) fn refresh_tray<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(desktop)]
    crate::tray::refresh_tray_menu(app);
    #[cfg(not(desktop))]
    let _ = app;
}

pub(crate) fn set_tooltip<R: Runtime>(app: &AppHandle<R>, text: &str) {
    #[cfg(desktop)]
    crate::tray::set_update_tooltip(app, text);
    #[cfg(not(desktop))]
    let _ = (app, text);
}

/// 更新流程结束（完成 / 失败）后把 tooltip 复位，避免长期停留在进度文案上。
pub(crate) fn reset_tooltip<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(desktop)]
    crate::tray::reset_tray_tooltip(app);
    #[cfg(not(desktop))]
    let _ = app;
}

pub(crate) fn notify<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

/// 主窗口是否在前台：存在 + 可见 + 已聚焦。
///
/// 三种「不在前台」都算需要通知：轻量模式销毁窗口（`get_webview_window("main")`
/// 为 None，见 `tray/actions.rs`）、隐藏到托盘（`hide()`）、最小化（失去焦点）。
/// 判不出来时按「不在前台」处理会多打扰用户，按「在前台」处理会漏通知，
/// 这里选择保守（`unwrap_or(false)` 视为不可见 → 发通知），因为漏通知是原始 bug。
pub(crate) fn main_window_in_foreground<R: Runtime>(app: &AppHandle<R>) -> bool {
    #[cfg(desktop)]
    {
        use tauri::Manager;
        match app.get_webview_window("main") {
            Some(window) => {
                window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
            }
            None => false,
        }
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        false
    }
}

/// 发现新版本时投递系统通知；返回是否真的投递了（供调用方打日志与单测断言）。
///
/// 只在主窗口不在前台、且该版本还没通知过时发一次。文案引导到托盘菜单：
/// `tauri-plugin-notification` 2.x 没有跨平台可靠的点击回调，本轮不做「点通知开窗口」。
pub(crate) fn notify_available<R: Runtime>(app: &AppHandle<R>, snapshot: &UpdateSnapshot) -> bool {
    let in_foreground = main_window_in_foreground(app);
    let notified = notified_version();
    let Some(version) = notify_decision(
        snapshot.phase,
        snapshot.latest.as_deref(),
        in_foreground,
        crate::is_screenshot_demo(),
        notified.as_deref(),
    ) else {
        return false;
    };
    notify(
        app,
        &format!("发现新版本 v{version}"),
        "可在托盘菜单或应用内升级",
    );
    remember_notified_version(&version);
    eprintln!("[更新] 已发送新版本通知 v{version}（主窗口不在前台）");
    true
}
