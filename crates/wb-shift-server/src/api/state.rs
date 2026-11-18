//! 进程内状态：WebUI 轮询缓存、切换任务协调与 Axum `State` 载体。
//!
//! 除「WorkBuddy 运行状态缓存」这类纯派生缓存外，可变状态都挂在 [`AppState`] 上，
//! 由 `router(state)` 注入。好处是状态隔离可测：每个 `router()`（尤其是测试）拿到独立
//! 的切换状态，不会因为并行测试共享静态变量而互相干扰。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

use wb_shift_core::modules::locks;
use wb_shift_core::modules::{process, variant::WbVariant};

/// WorkBuddy 运行状态缓存：Windows 上检测要跑 tasklist（慢），缓存几秒避免
/// 前端切 tab 频繁触发命令行导致卡顿/闪窗。按档位分别缓存。
#[cfg(target_os = "windows")]
static RUNNING_CACHE: Mutex<Option<(Instant, bool, WbVariant)>> = Mutex::new(None);

pub(crate) fn cached_workbuddy_running(variant: WbVariant) -> bool {
    #[cfg(target_os = "windows")]
    {
        // 纯派生缓存：锁中毒时取回旧值继续，命中与否都会在下一次检测时被覆盖；
        // 进程状态本身仍以 process::is_workbuddy_running 为准。
        let mut cache = locks::recover(&RUNNING_CACHE);
        if let Some((t, v, cached_variant)) = cache.as_ref() {
            if *cached_variant == variant && t.elapsed() < Duration::from_secs(3) {
                return *v;
            }
        }
        let v = process::is_workbuddy_running(variant);
        *cache = Some((Instant::now(), v, variant));
        v
    }
    #[cfg(not(target_os = "windows"))]
    {
        process::is_workbuddy_running(variant)
    }
}

/// 切换任务的运行标记与进度。
///
/// 运行标记用 `AtomicBool` 而不是 `Mutex<bool>`：抢占必须是一次原子操作
/// （两个并发请求不能都通过检查），而且它不需要「锁中毒」语义。
#[derive(Default)]
pub(crate) struct SwitchState {
    running: AtomicBool,
    progress: Mutex<Option<String>>,
}

impl SwitchState {
    /// 尝试成为唯一执行者；返回 false 表示已有任务在跑。
    pub(crate) fn try_begin(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// 记录进度文案。进度只服务轮询 UI，属于可重建缓存：锁中毒时取回并覆盖即可。
    pub(crate) fn set_progress(&self, message: impl Into<String>) {
        *locks::recover(&self.progress) = Some(message.into());
    }

    pub(crate) fn progress(&self) -> Option<String> {
        locks::recover(&self.progress).clone()
    }

    /// 由后台任务持有的运行守卫。**清理放在任务内部**，不依赖 HTTP 请求 future：
    /// 浏览器断开会让 `api_switch` 的 future 被 drop，但 `spawn_blocking` 里的切换
    /// 仍在继续；此时若在请求侧清标记，就会出现「后台还在写账号，WebUI 又能发起第二次
    /// 切换」的并发写。把 Drop 放进任务里，标记只会在真正结束（或 panic 展开）后清除。
    pub(crate) fn run_guard(self: &Arc<Self>) -> SwitchRunGuard {
        SwitchRunGuard {
            state: Arc::clone(self),
        }
    }
}

/// 切换任务的运行守卫：`Drop` 清除运行标记。
pub(crate) struct SwitchRunGuard {
    state: Arc<SwitchState>,
}

impl Drop for SwitchRunGuard {
    fn drop(&mut self) {
        self.state.running.store(false, Ordering::Release);
    }
}

/// 应用级共享状态（由 `router(state)` 注入各 handler）。
#[derive(Default)]
pub(crate) struct AppState {
    /// 用 `Arc` 包一层：后台任务需要把 `SwitchState` 的克隆带进 `spawn_blocking`，
    /// 由任务自己持有运行守卫（见 [`SwitchState::run_guard`]）。
    pub(crate) switch: Arc<SwitchState>,
}

pub(crate) type SharedState = Arc<AppState>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 并发抢占只能有一个成功：这是「不能开第二次切换」的第一道门。
    #[test]
    fn switch_state_only_allows_one_begin() {
        let state = SwitchState::default();
        assert!(state.try_begin());
        assert!(!state.try_begin(), "第二个执行者必须被拒绝");
        assert!(state.is_running());
    }

    /// 回归：请求 future 被取消（客户端断开）时，后台任务仍持有守卫，
    /// 运行标记必须保持，直到任务真正结束。
    #[test]
    fn switch_run_guard_keeps_running_after_request_future_is_dropped() {
        let state = Arc::new(SwitchState::default());
        assert!(state.try_begin());

        // 模拟 spawn_blocking：任务持有守卫，句柄被丢弃等价于请求 future 被取消。
        let task_state = Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            let _guard = task_state.run_guard();
            std::thread::sleep(Duration::from_millis(120));
        });
        drop(handle);

        assert!(state.is_running(), "后台任务还没结束时不得开放第二次切换");
        // 等任务自然结束：守卫在任务内部 Drop，标记随之清除。
        for _ in 0..50 {
            if !state.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!state.is_running(), "任务结束后必须清除运行标记");
    }

    /// 任务 panic 时守卫照样 Drop：不能把状态永久停在「运行中」。
    #[test]
    fn switch_run_guard_clears_flag_when_task_panics() {
        let state = Arc::new(SwitchState::default());
        assert!(state.try_begin());
        let task_state = Arc::clone(&state);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = task_state.run_guard();
            panic!("模拟切换任务内部 panic");
        }));
        assert!(result.is_err());
        assert!(!state.is_running(), "panic 展开后也必须清除运行标记");
    }

    #[test]
    fn progress_is_recorded_and_survives_guard_drop() {
        let state = Arc::new(SwitchState::default());
        assert!(state.try_begin());
        state.set_progress("开始切换账号…");
        drop(state.run_guard());
        assert_eq!(state.progress().as_deref(), Some("开始切换账号…"));
        assert!(!state.is_running());
    }
}
