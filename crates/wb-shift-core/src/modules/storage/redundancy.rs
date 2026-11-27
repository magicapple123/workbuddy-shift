//! 冗余副本的判定口径：**同一关联组内、同一 uid、同 variant 的存活副本数 > 1**。
//!
//! 为什么必须按 uid 分桶（issue #22 补充）：一个组里 A / B 两个账号各一份副本是**合法**
//! 的跨账号副本——它们各自只有 1 份。早期口径按 variant 统计组内成员数，把这种正常组也
//! 标成「冗余 N 份」，于是一键清理会删掉另一个账号的合法副本。跨账号副本不算冗余，
//! 只有同一账号在同一个组里留下多份才是（复制入口的重复产物 / 客户端自带的多副本）。
//!
//! 本模块只提供**索引与计数**，判定与清理取舍在 `clean/redundant.rs`；两侧共用同一份口径，
//! 避免「列表说冗余、清理时却按另一个标准删」。
//!
//! **两套口径，用途不同（不要混用）**：
//! - 关联表口径 [`GroupCopies::same_uid_copies`]：数**关联主表里的成员记录**。成员可能已被
//!   软删（`sessions.deleted_at` 非空）、或正文与行早已被客户端清掉，它们仍记在组里；
//! - 存活行口径 [`apply_live_copy_counts`]：只数**本机 `sessions` 表里未删除的行**。列表
//!   标记、冗余筛选与「一键清理会删几条」全部用这一套——因为清理只能删掉还活着的行。
//!
//! 只按关联表数会出现「按钮说有 1 条可清理、点下去报没有冗余副本」：软删成员被算进副本数，
//! 却既不能删也不该留（QA 在 issue #22 回归里实测到这个不一致）。

use super::sessions::Bundle;
use crate::modules::variant::WbVariant;
use serde_json::Value;
use std::collections::HashMap;

/// 关联组索引的一格：会话所属组 + 该账号在本组内的副本数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupCopies {
    /// 会话所属的关联组 id。
    pub group_id: String,
    /// **关联表口径**：同一组内同一账号的成员记录数（跨账号不计入）。
    ///
    /// 成员可能是软删行 / 已被客户端清掉的残留，因此这个数字**不等于**可清理条数；
    /// 列表标记与清理一律用存活行口径 [`apply_live_copy_counts`]。
    pub same_uid_copies: usize,
}

/// 关联组索引（原文由调用方提供，避免同一份快照解析两遍）。
///
/// 键是会话 id；值带组 id 与该会话**自己账号**在组内的副本数。成员缺 `variant` 时按组级
/// `variant` 归属（跨档组成员显式保存，与 `session_link` 的旧存储兼容规则一致）；
/// `state == "removed"` 的历史成员不计入（与改造前一致）。
pub fn group_index_from_text(text: &str, variant: WbVariant) -> HashMap<String, GroupCopies> {
    let mut out = HashMap::new();
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return out;
    };
    let Some(groups) = json.get("groups").and_then(Value::as_array) else {
        return out;
    };
    for group in groups {
        let group_id = group
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(members) = group.get("members").and_then(Value::as_array) else {
            continue;
        };
        let same_variant: Vec<&Value> = members
            .iter()
            .filter(|member| member_variant_of(member, group) == Some(variant.as_str()))
            .filter(|member| member.get("state").and_then(Value::as_str) != Some("removed"))
            .collect();
        // 先按 uid 计一遍，再落到每个成员上：每个会话都拿到**自己账号**的副本数。
        let mut by_uid: HashMap<&str, usize> = HashMap::new();
        for member in &same_variant {
            let uid = member
                .get("uid")
                .and_then(Value::as_str)
                .unwrap_or_default();
            *by_uid.entry(uid).or_default() += 1;
        }
        for member in same_variant {
            let Some(session_id) = member.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let uid = member
                .get("uid")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let same_uid_copies = by_uid.get(uid).copied().unwrap_or(1);
            out.insert(
                session_id.to_string(),
                GroupCopies {
                    group_id: group_id.clone(),
                    same_uid_copies,
                },
            );
        }
    }
    out
}

/// 按**存活行口径**重算资产包的副本数与冗余标记（列表标记 / 冗余筛选 / 一键清理的唯一口径）。
///
/// 按 `(group_id, uid)` 只数 `row_status == "live"` 的资产包：与 `clean/redundant.rs`
/// 的分桶规则逐字一致。`live_copies` 表示「本组内本账号的存活行条数」，
/// `redundant` 额外要求**本资产包自己也是存活行**——软删/无行的资产包不构成可清理项。
pub fn apply_live_copy_counts(bundles: &mut [Bundle]) {
    let mut live: HashMap<(String, String), usize> = HashMap::new();
    for bundle in bundles.iter() {
        if bundle.row_status != "live" {
            continue;
        }
        if let Some(group_id) = &bundle.group_id {
            *live
                .entry((group_id.clone(), bundle.uid.clone()))
                .or_default() += 1;
        }
    }
    for bundle in bundles.iter_mut() {
        let copies = bundle
            .group_id
            .as_ref()
            .and_then(|group_id| live.get(&(group_id.clone(), bundle.uid.clone())))
            .copied()
            .unwrap_or(0);
        bundle.live_copies = copies;
        bundle.redundant = bundle.row_status == "live" && copies > 1;
    }
}

/// 一键清理（`redundantAll`）实际会删掉的条数：`Σ(每个 (组, 账号) 桶的存活数 - 1)`。
///
/// 与 `clean/redundant.rs` 的取舍逐字一致（每桶保留最新一条）。列表按钮上的数字取这里，
/// 用户点下去删掉的就是这个数——两个数字必须同源。
pub fn cleanable_redundant_total(bundles: &[Bundle]) -> usize {
    let mut live: HashMap<(&str, &str), usize> = HashMap::new();
    for bundle in bundles {
        if bundle.row_status != "live" {
            continue;
        }
        let Some(group_id) = bundle.group_id.as_deref() else {
            continue;
        };
        *live.entry((group_id, bundle.uid.as_str())).or_default() += 1;
    }
    live.values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum()
}

/// 成员的档位：成员级 `variant` 优先，缺失时回退组级 `variant`（旧存储兼容）。
fn member_variant_of<'a>(member: &'a Value, group: &'a Value) -> Option<&'a str> {
    member
        .get("variant")
        .and_then(Value::as_str)
        .or_else(|| group.get("variant").and_then(Value::as_str))
}
