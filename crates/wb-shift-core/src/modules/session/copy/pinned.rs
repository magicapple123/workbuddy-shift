//! 把「会话置顶」同步到目标账号（issue #205）。
//!
//! 客户端的置顶状态**不在** `sessions` 表里，而是按账号存在
//! `<数据根>/storage/user-<uid>-personal/global/conversations.json` 的 `pinned` 数组
//! （每条为 `{"id": "<session_id>", "groupKey": "<工作目录>"}`）。跨账号复制会生成新的
//! session id，若不同步这份文件，源账号置顶的高频会话在目标账号里就散回普通列表，
//! 每次切号都要手工重新置顶。
//!
//! 三条不可让步的约束：
//!
//! - **单向 add**：只补目标缺失的条目，不镜像删除。目标账号可能自己置顶过别的会话，
//!   来源侧取消置顶不应反向强加给目标侧（与归档状态「单向粘滞、不反向传导」同口径）。
//! - **失败不阻塞**：置顶是增强数据。任何异常只落报告字段（`skippedReason`），绝不
//!   影响正文 / DB / 基线等关键路径；重跑天然幂等。
//! - **原子写 + 写前备份**：目标是客户端正在使用的 JSON，半截写入会让它读不出置顶。

use std::collections::{HashMap, HashSet};

use super::*;

/// 写前备份的相对目录：`<工具存储根>/backups/pinned/<utc_iso>/`。
const BACKUP_KIND: &str = "pinned";

/// 同一毫秒内为同一 uid 写备份时最多避让多少个序号（正常路径第 0 个就成功）。
const MAX_BACKUP_SEQUENCE: u32 = 1000;

/// 一次置顶同步的结果。只用于报告，不参与任何写入判定。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(in crate::modules::session) struct PinnedOutcome {
    /// 本次追加到目标账号的置顶条目数。
    pub(in crate::modules::session) added: usize,
    /// 非致命失败原因；`None` 表示正常（含「无需同步」）。
    pub(in crate::modules::session) skipped_reason: Option<String>,
}

impl PinnedOutcome {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            added: 0,
            skipped_reason: Some(reason.into()),
        }
    }

    /// 报告字段：`{"added": n}`，失败时追加 `skippedReason`。
    pub(in crate::modules::session) fn to_json(&self) -> Value {
        let mut value = json!({ "added": self.added });
        if let Some(reason) = &self.skipped_reason {
            value["skippedReason"] = json!(reason);
        }
        value
    }
}

/// 账号的置顶文件位置。
fn conversations_file(paths: &SessionPaths, uid: &str) -> PathBuf {
    paths
        .data_root
        .join("storage")
        .join(format!("user-{uid}-personal"))
        .join("global")
        .join("conversations.json")
}

/// 纯合并：按源 `pinned` 顺序，把「映射后目标 id 尚不存在」的条目追加到目标数组尾部。
///
/// - 目标已有的条目及其顺序**原样保留**（只追加，不重排）；
/// - 条目克隆源对象、仅替换 `id`，`groupKey` 等未知字段原样带过去；
/// - 幂等：同一映射重复执行不会重复追加。
///
/// 结构异常返回 `Err(原因)`：`added = 0` 且没有任何原因会让用户以为「已经同步过了」，
/// 而实际是文件结构不是我们预期的形状（客户端版本变化 / 手工改坏）。
fn merge_pinned(
    source_pinned: &[Value],
    target: &mut Value,
    mapping: &HashMap<&str, &str>,
) -> Result<usize, String> {
    let Some(object) = target.as_object_mut() else {
        return Err("目标账号置顶记录结构异常（顶层不是对象）".to_string());
    };
    let list = object.entry("pinned").or_insert_with(|| json!([]));
    let Some(list) = list.as_array_mut() else {
        return Err("目标账号置顶记录结构异常（pinned 不是数组）".to_string());
    };
    let mut existing: HashSet<String> = list
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str).map(String::from))
        .collect();
    let mut added = 0;
    for entry in source_pinned {
        let Some(source_id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(target_id) = mapping.get(source_id) else {
            continue;
        };
        if !existing.insert((*target_id).to_string()) {
            continue;
        }
        let mut cloned = entry.clone();
        if let Some(cloned) = cloned.as_object_mut() {
            cloned.insert("id".to_string(), json!(target_id));
        }
        list.push(cloned);
        added += 1;
    }
    Ok(added)
}

