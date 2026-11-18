//! 平台差异：dock 图标、窗口唤醒与任务栏主题监听。

use super::*;

/// macOS `RunEvent::Reopen`（点击 Dock / Finder 激活已运行应用）时显示主窗口。
///
/// 该事件在主线程派发，可直接走 `show_main_window`；窗口策略仍集中在 tray 模块。
#[cfg(target_os = "macos")]
pub(crate) fn show_main_window_on_reopen<R: Runtime>(app: &AppHandle<R>) {
    show_main_window(app);
}

pub(crate) fn emit_main_window_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    let _ = app.emit("main-window-visible", visible);
}

pub(crate) fn should_keep_tray_alive(code: Option<i32>) -> bool {
    code.is_none()
}

/// 托盘左键单击（抬起）是否应唤起主窗口。
///
/// - macOS：`show_menu_on_left_click` 保持 true，左键展开菜单；但 mouseUp 仍会派发
///   `Click`，所以这里必须显式返回 false，否则左键会「既弹菜单又唤窗」。
/// - Linux：Tauri 不派发 `TrayIconEvent`（仅 Windows / macOS 支持），该分支不会触发，
///   托盘点击行为仍由 libappindicator 决定（左右键都会出菜单）。
pub(crate) fn should_wake_main_window(button: MouseButton, button_state: MouseButtonState) -> bool {
    !cfg!(target_os = "macos")
        && matches!(
            (button, button_state),
            (MouseButton::Left, MouseButtonState::Up)
        )
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    // Recreate if the WebView is gone even when the flag is already false
    // (e.g. destroy() completed after a failed lightweight toggle).
    if LIGHTWEIGHT_MODE.load(Ordering::Acquire)
        || app.get_webview_window(MAIN_WINDOW_LABEL).is_none()
    {
        exit_lightweight(app);
        return;
    }
    apply_dock_visible(app, true);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    emit_main_window_visible(app, true);
}

pub(crate) fn apply_dock_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        let policy = if visible {
            ActivationPolicy::Regular
        } else {
            ActivationPolicy::Accessory
        };
        let _ = app.set_dock_visibility(visible);
        let _ = app.set_activation_policy(policy);
        if visible {
            restore_macos_dock_icon(app);
        } else {
            DOCK_ICON_GENERATION.fetch_add(1, Ordering::AcqRel);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            let _ = window.set_skip_taskbar(!visible);
        }
    }
}

/// Re-apply the Dock icon after returning to Regular.
///
/// `TransformProcessType` rebuilds the Dock tile from the running executable.
/// Packaged `.app` binaries have no icon of their own, and `tauri dev` is named
/// `exec`, so both need an explicit restore. Tauri only sets
/// `setApplicationIconImage` once on `RunEvent::Ready` (dev only).
/// `TransformProcessType` is asynchronous, so we also re-apply after a short delay.
///
/// Do **not** feed the raw `icon.png` here: it is a full-bleed opaque square.
/// `setApplicationIconImage` then bypasses the system squircle, which is why the
/// Dock icon lost rounded corners and changed size after “打开主窗口”.
#[cfg(target_os = "macos")]
pub(crate) fn restore_macos_dock_icon<R: Runtime>(app: &AppHandle<R>) {
    let generation = DOCK_ICON_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    apply_macos_app_icon();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if DOCK_ICON_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        let _ = app.run_on_main_thread(move || {
            if DOCK_ICON_GENERATION.load(Ordering::Acquire) == generation {
                apply_macos_app_icon();
            }
        });
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn apply_macos_app_icon() {
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    // SAFETY: tray / window events and run_on_main_thread all run on the AppKit main thread.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);

    if let Some(icon) = macos_bundle_dock_icon() {
        unsafe { app.setApplicationIconImage(Some(&icon)) };
        return;
    }

    // `tauri dev` has no `.app` bundle; keep the PNG fallback so Dock is not "exec".
    const APP_ICON_PNG: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.png"));
    let data = NSData::with_bytes(APP_ICON_PNG);
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    unsafe { app.setApplicationIconImage(Some(&icon)) };
}

/// Finder-composited app icon (system squircle already applied).
#[cfg(target_os = "macos")]
pub(crate) fn macos_bundle_dock_icon() -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let bundle = app_bundle_path_from_exe(&exe)?;
    let path = NSString::from_str(&bundle.to_string_lossy());
    Some(NSWorkspace::sharedWorkspace().iconForFile(&path))
}

