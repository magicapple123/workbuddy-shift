//! 把「工作记忆」同步到目标账号（issue #209）。
//!
//! 客户端的工作记忆是**每个账号一份 Markdown**：`<数据根>/user-<uid>-personal/MEMORY.md`。
//! 注意它与置顶 / 偏好不同——不在 `storage/` 下，而是直接挂在数据根的账号目录里
//! （`WbVariant::data_root()` 下即 `.workbuddy` / `.workbuddy-ai`）。跨账号复制会话时，
//! 源账号长期积累的偏好在目标账号里完全没有，新账号的对话要从零摸索。
//!
//! 三条不可让步的约束：
//!
//! - **只追加，不覆盖**：`MEMORY.md` 是用户自己写的笔记，目标账号可能已经写了自己的偏好。
//!   按行去重后只把源里目标缺失的行按源顺序追加到末尾；不改写已有行、不重排、不删除。
//! - **失败不阻塞**：工作记忆是增强数据。任何异常只落报告字段（`skippedReason`），绝不
//!   影响正文 / DB / 复制结果；重跑天然幂等。
//! - **原子写 + 写前备份**：目标是客户端正在读的 Markdown，半截写入会丢记忆。
//!
//! 与 `pinned.rs` 的差异只有一处：目标文件**不存在**时可以创建（账号目录存在即可）。
//! 那份只有追加内容的文件语义完整，不会像 `conversations.json` 那样造出结构未知的数据。

use std::collections::HashSet;

use super::*;

/// 写前备份的相对目录：`<工具存储根>/backups/memory/<utc_iso>/`。
const BACKUP_KIND: &str = "memory";

/// 账号工作记忆的固定文件名（客户端按此名字读取，不可改）。
const FILE_NAME: &str = "MEMORY.md";

/// 一次工作记忆同步的结果。只用于报告，不参与任何写入判定。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(in crate::modules::session) struct MemoryOutcome {
    /// 本次追加到目标账号的行数（`0` 表示无需同步或已是最新）。
    pub(in crate::modules::session) appended_lines: usize,
    /// 非致命失败原因；`None` 表示正常（含「无需同步」）。
    pub(in crate::modules::session) skipped_reason: Option<String>,
}

impl MemoryOutcome {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            appended_lines: 0,
            skipped_reason: Some(reason.into()),
        }
    }

    /// 报告字段：`{"appendedLines": n}`，失败时追加 `skippedReason`。
    pub(in crate::modules::session) fn to_json(&self) -> Value {
        let mut value = json!({ "appendedLines": self.appended_lines });
        if let Some(reason) = &self.skipped_reason {
            value["skippedReason"] = json!(reason);
        }
        value
    }
}

/// 账号数据目录（`<数据根>/user-<uid>-personal`）：工作记忆与客户端私有数据都在这里。
fn account_dir(paths: &SessionPaths, uid: &str) -> PathBuf {
    paths.data_root.join(format!("user-{uid}-personal"))
}

/// 账号工作记忆文件位置。
fn memory_file(paths: &SessionPaths, uid: &str) -> PathBuf {
    account_dir(paths, uid).join(FILE_NAME)
}

/// 追加时使用的换行符：沿用目标文件已有的口径（CRLF 文件里不掺 LF）。
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 纯计算：源里「目标还没有」的非空行，按源顺序返回（含源内部重复行的去重）。
///
/// 比较口径是**去掉首尾空白后逐字相等**：仅差行尾空白的行视为同一行，不重复追加；
/// 追加内容保留源行原样（不去改写用户写下的文本）。
fn missing_lines(source: &str, target: &str) -> Vec<String> {
    let mut seen: HashSet<String> = target
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    let mut missing: Vec<String> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 空行不参与去重也不搬运：追加空行只会让目标文件多出无意义的空白。
        if seen.insert(trimmed.to_string()) {
            missing.push(line.to_string());
        }
    }
    missing
}

