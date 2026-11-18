//! 统一更新服务：检查 / 下载 / 安装重启的单一状态机。
//!
//! 托盘与前端弹窗共用同一份快照：状态变更统一经 [`set_phase`] 写快照 → emit
//! `update-state` → 刷新托盘菜单；前端只订阅事件，不再自己调用 JS 版 updater。
//! 主窗口关闭（轻量模式、隐藏到托盘）时托盘仍可独立走完 检查 → 下载 → 重启。
//!
//! 检查复用 `wb_shift_core::modules::update::update_check`（release 资产端点 + 302 兜底，
//! 不占 GitHub API 配额）；下载走 tauri-plugin-updater 的 Rust API（签名校验在
//! `Update::download` 内部完成），代理取自 `update::load_github_config()` 的 `proxy`。
//!
//! **安装时机**：下载完成的包暂存在内存（`PACKAGE`），由用户点「重启以完成升级」时才
//! 安装。Windows NSIS 安装器要求应用退出后才能完成安装，下载即安装等于强制自动重启；
//! macOS 安装可能触发 `/Applications` 写授权，放在用户主动动作时语义才自然。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;

use wb_shift_core::modules::config::now_secs;
use wb_shift_core::modules::update;

mod download;
mod install;
mod state;
mod tray_bridge;

pub use download::*;
pub use install::*;
pub use state::*;
pub(crate) use tray_bridge::*;

// ---------------------------------------------------------------------------
// 互斥
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// 快照读写（无宿主依赖：单测直接驱动这一层）
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// 对外能力
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// 下载 / 安装内部实现
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// 宿主出口：广播 / 托盘刷新 / tooltip / 通知
// ---------------------------------------------------------------------------
/// 后台首次检查延迟：启动 15 秒后（替代前端 30 分钟轮询的首屏延迟）。
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);

/// 后台检查间隔（30 分钟）。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

const BUSY_MESSAGE: &str = "更新任务正在进行中，请稍候";

const NO_PACKAGE_MESSAGE: &str = "没有可安装的更新包，请先下载更新包";

const DEMO_DOWNLOAD_MESSAGE: &str = "演示模式已跳过更新下载";

const DEMO_RESTART_MESSAGE: &str = "演示模式已跳过更新安装";

/// 检查更新：走状态机（Checking → Available / UpToDate / Error），返回与旧
/// `check_update` 完全同形的结果（前端展示与 webui 侧契约不变）。
///
/// 已有更新任务在跑时直接返回忙碌错误，不并发触发第二次检查。
pub async fn check<R: Runtime>(app: &AppHandle<R>, proxy: Option<&str>, force: bool) -> Value {
    let Some(_busy) = BusyGuard::acquire() else {
        // 互斥拒绝不写日志到 stderr 之外：托盘文案已表达同一件事。
        eprintln!("[更新] 已有任务在跑，跳过本次检查");
        let mut value = json!({
            "ok": false,
            "error": BUSY_MESSAGE,
            "message": BUSY_MESSAGE,
        });
        value["releaseUrl"] = json!(release_url());
        return value;
    };
    eprintln!("[更新] 检查更新（force={force}）");
    set_phase(app, begin_check());
    let result = update::update_check(proxy, force).await;
    let snapshot = set_phase(app, apply_check_result(&result));
    eprintln!(
        "[更新] 检查结果：{:?} latest={:?}",
        snapshot.phase, snapshot.latest
    );
    result
}

