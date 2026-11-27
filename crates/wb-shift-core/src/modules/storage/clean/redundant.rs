//! 冗余副本的清理计划：按 (关联组, 账号) 分桶，每桶保留一条，其余进回收站。
//!
//! 两条硬约束（issue #22「设置里也没有一键删除冗余副本的选项」）：
//! 1. **跨账号的合法副本不删**：桶的键是 `(group_id, uid)`，A / B 各自那一份分别落在
//!    不同桶里，桶内只有 1 条 → 不产生删除项；
//! 2. **绝不整桶删光**：`keepSessionId` 为空时按 `updated_at` 最新的一条保留，其余删除。
//!    旧实现里 `Some(id) != keep` 在 `keep` 为 `None` 时恒真，会把该组**全部**活副本删掉。
//!
//! 只做计划（`Vec<Work>` / `Vec<RowWork>`），执行仍走 `clean::execute`（回收站 + 整库备份 +
//! 行快照），因此本模块不直接碰文件系统。

use super::*;
use std::collections::HashMap;

/// 一个待清理的桶：同一个关联组里同一账号的多份副本。
struct Bucket<'a> {
    group_id: String,
    uid: String,
    keep: &'a Bundle,
    doomed: Vec<&'a Bundle>,
}

/// `target.kind = "redundant"`：单个关联组的同账号冗余副本。
///
/// `keep_session_id` 指定保留哪一条（可空）；它必须落在本次可清理的候选里，否则报错
/// ——静默忽略用户指定的保留项会让「保哪条」变成猜的。
pub(super) fn plan_group(
    plan: &mut Plan,
    ctx: &CleanContext,
    all: &[Bundle],
    group_id: &str,
    keep_session_id: Option<&str>,
) -> Result<(), String> {
    let candidates: Vec<&Bundle> = all
        .iter()
        .filter(|bundle| bundle.group_id.as_deref() == Some(group_id))
        .collect();
    if let Some(keep) = keep_session_id {
        if !candidates.iter().any(|bundle| bundle.id == keep) {
            return Err(format!("指定的保留会话不属于该关联组：{keep}"));
        }
    }
    let buckets = build_buckets(candidates, keep_session_id);
    apply(plan, ctx, buckets, "该关联组没有可清理的冗余副本")
}

/// `target.kind = "redundantAll"`：一次清理**所有**同账号冗余桶（每组每账号留最新一条）。
///
/// 不接受 `keepSessionId`：批量清理的取舍只有一条规则（保留最新的），逐组指定保留项
/// 属于单个组清理（`redundant`）的用途。
pub(super) fn plan_all(plan: &mut Plan, ctx: &CleanContext, all: &[Bundle]) -> Result<(), String> {
    let candidates: Vec<&Bundle> = all.iter().filter(|bundle| bundle.redundant).collect();
    if candidates.is_empty() {
        return Err("没有检测到冗余副本".to_string());
    }
    let buckets = build_buckets(candidates, None);
    apply(plan, ctx, buckets, "没有检测到冗余副本")
}

/// 按 (组, 账号) 分桶，桶内 > 1 条时才产生删除项。
///
/// 只考虑有存活行的资产包：无行/软删的副本删不掉「数据库行」，不参与取舍（也就不会
/// 因为它们的 `updated_at` 更新而被误当成保留项）。
fn build_buckets<'a>(candidates: Vec<&'a Bundle>, keep: Option<&str>) -> Vec<Bucket<'a>> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut grouped: HashMap<(String, String), Vec<&'a Bundle>> = HashMap::new();
    for bundle in candidates {
        if bundle.row_status != "live" {
            continue;
        }
        let Some(group_id) = bundle.group_id.clone() else {
            continue;
        };
        let key = (group_id, bundle.uid.clone());
        if !grouped.contains_key(&key) {
            order.push(key.clone());
        }
        grouped.entry(key).or_default().push(bundle);
    }
    let mut buckets = Vec::new();
    for key in order {
        let mut items = grouped.remove(&key).unwrap_or_default();
        if items.len() < 2 {
            continue;
        }
        // keep 命中本桶时保留它；否则保留 `updated_at` 最新的一条（同刻按 id 升序取第一个，
        // 保证同一份快照上重复计算得到同一个保留项）。
        let keep_index = keep
            .and_then(|keep| items.iter().position(|bundle| bundle.id == keep))
            .unwrap_or_else(|| {
                items
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.updated_at
                            .cmp(&b.updated_at)
                            .then_with(|| b.id.cmp(&a.id))
                    })
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            });
        let keep_bundle = items.remove(keep_index);
        buckets.push(Bucket {
            group_id: key.0,
            uid: key.1,
            keep: keep_bundle,
            doomed: items,
        });
    }
    buckets
}

/// 把桶里的删除项落成计划（文件子项 + 数据库行），并写清保留规则。
///
/// `empty_reason` 是没有可删项时的错误文案：单个组与批量清理对「空」的解释不同。
fn apply(
    plan: &mut Plan,
    ctx: &CleanContext,
    buckets: Vec<Bucket<'_>>,
    empty_reason: &str,
) -> Result<(), String> {
    if buckets.is_empty() {
        return Err(empty_reason.to_string());
    }
    let mut copies = 0_usize;
    for bucket in &buckets {
        for bundle in &bucket.doomed {
            for entry in bundle.entries.iter().filter(|entry| entry.exists) {
                plan.works.push(Work {
                    root_id: variant_root_id(ctx.variant),
                    root: ctx.data_root.clone(),
                    path: entry.path.clone(),
                    usage: entry.usage,
                    category: format!("session.{}", entry.kind),
                    session_id: Some(bundle.id.clone()),
                });
            }
            plan.rows.push(RowWork {
                bundle: (*bundle).clone(),
            });
            copies += 1;
        }
        plan.labels.push(format!(
            "组 {} / 账号 {} 保留「{}」",
            bucket.group_id, bucket.uid, bucket.keep.title
        ));
    }
    let groups = buckets.len();
    // 计划与结果只差时态：确认框看到「将删除」，完成后的提示看到「已删除」。
    plan.note = Some(format!(
        "将删除 {copies} 条重复副本（{groups} 组，每组保留最新一条）；\
         跨账号的合法副本不会删除，删除项会进回收站、可恢复"
    ));
    plan.note_done = Some(format!(
        "已删除 {copies} 条重复副本（{groups} 组），每组保留最新一条；跨账号的合法副本未删除"
    ));
    plan.labels
        .push(format!("冗余副本：{groups} 组 · {copies} 条"));
    plan.warnings
        .push("删除后云端同步可能重新拉回；删除项在回收站，可恢复".to_string());
    Ok(())
}
