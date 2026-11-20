//! 会话操作日志、跨进程互斥与恢复记录。

use crate::modules::session::SessionPaths;
use crate::modules::session_backup;
use crate::modules::variant::WbVariant;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// 操作日志格式版本。
pub const OPERATION_VERSION: u32 = 1;
/// 每档位保留的已完成操作日志条数（未完成的一律保留）。
pub const KEEP_COMPLETED_OPERATIONS: usize = 20;
/// 操作日志扫描不完整时的上报前缀（扫描失败不等于「没有」）。
pub const OP_SCAN_PROBLEM_PREFIX: &str = "操作记录扫描不完整：";

/// 操作日志（design §2 Operation / §4）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub version: u32,
    pub operation_id: String,
    /// `copy`（第一步）／`sync`（第二步预留）。
    pub kind: String,
    pub variant: WbVariant,
    /// 源档位：跨档复制时与 [`Self::variant`]（目标档）不同；同档与旧记录缺省为 `None`。
    ///
    /// 恢复时据此解析源档数据根（正文与源行读取），旧记录一律按同档处理。
    #[serde(default)]
    pub source_variant: Option<WbVariant>,
    /// 目标组 id：新建组时在写入前预分配，恢复时复用同一个组。
    pub group_id: String,
    pub source: OperationMember,
    pub target: OperationMember,
    /// 本次写入的正文归一化总摘要（快照），恢复时据此判断中间产物是否被改动。
    pub expected_content_digest: String,
    pub expected_record_count: usize,
    pub phase: OpPhase,
    #[serde(default)]
    pub backup: Option<String>,
    /// 本版生命周期标记；只有带标记的可靠终态才授权回收备份（design §3）。
    #[serde(default)]
    pub lifecycle_version: Option<u32>,
    /// `cleaned`（备份已回收）| `safeTerminated`（已验证安全终止，允许回收）。
    #[serde(default)]
    pub cleanup_state: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// 用户显式忽略会话账本（用量 + 中断记录）：本次复制永久不带账本，不做后台补写。
    ///
    /// 旧记录没有这个字段，按 `false` 读——即旧副本仍按「应当带账本」处理。
    #[serde(default)]
    pub aux_skipped: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 操作涉及的账号/会话身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationMember {
    #[serde(default)]
    pub account_id: Option<String>,
    pub uid: String,
    pub session_id: String,
}

/// 操作阶段：只有走到 `Completed` 才算完整成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpPhase {
    /// 已预分配目标 UUID，尚未写入任何副本。
    Prepared,
    BodyWritten,
    DbWritten,
    MappingWritten,
    LinksCommitted,
    Completed,
    /// 未写入任何内容即放弃（例如源会话已被删除），不是成功。
    Abandoned,
}

impl OpPhase {
    pub fn is_unfinished(self) -> bool {
        !matches!(self, OpPhase::Completed | OpPhase::Abandoned)
    }
}

/// 操作日志扫描结果：解析失败的文件必须显式上报，不能当成没有。
///
/// `complete` 为 false 表示目录不可读或存在枚举失败：此时调用方不得把扫描结果
/// 当作「不存在对应操作」来授权删除（design §5）。
#[derive(Debug, Default, Clone)]
pub struct OperationScan {
    pub operations: Vec<Operation>,
    pub problems: Vec<String>,
    pub complete: bool,
}

/// 恢复/异常项上报。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryIssue {
    pub operation_id: String,
    pub reason: String,
    /// 重试可能成功（例如映射库暂不可用）；false 表示需要人工处理，不得盲目重放。
    pub retryable: bool,
}

/// 临时备份残留的只读上报项（复制/同步/恢复报告共用同一结构）。
///
/// `state` 为 `cleanupPending`（待清理，下次维护入口重试）或 `needsRecovery`
/// （待恢复/状态不可验证，需人工确认）。待清理不是错误，不设置报告级 needsRecovery。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporaryFileIssue {
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: String,
    pub reason: String,
}

impl TemporaryFileIssue {
    /// 待清理：业务已确认完成，只是本轮没清理成功。
    pub fn cleanup_pending(
        operation_id: String,
        session_id: Option<String>,
        title: Option<String>,
        reason: String,
    ) -> Self {
        Self {
            operation_id,
            session_id,
            title,
            state: "cleanupPending".to_string(),
            reason,
        }
    }