/// 后台定时检查：首次延迟 [`FIRST_CHECK_DELAY`]，之后每 [`CHECK_INTERVAL`] 一次。
///
/// 常驻循环随进程存活，轻量模式（WebView 销毁）与主窗口关闭时同样在跑；
/// 未带 force 走 core 的 6 小时缓存，因此多数轮次不发网络请求。
pub fn spawn_periodic_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let _ = check(&app, None, false).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// 快照 / BUSY / PACKAGE 都是进程级单例，用例之间必须串行。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reset() {
        *snapshot_cell()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = UpdateSnapshot::idle();
        BUSY.store(false, Ordering::Release);
    }

    #[test]
    fn phases_flow_idle_available_downloading_ready_to_restart() {
        let _guard = lock();
        reset();
        assert_eq!(snapshot().phase, UpdatePhase::Idle);

        assert_eq!(begin_check().phase, UpdatePhase::Checking);

        let available = apply_check_result(&json!({
            "ok": true,
            "latest": "9.9.9",
            "hasUpdate": true,
            "checkedAt": 1_700_000_000,
        }));
        assert_eq!(available.phase, UpdatePhase::Available);
        assert_eq!(available.latest.as_deref(), Some("9.9.9"));
        assert_eq!(available.checked_at, Some(1_700_000_000));

        let downloading = begin_download("9.9.9");
        assert_eq!(downloading.phase, UpdatePhase::Downloading);
        assert_eq!(downloading.latest.as_deref(), Some("9.9.9"));
        assert_eq!(downloading.percent, None);

        assert_eq!(advance_progress(30, Some(100)), Some(30));
        assert_eq!(snapshot().percent, Some(30));

        let ready = mark_ready_to_restart("9.9.9");
        assert_eq!(ready.phase, UpdatePhase::ReadyToRestart);
        assert_eq!(ready.percent, Some(100));
        assert_eq!(ready.latest.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn check_without_update_lands_on_up_to_date() {
        let _guard = lock();
        reset();
        let snapshot =
            apply_check_result(&json!({"ok": true, "latest": "0.1.0", "hasUpdate": false}));
        assert_eq!(snapshot.phase, UpdatePhase::UpToDate);
        assert_eq!(snapshot.latest, None);
        assert!(snapshot.checked_at.is_some());
    }

    #[test]
    fn failed_check_lands_on_error_with_message() {
        let _guard = lock();
        reset();
        let snapshot = apply_check_result(&json!({
            "ok": false,
            "error": "网络请求失败",
            "message": "网络请求失败（code=-1）",
        }));
        assert_eq!(snapshot.phase, UpdatePhase::Error);
        assert_eq!(snapshot.message.as_deref(), Some("网络请求失败（code=-1）"));
        assert_eq!(snapshot.latest, None);
    }

    /// 已有目标版本时，复查失败必须回到 Available，不能清掉「升级到 vX」入口。
    #[test]
    fn failed_recheck_keeps_available_target() {
        let _guard = lock();
        reset();
        apply_check_result(&json!({
            "ok": true,
            "latest": "9.9.9",
            "hasUpdate": true,
        }));
        assert_eq!(begin_check().phase, UpdatePhase::Checking);
        assert_eq!(snapshot().latest.as_deref(), Some("9.9.9"));
        let snapshot = apply_check_result(&json!({
            "ok": false,
            "error": "网络请求失败",
            "message": "网络请求失败（code=-1）",
        }));
        assert_eq!(snapshot.phase, UpdatePhase::Available);
        assert_eq!(snapshot.latest.as_deref(), Some("9.9.9"));
        assert_eq!(snapshot.message, None);
    }

    #[test]
    fn failed_check_from_idle_still_errors() {
        let _guard = lock();
        reset();
        begin_check();
        let snapshot = apply_check_result(&json!({
            "ok": false,
            "error": "网络请求失败",
            "message": "网络请求失败（code=-1）",
        }));
        assert_eq!(snapshot.phase, UpdatePhase::Error);
        assert_eq!(snapshot.latest, None);
        assert_eq!(snapshot.message.as_deref(), Some("网络请求失败（code=-1）"));
    }

    /// 下载中 / 待重启不被后台检查改写：托盘升级入口不能闪掉。
    #[test]
    fn check_does_not_downgrade_downloading_or_ready_to_restart() {
        let _guard = lock();
        reset();
        begin_download("9.9.9");
        assert_eq!(begin_check().phase, UpdatePhase::Downloading);
        assert_eq!(
            apply_check_result(&json!({"ok": true, "latest": "9.9.10", "hasUpdate": true})).phase,
            UpdatePhase::Downloading
        );

        mark_ready_to_restart("9.9.9");
        assert_eq!(begin_check().phase, UpdatePhase::ReadyToRestart);
        assert_eq!(
            apply_check_result(&json!({"ok": true, "latest": "9.9.10", "hasUpdate": true})).phase,
            UpdatePhase::ReadyToRestart
        );
        assert_eq!(snapshot().phase, UpdatePhase::ReadyToRestart);
    }

    #[test]
    fn failed_download_keeps_target_version_for_retry() {
        let _guard = lock();
        reset();
        apply_check_result(&json!({"ok": true, "latest": "9.9.9", "hasUpdate": true}));
        begin_download("9.9.9");
        let snapshot = fail("下载更新包失败：连接超时".to_string());
        assert_eq!(snapshot.phase, UpdatePhase::Error);
        assert_eq!(snapshot.latest.as_deref(), Some("9.9.9"));
        assert_eq!(snapshot.percent, None);
    }

    #[test]
    fn busy_guard_rejects_second_entry_until_released() {
        let _guard = lock();
        reset();
        let first = BusyGuard::acquire();
        assert!(first.is_some(), "首次进入应拿到互斥");
        assert!(BusyGuard::acquire().is_none(), "并发进入必须被拒");
        drop(first);
        let second = BusyGuard::acquire();
        assert!(second.is_some(), "释放后应重新可进入");
        drop(second);
    }

    #[test]
    fn restart_without_package_returns_error_instead_of_panicking() {
        let _guard = lock();
        *package_cell()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let error = match take_package_for_install() {
            Ok(_) => panic!("无包时不应返回可安装的更新包"),
            Err(error) => error,
        };
        assert!(error.contains("没有可安装的更新包"), "{error}");
    }

    #[test]
    fn percent_conversion_clamps_and_handles_unknown_total() {
        assert_eq!(percent_of(0, Some(100)), Some(0));
        assert_eq!(percent_of(1, Some(3)), Some(33));
        assert_eq!(percent_of(99, Some(100)), Some(99));
        assert_eq!(percent_of(100, Some(100)), Some(100));
        assert_eq!(percent_of(150, Some(100)), Some(100), "超出总量按 100 截断");
        assert_eq!(percent_of(10, None), None, "无 Content-Length 时不报百分比");
        assert_eq!(percent_of(10, Some(0)), None, "总量为 0 不除零");
    }

    #[test]
    fn progress_snapshot_written_only_on_integer_change() {
        let _guard = lock();
        reset();
        begin_download("9.9.9");
        assert_eq!(advance_progress(1, Some(1000)), Some(0));
        assert_eq!(
            advance_progress(2, Some(1000)),
            None,
            "同一整数百分比不重复写"
        );
        assert_eq!(advance_progress(10, Some(1000)), Some(1));
        assert_eq!(snapshot().percent, Some(1));
        assert_eq!(advance_progress(10, None), None, "总量未知不写百分比");
    }

    #[test]
    fn menu_rebuilds_once_per_ten_percent_bucket() {
        let _guard = lock();
        MENU_PROGRESS_BUCKET.store(0, Ordering::Release);
        assert!(!should_rebuild_menu(0));
        assert!(!should_rebuild_menu(3));
        assert!(!should_rebuild_menu(9));
        assert!(should_rebuild_menu(10));
        assert!(!should_rebuild_menu(11));
        assert!(should_rebuild_menu(100), "跨档必须重建");
        assert!(!should_rebuild_menu(100), "同档不重复重建");
    }

    /// 事件 / 命令负载的字段契约（前端 `UpdateSnapshot` 按此解析）。
    #[test]
    fn snapshot_serializes_camel_case_contract() {
        let value = serde_json::to_value(UpdateSnapshot::idle()).unwrap();
        assert_eq!(value["phase"], json!("idle"));
        assert_eq!(value["latest"], json!(null));
        assert_eq!(value["percent"], json!(null));
        assert_eq!(value["message"], json!(null));
        assert_eq!(value["checkedAt"], json!(null));

        let value = serde_json::to_value(mark_ready_to_restart("1.2.3")).unwrap();
        assert_eq!(value["phase"], json!("readyToRestart"));
        assert_eq!(value["latest"], json!("1.2.3"));
        assert_eq!(value["percent"], json!(100));
    }

    #[test]
    fn download_tooltip_reports_percent_or_unknown() {
        assert_eq!(download_tooltip(Some(42)), "正在下载更新 42%");
        assert_eq!(download_tooltip(None), "正在下载更新…");
    }
}
