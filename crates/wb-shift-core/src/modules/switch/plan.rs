//! 切换前置：recovery 阻断判定/详情/JSON 与无重启写入拒绝。

use super::*;

/// 恢复报告里是否存在阻碍启动的问题：未恢复一致的中间产物必须先处理。
///
/// 拿不到档位锁、或中间产物被改动/丢失都属此类（design §4 / §5）：此时继续写认证并
/// 启动 App 会让 Official App 在最坏状态下打开会话，因此暂停切换与启动。
/// `retryable=true` 沿用复制侧「可延后重试且不阻断启动」的约定；同步恢复失败必须
/// 返回 false，即使故障只是暂时的，也不能带着半完成的正文/数据库/基线启动。
pub fn recovery_blocks_startup(report: &RecoveryReport) -> bool {
    report.needs_recovery.iter().any(|issue| !issue.retryable)
}

/// 阻碍启动的原因汇总（用于错误文案）。
///
/// 必须带操作标识：阻断分支只返回 Err 字符串，前端 catch 不能只看到原因。
pub fn recovery_blocking_detail(report: &RecoveryReport) -> String {
    report
        .needs_recovery
        .iter()
        .filter(|issue| !issue.retryable)
        .map(|issue| format!("{}：{}", issue.operation_id, issue.reason))
        .collect::<Vec<String>>()
        .join("；")
}

/// 恢复报告的前端投影（与复制/同步报告同形，便于展示恢复信息）。
pub fn recovery_report_json(report: &RecoveryReport) -> Value {
    json!({
        "recovered": report.recovered.len(),
        "abandoned": report.abandoned.len(),
        "needsRecovery": report
            .needs_recovery
            .iter()
            .map(|issue| json!({
                "operationId": issue.operation_id,
                "reason": issue.reason,
                "retryable": issue.retryable,
            }))
            .collect::<Vec<Value>>(),
        // 待清理/待恢复的临时备份残留：与复制/同步报告同一结构。
        "temporaryFiles": report.temporary_files,
    })
}

/// 某档位当前未完成操作的 id 集合。
///
/// 用于区分「本次新产生的未完成写入」与「切换开头已尝试恢复的历史残留」：
/// 后者若是可重试类（例如映射库暂不可用），按既有口径不阻断切换。
pub(super) fn pending_operation_ids(paths: &SessionPaths, variant: WbVariant) -> BTreeSet<String> {
    session_link::pending_operations(paths, variant)
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect()
}

/// 本次会话写入新留下的未完成操作（按创建顺序，便于稳定输出）。
pub(super) fn newly_unfinished_writes(
    before: &BTreeSet<String>,
    after: Vec<Operation>,
) -> Vec<Operation> {
    let mut created: Vec<Operation> = after
        .into_iter()
        .filter(|operation| !before.contains(&operation.operation_id))
        .collect();
    created.sort_by_key(|operation| operation.created_at);
    created
}

/// 未完成写入的说明文案：会话标识 + 操作标识 + 原因。
pub(super) fn unfinished_writes_detail(writes: &[Operation]) -> String {
    writes
        .iter()
        .map(|operation| {
            let reason = operation
                .last_error
                .clone()
                .unwrap_or_else(|| "尚未完成".to_string());
            format!(
                "{}（操作 {}）：{reason}",
                operation.target.session_id, operation.operation_id
            )
        })
        .collect::<Vec<String>>()
        .join("；")
}

/// restart=false 时携带写入意图：显式拒绝对应的会话操作（不静默丢弃，design §4.1）。
pub(super) fn reject_session_writes_without_restart(
    has_copy: bool,
    has_sync: bool,
) -> (Option<Value>, Option<Value>) {
    let copy = has_copy.then(|| json!({ "error": COPY_WITHOUT_RESTART_MESSAGE }));
    let sync = has_sync.then(|| {
        json!({
            "synced": [],
            "skipped": [],
            "errors": [{ "error": SYNC_WITHOUT_RESTART_MESSAGE }],
        })
    });
    (copy, sync)
}