/// 把目标文件原字节备份到 `backup_root/<uid>-MEMORY.md`。
///
/// 备份失败即放弃本次写入：宁可少同步一次记忆，也不能在没有回滚副本的情况下改
/// 客户端正在读的文件。
fn backup_target(target_file: &Path, uid: &str, backup_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(backup_root)
        .map_err(|error| format!("工作记忆备份目录创建失败：{error}"))?;
    let destination = backup_root.join(format!("{uid}-{FILE_NAME}"));
    std::fs::copy(target_file, &destination)
        .map_err(|error| format!("工作记忆备份失败：{error}"))?;
    Ok(())
}

/// 把源账号的工作记忆单向追加到目标账号（只追加、幂等、写前备份）。
///
/// 由用户按操作勾选（复制弹窗里的「同时复制工作记忆」）：未勾选时调用方**完全不调用**
/// 本函数，连源文件都不读。
pub(in crate::modules::session) fn sync_memory(
    source_paths: &SessionPaths,
    source_uid: &str,
    target_paths: &SessionPaths,
    target_uid: &str,
) -> MemoryOutcome {
    let backup_root = crate::modules::config::backup_dir()
        .join(BACKUP_KIND)
        .join(crate::modules::config::utc_iso());
    sync_memory_with_backup_root(
        source_paths,
        source_uid,
        target_paths,
        target_uid,
        &backup_root,
    )
}

/// [`sync_memory`] 的可注入备份根版本（单测用临时目录，绝不写真实 `~/.workbuddy-shift`）。
pub(in crate::modules::session) fn sync_memory_with_backup_root(
    source_paths: &SessionPaths,
    source_uid: &str,
    target_paths: &SessionPaths,
    target_uid: &str,
    backup_root: &Path,
) -> MemoryOutcome {
    if source_uid.is_empty() || target_uid.is_empty() {
        return MemoryOutcome::default();
    }
    let source_file = memory_file(source_paths, source_uid);
    if !source_file.exists() {
        return MemoryOutcome::default();
    }
    if !source_file.is_file() {
        return MemoryOutcome::skipped(format!(
            "源账号工作记忆不是普通文件：{}",
            source_file.display()
        ));
    }
    // 目标账号目录不存在 = 客户端从没为该账号落过数据：不凭空造出账号目录。
    let target_dir = account_dir(target_paths, target_uid);
    if !target_dir.is_dir() {
        return MemoryOutcome::default();
    }
    let source_text = match std::fs::read_to_string(&source_file) {
        Ok(text) => text,
        Err(error) => return MemoryOutcome::skipped(format!("源账号工作记忆读取失败：{error}")),
    };
    // 源文件为空（只有空白）：没有可搬运的内容，按 no-op 处理，不报错。
    if source_text.trim().is_empty() {
        return MemoryOutcome::default();
    }
    let target_file = target_dir.join(FILE_NAME);
    if target_file.exists() && !target_file.is_file() {
        return MemoryOutcome::skipped(format!(
            "目标账号工作记忆不是普通文件：{}",
            target_file.display()
        ));
    }
    // 目标文件可以不存在：账号目录在就是合法账号，创建一份只有追加内容的文件即可。
    let has_target_file = target_file.is_file();
    let target_text = if has_target_file {
        match std::fs::read_to_string(&target_file) {
            Ok(text) => text,
            Err(error) => {
                return MemoryOutcome::skipped(format!("目标账号工作记忆读取失败：{error}"));
            }
        }
    } else {
        String::new()
    };
    let missing = missing_lines(&source_text, &target_text);
    if missing.is_empty() {
        return MemoryOutcome::default();
    }
    // 目标不存在时没有可备份的原字节：新文件直接创建，不做空备份。
    if has_target_file {
        if let Err(reason) = backup_target(&target_file, target_uid, backup_root) {
            return MemoryOutcome::skipped(reason);
        }
    }
    let ending = line_ending(&target_text);
    let mut content = target_text;
    // 目标原有内容逐字保留，只在末尾补分隔：用户的行不能因为追加而变形。
    if !content.is_empty() && !content.ends_with('\n') {
        content.push_str(ending);
    }
    for line in &missing {
        content.push_str(line);
        content.push_str(ending);
    }
    if let Err(error) = session_backup::durable_write_str(&target_file, &content) {
        return MemoryOutcome::skipped(format!("工作记忆写入失败：{error}"));
    }
    MemoryOutcome {
        appended_lines: missing.len(),
        skipped_reason: None,
    }
}
