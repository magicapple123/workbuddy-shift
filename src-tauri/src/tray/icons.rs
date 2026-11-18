//! 托盘图标素材与任务栏主题适配。

#[cfg(windows)]
use super::*;

/// macOS 托盘图标：单色模板素材（系统按明暗主题自动着色）。
///
/// 注意：函数名 `tray_icon` 为跨平台统称；macOS 侧历史名为 `menu_bar_icon`。
#[cfg(target_os = "macos")]
pub(crate) fn tray_icon() -> tauri::image::Image<'static> {
    const ICON: &[u8; 36 * 36 * 4] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-template.rgba"
    ));
    tauri::image::Image::new(ICON, 36, 36)
}

/// Windows 托盘图标：按「Windows 模式」在黑白猫之间选择，读不到主题就用彩色素材兜底。
#[cfg(windows)]
pub(crate) fn tray_icon() -> tauri::image::Image<'static> {
    match tray_icon_variant(taskbar_uses_light_theme()) {
        TrayIconVariant::MonoBlack => mono_black_icon(),
        TrayIconVariant::MonoWhite => mono_white_icon(),
        TrayIconVariant::Color => color_icon(),
    }
}

/// Linux 托盘图标：彩色应用图标。
///
/// Linux 没有模板图标语义，也无法可靠判断任务栏底色（面板主题与发行版相关），
/// 只能用一份在深浅底色下都还看得清的彩色素材。
#[cfg(all(not(target_os = "macos"), not(windows)))]
pub(crate) fn tray_icon() -> tauri::image::Image<'static> {
    color_icon()
}

/// 彩色应用图标（32×32，透明底）。Windows 读不到主题时的兜底，也是 Linux 的唯一素材。
#[cfg(not(target_os = "macos"))]
pub(crate) fn color_icon() -> tauri::image::Image<'static> {
    const ICON: &[u8; 32 * 32 * 4] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-color.rgba"
    ));
    tauri::image::Image::new(ICON, 32, 32)
}

/// 深色单色猫：浅色任务栏下使用（与 macOS 模板同一份猫形，由生成脚本重着色）。
#[cfg(windows)]
pub(crate) fn mono_black_icon() -> tauri::image::Image<'static> {
    const ICON: &[u8; 32 * 32 * 4] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-mono-black.rgba"
    ));
    tauri::image::Image::new(ICON, 32, 32)
}

/// 浅色单色猫：深色任务栏下使用。
#[cfg(windows)]
pub(crate) fn mono_white_icon() -> tauri::image::Image<'static> {
    const ICON: &[u8; 32 * 32 * 4] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-mono-white.rgba"
    ));
    tauri::image::Image::new(ICON, 32, 32)
}

/// Windows 托盘图标形状。
///
/// Windows 不会给第三方托盘图标自动配色（没有 macOS 的模板语义），
/// 只能按任务栏底色自己挑素材。
#[cfg(any(windows, test))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrayIconVariant {
    /// 深色猫：用于浅色任务栏。
    MonoBlack,
    /// 浅色猫：用于深色任务栏。
    MonoWhite,
    /// 彩色应用图标：读不到任务栏主题时的兜底。
    Color,
}

/// 由「Windows 模式」是否为浅色决定用哪套素材。
///
/// `None`（读不到设置）时退回彩色素材，而不是盲猜黑白：猜错的那一版会在对应底色上
/// 彻底看不见，而彩色素材在两种底色下都能辨认。
#[cfg(any(windows, test))]
pub(crate) fn tray_icon_variant(light_taskbar: Option<bool>) -> TrayIconVariant {
    match light_taskbar {
        Some(true) => TrayIconVariant::MonoBlack,
        Some(false) => TrayIconVariant::MonoWhite,
        None => TrayIconVariant::Color,
    }
}
