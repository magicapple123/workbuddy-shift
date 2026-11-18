//! 插件 / IDE 侧的**关联组存活判定**：管理页列表与批量清理共用同一份结论与同一份原因。
//!
//! 「会话还在不在」只有一个判定点（`vscode_session_sync::liveness`：索引命中**或**会话目录
//! 存在即存活）。这里负责三件事：
//!
//! 1. 把会话级结论汇总成组级结论（[`GroupLiveness`]）；
//! 2. 按账号缓存工作区扫描（[`LivenessJudge`]）——管理页一次可能判几十上百个组，同一账号
//!    不该扫第二遍盘；
//! 3. 产出「清理会被拒绝」的**原因文案**（[`GroupScan::blocked`]），让管理页能把
//!    「可点但必失败」的入口变成「禁用 + 说明」。
//!
//! 读路径与写路径的差异只有对 `Unknown` 的取舍，判定与原因本身是同一份：
//!
//! - 管理页（[`super::view::visible_groups_at`]）fail-open：`Unknown` 一律展示，
//!   同时把阻塞原因交给前端；
//! - 批量清理（[`super::maintenance::dead_groups`]）fail-closed：`Unknown` 拒绝整次清理。
//!
//! 只有「所有成员都确为 [`GroupLiveness::Gone`]」才允许隐藏 / 删除，因此**管理页隐藏的组
//! 与清理能删的组严格同一集合**（隐藏的 = 可清理的）。

use super::*;
use crate::modules::session::{group_any_alive, LiveSessionRows};
use crate::modules::vscode_session_sync::{ConversationIndexState, LivenessJudge, SessionLiveness};

/// 判定不可用时中止清理的统一文案（索引读不到就不猜）。
pub(super) const PRUNE_UNKNOWN_REASON: &str =
    "当前无法判定会话是否仍然存在，已取消清理。请确认客户端数据目录可正常读取后重试。";

/// 数据根 / 存储不可解析时的统一文案。
///
/// 只在**拿不到判定依据的入口**使用（插件 / IDE 的数据目录解析不到、客户端没有索引规格）。
/// 与索引四态无关的常规场景（索引可读但会话已删除）不再返回「暂不支持」类文案。
pub(super) fn prune_unsupported_reason(client: SessionClient) -> String {
    match client {
        SessionClient::Workbuddy => PRUNE_UNKNOWN_REASON.to_string(),
        SessionClient::VscodeExt => {
            "未找到 VS Code CodeBuddy 插件的会话数据目录，无法判定会话是否仍然存在，已取消清理。\
             请先打开 VS Code 并登录该插件后重试。"
                .to_string()
        }
        SessionClient::CodebuddyIde => {
            "未找到 CodeBuddy IDE 的会话数据目录，无法判定会话是否仍然存在，已取消清理。\
             请先打开 CodeBuddy IDE 并登录一次后重试。"
                .to_string()
        }
    }
}

/// 用到的客户端名（错误 / 阻塞文案里要能看出是哪一个客户端）。
fn client_label(client: SessionClient) -> &'static str {
    match client {
        SessionClient::Workbuddy => "WorkBuddy",
        SessionClient::VscodeExt => "VS Code 插件",
        SessionClient::CodebuddyIde => "CodeBuddy IDE",
    }
}

/// 某账号的索引不可判定时的可读原因：说清「哪个账号、为什么判不了」。
fn unknown_member_reason(
    client: SessionClient,
    uid: &str,
    state: &ConversationIndexState,
) -> String {
    let why = match state {
        ConversationIndexState::Absent => "找不到该账号的会话历史目录",
        ConversationIndexState::Partial(_) => {
            "该账号的会话索引只有部分工作区可读，损坏的分片里可能还有该会话"
        }
        // `Available` 不会走到这里；当成「读不到」处理，保持文案与行为一致。
        ConversationIndexState::Unavailable | ConversationIndexState::Available(_) => {
            "该账号的会话索引无法解析（index.json 缺失或已损坏）"
        }
    };
    format!(
        "无法判定「{}」账号 {uid} 的会话：{why}，已取消清理。请先打开该客户端确认账号数据正常后重试。",
        client_label(client)
    )
}

/// 组内存活状态汇总（插件 / IDE 侧）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GroupLiveness {
    /// 至少一个成员的代表会话确实存在 → 展示、不清理。
    Alive,
    /// 所有成员的代表会话都已确认消失 → 隐藏、可清理。
    Gone,
    /// 无法判定（附「哪个账号、哪一态」供拒绝文案使用）→ 展示、拒绝清理。
    Unknown {
        uid: String,
        state: ConversationIndexState,
    },
}

/// 插件 / IDE 的判定器封装：数据根或索引规格解析不到时就是 `None`。
pub(super) struct IndexLiveness {
    judge: LivenessJudge,
}

