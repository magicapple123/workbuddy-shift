//! 托盘点击/轻量模式/签到与更新动作。

use super::*;

pub(crate) fn toggle_lightweight<R: Runtime>(app: &AppHandle<R>) {
    if LIGHTWEIGHT_MODE.load(Ordering::Acquire) {
        exit_lightweight(app);
    } else {
        enter_lightweight(app);
    }
}

pub(crate) fn enter_lightweight<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if window.destroy().is_err() {
            // CheckMenuItem may have already toggled visually; restore it.
            refresh_tray_menu(app);
            return;
        }
    }
    apply_dock_visible(app, false);
    LIGHTWEIGHT_MODE.store(true, Ordering::Release);
    refresh_tray_menu(app);
}

pub(crate) fn exit_lightweight<R: Runtime>(app: &AppHandle<R>) {
    // Regular / dock first so a from_config window is not created while Accessory.
    apply_dock_visible(app, true);
    if app.get_webview_window(MAIN_WINDOW_LABEL).is_none() {
        let Some(config) = app
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == MAIN_WINDOW_LABEL)
            .cloned()
        else {
            apply_dock_visible(app, false);
            refresh_tray_menu(app);
            return;
        };
        if WebviewWindowBuilder::from_config(app, &config)
            .and_then(|builder| builder.build())
            .is_err()
        {
            apply_dock_visible(app, false);
            refresh_tray_menu(app);
            return;
        }
    }
    // Window exists now: Windows skip_taskbar was a no-op before recreate.
    apply_dock_visible(app, true);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    emit_main_window_visible(app, true);
    LIGHTWEIGHT_MODE.store(false, Ordering::Release);
    refresh_tray_menu(app);
}

pub(crate) fn open_github<R: Runtime>(app: &AppHandle<R>) {
    let url = format!(
        "https://github.com/{}/{}",
        update::GITHUB_OWNER,
        update::GITHUB_REPO
    );
    let _ = app.opener().open_url(url, None::<&str>);
}

pub(crate) struct CheckinBusyGuard<R: Runtime> {
    pub(crate) app: AppHandle<R>,
}

pub(crate) fn start_checkin_all<R: Runtime>(app: &AppHandle<R>) {
    if crate::is_screenshot_demo() {
        set_tray_tooltip(app, "README 截图演示模式");
        return;
    }
    if CHECKIN_BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    bump_tooltip_generation();
    refresh_tray_menu(app);
    set_tray_tooltip(app, "正在签到…");

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _busy = CheckinBusyGuard { app: app.clone() };
        // 托盘「一键签到」保持立即语义，不遵守签到时间段（respect_window=false）。
        let payload = checkin::run_checkin_all(None, false).await;
        let text = format_checkin_tooltip(&payload);
        if checkin_succeeded(&payload) {
            notify_checkin(&app, &text);
        } else {
            let generation = bump_tooltip_generation();
            set_tray_tooltip(&app, &text);
            restore_tooltip_after(app, generation);
        }
    });
}

/// 托盘「检查更新」：用户主动触发，force=true 绕过 core 的 6 小时缓存。
///
/// 菜单回调和「一键签到」同在主线程，异步检查必须 spawn（见 `start_checkin_all`）。
pub(crate) fn start_update_check<R: Runtime>(app: &AppHandle<R>) {
    if crate::is_screenshot_demo() {
        set_tray_tooltip(app, "README 截图演示模式");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = crate::update_service::check(&app, None, true).await;
    });
}

/// 托盘「升级到 vX.Y.Z」/「更新失败，点击重试」：启动后台下载。
///
/// 互斥与演示模式短路都在更新服务内（下载是长任务，托盘不持有它的生命周期）。
pub(crate) fn start_update_download<R: Runtime>(app: &AppHandle<R>) {
    if crate::is_screenshot_demo() {
        set_tray_tooltip(app, "README 截图演示模式");
        return;
    }
    let _ = crate::update_service::start_download(app);
}

