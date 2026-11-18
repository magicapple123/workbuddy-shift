//! Desktop tray: menu-bar icon, dock visibility, lightweight mode, and check-in.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, Runtime, WebviewWindowBuilder, Window, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use wb_shift_core::modules::{checkin, update};

use crate::update_service::{UpdatePhase, UpdateSnapshot};

mod actions;
mod icons;
mod menu;
mod platform;

pub(crate) use actions::*;
pub(crate) use icons::*;
pub(crate) use menu::*;
pub(crate) use platform::*;

#[cfg(test)]
mod tests;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(unused_variables)
)]
const TRAY_ID: &str = "main-menu-bar";

const MAIN_WINDOW_LABEL: &str = "main";

const DEFAULT_TOOLTIP: &str = "WorkBuddy Shift";

const CHECKIN_TOOLTIP_RESTORE_SECS: u64 = 8;

/// 系统自启注册的启动参数：仅携带该精确参数的启动进入静默托盘模式。
pub const SILENT_STARTUP_ARG: &str = "--hidden";

static LIGHTWEIGHT_MODE: AtomicBool = AtomicBool::new(false);

static CHECKIN_BUSY: AtomicBool = AtomicBool::new(false);

static TOOLTIP_GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
static DOCK_ICON_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon())
        // 模板图标（系统按明暗自适应着色）是 macOS 独有的语义；
        // Windows/Linux 会忽略该标记并直接贴原始像素，用单色剪影会显示成纯白方块。
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip(DEFAULT_TOOLTIP)
        .menu(&menu)
        // 左键弹菜单只保留在 macOS：菜单栏图标的惯例本就是左键展开菜单。
        // Windows 的惯例相反——左键唤起主界面、右键出菜单，见 on_tray_icon_event。
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                if should_wake_main_window(button, button_state) {
                    show_main_window(tray.app_handle());
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open-main-window" => show_main_window(app),
            "open-github" => open_github(app),
            "checkin-all" => start_checkin_all(app),
            "check-update" => start_update_check(app),
            "update-now" => start_update_download(app),
            "update-restart" => start_update_restart(app),
            "lightweight-mode" => toggle_lightweight(app),
            "companion-toggle" => crate::companion::toggle_rail(app),
            "companion-settings" => crate::companion::open_settings_from_tray(app),
            "quit-app" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    #[cfg(windows)]
    watch_taskbar_theme(app.handle().clone());

    Ok(())
}

pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        apply_dock_visible(window.app_handle(), false);
        emit_main_window_visible(window.app_handle(), false);
    }
}

/// 判断本次启动是否携带精确的 `--hidden` 参数（系统自启触发）。
///
/// 必须整参相等，禁止子串匹配，避免 `--hidden-x`、`x--hidden` 等误入静默模式。
pub fn is_silent_startup(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    args.into_iter()
        .any(|arg| arg.as_ref() == SILENT_STARTUP_ARG)
}

/// 第二次启动是否需要把既有实例唤醒到前台。
///
/// 静默启动（精确 `--hidden`，自启重复触发）不打扰用户：不显示窗口、不改 Dock 状态。
pub fn should_activate_on_second_launch(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    !is_silent_startup(args)
}

/// 单实例插件回调：已有实例时后启动进程已退出，这里处理既有实例的反应。
///
/// 参数由插件回传，**含 argv[0]**；精确 `--hidden` 判定与自启路径一致。
/// 回调运行在 tokio worker 线程，而 `show_main_window` 会经 `apply_dock_visible`
/// 直接操作 AppKit（`MainThreadMarker::new_unchecked`），必须跳回主线程执行。
pub fn on_second_instance<R: Runtime>(app: &AppHandle<R>, args: Vec<String>) {
    if !should_activate_on_second_launch(&args) {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show_main_window(&handle));
}

/// 在事件循环呈现应用前决定首次启动的主窗口可见性。
///
/// `main` 窗口由 `tauri.conf.json` 配置创建为不可见，此处做出第一次
/// show / hide 决策，静默启动不会先闪出主窗口：
///
/// - 普通启动（无 `--hidden`）：走既有 `show_main_window` 路径，
///   先恢复 Regular / Dock，再 show / unminimize / focus。
/// - 静默启动（`--hidden`）：窗口保持隐藏，隐藏 Dock / 任务栏入口，
///   只保留托盘；不设置 `LIGHTWEIGHT_MODE`（WebView 仍然存在）。
///
/// 之后从托盘「打开主界面」仍走 `show_main_window`，与隐藏窗口完全一致。
pub fn setup_startup_visibility<R: Runtime>(app: &AppHandle<R>, silent: bool) {
    if silent {
        apply_dock_visible(app, false);
        emit_main_window_visible(app, false);
    } else {
        show_main_window(app);
    }
}

/// Keep the tray process alive when the last window is destroyed (lightweight mode).
///
/// `code == None` is Tauri's runtime exit after zero windows remain.
/// `code == Some(_)` is an explicit `app.exit()` / restart — let those through.
pub fn on_run_event(event: RunEvent) {
    if let RunEvent::ExitRequested { api, code, .. } = event {
        if should_keep_tray_alive(code) {
            api.prevent_exit();
        }
    }
}

/// 投递 core 组装好的自动轮换推迟提示（`rotate::run_rotate_cycle` 返回体里的 `notify`）。
///
/// 走系统通知而不是托盘 tooltip：轮换是后台行为，用户此时多半没看着窗口。
/// 标题与正文都取自 core（文案唯一构造点在 `rotate`），宿主不自造措辞；
/// 无头 server 不投递，只保留日志与返回字段。
pub fn notify_rotate_deferred<R: Runtime>(app: &AppHandle<R>, notify: &Value) {
    let title = notify
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("WorkBuddy Shift");
    let Some(body) = notify.get("body").and_then(Value::as_str) else {
        return;
    };
    if body.is_empty() {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}
