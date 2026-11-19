use super::{
    format_checkin_tooltip, is_silent_startup, should_activate_on_second_launch,
    should_keep_tray_alive, should_wake_main_window, tray_icon_variant, MouseButton,
    MouseButtonState, TrayIconVariant,
};
// 平台专属依赖按「真正用到它的用例所在平台」导入，避免在另一平台上成为
// unused import（CI 的 `-D warnings` 会把 unused 直接判错）：
// - `tray_icon()` 只被 macOS 与非 Windows 的素材用例调用；
// - `taskbar_uses_light_theme()` 只被 Windows 的真机自检用例调用，其定义本身也是
//   `#[cfg(windows)]`，无条件导入会让非 Windows 目标直接找不到该名字。
#[cfg(windows)]
use super::taskbar_uses_light_theme;
#[cfg(not(windows))]
use super::tray_icon;
use serde_json::json;

#[test]
fn runtime_exit_with_no_code_keeps_tray() {
    assert!(should_keep_tray_alive(None));
    assert!(!should_keep_tray_alive(Some(0)));
}

#[cfg(target_os = "macos")]
#[test]
fn tray_icon_has_transparency_and_antialiasing() {
    let icon = tray_icon();
    assert_eq!((icon.width(), icon.height()), (36, 36));
    assert!(icon
        .rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[3] == 0));
    assert!(icon
        .rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| (1..=254).contains(&pixel[3])));
}

/// Linux：托盘素材必须是**彩色透明底**，避免退化成白块 / 白底方图。
#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn tray_icon_is_colored_on_transparent_background() {
    let icon = tray_icon();
    assert_eq!((icon.width(), icon.height()), (32, 32));
    let px: Vec<&[u8]> = icon.rgba().chunks_exact(4).collect();
    assert!(px.iter().any(|p| p[3] == 255), "应存在不透明像素");
    assert!(
        px.iter().any(|p| p[3] == 0),
        "背景必须透明：满幅不透明方图会在深色任务栏上显示为白底方块"
    );
    let colored = px.iter().filter(|p| p[3] > 200).any(|p| {
        (p[0] as i32 - p[1] as i32).abs() > 12
            || (p[1] as i32 - p[2] as i32).abs() > 12
            || (p[0] as i32 - p[2] as i32).abs() > 12
    });
    assert!(
        colored,
        "托盘图标必须是彩色的（Windows 不支持模板图标语义）"
    );
}

/// 单色素材本身的性质：透明底、单一墨色，且黑猫确实比白猫暗。
///
/// 这里直接读文件而不是走 `tray_icon()`，让 Windows 之外也能校验素材。
#[test]
fn mono_icons_are_transparent_single_ink() {
    let black: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-mono-black.rgba"
    ));
    let white: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/icons/tray-icon-mono-white.rgba"
    ));
    for (name, bytes) in [("黑猫", black), ("白猫", white)] {
        assert_eq!(bytes.len(), 32 * 32 * 4, "{name}素材尺寸应为 32×32");
        let px: Vec<&[u8]> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p.as_slice())
            .collect();
        assert!(px.iter().any(|p| p[3] == 0), "{name}背景必须透明");
        assert!(px.iter().any(|p| p[3] == 255), "{name}应存在不透明像素");
        let inks: std::collections::HashSet<&[u8]> =
            px.iter().filter(|p| p[3] > 0).map(|p| &p[..3]).collect();
        assert_eq!(
            inks.len(),
            1,
            "{name}必须是单一墨色（模板剪影不该有彩色边缘）"
        );
    }
    let ink = |bytes: &[u8]| {
        let p = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .find(|p| p[3] == 255)
            .unwrap()
            .as_slice();
        p[0] as u32 + p[1] as u32 + p[2] as u32
    };
    assert!(ink(black) < ink(white), "黑猫必须比白猫暗");
}

/// 主题 → 素材的映射：浅色任务栏配黑猫，深色配白猫，读不到则退回彩色。
#[test]
fn tray_icon_variant_follows_taskbar_theme() {
    assert_eq!(
        tray_icon_variant(Some(true)),
        TrayIconVariant::MonoBlack,
        "浅色任务栏下白猫会隐形，必须用深色猫"
    );
    assert_eq!(
        tray_icon_variant(Some(false)),
        TrayIconVariant::MonoWhite,
        "深色任务栏下黑猫会隐形，必须用浅色猫"
    );
    assert_eq!(
        tray_icon_variant(None),
        TrayIconVariant::Color,
        "读不到主题时退回彩色素材，不能盲猜黑白"
    );
}

