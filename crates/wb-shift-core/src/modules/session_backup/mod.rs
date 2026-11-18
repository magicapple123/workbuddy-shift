//! 会话临时备份的生命周期：成功清理、未完成保护（design §2–§6）。
//!
//! 备份只用于操作安全与中断恢复，不做长期留档：操作可靠完成后立即回收本次
//! 专属临时目录；未完成（运行中/失败/中断/待恢复/状态不可验证）时保护材料。
//!
//! 三条硬约束：
//! - 删除授权来自「可靠落盘的业务完成状态」，不是 UI 成功文案，也不是缺日志；
//! - 删除范围只限本版自建、归属可验证的操作专属目录（可信根 + 校验后的 UUID 推导）；
//! - 清理失败不回滚业务、不报告操作失败，保留可重试依据，下次维护入口补清理。

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::modules::config::now_ms;
use crate::modules::session::SessionPaths;
use crate::modules::session_link::{
    self, OpPhase, Operation, TemporaryFileIssue, OP_SCAN_PROBLEM_PREFIX,
};
use crate::modules::variant::WbVariant;

/// 维护记录格式版本；读到其它版本一律保护现场并上报，不猜测。
pub const LIFECYCLE_VERSION: u32 = 1;
/// 本版写出的操作日志携带的生命周期标记（protected + Completed 补转只认它）。
pub const OPERATION_LIFECYCLE_VERSION: u32 = 1;
/// 操作专属临时目录根：`backups/session-transactions/{variant}/{operationId}/`。
pub const TRANSACTIONS_DIR_NAME: &str = "session-transactions";
/// 维护记录目录：`session-links/backup-lifecycle/{variant}/{operationId}.json`。
pub const LIFECYCLE_DIR_NAME: &str = "backup-lifecycle";
/// 业务日志里标记「本次备份已清理」。
pub const CLEANUP_STATE_CLEANED: &str = "cleaned";
/// 业务日志里标记「已验证安全终止」（未写业务，允许回收）。
pub const CLEANUP_STATE_SAFE_TERMINATED: &str = "safeTerminated";

/// 当前平台能否持久化目录项更新（Windows 无法对目录句柄 fsync）。
pub const DIRECTORY_SYNC_SUPPORTED: bool = cfg!(unix);

/// 生命周期状态：三种状态之间的推进都由可靠落盘保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupState {
    /// 记录已可靠写入，正在准备备份；此阶段契约禁止任何业务写入。
    Allocating,
    /// 备份完备、即将或已经写业务；禁止回收，直到得到可验证终态。
    Protected,
    /// 已可靠确认业务完成或已验证安全终止；允许幂等回收。
    CleanupPending,
}

impl BackupState {
    pub fn as_str(self) -> &'static str {
        match self {
            BackupState::Allocating => "allocating",
            BackupState::Protected => "protected",
            BackupState::CleanupPending => "cleanupPending",
        }
    }
}

/// 操作级维护记录：独立于历史操作日志的数量裁剪，是补清理的持久依据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupLifecycle {
    pub version: u32,
    pub operation_id: String,
    pub variant: WbVariant,
    /// `copy` | `sync`。
    pub kind: String,
    pub state: BackupState,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub last_error: Option<String>,
    /// 目标会话 id（报告里帮助用户定位）。
    #[serde(default)]
    pub session_id: Option<String>,
    /// 目标会话标题（能取到时优先展示）。
    #[serde(default)]
    pub title: Option<String>,
}

/// 维护记录扫描结果：读取/枚举/解析失败必须显式上报，不能当成「没有」。
#[derive(Debug, Default, Clone)]
pub struct LifecycleScan {
    pub records: Vec<BackupLifecycle>,
    pub problems: Vec<String>,
}

/// 单次清理的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// 目录与维护记录都已回收。
    Cleaned,
    /// 本轮未清理成功（例如权限错误/文件被占用），保留依据待下次重试。
    Pending { reason: String },
    /// 状态不可验证或归属异常：保留材料并上报，不做删除。
    Protected { reason: String },
}

mod backup;
mod cleanup;
mod lifecycle;

pub use backup::{
    durable_write, durable_write_str, ensure_full_synchronous, lifecycle_root, sync_dir, sync_file,
    sync_tree, transaction_dir, transactions_root,
};
pub use cleanup::{
    cleanup_after_success, cleanup_operation, maintain, reclaim_unwritten, record_cleanup_failure,
};
pub use lifecycle::{
    advance_record, begin_operation, load_lifecycle, mark_cleanup_pending, mark_protected,
    referenced_operation_ids, save_lifecycle, scan_lifecycle,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::session_link::{OpPhase, Operation, OperationMember, OPERATION_VERSION};

    #[allow(unused_imports)]
    use super::backup::*;
    #[allow(unused_imports)]
    use super::cleanup::*;
    #[allow(unused_imports)]
    use super::lifecycle::*;
    include!("tests/backup.rs");
    include!("tests/cleanup.rs");
}
