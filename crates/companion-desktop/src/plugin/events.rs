//! 窗口事件、托盘栏移动与位置落盘。

use super::*;

/// Rail move timestamps in milliseconds: `last` is written by
/// `WindowEvent::Moved`, `checked` remembers the value a correction pass has
/// already handled so one settled position is processed once.
pub(crate) struct MoveWatch {
    pub(crate) last: AtomicU64,
    pub(crate) checked: AtomicU64,
}

impl MoveWatch {
    pub(crate) fn new() -> Self {
        Self {
            last: AtomicU64::new(0),
            checked: AtomicU64::new(0),
        }
    }
    pub(crate) fn touch(&self) {
        self.last.store(now_ms(), Ordering::Release);
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
