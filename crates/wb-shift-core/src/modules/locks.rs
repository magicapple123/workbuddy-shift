//! 进程内锁的获取 helper：把「锁中毒」从 panic 变成可处理的分支。
//!
//! 约定见 `AGENTS.md` 的「Rust 约定」：生产代码不得 `lock().unwrap()`。
//!
//! 两类语义必须区分，不能混用：
//!
//! - [`guard`]：**业务状态**。中毒说明持锁线程 panic 过，临界区可能只写了一半，
//!   调用方应把错误文案返回给用户（HTTP/命令边界），而不是继续用可能不一致的数据。
//! - [`recover`]：只允许用于 (a) `()` 串行化守卫、(b) 可重建的进程内缓存。
//!   这两类值的正确性不依赖临界区完整执行：守卫没有数据；缓存下一次请求会整体覆盖。
//!   在别处使用等于静默吞掉 panic，评审时应拒绝。

use std::sync::{Mutex, MutexGuard, PoisonError};

/// 取业务状态的锁；中毒时返回稳定、可展示的错误文案。
///
/// `what` 是给用户看的名词（如「登录」「切换」），不要传文件路径或内部类型名。
pub fn guard<'a, T>(mutex: &'a Mutex<T>, what: &str) -> Result<MutexGuard<'a, T>, String> {
    mutex.lock().map_err(|_| {
        format!("{what}状态锁已损坏（上一次操作异常中断），请重启 WorkBuddy Shift 后重试")
    })
}

/// 取串行化守卫或可重建缓存；中毒时取回内部值继续（理由见模块文档）。
pub fn recover<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn guard_returns_error_after_poison() {
        let mutex = Arc::new(Mutex::new(1_u32));
        let clone = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = clone.lock().expect("首次加锁");
            panic!("故意在持锁时 panic");
        })
        .join();

        let error = guard(&mutex, "测试").expect_err("中毒后必须返回错误");
        assert!(error.contains("测试状态锁已损坏"), "{error}");
    }

    #[test]
    fn recover_returns_inner_value_after_poison() {
        let mutex = Arc::new(Mutex::new(7_u32));
        let clone = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = clone.lock().expect("首次加锁");
            panic!("故意在持锁时 panic");
        })
        .join();

        assert_eq!(*recover(&mutex), 7);
    }
}
