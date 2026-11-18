//! 工作区索引（`index.json`）的遍历与「索引可用性」四态判定。
//!
//! 插件的会话「行」就是工作区索引里的会话条目（正文按
//! `history/<workspaceHash>/<conversationId>/` 存放）：被删除或改归属后条目消失，与会话被
//! 软删除同语义。但「条目不在索引里」只有在**索引确实读得全**时才可信：
//!
//! - [`ConversationIndexState::Absent`]：历史目录不存在（账号从未用过 / 数据根不对）；
//! - [`ConversationIndexState::Unavailable`]：目录在，但一份 `index.json` 都没解析成功
//!   （`read_dir` 失败、JSON 损坏、`conversations` 字段缺失或类型不对、没有任何工作区子目录）；
//! - [`ConversationIndexState::Partial`]：只有部分工作区索引解析成功——`ids` 是可读分片的
//!   并集，损坏的分片里可能正装着要找的会话，因此「不在 `ids` 里」**不能**证明会话已消失；
//! - [`ConversationIndexState::Available`]：每一份工作区索引都解析成功，此时「不在 `ids` 里」
//!   才可信（`ids` 可以为空集，表示客户端确实没有会话）。
//!
//! 两种消费方对前三态的取舍相反：预览/列表 **fail-open**（没有完整依据就不参与过滤），
//! 批量清理 **fail-closed**（无法确认存活就拒绝执行，绝不「读不到就当全死」）。`Partial`
//! 在清理侧的落点是「能找到就活、找不到算未知」：不会凭损坏的分片删组，也不会因为一处
//! 损坏就永久挡住全部清理。
//!
//! 四态与定位表来自**同一次遍历**（[`conversation_index_scan`]）：判定会话存活时每多扫一遍
//! 目录就要多读几十份 `index.json`，所以状态与 `会话 id → 定位` 一次算清、各自取用。
//!
//! 索引条目**不是**唯一的存活证据：会话正文按 `history/<workspaceHash>/<conversationId>/`
//! 存放，条目可能因为 `index.json` 损坏、半截写入或客户端改归属而缺失，但目录还在就说明
//! 会话确实存在。因此这次遍历同时收集「该账号磁盘上存在的会话目录名集合」
//! （[`ConversationIndexScan::session_dirs`]），存活判定的优先级是
//! **索引命中 → 目录存在 → 索引未命中且目录也不存在**（见 [`super::liveness`]）。
//!
//! 目录集合必须在**同一次遍历**里顺手收集（进每个工作区子目录时 `read_dir` 一次），
//! **不得**为每个待判定的会话单独 `is_dir()`：管理页一次可能判几十上百个会话。

use super::*;

/// 会话在客户端数据里的定位信息：所属工作区 hash 与展示标题。
#[derive(Debug, Clone)]
pub(super) struct ConversationLocator {
    pub(super) workspace_hash: String,
    pub(super) title: String,
}

/// 工作区索引的可用性四态（见模块注释）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationIndexState {
    /// 历史目录不存在：账号从未用过该客户端（或数据根不对）→ 没有判定依据。
    Absent,
    /// 目录存在但没有任何一份 `index.json` 解析成功 → 没有判定依据。
    Unavailable,
    /// 只有部分工作区索引解析成功：`ids` 是**可读分片**里的会话，损坏分片里可能还有别的。
    ///
    /// 「不在 `ids` 里」只证明「不在可读的那几份索引里」，不能证明会话已消失，所以
    /// 读路径按无依据处理（fail-open），写路径不得据此删组（fail-closed）。
    Partial(BTreeSet<String>),
    /// 每一份工作区索引都解析成功：`ids` 为该账号当前真实持有的会话 id（**可能为空集**）。
    Available(BTreeSet<String>),
}

/// 一次工作区索引遍历的完整结果：定位表 + 可用性原料 + 目录证据。
#[derive(Debug, Clone, Default)]
pub(super) struct ConversationIndexScan {
    /// 历史根是否为目录（`false` → [`ConversationIndexState::Absent`]）。
    pub(super) history_present: bool,
    /// **含 `index.json` 的工作区目录数**（[`Self::state`] 的分母）。
    ///
    /// 只统计真的有索引文件的工作区：目录存在但没有 `index.json` 通常是客户端正常状态
    /// （空工作区 / 尚未写入），把它算进分母会让 `parsed < 分母` 永远成立，该客户端清理
    /// 被永久判成 `Partial` 而拒绝。会话真的存在时由 [`Self::session_dirs`] 兜底，
    /// 所以这个放宽不会导致误删。
    pub(super) index_workspaces: usize,
    /// 成功解析出 `conversations` 数组的 `index.json` 份数（`0` → `Unavailable`）。
    pub(super) parsed: usize,
    /// `会话 id → 定位信息`。
    pub(super) locators: BTreeMap<String, ConversationLocator>,
    /// 磁盘上**确实存在**的会话目录名（`history/<workspaceHash>/<conversationId>/`）。
    ///
    /// 与 [`Self::locators`] 同一次遍历收集（进每个工作区目录时 `read_dir` 一次）：
    /// 索引条目缺失但目录还在 → 会话确实存在。收集失败（权限、并发删除）只会让集合变小，
    /// 判定退化为「只有索引证据」，不会误报会话存在。
    pub(super) session_dirs: BTreeSet<String>,
}

