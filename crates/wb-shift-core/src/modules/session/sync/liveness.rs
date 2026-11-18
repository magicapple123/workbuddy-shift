//! 关联组成员的代表成员口径与「会话行是否仍然存活」判定（预览、管理页列表、批量清理共用）。
//!
//! 已删除/不存在的会话不可能同步（判定必然 unknown），继续列出只会让「关联会话」堆满
//! `(无标题)` + 「无法确认」。本模块只做判定，不写任何数据。
//!
//! 判定是三态的（`Some(true)` 存活 / `Some(false)` 已失效 / `None` 无判定依据），因为读与写
//! 两条路径的正确取舍相反：
//!
//! - 预览与列表在 `None` 时 **fail-open**（保留改造前行为）：索引读不到时静默清空列表，
//!   用户得不到任何解释，而 `storeStatus` 仍是 `ready`；
//! - 批量清理在 `None` 时 **必须中止**：破坏性操作不许按「猜」的结果删数据。

use super::*;

/// 代表成员口径的唯一实现放在 [`session_link`]（插件侧预览共用同一口径），这里只转出。
pub(crate) use crate::modules::session_link::representative_member;

/// 组内某账号的代表成员是否仍有存活会话行（行缺失、已删除、归属变更都算失效）。
///
/// 只认代表成员：当前副本没了就是没了，不能因为历史旧副本（superseded）的行还在就继续
/// 展示——那种组的内容判定必然 unknown，正是「`(无标题)` + 无法确认」的来源。
/// 成员状态（active/stale）本身不参与过滤：那是「可见但不可执行」的既有契约。
///
/// 返回 `None` 表示**没有判定依据**（存活索引不可用：库打不开、缺表、缺 `user_id` 列），
/// 由调用方按读写路径各自的语义处理。
fn member_row_alive(
    live: &mut LiveSessionRows,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    group: &LinkGroup,
    uid: &str,
) -> Option<bool> {
    let Some(member) = representative_member(group, uid) else {
        // 组内没有该账号的成员：没有会话行可谈，按失效处理（与改造前一致）。
        return Some(false);
    };
    // 成员档位的数据根由调用方解析：跨档组源读源档、目标读目标档。
    let paths = member_paths(group, member);
    live.live_ids(&paths, &member.uid)
        .map(|ids| ids.contains(member.session_id.as_str()))
}

/// 两个参与账号是否都还有存活会话行：任一失效即整组不可同步。
///
/// 这类组的内容判定必然是 unknown（行都不存在，无从核对正文），因此直接从预览里剔除。
/// 只做读路径过滤，不改写关联存储——用户数据与该组的其它账号副本保持原样。
/// 索引不可用时按「存活」处理（fail-open），与逐条口径 `session_row_owner` 同结论。
pub(in crate::modules::session) fn group_rows_alive(
    live: &mut LiveSessionRows,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    group: &LinkGroup,
    source_uid: &str,
    target_uid: &str,
) -> bool {
    [source_uid, target_uid]
        .iter()
        .all(|uid| member_row_alive(live, member_paths, group, uid).unwrap_or(true))
}

/// 组内是否还有**任意**存活的成员会话行（管理页列表过滤与批量清理共用）。
///
/// - `Some(true)`：至少有一个成员的代表会话还在，组不失效；
/// - `Some(false)`：组内所有成员指向的会话行都已不存活（行缺失 / 已删除 / 归属变更），
///   即「会话管理页隐藏掉的组」；
/// - `None`：有成员无法判定，读路径按「存活」处理、清理路径必须中止。
///
/// 按成员 `uid` 去重：同一账号的多条成员记录（active + superseded）由代表成员代表，
/// 避免把「旧副本还在、当前副本已删」误判成组还活着。
pub(crate) fn group_any_alive(
    live: &mut LiveSessionRows,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    group: &LinkGroup,
) -> Option<bool> {
    let mut uids: Vec<&str> = group
        .members
        .iter()
        .map(|member| member.uid.as_str())
        .collect();
    uids.sort_unstable();
    uids.dedup();
    let mut unknown = false;
    for uid in uids {
        match member_row_alive(live, member_paths, group, uid) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}
