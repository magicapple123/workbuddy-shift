//! 复制前的内容级去重：目标账号已有「同标题 + 同工作目录 + 同正文」的会话时直接复用
//! （issue #22「同步时相同会话没有跳过」）。
//!
//! 为什么需要这一步：`resolve_target` 只在**关联主表**里找组，找到组才算「已关联」。
//! 用户在客户端里手动同步过、或用历史版本重复复制过时，目标账号里可能存在一份内容
//! 完全相同、但从未建立关联的会话；只看关联表就会再写一份，于是每操作一次多一条同名
//! 同内容的会话。这里补上「按内容找」的入口：命中即登记为成员并返回「已关联」。
//!
//! 判定口径（宁可漏判，不可错判）——三条全满足才算同一条：
//! 1. 展示标题相同且非占位（客户端 `（无标题）` 不参与比较）；
//! 2. `cwd` 相同（客户端按 `cwd` 归项目，跨目录的同名会话不是同一条）；
//! 3. 正文的**比较口径**摘要相同（[`comparable_digest`]）。
//!
//! 为什么不用 `full_digest`（原始字节摘要）：正文每一行都带 `sessionId`，副本必然改写
//! 成自己的 id，原始字节摘要必然不同——拿它比较会把所有真实副本判成「不同」。正确
//! 口径是 `session_link::normalize_jsonl` 归一化后的摘要（把**本副本的 id** 换成固定
//! 标记再算），它因此与具体 id 无关；再用 `comparable_line_digests` 把「复制/同步写目标
//! 时对末条 assistant 消息做的 `incomplete` 归一留痕」折算成等价形式（与
//! [`session_link::decide_sync`] 同一口径）。这一份才是能跨副本对得上的内容身份。
//!
//! 边界：已属于**任何**关联组的会话不参与匹配。关联主表的身份唯一性不变量要求
//! (variant, uid, sessionId) 只属于一个组，把一个已在别的组里的会话再拉进本组会让
//! 保存被校验拒绝；这种「同内容但已被别的来源占用」的候选一律跳过（宁可多复制一份，
//! 也不破坏关联表一致性）。

use super::plan::CopyContext;
use super::*;
use std::cell::RefCell;
use std::path::PathBuf;

/// 展示标题为空时的占位（与 `session_display_title` / 存储侧的 `display_title` 同一文案）。
const NO_TITLE_PLACEHOLDER: &str = "(无标题)";

/// 索引里的一行：会话 id、展示标题、工作目录。
type SessionRow = (String, String, String);

/// 索引键：数据根 + 账号 uid（同一数据根下不同账号各有一份列表）。
type IndexKey = (PathBuf, String);

/// 一批复制内复用的会话行索引。
///
/// 每复制一条会话都要查「源账号 / 目标账号有哪些会话」，而 `list_sessions_for_user_at`
/// 每次都要打开数据库并扫一遍 `projects/`。整批复制（切号时勾选几十条）会因此重复上百次；
/// 这里按 `(data_root, uid)` 缓存：同档复制时源与目标共用同一个数据根，整批只扫一次。
#[derive(Default)]
pub(in crate::modules::session) struct SessionRowIndex {
    entries: RefCell<Vec<(IndexKey, Vec<SessionRow>)>>,
}

impl SessionRowIndex {
    /// 某数据根下某账号的会话行（首次调用时扫描，之后复用同一份结果）。
    ///
    /// 返回克隆而不是借用：`CopyContext` 以 `&` 共享，返回内部引用会让调用期间一直持有
    /// `RefCell` 借用；几十行的克隆代价远小于一次数据库打开。
    fn rows(&self, paths: &SessionPaths, uid: &str) -> Vec<SessionRow> {
        let key = (paths.data_root.clone(), uid.to_string());
        if let Some(found) = self
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.0 == key)
            .map(|entry| entry.1.clone())
        {
            return found;
        }
        let rows = read_rows(paths, uid);
        self.entries.borrow_mut().push((key, rows.clone()));
        rows
    }
}

/// 读取某数据根下某账号的会话行（读不到一律按「没有」处理：去重是优化，不是正确性前提）。
fn read_rows(paths: &SessionPaths, uid: &str) -> Vec<SessionRow> {
    let value = list_sessions_for_user_at(paths, uid);
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            let title = item.get("title").and_then(Value::as_str).unwrap_or("");
            let cwd = item.get("cwd").and_then(Value::as_str).unwrap_or("");
            Some((id.to_string(), title.to_string(), cwd.to_string()))
        })
        .collect()
}

