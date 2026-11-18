//! hook 信号消费：把 `~/.workbuddy-shift/hook-events.jsonl` 里的 `Stop` / `FinalStop` payload
//! 变成限额台账条目。
//!
//! 数据流：hook 脚本 append payload（行级、O_APPEND）→ 本模块按**已读 offset** 逐行消费
//! （只有完整行才处理，写到一半的行留到下一轮）→ 识别限额文案（`QUOTA_MARKERS`）并取官方
//! 恢复时刻（`limits::parse_reset_at`）→ 归因（CLI：该 429 请求发出时该会话的进程启动时刻
//! 不早于 `state.json` 写入时刻即当前账号；WorkBuddy：登录态文件 / 会话表）→ 入账
//! （内存 + 落盘）→ 由宿主 emit `rate-limits-updated` 通知前端。
//!
//! 关键决定（实施时定的开放点）：
//! - **消费方式**：记 offset + 到 1 MB 轮转（`rename` 后再读旧 inode 的增量），不裁剪写入方
//!   正在追加的文件；轮转失败不动文件，宁可继续增长也不丢事件。
//! - **CLI 归因**：key 是**进程级快照**（切换只对新进程生效）。「活进程 key == 当前账号」
//!   是显式不变式（INV，见 `.trellis/spec/wb-shift-core/backend/rate-limit-ledger.md`）：
//!   手动切换先关进程再写 `state.json`，自动轮换只在没有存活会话时才切。因此归因只需
//!   「读当前账号 + 一个陈旧守卫」，判据是**该 429 请求自身的时刻**（`request_time_of`：
//!   文案尾部 `(requestId/sessionId)` 里的 UUIDv7 requestId，前 48 位 = 毫秒时刻）：
//!   取「请求时刻该会话的进程启动时刻」（`sessions/<pid>.json` 的 `startedAt`，兜底
//!   transcript 首行），`startedAt ≥ state.json mtime` → 当前账号；早于 mtime ⇒ INV 被破坏
//!   （有进程持旧 key）⇒ 丢弃 + 告警。用请求时刻而不是消费时刻，是为了挡住「会话恢复 /
//!   压缩时新进程重放上一轮（旧账号）的 429 文案」——`Stop` 的 `last_assistant_message`
//!   是「会话最后一条助手消息」，可以属于更早的一轮、更早的账号（2026-09-21 本机实证）。
//!   请求时刻取不到 / 不合理 / 快照取不到都丢弃：宁可少显示，不可显示错账号。
//! - **WorkBuddy 归因**：① 客户端登录态文件里的 uid → 账号库；② 兜底用会话 id 查 `sessions`
//!   表（与日志扫描同源）；都拿不到就丢弃（宁可少显示，不可显示错账号）。
//! - **轮询节奏**：1 秒一次 `stat`（未变则不做任何读取），hook 事件要求秒级可见；
//!   不引入 `notify` 依赖（本 crate 不新增依赖）。
//!
//! 事件 payload 不含账号 uid：CLI 的 key 归属由「429 请求时刻的进程快照」决定，WorkBuddy 由
//! 触发当轮的登录态决定，因此必须在事件到达时归因，不能延后。

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::modules::account;
use crate::modules::codebuddy_cli;
use crate::modules::config::{atomic_write, home_dir, now_ms, store_dir};
use crate::modules::limits::{self, Resolved};
use crate::modules::rate_limit_hook;
use crate::modules::variant::WbVariant;

mod parse;
mod state;
mod watch;

pub use state::state_path;
pub(crate) use watch::{hook_entries, rate_limit_enabled};
pub use watch::{last_event_at, spawn_watcher};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::parse::*;
    use super::state::*;
    use super::watch::*;
    include!("tests/fixtures.rs");
    include!("tests/parse.rs");
    include!("tests/attribution.rs");
    include!("tests/lifecycle.rs");
}