/// 托盘「重启以完成升级」：安装已下载的包并重启。
///
/// 安装会解压整包 / 替换应用（macOS 未授权时弹系统授权框），必须离开主线程执行。
pub(crate) fn start_update_restart<R: Runtime>(app: &AppHandle<R>) {
    if crate::is_screenshot_demo() {
        set_tray_tooltip(app, "README 截图演示模式");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = crate::update_service::restart(&app).await;
    });
}

pub(crate) fn notify_checkin<R: Runtime>(app: &AppHandle<R>, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title("WorkBuddy Shift")
        .body(body)
        .show();
}

/// 是否应弹签到完成通知。
///
/// `inactive`（该档位未开放签到活动，如国际版）不是失败：它既不算成功也不重试，
/// 因此不阻断通知；`error` 仍然算失败。
pub(crate) fn checkin_succeeded(value: &Value) -> bool {
    let Some(accounts) = value.get("accounts").and_then(Value::as_array) else {
        return false;
    };
    !accounts.is_empty()
        && accounts.iter().all(|account| {
            matches!(
                account.get("result").and_then(Value::as_str),
                Some("success" | "already" | "inactive")
            )
        })
}

pub(crate) fn restore_tooltip_after<R: Runtime>(app: AppHandle<R>, generation: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(CHECKIN_TOOLTIP_RESTORE_SECS)).await;
        if TOOLTIP_GENERATION.load(Ordering::Acquire) == generation {
            set_tray_tooltip(&app, DEFAULT_TOOLTIP);
        }
    });
}

pub(crate) fn bump_tooltip_generation() -> u64 {
    TOOLTIP_GENERATION.fetch_add(1, Ordering::AcqRel) + 1
}

pub(crate) fn set_tray_tooltip<R: Runtime>(app: &AppHandle<R>, text: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
    }
}

/// 更新服务写入 tooltip（下载进度）。
///
/// 先 bump generation：任何迟到的签到 tooltip 恢复计时器都会因代际不符而放弃，
/// 不会把下载进度覆盖回默认文案。
pub(crate) fn set_update_tooltip<R: Runtime>(app: &AppHandle<R>, text: &str) {
    bump_tooltip_generation();
    set_tray_tooltip(app, text);
}

/// 更新流程结束（完成 / 失败）后复位 tooltip。
pub(crate) fn reset_tray_tooltip<R: Runtime>(app: &AppHandle<R>) {
    bump_tooltip_generation();
    set_tray_tooltip(app, DEFAULT_TOOLTIP);
}

impl UpdateMenuAction {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Check => "check-update",
            Self::Download => "update-now",
            Self::Restart => "update-restart",
        }
    }
}

pub(crate) fn format_checkin_tooltip(value: &Value) -> String {
    if value.get("status").and_then(Value::as_str) == Some("skipped")
        && value.get("reason").and_then(Value::as_str) == Some("already_running")
    {
        return "签到任务正在进行，请稍后再试".to_string();
    }
    let Some(accounts) = value.get("accounts").and_then(Value::as_array) else {
        return "没有可签到的账号".to_string();
    };
    if accounts.is_empty() {
        return "没有可签到的账号".to_string();
    }

    let mut ok = 0;
    let mut already = 0;
    let mut err = 0;
    let mut inactive = 0;
    for account in accounts {
        match account.get("result").and_then(Value::as_str) {
            Some("success") => ok += 1,
            Some("already") => already += 1,
            Some("error") => err += 1,
            Some("inactive") => inactive += 1,
            _ => {}
        }
    }
    let mut text = format!("签到完成：成功 {ok}，已签 {already}，失败 {err}");
    // 国际版无签到活动：单列「未开放」，避免被误读成失败或漏报。
    if inactive > 0 {
        text.push_str(&format!("，未开放 {inactive}"));
    }
    text
}