/// 真机自检：Windows 上必须读得到「Windows 模式」。
///
/// 这条断言同时是「自适应方案在真实机器上到底行不行」的答案：读得到就按主题切换，
/// 读不到则生产代码会退回彩色素材（不会显示异常，但自适应形同虚设）。
/// 只在 Windows 上跑。
#[cfg(windows)]
#[test]
fn taskbar_theme_is_readable_on_windows() {
    assert!(
        taskbar_uses_light_theme().is_some(),
        "读不到 SystemUsesLightTheme：自适应不可用，应改为固定素材"
    );
}

/// 左键「抬起」才唤窗；右键、按下都不唤窗。
///
/// macOS 例外：左键要留给菜单，唤窗判定必须为 false。
#[test]
fn tray_left_click_release_wakes_main_window_only_off_macos() {
    assert_eq!(
        should_wake_main_window(MouseButton::Left, MouseButtonState::Up),
        !cfg!(target_os = "macos")
    );
    assert!(!should_wake_main_window(
        MouseButton::Left,
        MouseButtonState::Down
    ));
    assert!(!should_wake_main_window(
        MouseButton::Right,
        MouseButtonState::Up
    ));
    assert!(!should_wake_main_window(
        MouseButton::Middle,
        MouseButtonState::Up
    ));
}
#[test]
fn silent_startup_matches_exact_hidden_arg() {
    assert!(is_silent_startup(["--hidden"]));
    assert!(is_silent_startup(["workbuddy-shift-desktop", "--hidden"]));
    assert!(is_silent_startup([
        "workbuddy-shift-desktop",
        "--hidden",
        "--debug"
    ]));
}

#[test]
fn silent_startup_rejects_unrelated_and_substring_args() {
    assert!(!is_silent_startup(Vec::<&str>::new()));
    assert!(!is_silent_startup(["workbuddy-shift-desktop"]));
    assert!(!is_silent_startup(["workbuddy-shift-desktop", "--debug"]));
    assert!(!is_silent_startup([
        "workbuddy-shift-desktop",
        "--hidden=true"
    ]));
    assert!(!is_silent_startup(["workbuddy-shift-desktop", "-hidden"]));
    assert!(!is_silent_startup([
        "workbuddy-shift-desktop",
        "--hidden-x"
    ]));
    assert!(!is_silent_startup(["workbuddy-shift-desktop", "x--hidden"]));
}

#[test]
fn second_launch_with_exact_hidden_arg_does_not_activate() {
    // 插件回传的 args 含 argv[0]，静默判定必须整参相等。
    assert!(!should_activate_on_second_launch([
        "workbuddy-shift-desktop",
        "--hidden"
    ]));
}

#[test]
fn second_launch_activates_unless_exact_hidden_arg() {
    assert!(should_activate_on_second_launch(Vec::<&str>::new()));
    assert!(should_activate_on_second_launch([
        "workbuddy-shift-desktop"
    ]));
    assert!(should_activate_on_second_launch([
        "workbuddy-shift-desktop",
        "--debug"
    ]));
    assert!(should_activate_on_second_launch([
        "workbuddy-shift-desktop",
        "--hidden-x"
    ]));
    assert!(should_activate_on_second_launch([
        "workbuddy-shift-desktop",
        "x--hidden"
    ]));
}

#[test]
fn tooltip_when_no_accounts() {
    assert_eq!(
        format_checkin_tooltip(&json!({"accounts": []})),
        "没有可签到的账号"
    );
}

#[test]
fn tooltip_when_accounts_missing() {
    assert_eq!(format_checkin_tooltip(&json!({})), "没有可签到的账号");
}

#[test]
fn tooltip_reports_overlapping_checkin_as_busy() {
    assert_eq!(
        format_checkin_tooltip(&json!({
            "accounts": [],
            "status": "skipped",
            "reason": "already_running"
        })),
        "签到任务正在进行，请稍后再试"
    );
}

#[test]
fn tooltip_summarizes_success_already_error() {
    let payload = json!({
        "accounts": [
            {"result": "success"},
            {"result": "success"},
            {"result": "already"},
            {"result": "error"},
            {"result": "error"},
            {"result": "error"}
        ]
    });
    assert_eq!(
        format_checkin_tooltip(&payload),
        "签到完成：成功 2，已签 1，失败 3"
    );
}

/// 国际版账号签到结果为 inactive：单列「未开放」，不计入失败。
#[test]
fn tooltip_separates_inactive_variant_results() {
    let payload = json!({
        "accounts": [
            {"result": "success", "variant": "cn"},
            {"result": "inactive", "inactive": true, "variant": "ai"}
        ]
    });
    assert_eq!(
        format_checkin_tooltip(&payload),
        "签到完成：成功 1，已签 0，失败 0，未开放 1"
    );
}

