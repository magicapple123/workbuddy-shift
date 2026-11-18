//! 备份与回滚：备份根、索引备份、回滚与目录恢复。

use super::*;

// --- 快进（design §6.1） ---------------------------------------------------

/// 备份根目录：`<工具存储根>/backups/<backup_kind>/<utc_iso>/`（与复制路径同布局）。
///
/// 从 [`SessionPaths::backup_root`] 派生（而不是直接取 [`backup_dir`]），
/// 单测才能注入临时存储根而不污染真实 `~/.workbuddy-shift`。
pub(super) fn backup_root(paths: &SessionPaths, backup_kind: &str) -> PathBuf {
    paths.backup_root().join(backup_kind).join(utc_iso())
}

/// 备份目标会话索引，返回备份目录。
pub(super) fn begin_index_backup(
    paths: &SessionPaths,
    plan: &SyncItemPlan,
) -> Result<PathBuf, String> {
    let dir = backup_root(paths, plan.backup_kind)
        .join(&plan.workspace_hash)
        .join(&plan.target_conversation_id);
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建索引备份目录失败：{error}"))?;
    for name in ["index.json", ".index_bak.json"] {
        let from = plan.target_dir.join(name);
        if from.is_file() {
            std::fs::copy(&from, dir.join(name))
                .map_err(|error| format!("备份目标索引失败（{name}）：{error}"))?;
        }
    }
    Ok(dir)
}

/// 回滚快进：索引按备份恢复、删除本次新建的消息文件。
///
/// 失败必须**上报**而不是丢弃：索引没恢复、新增文件没删掉，调用方都不能对用户声称
/// 「已同步内容已回滚」。返回的失败说明自带备份路径，用户可据此手工恢复。
/// 成败口径与原来一致（备份存在则 `copy` 回、不存在则删目标），只是不再吞错：
/// `NotFound` 视为成功（幂等删除，文件本就不在），只有真实失败才计入。
///
/// 本次追加的附件**不回滚**：它们是纯增量，且同名覆盖前目标侧的原始副本仍在源账号。
pub(crate) fn rollback_index(
    plan: &SyncItemPlan,
    backup_dir: &Path,
    created: &[PathBuf],
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    for name in ["index.json", ".index_bak.json"] {
        let backup = backup_dir.join(name);
        let target = plan.target_dir.join(name);
        if backup.is_file() {
            if let Err(error) = std::fs::copy(&backup, &target) {
                failures.push(format!("索引 {name} 未恢复：{error}"));
            }
        } else if let Err(error) = remove_file_if_exists(&target) {
            failures.push(format!("索引 {name} 未删除：{error}"));
        }
    }
    // 新增消息可能上百条：逐条列名会把用户可见文案撑爆，这里只报失败条数。
    let undeleted = created
        .iter()
        .filter(|path| remove_file_if_exists(path).is_err())
        .count();
    if undeleted > 0 {
        failures.push(format!("本次新增的 {undeleted} 个消息文件未删除"));
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!(
        "副本回滚未完成（{}），请从备份目录 {} 手工恢复",
        failures.join("；"),
        backup_dir.display()
    ))
}

/// 从备份目录整目录恢复目标会话目录。
///
/// 失败必须**上报**而不是丢弃：目标目录已先被清空，`copy_dir_recursive` 又会在读源之前
/// 先建出目标目录，恢复失败意味着副本只剩一个空目录（或半成品），调用方绝不能对用户
/// 声称「已回滚」。失败文案自带备份路径，用户可据此手工恢复。
pub(crate) fn restore_dir(backup_dir: &Path, target_dir: &Path) -> Result<(), String> {
    vscode_session::remove_dir_all_if_exists(target_dir);
    vscode_session::copy_dir_recursive(backup_dir, target_dir).map_err(|error| {
        format!(
            "副本目录未恢复（{error}），请从备份目录 {} 手工恢复",
            backup_dir.display()
        )
    })
}

/// 回滚后的用户可见文案：回滚成功时与原来的文案逐字一致；回滚失败时追加
/// [`restore_dir`] / [`rollback_index`] 的说明（未回滚 + 备份路径），不掩盖失败。
pub(super) fn rollback_message(restored: Result<(), String>, message: &str) -> String {
    match restored {
        Ok(()) => message.to_string(),
        Err(error) => format!("{message}；且{error}"),
    }
}
