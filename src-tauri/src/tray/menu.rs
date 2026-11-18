//! 托盘菜单构建与更新动作映射。

use super::*;

impl<R: Runtime> Drop for CheckinBusyGuard<R> {
    fn drop(&mut self) {
        CHECKIN_BUSY.store(false, Ordering::Release);
        refresh_tray_menu(&self.app);
    }
}

/// 重建托盘菜单（更新服务在阶段切换 / 下载跨 10% 时调用）。
///
/// 注意：不要在指针进入 / 点击回调里重建——菜单正在展示时 `set_menu` 会让它闪掉。
pub(crate) fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    let Ok(menu) = build_tray_menu(app) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// 托盘更新入口的三种动作（菜单项 id 即对外契约）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UpdateMenuAction {
    /// 检查更新。
    Check,
    /// 下载 / 重试下载。
    Download,
    /// 安装并重启。
    Restart,
}

/// 更新菜单项的阶段映射：`(动作, 文案, 是否可点)`。
///
/// 菜单与前端弹窗读同一份快照，因此同一时刻两处显示的是同一阶段。
/// `Error` 按是否已知目标版本区分两种重试文案：已知版本 → 重试下载；未知 → 重试检查。
pub(crate) fn update_menu_spec(snapshot: &UpdateSnapshot) -> (UpdateMenuAction, String, bool) {
    match snapshot.phase {
        UpdatePhase::Idle | UpdatePhase::UpToDate => {
            (UpdateMenuAction::Check, "检查更新".to_string(), true)
        }
        UpdatePhase::Checking => (UpdateMenuAction::Check, "正在检查…".to_string(), false),
        UpdatePhase::Available => (
            UpdateMenuAction::Download,
            match snapshot.latest.as_deref() {
                Some(latest) => format!("升级到 v{latest}"),
                None => "升级到新版本".to_string(),
            },
            true,
        ),
        UpdatePhase::Downloading => (
            UpdateMenuAction::Download,
            match snapshot.percent {
                Some(percent) => format!("正在下载更新 {percent}%"),
                None => "正在下载更新…".to_string(),
            },
            false,
        ),
        UpdatePhase::ReadyToRestart => (
            UpdateMenuAction::Restart,
            "重启以完成升级".to_string(),
            true,
        ),
        UpdatePhase::Error => {
            if snapshot.latest.is_some() {
                (
                    UpdateMenuAction::Download,
                    "更新失败，点击重试".to_string(),
                    true,
                )
            } else {
                (
                    UpdateMenuAction::Check,
                    "检查更新失败，点击重试".to_string(),
                    true,
                )
            }
        }
    }
}

pub(crate) fn build_tray_menu<R: Runtime, M: Manager<R>>(app: &M) -> tauri::Result<Menu<R>> {
    let open_item = MenuItem::with_id(app, "open-main-window", "打开主界面", true, None::<&str>)?;
    let github_item = MenuItem::with_id(app, "open-github", "打开 GitHub", true, None::<&str>)?;
    // 档位区分在 core 判定：无签到活动的档位（国际版）不参与「待签到」集合，
    // 否则这些账号永远不会产生签到日志，托盘会一直显示「可签到」。
    let checked_in = checkin::all_accounts_checked_in_today();
    let (checkin_label, checkin_enabled) = if CHECKIN_BUSY.load(Ordering::Acquire) {
        ("一键签到", false)
    } else if checked_in {
        ("已签到", false)
    } else {
        ("一键签到", true)
    };
    let checkin_item = MenuItem::with_id(
        app,
        "checkin-all",
        checkin_label,
        checkin_enabled,
        None::<&str>,
    )?;
    let (update_action, update_label, update_enabled) =
        update_menu_spec(&crate::update_service::snapshot());
    let update_item = MenuItem::with_id(
        app,
        update_action.id(),
        update_label,
        update_enabled,
        None::<&str>,
    )?;
    let lightweight_item = CheckMenuItem::with_id(
        app,
        "lightweight-mode",
        "轻量模式",
        true,
        LIGHTWEIGHT_MODE.load(Ordering::Acquire),
        None::<&str>,
    )?;
    let companion_enabled =
        !crate::is_screenshot_demo() && companion_desktop::is_enabled(app.app_handle());
    let companion_toggle = MenuItem::with_id(
        app,
        "companion-toggle",
        "显示 / 隐藏悬浮窗",
        companion_enabled,
        None::<&str>,
    )?;
    let companion_settings = MenuItem::with_id(
        app,
        "companion-settings",
        "悬浮窗设置",
        companion_enabled,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit-app", "退出应用", true, None::<&str>)?;

    MenuBuilder::new(app)
        .item(&open_item)
        .item(&github_item)
        .item(&checkin_item)
        .separator()
        .item(&update_item)
        .separator()
        .item(&lightweight_item)
        .separator()
        .item(&companion_toggle)
        .item(&companion_settings)
        .separator()
        .item(&quit_item)
        .build()
}