#[test]
fn checkin_succeeded_requires_all_ok() {
    use super::checkin_succeeded;
    assert!(!checkin_succeeded(&json!({})));
    assert!(!checkin_succeeded(&json!({"accounts": []})));
    assert!(checkin_succeeded(&json!({
        "accounts": [{"result": "success"}, {"result": "already"}]
    })));
    assert!(!checkin_succeeded(&json!({
        "accounts": [{"result": "success"}, {"result": "error"}]
    })));
    // inactive 不是失败：仍弹通知，但绝不伪造成成功。
    assert!(checkin_succeeded(&json!({
        "accounts": [{"result": "success"}, {"result": "inactive"}]
    })));
    assert!(!checkin_succeeded(&json!({
        "accounts": [{"result": "inactive"}, {"result": "error"}]
    })));
}

#[test]
fn app_bundle_path_from_packaged_exe() {
    use std::path::Path;
    let exe = Path::new("/Applications/workbuddy-shift.app/Contents/MacOS/workbuddy-shift-desktop");
    assert_eq!(
        super::app_bundle_path_from_exe(exe),
        Some(Path::new("/Applications/workbuddy-shift.app"))
    );
}

#[test]
fn app_bundle_path_none_outside_app_bundle() {
    use std::path::Path;
    assert!(super::app_bundle_path_from_exe(Path::new("/tmp/exec")).is_none());
    assert!(super::app_bundle_path_from_exe(Path::new(
        "/Users/x/target/debug/workbuddy-shift-desktop"
    ))
    .is_none());
    assert!(super::app_bundle_path_from_exe(Path::new(
        "/Applications/workbuddy-shift.app/Contents/Resources/icon.icns"
    ))
    .is_none());
}

#[test]
fn tooltip_ignores_unknown_results() {
    let payload = json!({
        "accounts": [
            {"result": "success"},
            {"result": "skipped"},
            {"result": null}
        ]
    });
    assert_eq!(
        format_checkin_tooltip(&payload),
        "签到完成：成功 1，已签 0，失败 0"
    );
}

/// 托盘更新入口的阶段映射（文案见 design §6，菜单与前端弹窗共用同一快照）。
#[test]
fn update_menu_spec_maps_each_phase_to_its_entry() {
    use super::{update_menu_spec, UpdateMenuAction, UpdatePhase, UpdateSnapshot};

    let snapshot = |phase: UpdatePhase, latest: Option<&str>, percent: Option<u8>| UpdateSnapshot {
        phase,
        latest: latest.map(str::to_string),
        percent,
        message: None,
        checked_at: None,
        busy_kind: None,
        busy_since: None,
        manual_install_required: false,
    };

    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Idle, None, None)),
        (UpdateMenuAction::Check, "检查更新".to_string(), true)
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::UpToDate, None, None)),
        (UpdateMenuAction::Check, "检查更新".to_string(), true)
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Checking, None, None)),
        (UpdateMenuAction::Check, "正在检查…".to_string(), false),
        "检查中不可重复点击"
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Available, Some("0.1.48"), None)),
        (
            UpdateMenuAction::Download,
            "升级到 v0.1.48".to_string(),
            true
        )
    );
    assert_eq!(
        update_menu_spec(&snapshot(
            UpdatePhase::Downloading,
            Some("0.1.48"),
            Some(42)
        )),
        (
            UpdateMenuAction::Download,
            "正在下载更新 42%".to_string(),
            false
        )
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Downloading, Some("0.1.48"), None)),
        (
            UpdateMenuAction::Download,
            "正在下载更新…".to_string(),
            false
        ),
        "总量未知时只显示进行中，不显示假百分比"
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::ReadyToRestart, Some("0.1.48"), None)),
        (
            UpdateMenuAction::Restart,
            "重启以完成升级".to_string(),
            true
        ),
        "重启时机由用户决定，不自动重启"
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Error, Some("0.1.48"), None)),
        (
            UpdateMenuAction::Download,
            "更新失败，点击重试".to_string(),
            true
        ),
        "已知目标版本 → 重试的是下载"
    );
    assert_eq!(
        update_menu_spec(&snapshot(UpdatePhase::Error, None, None)),
        (
            UpdateMenuAction::Check,
            "检查更新失败，点击重试".to_string(),
            true
        ),
        "未知版本 → 重试的是检查"
    );
}