/// 按客户端与其数据根构造判定器；缺任一项（数据根 / 索引规格）返回 `None`。
///
/// 生产路径由 `view::list_at` 传入 `client.data_root()`；测试用临时目录注入
/// ——**不得**在测试里走真实数据根（AGENTS.md 第 8 节）。
pub(super) fn index_liveness(client: SessionClient, root: Option<&Path>) -> Option<IndexLiveness> {
    let spec = client.store_spec()?;
    let root = root?;
    Some(IndexLiveness {
        judge: LivenessJudge::new(spec, root),
    })
}

impl IndexLiveness {
    /// 组内存活汇总：按 uid 排序逐个判定，任一个存活即整组存活（无需再判其余账号）。
    ///
    /// 判定顺序与存储顺序无关（[`group_uids`] 排序去重），因此同样的现场总是得到同样的
    /// 结论——包括「拒绝文案里报哪个账号」。
    pub(super) fn group(&mut self, group: &LinkGroup) -> GroupLiveness {
        let mut unknown: Option<(String, ConversationIndexState)> = None;
        for uid in group_uids(group) {
            match self.judge.member_liveness(group, uid) {
                SessionLiveness::Alive => return GroupLiveness::Alive,
                SessionLiveness::Gone => {}
                // 保留最后一次「判不了」的账号：与改造前的拒绝文案口径一致。
                SessionLiveness::Unknown(state) => unknown = Some((uid.to_string(), state)),
            }
        }
        match unknown {
            Some((uid, state)) => GroupLiveness::Unknown { uid, state },
            None => GroupLiveness::Gone,
        }
    }
}

/// 一次组扫描的完整结果：哪些组已失效 + 清理是否会被阻塞（原因）。
///
/// 读路径（管理页）与写路径（批量清理）用**同一次**扫描，所以
/// 「`blocked` 非空」与「`prune` 会整次拒绝」是同一件事，不会出现入口可点但点了必失败。
pub(super) struct GroupScan<'a> {
    /// 已确认**全部成员失效**的组（按存储顺序）。
    pub(super) dead: Vec<&'a LinkGroup>,
    /// 清理会被拒绝的原因；`None` = 本次清理可以正常执行。
    ///
    /// 文案与 `prune_dead_groups` 的报错逐字相同：管理页禁用的理由就是后端会拒绝的理由。
    pub(super) blocked: Option<String>,
}

/// 扫一遍 scope 内的组，得到「失效组 + 阻塞原因」。
///
/// 判定按客户端分派：`Workbuddy` 读 `sessions` 行，插件 / IDE 用工作区索引 + 会话目录证据。
/// 两侧的「无法判定」都必须同时成立两件事：读路径不隐藏（fail-open）、写路径整次拒绝
/// （fail-closed）——所以 `blocked` 与 `dead` 在同一趟里一起产出。
pub(super) fn scan_groups<'a>(
    store: &'a session_link::LinkStore,
    client: SessionClient,
    scope: Option<WbVariant>,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    index_root: Option<&Path>,
) -> GroupScan<'a> {
    let mut scan = GroupScan {
        dead: Vec::new(),
        blocked: None,
    };
    let in_scope = store
        .groups
        .iter()
        .filter(|group| group_is_in_scope(client, scope, group));
    match client {
        SessionClient::Workbuddy => {
            // 存活索引按数据库文件缓存：同一档位的成员只查一次库。
            let mut live = LiveSessionRows::new();
            for group in in_scope {
                match group_any_alive(&mut live, member_paths, group) {
                    Some(true) => {}
                    Some(false) => scan.dead.push(group),
                    // 判定不可用（读不到库 / 表）：读路径不隐藏，写路径必然拒绝。
                    None => {
                        scan.blocked
                            .get_or_insert_with(|| PRUNE_UNKNOWN_REASON.to_string());
                    }
                }
            }
        }
        SessionClient::VscodeExt | SessionClient::CodebuddyIde => {
            match index_liveness(client, index_root) {
                // 数据根 / 索引规格解析不到：没有任何判定依据。读路径 fail-open 全展示，
                // 写路径拒绝并说明是哪个客户端。
                None => scan.blocked = Some(prune_unsupported_reason(client)),
                Some(mut judge) => {
                    for group in in_scope {
                        match judge.group(group) {
                            GroupLiveness::Alive => {}
                            GroupLiveness::Gone => scan.dead.push(group),
                            GroupLiveness::Unknown { uid, state } => {
                                scan.blocked.get_or_insert_with(|| {
                                    unknown_member_reason(client, &uid, &state)
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    scan
}

/// 组内参与的账号（去重后按 uid 排序，保证判定顺序与存储顺序无关）。
///
/// 同一账号可能有 active + superseded 多条成员记录，由代表成员代表其存活状态
/// （否则「旧副本还在、当前副本已删」会被误判成组还活着）。
pub(super) fn group_uids(group: &LinkGroup) -> Vec<&str> {
    let mut uids: Vec<&str> = group
        .members
        .iter()
        .map(|member| member.uid.as_str())
        .collect();
    uids.sort_unstable();
    uids.dedup();
    uids
}