/// 目标账号里与源会话内容完全相同、且尚未参与任何关联组的既有会话。
///
/// 返回候选的会话 id 与它的正文快照（快照用作配对基线，避免为写基线再读一次正文）。
/// 候选按 `updated_at` 倒序遍历（`list_sessions_for_user_at` 的排序），命中即取最新的一份。
pub(in crate::modules::session) fn find_same_content_session(
    context: &CopyContext,
    cid: &str,
    source_snapshot: &ContentSnapshot,
) -> Option<(String, ContentSnapshot)> {
    // 标题 / 工作目录先按「源账号视角」取：两个条件都只用于**过滤候选**，取不到就放弃
    // 匹配（源行缺失的会话本来也走不到这里，保守返回 None 不影响复制本体）。
    let (title, cwd) = {
        let rows = context.rows.rows(context.source_paths, context.source_uid);
        let row = rows.iter().find(|row| row.0 == cid)?;
        (row.1.clone(), row.2.clone())
    };
    if title.trim().is_empty() || title == NO_TITLE_PLACEHOLDER {
        return None;
    }
    let expected = comparable_digest(&source_snapshot.normalized);
    // 关联主表只读一次：候选是否已被别的组占用按这张快照判断。
    let store = match session_link::load_store(context.paths) {
        StoreState::Ready(store) => Some(store),
        // 不可用会在 `resolve_target` 处已经拒绝复制；到这里只能是「首次使用」。
        StoreState::Missing | StoreState::Unavailable(_) => None,
    };
    // 候选先按标题 + 目录 + 未被别的组占用过滤，通常只剩个位数：不为整账号所有会话读正文。
    let candidates = context.rows.rows(context.paths, context.target_uid);
    for candidate in &candidates {
        let (candidate_id, candidate_title, candidate_cwd) = (
            candidate.0.as_str(),
            candidate.1.as_str(),
            candidate.2.as_str(),
        );
        // 同一会话（cp 回原账号等场景）不在这里处理：那是 `resolve_target` 的职责。
        if candidate_id == cid || candidate_title != title || candidate_cwd != cwd {
            continue;
        }
        if store.as_ref().is_some_and(|store| {
            session_link::find_group_for_identity(
                store,
                context.variant,
                context.target_uid,
                candidate_id,
            )
            .is_some()
        }) {
            continue;
        }
        let Some(path) = find_project_jsonl(context.paths, candidate_id) else {
            continue;
        };
        if let ContentState::Ready(snapshot) =
            session_link::read_content_snapshot(&path, candidate_id)
        {
            if comparable_digest(&snapshot.normalized) == expected {
                return Some((candidate_id.to_string(), snapshot));
            }
        }
    }
    None
}

/// 内容身份的比较口径总摘要：与 [`session_link::decide_sync`] 完全一致。
fn comparable_digest(normalized: &NormalizedContent) -> String {
    session_link::total_digest_of(&normalized.comparable_line_digests)
}

/// 把目标账号里已有的同内容会话登记为组内成员（源与目标都登记），返回组 id。
///
/// 与复制路径共用 [`commit_links`]：组不存在时新建、目标账号旧的失效成员被显式
/// supersede、成员对写入共同基线。本次**不写**正文与数据库行——既有的那一条就是副本，
/// 报告里的 `alreadyLinked` 因此不是「没做成」，而是「已经同步好了」。
///
/// 只写关联主表与基线（本工具自己的数据），不碰客户端 DB / 正文，因此不需要复制路径上
/// 那套整库备份与操作日志：没有可恢复的中间产物。`Operation` 仅作为 `commit_links`
/// 的入参构造，不落盘（`operation_id` 留空）。
pub(in crate::modules::session) fn link_existing_member(
    context: &CopyContext,
    cid: &str,
    target_session_id: &str,
    group_id: Option<String>,
    target_content: &NormalizedContent,
) -> Result<String, String> {
    // 首次使用时先落地空的关联存储（与复制路径同一顺序约定：主文件先于任何成员记录）。
    ensure_link_store_ready(context.paths)?;
    let operation = Operation {
        version: OPERATION_VERSION,
        operation_id: String::new(),
        kind: OPERATION_KIND_COPY.to_string(),
        variant: context.variant,
        // 跨档时源成员记源档，跨档组的身份基础才不会记错。
        source_variant: (context.source_variant != context.variant)
            .then_some(context.source_variant),
        group_id: group_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        source: OperationMember {
            account_id: context.source_account_id.clone(),
            uid: context.source_uid.to_string(),
            session_id: cid.to_string(),
        },
        target: OperationMember {
            account_id: context.target_account_id.clone(),
            uid: context.target_uid.to_string(),
            session_id: target_session_id.to_string(),
        },
        // 基线用**目标这一份实际内容**：它是「两边共同的正文」，与复制路径里
        // 「写入的那份正文即基线」的口径一致。
        expected_content_digest: target_content.total_digest.clone(),
        expected_record_count: target_content.record_count,
        phase: OpPhase::Completed,
        backup: None,
        lifecycle_version: None,
        cleanup_state: None,
        last_error: None,
        aux_skipped: false,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    commit_links(context.paths, context.variant, &operation, target_content)
}