/// 把目标文件原字节备份到
/// `backup_root/<uid>-conversations-<毫秒时间戳>-<序号>.json`。
///
/// 备份失败即放弃本次写入：宁可少同步一次置顶，也不能在没有回滚副本的情况下改
/// 客户端正在使用的文件。
///
/// 文件名必须能区分**同一秒内的多次写入**：`backup_root` 的日期层是秒级 `utc_iso()`，
/// 同一次切换里同一账号可能被写两次（多个目标成员 / 组内多轮传导），用固定名字会把
/// 前一份备份覆盖成中间态，回滚时就只剩一个「看起来正常」的坏副本。
fn backup_target(target_file: &Path, uid: &str, backup_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(backup_root)
        .map_err(|error| format!("置顶备份目录创建失败：{error}"))?;
    let stamp = crate::modules::config::now_ms();
    // 毫秒也可能撞（同一次调用里连写两次是毫秒级）：再按序号避让到第一个不存在的名字。
    // 序号固定 4 位补零，保证文件名排序 = 写入顺序。绝不复用已有文件名 ——
    // 覆盖备份等于丢弃回滚点。
    for sequence in 0..MAX_BACKUP_SEQUENCE {
        let destination =
            backup_root.join(format!("{uid}-conversations-{stamp}-{sequence:04}.json"));
        if destination.exists() {
            continue;
        }
        std::fs::copy(target_file, &destination)
            .map_err(|error| format!("置顶备份失败：{error}"))?;
        return Ok(());
    }
    Err(format!(
        "置顶备份失败：{uid} 在 {stamp} 内的备份文件序号已用尽（{MAX_BACKUP_SEQUENCE} 个）"
    ))
}

/// 同步路径：把源成员的置顶传导给同组其它成员（单向 add）。
///
/// 关联组里每个成员是不同账号上的副本、各有自己的 session id，所以不能复用复制路径的
/// `pairs`——要按组内成员关系逐个建映射。跨档成员按各自 variant 解析数据根；目标账号
/// 没有置顶文件时该成员自然 no-op，不影响其它成员。
pub(in crate::modules::session) fn sync_pinned_for_groups(
    store: &crate::modules::session_link::LinkStore,
    source_uid: &str,
    group_ids: &[String],
) -> PinnedOutcome {
    let backup_root = crate::modules::config::backup_dir()
        .join(BACKUP_KIND)
        .join(crate::modules::config::utc_iso());
    sync_pinned_for_groups_with(
        store,
        source_uid,
        group_ids,
        &backup_root,
        SessionPaths::for_variant,
    )
}

/// [`sync_pinned_for_groups`] 的可注入版本：备份根与「档位 → 数据根」都可替换。
///
/// 生产传 [`SessionPaths::for_variant`]；单测传临时目录，绝不读写真实账号数据。
pub(in crate::modules::session) fn sync_pinned_for_groups_with(
    store: &crate::modules::session_link::LinkStore,
    source_uid: &str,
    group_ids: &[String],
    backup_root: &Path,
    resolve_paths: impl Fn(WbVariant) -> SessionPaths,
) -> PinnedOutcome {
    use crate::modules::session_link::{member_variant, MemberState};

    let mut outcome = PinnedOutcome::default();
    for group_id in group_ids {
        let Some(group) = store.groups.iter().find(|group| &group.id == group_id) else {
            continue;
        };
        // 只认「当前接力成员」：stale / superseded 的副本已经不参与同步，给它们置顶
        // 只会在账号列表里留下一条指向废弃会话的条目。
        let Some(source) = group
            .members
            .iter()
            .find(|member| member.uid == source_uid && member.state == MemberState::Active)
        else {
            continue;
        };
        let source_paths = resolve_paths(member_variant(group, source));
        for member in &group.members {
            if member.uid == source_uid || member.state != MemberState::Active {
                continue;
            }
            let target_paths = resolve_paths(member_variant(group, member));
            let pairs = [(source.session_id.clone(), member.session_id.clone())];
            let one = sync_pinned_with_backup_root(
                &source_paths,
                source_uid,
                &target_paths,
                &member.uid,
                &pairs,
                backup_root,
            );
            outcome.added += one.added;
            if outcome.skipped_reason.is_none() {
                outcome.skipped_reason = one.skipped_reason;
            }
        }
    }
    outcome
}

