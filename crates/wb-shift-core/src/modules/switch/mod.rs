//! 账号切换：备份 → 关进程 → 恢复/复制会话（可选）→ 写认证 → 启动。
//!
//! 对照 server.py `switch_account`。切换过程中通过进度回调向前端推送实时进度，
//! 避免界面长时间无反馈被误认为卡死。core 不依赖 Tauri，进度回调由宿主适配
//! （桌面端转发为 `switch-progress` 事件，HTTP 端写入轮询/SSE）。
//!
//! 顺序要点（design §1）：互斥 → 关进程 → 恢复/复制/同步会话 → 写认证 → 启动。
//! 复制与同步顺序调用、共用同一把档位操作锁，且在「关闭 App + 恢复未完成写入」之后
//! 才执行；会话写入一律在 WorkBuddy 停止写入之后。普通会话操作失败不阻止认证切换，
//! 只有「无法安全恢复的中间产物」才暂停切换并明确报告恢复需求。
//!
//! 暂停启动的判定有两次，口径相同（design §5 / R5）：
//!   1. 写入之前——切换开头恢复未完成写入后仍有不可安全恢复的中间产物；
//!   2. 写入之后——本次复制/同步新留下的未完成操作（正文/数据库/组表任一阶段失败都会
//!      保留操作记录），不能带着不一致的会话内容写认证并启动 App。

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::modules::account;
use crate::modules::auth_file;
use crate::modules::process::{close_workbuddy, launch_workbuddy};
use crate::modules::session::{self, SessionPaths, SyncSelection};
use crate::modules::session_link::{
    self, Operation, RecoveryReport, LOCK_BUSY_MESSAGE_PREFIX, LOCK_UNAVAILABLE_MESSAGE_PREFIX,
};
use crate::modules::settings_sync;
use crate::modules::variant::WbVariant;

/// 切换进度回调（宿主注入，如 Tauri `app.emit` 或 HTTP 进度缓存）。
pub type ProgressFn = Box<dyn Fn(&str) + Send + Sync>;

/// `restart=false` 携带会话写入意图时的拒绝文案（复制/同步各自一条）。
pub const COPY_WITHOUT_RESTART_MESSAGE: &str =
    "本次切换未重启 WorkBuddy（restart=false），已拒绝会话复制请求；如需复制请勾选重启切换";
pub const SYNC_WITHOUT_RESTART_MESSAGE: &str =
    "本次切换未重启 WorkBuddy（restart=false），已拒绝会话同步请求；如需同步请勾选重启切换";

mod execute;
mod plan;

pub use execute::switch_account;
pub use plan::{recovery_blocking_detail, recovery_blocks_startup, recovery_report_json};

#[cfg(test)]
mod tests {
    //! 编排层可注入纯函数的单测：切换全流程（关进程/写认证/启动）需真机验收，
    //! 这里覆盖「不可安全恢复必须暂停启动」「本次新留下的未完成写入必须暂停启动」
    //! 与「restart=false 拒绝写入意图」三条判定。

    use super::plan::*;
    include!("tests/execute.rs");
}