    /// 待恢复：材料必须保留，需要用户/恢复流程处理。
    pub fn needs_recovery(
        operation_id: String,
        session_id: Option<String>,
        title: Option<String>,
        reason: String,
    ) -> Self {
        Self {
            operation_id,
            session_id,
            title,
            state: "needsRecovery".to_string(),
            reason,
        }
    }
}

/// 恢复报告。
#[derive(Debug, Default, Clone)]
pub struct RecoveryReport {
    /// 本次补齐并标记完成的操作 id。
    pub recovered: Vec<String>,
    /// 未写入任何内容即放弃的操作 id。
    pub abandoned: Vec<String>,
    /// 需要人工处理（中间产物被改动/丢失），不得盲目重放。
    pub needs_recovery: Vec<RecoveryIssue>,
    /// 临时备份残留：待清理与待恢复（不改变 `is_clean`/启动阻断口径）。
    pub temporary_files: Vec<TemporaryFileIssue>,
}

impl RecoveryReport {
    pub fn is_clean(&self) -> bool {
        self.needs_recovery.is_empty()
    }

    /// 本次恢复是否什么都没做（宿主据此决定是否回报详情）。
    pub fn is_empty(&self) -> bool {
        self.recovered.is_empty()
            && self.abandoned.is_empty()
            && self.needs_recovery.is_empty()
            && self.temporary_files.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 锁定（design §2 / §4.1）
// ---------------------------------------------------------------------------

/// 持锁的锁文件句柄；Drop 时释放。
pub struct FileLock {
    file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 锁失败原因：Busy = 已被其它进程持有；Unavailable = 无法建立互斥。
#[derive(Debug)]
pub enum LockError {
    Busy,
    Unavailable(String),
}

/// 档位操作锁的名字（[`LockError::message`] 的 `what`）：失败文案以它开头。
pub const VARIANT_OPS_LOCK_NAME: &str = "会话操作";
/// 档位操作锁被其它进程占用时的失败文案前缀。
///
/// 宿主（switch.rs）用它与 [`LOCK_UNAVAILABLE_MESSAGE_PREFIX`] 判断「拿不到档位锁」，
/// 不再嗅探整句错误文案；单测 `lock_failure_prefixes_match_error_messages` 保证常量
/// 与 [`LockError::message`] 的输出不漂移。
pub const LOCK_BUSY_MESSAGE_PREFIX: &str = "会话操作正被其它进程占用";
/// 档位操作锁无法建立互斥（不可用）时的失败文案前缀。
pub const LOCK_UNAVAILABLE_MESSAGE_PREFIX: &str = "会话操作不可用";

impl LockError {
    /// 锁失败文案。`what` 是被锁对象名；档位操作锁传 [`VARIANT_OPS_LOCK_NAME`]，
    /// 得到的文案分别以 [`LOCK_BUSY_MESSAGE_PREFIX`] / [`LOCK_UNAVAILABLE_MESSAGE_PREFIX`]
    /// 开头。
    pub fn message(&self, what: &str) -> String {
        match self {
            LockError::Busy => format!("{what}正被其它进程占用，请稍后重试"),
            LockError::Unavailable(reason) => format!("{what}不可用：{reason}"),
        }
    }
}

/// 以排他方式尝试锁定文件（`std::fs::File::try_lock`，1.89+ 稳定）。
///
/// 明确被占用才返回 Busy；其余失败一律 Unavailable——无互斥保证时不得继续写。
pub fn try_lock_file(path: &Path) -> Result<FileLock, LockError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| LockError::Unavailable(error.to_string()))?;
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| LockError::Unavailable(error.to_string()))?;
    match file.try_lock() {
        Ok(()) => Ok(FileLock { file }),
        Err(TryLockError::WouldBlock) => Err(LockError::Busy),
        Err(TryLockError::Error(error)) => Err(LockError::Unavailable(error.to_string())),
    }
}

/// 档位操作锁：覆盖一次完整的会话操作（查询关联 → 写副本 → 提交关联）。
///
/// 并发请求与中断重试靠它串行化；被占用时直接拒绝，不排队、不绕过。
pub fn try_acquire_variant_ops_lock(
    paths: &SessionPaths,
    variant: WbVariant,
) -> Result<FileLock, String> {
    try_lock_file(&paths.variant_ops_lock_file(variant))
        .map_err(|error| error.message(VARIANT_OPS_LOCK_NAME))
}