/// 同步置顶状态：`pairs` 是 `(源 session id, 目标 session id)`。
///
/// 源文件缺失、目标文件缺失、`pinned` 为空、映射为空都按 no-op 处理——目标文件由
/// 客户端自己创建，我们不凭空造一个结构未知的 `conversations.json`。
pub(in crate::modules::session) fn sync_pinned(
    source_paths: &SessionPaths,
    source_uid: &str,
    target_paths: &SessionPaths,
    target_uid: &str,
    pairs: &[(String, String)],
) -> PinnedOutcome {
    let backup_root = crate::modules::config::backup_dir()
        .join(BACKUP_KIND)
        .join(crate::modules::config::utc_iso());
    sync_pinned_with_backup_root(
        source_paths,
        source_uid,
        target_paths,
        target_uid,
        pairs,
        &backup_root,
    )
}

/// [`sync_pinned`] 的可注入备份根版本（单测用临时目录，绝不写真实 `~/.workbuddy-shift`）。
pub(in crate::modules::session) fn sync_pinned_with_backup_root(
    source_paths: &SessionPaths,
    source_uid: &str,
    target_paths: &SessionPaths,
    target_uid: &str,
    pairs: &[(String, String)],
    backup_root: &Path,
) -> PinnedOutcome {
    if pairs.is_empty() || source_uid.is_empty() || target_uid.is_empty() {
        return PinnedOutcome::default();
    }
    let source_file = conversations_file(source_paths, source_uid);
    let target_file = conversations_file(target_paths, target_uid);
    if !source_file.is_file() || !target_file.is_file() {
        return PinnedOutcome::default();
    }
    let source_text = match std::fs::read_to_string(&source_file) {
        Ok(text) => text,
        Err(error) => return PinnedOutcome::skipped(format!("源账号置顶记录读取失败：{error}")),
    };
    let source_json: Value = match serde_json::from_str(&source_text) {
        Ok(value) => value,
        Err(error) => return PinnedOutcome::skipped(format!("源账号置顶记录无法解析：{error}")),
    };
    let source_pinned: Vec<Value> = source_json
        .get("pinned")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if source_pinned.is_empty() {
        return PinnedOutcome::default();
    }
    let target_text = match std::fs::read_to_string(&target_file) {
        Ok(text) => text,
        Err(error) => return PinnedOutcome::skipped(format!("目标账号置顶记录读取失败：{error}")),
    };
    let mut target_json: Value = match serde_json::from_str(&target_text) {
        Ok(value) => value,
        Err(error) => return PinnedOutcome::skipped(format!("目标账号置顶记录无法解析：{error}")),
    };
    let mapping: HashMap<&str, &str> = pairs
        .iter()
        .map(|(source, target)| (source.as_str(), target.as_str()))
        .collect();
    // 结构异常（顶层不是对象 / `pinned` 不是数组）一律报原因且**不写文件**：
    // `added = 0` 却不说话，用户会把它读成「已经同步过了」。
    let added = match merge_pinned(&source_pinned, &mut target_json, &mapping) {
        Ok(added) => added,
        Err(reason) => return PinnedOutcome::skipped(reason),
    };
    if added == 0 {
        return PinnedOutcome::default();
    }
    if let Err(reason) = backup_target(&target_file, target_uid, backup_root) {
        return PinnedOutcome::skipped(reason);
    }
    let text = match serde_json::to_string_pretty(&target_json) {
        Ok(text) => text,
        Err(error) => return PinnedOutcome::skipped(format!("置顶记录序列化失败：{error}")),
    };
    if let Err(error) = session_backup::durable_write_str(&target_file, &text) {
        return PinnedOutcome::skipped(format!("置顶记录写入失败：{error}"));
    }
    PinnedOutcome {
        added,
        skipped_reason: None,
    }
}
