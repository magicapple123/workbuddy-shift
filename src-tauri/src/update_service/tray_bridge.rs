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
