//! 进程内锁的获取 helper：把「锁中毒」从 panic 变成可处理的分支。
//!
//! 与 `wb-shift-core` / `companion-core` 的同名模块语义一致；插件层刻意不依赖
//! 业务 crate（见 docs/ARCHITECTURE.md 的依赖方向），因此保留一份最小实现。
//!
//! - [`guard`]：关键状态（采集器连接、交互区域），中毒返回错误文案；
//! - [`recover`]：只用于可重建缓存（快照）与「下一次写入会整体覆盖」的句柄。

use std::sync::{Mutex, MutexGuard, PoisonError};

/// 取关键状态的锁；中毒时返回可展示的错误文案。
pub fn guard<'a, T>(mutex: &'a Mutex<T>, what: &str) -> Result<MutexGuard<'a, T>, String> {
    mutex
        .lock()
        .map_err(|_| format!("{what}状态锁已损坏（上一次操作异常中断），请重启应用后重试"))
}

/// 取可重建缓存；中毒时取回内部值继续（下一次轮询会整体覆盖）。
pub fn recover<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