/// `Foo.app/Contents/MacOS/binary` → `Foo.app`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn app_bundle_path_from_exe(exe: &std::path::Path) -> Option<&std::path::Path> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()?.to_str()? == "app").then_some(bundle)
}

/// 「Windows 模式」是否为浅色（任务栏与开始菜单跟随它，而**不是**应用模式）。
///
/// 必须读 `SystemUsesLightTheme`：`AppsUseLightTheme` 是应用模式（窗口、对话框），
/// 两者可以不一致（「应用浅色 + 系统深色」是常见组合）。Tauri 的
/// `WindowEvent::ThemeChanged` 走的正是 `AppsUseLightTheme`（tao 的 `should_use_dark_mode`），
/// 所以它不能用来判断任务栏底色——跟着它切图标恰好会在浅色任务栏上贴出白猫。
///
/// 返回 `None` 表示读不到（键不存在、权限异常等），由调用方兜底。
#[cfg(windows)]
pub(crate) fn taskbar_uses_light_theme() -> Option<bool> {
    use std::ffi::c_void;
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let subkey = wide(PERSONALIZE_KEY);
    let value = wide("SystemUsesLightTheme");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 两个字符串均以 NUL 结尾且在本调用期间存活；`data` / `size` 指向本函数栈上的
    // 有效内存，缓冲区类型（DWORD）与 RRF_RT_REG_DWORD 一致。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            &mut data as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data != 0)
}

/// 跟随「Windows 模式」切换托盘图标。
///
/// 主题变化没有任何可用的 Tauri 事件（见 `taskbar_uses_light_theme`），因此在后台线程用
/// `RegNotifyChangeKeyValue` 阻塞等待注册表键被改动，醒来后重读并换图标：用户切主题时
/// 任务栏会立刻重绘，图标晚一步就会在对应底色上消失。
///
/// 开键失败、通知失败、或托盘已被销毁时线程直接退出：图标停留在启动时选定的那一版，
/// 不会影响其他功能。线程随进程退出而结束。
#[cfg(windows)]
pub(crate) fn watch_taskbar_theme<R: Runtime>(app: AppHandle<R>) {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
        REG_NOTIFY_CHANGE_LAST_SET,
    };

    std::thread::spawn(move || {
        let subkey = wide(PERSONALIZE_KEY);
        let mut key: HKEY = 0;
        // SAFETY: subkey 以 NUL 结尾且存活到调用结束；`key` 是本函数栈上的有效输出参数。
        let opened =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_NOTIFY, &mut key) };
        if opened != ERROR_SUCCESS {
            return;
        }
        loop {
            // SAFETY: `key` 由上面的 RegOpenKeyExW 打开且尚未关闭；事件句柄传 0 且
            // fAsynchronous = 0，表示同步等待——本调用会阻塞到该键被改动。
            let notified =
                unsafe { RegNotifyChangeKeyValue(key, 1, REG_NOTIFY_CHANGE_LAST_SET, 0, 0) };
            if notified != ERROR_SUCCESS {
                break;
            }
            match app.tray_by_id(TRAY_ID) {
                Some(tray) => {
                    let _ = tray.set_icon(Some(tray_icon()));
                }
                None => break,
            }
        }
        // SAFETY: `key` 由 RegOpenKeyExW 打开，且每条路径上只在这里关闭一次。
        unsafe { RegCloseKey(key) };
    });
}

#[cfg(windows)]
pub(crate) const PERSONALIZE_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// 转成以 NUL 结尾的 UTF-16，供 Win32 宽字符 API 使用。
#[cfg(windows)]
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