/// Namespace-wide operation lock for IDE/extension stores shared across WorkBuddy variants.
/// The caller holds it across duplicate checks, copy/sync, and link registration.
pub fn try_acquire_client_ops_lock(paths: &SessionPaths) -> Result<FileLock, String> {
    try_lock_file(&paths.client_ops_lock_file()).map_err(|error| match error {
        LockError::Busy => "该客户端正在执行其它会话操作，请稍后重试".to_string(),
        LockError::Unavailable(reason) => format!("无法建立客户端会话操作锁：{reason}"),
    })
}

// 操作日志持久化与扫描
fn operation_file(paths: &SessionPaths, operation_id: &str) -> PathBuf {
    paths.operations_dir().join(format!("{operation_id}.json"))
}

/// 原子写入操作日志（临时文件 + 持久化屏障 + rename + 父目录持久化）。
pub fn save_operation(paths: &SessionPaths, operation: &Operation) -> Result<(), String> {
    std::fs::create_dir_all(paths.operations_dir()).map_err(|error| error.to_string())?;
    let content = serde_json::to_string_pretty(operation).map_err(|error| error.to_string())?;
    session_backup::durable_write_str(&operation_file(paths, &operation.operation_id), &content)
        .map_err(|error| format!("操作记录写入失败：{error}"))
}

/// 扫描全部操作日志（不区分档位）；解析失败的文件作为问题上报。
pub fn scan_operations(paths: &SessionPaths) -> OperationScan {
    let mut scan = OperationScan {
        operations: Vec::new(),
        problems: Vec::new(),
        complete: true,
    };
    let entries = match std::fs::read_dir(paths.operations_dir()) {
        Ok(entries) => entries,
        // 尚无任何操作日志：空结果且完整（首次使用）。
        Err(error) if error.kind() == ErrorKind::NotFound => return scan,
        Err(error) => {
            scan.complete = false;
            scan.problems.push(format!("操作日志目录不可读：{error}"));
            return scan;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                scan.complete = false;
                scan.problems.push(format!("操作日志目录枚举失败：{error}"));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<Operation>(&text).map_err(|error| error.to_string())
            }) {
            Ok(operation) => scan.operations.push(operation),
            Err(reason) => scan.problems.push(format!("{name}: {reason}")),
        }
    }
    scan.operations
        .sort_by_key(|operation| operation.created_at);
    scan
}

/// 收集某档位未完成的操作。
pub fn pending_operations(paths: &SessionPaths, variant: WbVariant) -> Vec<Operation> {
    scan_operations(paths)
        .operations
        .into_iter()
        .filter(|operation| operation.variant == variant && operation.phase.is_unfinished())
        .collect()
}

/// 查找与本次请求同一（来源会话 → 目标账号）的未完成操作，避免重复副本。
pub fn find_pending_operation<'a>(
    operations: &'a [Operation],
    source_uid: &str,
    source_session_id: &str,
    target_uid: &str,
) -> Option<&'a Operation> {
    operations.iter().find(|operation| {
        operation.phase.is_unfinished()
            && operation.source.uid == source_uid
            && operation.source.session_id == source_session_id
            && operation.target.uid == target_uid
    })
}

/// 清理已完成/已放弃的历史操作日志，每档位保留最近 `keep` 条。
///
/// 仍被维护记录引用的日志一律不裁剪：它们是补清理/补转的唯一依据（design §6）。
pub fn prune_operations(paths: &SessionPaths, variant: WbVariant, keep: usize) -> usize {
    let scan = scan_operations(paths);
    if !scan.complete {
        return 0;
    }
    // 维护记录扫描不完整（损坏/未知版本/归属不一致）时状态不明：一律不裁剪。
    let referenced = match session_backup::referenced_operation_ids(paths) {
        Ok(referenced) => referenced,
        Err(_) => return 0,
    };
    let mut finished: Vec<&Operation> = scan
        .operations
        .iter()
        .filter(|operation| {
            operation.variant == variant
                && !operation.phase.is_unfinished()
                && !referenced.contains(&operation.operation_id)
        })
        .collect();
    if finished.len() <= keep {
        return 0;
    }
    finished.sort_by_key(|operation| std::cmp::Reverse(operation.updated_at));
    let mut removed = 0usize;
    for operation in finished.into_iter().skip(keep) {
        if std::fs::remove_file(operation_file(paths, &operation.operation_id)).is_ok() {
            removed += 1;
        }
    }
    removed
}