impl ConversationIndexScan {
    /// 本次遍历对应的四态（`Absent` → `Unavailable` → `Partial` → `Available` 优先级固定）。
    pub(super) fn state(&self) -> ConversationIndexState {
        if !self.history_present {
            return ConversationIndexState::Absent;
        }
        if self.parsed == 0 {
            return ConversationIndexState::Unavailable;
        }
        let ids: BTreeSet<String> = self.locators.keys().cloned().collect();
        if self.parsed < self.index_workspaces {
            // 有索引文件却没读到：`ids` 只是可读分片的并集，不能当作「该账号的全部会话」。
            return ConversationIndexState::Partial(ids);
        }
        ConversationIndexState::Available(ids)
    }

    /// 该账号是否**确实持有**这个会话：索引条目命中（含 `Partial` 下的可读分片）或
    /// `history/<workspaceHash>/<conversationId>/` 目录存在。
    ///
    /// 这是「会话存在」的正向证据，与索引四态无关：四态只决定「未命中」能不能当作消失。
    pub(super) fn holds(&self, session_id: &str) -> bool {
        self.locators.contains_key(session_id) || self.session_dirs.contains(session_id)
    }
}

// ---------------------------------------------------------------------------
// 定位（按 md5(工作区) 分桶，会话 id 不含工作区信息 → 反查一次索引）
// ---------------------------------------------------------------------------

/// 扫某账号的工作区索引，得到「定位表 + 可用性原料 + 会话目录证据」（单次遍历）。
///
/// 客户端把会话按 `md5(工作区)` 分桶，会话 id 本身无法还原工作区；这里按工作区目录名
/// 排序处理，同名会话（不该出现）取字典序最小者，保证结果与目录遍历顺序无关。
pub(super) fn conversation_index_scan(
    spec: SessionStoreSpec,
    root: &Path,
    uid: &str,
) -> ConversationIndexScan {
    let mut scan = ConversationIndexScan::default();
    let history = vscode_session::history_root_in(spec, root, uid);
    if !history.is_dir() {
        // 账号从未用过该客户端 / 数据根不对：既没有定位表，也没有判定依据。
        return scan;
    }
    scan.history_present = true;
    // `read_dir` 失败（权限、并发删除）时保持 `parsed = 0`：目录在但读不到 → Unavailable。
    let Ok(entries) = std::fs::read_dir(&history) else {
        return scan;
    };
    let workspaces: BTreeSet<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    for ws_dir in workspaces {
        // 目录证据与索引解析共用这一次遍历：会话条目可能缺失，目录还在就说明会话存在。
        collect_session_dirs(&ws_dir, &mut scan.session_dirs);
        let index_path = ws_dir.join("index.json");
        // **分母只统计含 `index.json` 的工作区**：目录在却没有索引文件，通常是客户端正常
        // 状态（空工作区 / 尚未写入）。把它算进分母会让 `parsed < 分母` 永远成立，该客户端
        // 的清理被永久判成 `Partial` 而整次拒绝；会话真的存在时目录证据会兜底，放宽安全。
        if !index_path.is_file() {
            continue;
        }
        scan.index_workspaces += 1;
        let Some(index) = vscode_session::read_json(&index_path) else {
            // 有索引文件却解析不出来：**计入分母**、不计 `parsed` → `Partial`（不得当成可读）。
            continue;
        };
        let Some(conversations) = index.get("conversations").and_then(Value::as_array) else {
            continue;
        };
        // 只有「读到并解析成数组」才算这份索引用得上：空数组也算（客户端确实没有会话）。
        scan.parsed += 1;
        let workspace_hash = ws_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        for conversation in conversations {
            let Some(id) = conversation
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let title = conversation
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("(无标题)")
                .to_string();
            scan.locators
                .entry(id.to_string())
                .or_insert_with(|| ConversationLocator {
                    workspace_hash: workspace_hash.clone(),
                    title,
                });
        }
    }
    scan
}

/// 收集某工作区目录下的会话子目录名（`index.json` 等普通文件会被跳过）。
///
/// 普通目录项用 `DirEntry::file_type()` 判定（能拿到目录项类型时不额外发起系统调用）；
/// **符号链接 / junction 不跟随**，只对这类目录项补一次跟随链接的 `path().is_dir()`
/// （每个链接一个 stat）。漏掉链接会让「目录证据」凭空消失：索引同时丢了条目时就误判
/// [`super::SessionLiveness::Gone`]，那是会真删组的错误方向。
fn collect_session_dirs(workspace_dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(workspace_dir) else {
        // 权限 / 并发删除：收集不到就当没有目录证据，索引四态仍然照原样判定。
        return;
    };
    for entry in entries.flatten() {
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_dir() => true,
            // 链接可能指向真实会话目录：跟随一次再判。
            Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
            _ => false,
        };
        if !is_dir {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.trim().is_empty() {
            out.insert(name.to_string());
        }
    }
}

/// 扫某账号的工作区索引，得到 `会话 id → 定位信息`。
///
/// 与 [`conversation_index_scan`] 是同一份遍历（这里只取定位表）：不存在第二份遍历逻辑，
/// 因此「哪份索引算读到了」「哪些会话目录在」的判断永远一致。存活判定见
/// [`super::liveness`]。
pub(super) fn conversation_index(
    spec: SessionStoreSpec,
    root: &Path,
    uid: &str,
) -> BTreeMap<String, ConversationLocator> {
    conversation_index_scan(spec, root, uid).locators
}

/// 某账号某会话的目录：`history/<workspaceHash>/<conversationId>`。
pub(super) fn conversation_dir(
    spec: SessionStoreSpec,
    root: &Path,
    uid: &str,
    workspace_hash: &str,
    conversation_id: &str,
) -> PathBuf {
    vscode_session::history_root_in(spec, root, uid)
        .join(workspace_hash)
        .join(conversation_id)
}
