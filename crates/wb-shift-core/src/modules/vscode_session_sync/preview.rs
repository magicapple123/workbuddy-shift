//! 关联预览：链接预览入口、成员绑定与预览分组载荷。

use super::*;

// ---------------------------------------------------------------------------
// R2：切换预览（design §4 / §6）
// ---------------------------------------------------------------------------

/// 预览「当前插件账号 → 目标账号」可同步的关联会话（只读，不写任何会话正文）。
pub fn links_preview(target_acc: &Value) -> Result<Value, String> {
    let root = vscode_session::ext_data_root()
        .ok_or_else(|| "未找到 VS Code CodeBuddy 插件数据目录，无法同步会话".to_string())?;
    let source_uid = vscode_ext::active_ext_uid()
        .filter(|uid| !uid.trim().is_empty())
        .ok_or_else(|| {
            "未检测到 VS Code CodeBuddy 插件当前登录账号，无法同步会话。请先在 VS Code 中登录该插件后重试。"
                .to_string()
        })?;
    links_preview_at(
        &root,
        &SessionPaths::for_vscode_ext(),
        &source_uid,
        target_acc,
    )
}

/// [`links_preview`] 的可测实现：显式传入数据根、存储路径与来源 uid（VS Code 插件数据仓）。
pub fn links_preview_at(
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
) -> Result<Value, String> {
    links_preview_in(VSCODE_STORE, root, paths, source_uid, target_acc)
}

/// [`links_preview`] 的数据仓参数化版本（VS Code 插件：不按 variant 过滤）。
pub fn links_preview_in(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
) -> Result<Value, String> {
    links_preview_in_filtered(spec, root, paths, source_uid, target_acc, None)
}

/// IDE 共用关联表时的预览：只列出 `variant` 与当前档位一致的组。
pub(crate) fn links_preview_in_for_variant(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    variant: WbVariant,
) -> Result<Value, String> {
    links_preview_in_filtered(spec, root, paths, source_uid, target_acc, Some(variant))
}

pub(super) fn links_preview_in_filtered(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    variant_filter: Option<WbVariant>,
) -> Result<Value, String> {
    let target_uid = target_uid_of(target_acc)?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需同步会话".to_string());
    }
    // 两侧数据仓都没有档位能力探测：同一数据仓可跨档位复制。
    let mut report = json!({
        "supported": true,
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "groups": [],
    });
    match session_link::load_store(paths) {
        StoreState::Missing => report["storeStatus"] = json!("missing"),
        StoreState::Unavailable(reason) => {
            report["storeStatus"] = json!("unavailable");
            report["storeError"] = json!(reason);
        }
        StoreState::Ready(store) => {
            report["storeStatus"] = json!("ready");
            // 定位表、可用性四态与磁盘目录证据来自同一次遍历：判定存活不该为此再扫一遍盘。
            // 只剩「索引命中 / 目录存在 / 都没有」这一份组合判定（见 `liveness`）。
            let source_scan = conversation_index_scan(spec, root, source_uid);
            let target_scan = conversation_index_scan(spec, root, &target_uid);
            let context = SyncContext {
                spec,
                paths,
                root,
                source_uid,
                target_uid: &target_uid,
                source_index: &source_scan.locators,
                target_index: &target_scan.locators,
                variant_filter,
            };
            let groups: Vec<Value> = store
                .groups
                .iter()
                .filter(|group| {
                    passes_variant_filter(variant_filter, group.variant)
                        && has_member_for(group, source_uid)
                        && has_member_for(group, &target_uid)
                        // 只列两侧代表会话仍存在的组：会话被删除或改归属后不可能同步
                        // （判定必然是 unknown），继续列出只会是「(无标题)」死条目。
                        // 「仍在」的判据是索引命中**或**会话目录还在（目录是更强证据：
                        // `index.json` 损坏丢了条目时，组仍然要能同步）。
                        && member_session_alive(&source_scan, source_uid, group)
                        && member_session_alive(&target_scan, &target_uid, group)
                })
                .map(|group| preview_group_item(&context, group))
                .collect();
            report["groups"] = json!(groups);
        }
    }
    Ok(report)
}

/// 目标账号 uid（缺 uid 时按可读原因拒绝）。
pub(super) fn target_uid_of(target_acc: &Value) -> Result<String, String> {
    account::get_str(target_acc, "uid")
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "目标账号缺少 uid，无法同步会话".to_string())
}

/// `None` 表示不过滤（插件路径）；`Some` 时组档位必须一致（IDE 共用命名空间）。
pub(super) fn passes_variant_filter(filter: Option<WbVariant>, variant: WbVariant) -> bool {
    match filter {
        Some(expected) => variant == expected,
        None => true,
    }
}

/// 组内是否存在该账号的成员（任意状态）：双方都有成员才谈得上「共同参与」。
pub(super) fn has_member_for(group: &LinkGroup, uid: &str) -> bool {
    group.members.iter().any(|member| member.uid == uid)
}

/// 该账号的**代表会话**是否还活着（预览的过滤口径）。
///
/// 插件的会话「行」由工作区索引条目与会话目录共同证明（见 [`super::liveness`]）：
/// 索引命中或 `history/<workspaceHash>/<conversationId>/` 还在都算存在；只有
/// [`SessionLiveness::Gone`]（索引读得全、目录也没有）才过滤掉这一组。
/// `Unknown`（索引不完整）一律 fail-open 保留——把「读不到」当成「没了」会让用户
/// 以为会话消失。
fn member_session_alive(scan: &ConversationIndexScan, uid: &str, group: &LinkGroup) -> bool {
    match session_link::representative_member(group, uid) {
        // 代表成员口径与标题/摘要一致：只看当前副本，不看历史 superseded 副本。
        Some(member) => !matches!(
            session_liveness(scan, &member.session_id),
            SessionLiveness::Gone
        ),
        // 组内没有该账号的成员：`has_member_for` 已拦截，这里按失效处理保持一致。
        None => false,
    }
}

/// 成员摘要（只含身份与状态，不含正文）。
pub(super) fn member_summary(member: Option<&LinkMember>) -> Value {
    match member {
        Some(member) => json!({
            "memberId": member.member_id,
            "uid": member.uid,
            "accountId": member.account_id,
            "sessionId": member.session_id,
            "state": member.state.as_str(),
        }),
        None => Value::Null,
    }
}

/// 成员在预览时刻的内容绑定（不可验证时摘要留空，判定必然为 unknown）。
pub(super) fn member_binding(member: &LinkMember, content: &ContentState) -> PreviewMemberBinding {
    let (raw_digest, normalized_digest, record_count) = match content {
        ContentState::Ready(snapshot) => (
            snapshot.full_digest.clone(),
            snapshot.normalized.total_digest.clone(),
            snapshot.normalized.record_count,
        ),
        _ => (String::new(), String::new(), 0),
    };
    PreviewMemberBinding {
        member_id: member.member_id.clone(),
        account_id: member.account_id.clone(),
        uid: member.uid.clone(),
        session_id: member.session_id.clone(),
        raw_digest,
        normalized_digest,
        record_count,
    }
}

/// 由实时状态组装预览绑定（预览与执行前核对共用同一份装配逻辑）。
pub(super) fn live_preview_binding(
    group: &LinkGroup,
    source_member: &LinkMember,
    target_member: &LinkMember,
    source_content: &ContentState,
    target_content: &ContentState,
    baseline: &BaselineState,
    verdict: SyncVerdict,
) -> PreviewBinding {
    PreviewBinding {
        variant: group.variant,
        group_id: group.id.clone(),
        group_fingerprint: session_link::group_fingerprint(group),
        source: member_binding(source_member, source_content),
        target: member_binding(target_member, target_content),
        baseline_ref: session_link::find_pair_base(
            group,
            &source_member.member_id,
            &target_member.member_id,
        )
        .map(|pair| pair.baseline_ref.clone()),
        baseline_total_digest: baseline.ready().map(|record| record.total_digest.clone()),
        baseline_record_count: baseline.ready().map(|record| record.record_count),
        verdict,
        // VS Code / CN IDE 侧没有 sessions 表，不感知归档状态：永不提供状态同步动作。
        source_status: None,
        target_status: None,
        archive_action: None,
    }
}

/// 内容里的记录数（不可验证时为 0，仅用于展示）。
pub(super) fn record_count_of(content: &ContentState) -> usize {
    match content {
        ContentState::Ready(snapshot) => snapshot.normalized.record_count,
        _ => 0,
    }
}

/// 单个关联组的预览项（形状对齐 WorkBuddy 侧 `preview_group_item`）。
///
/// 记录数与差集只用于向用户解释；能否勾选只由判定结果决定（design §3.2）。
pub(super) fn preview_group_item(context: &SyncContext<'_>, group: &LinkGroup) -> Value {
    // 代表成员口径与 WorkBuddy 侧一致：取该账号当前的副本，而不是第一条（旧 superseded
    // 成员可能排在前面且会话已删除，会把可同步的组显示成 `(无标题)`）。
    let source_any = session_link::representative_member(group, context.source_uid);
    let target_any = session_link::representative_member(group, context.target_uid);
    let title = source_any
        .map(|member| context.title_of(context.source_uid, &member.session_id))
        .unwrap_or_else(|| "(无标题)".to_string());
    let source_summary = member_summary(source_any);
    let target_summary = member_summary(target_any);

    let (Some(source_member), Some(target_member)) = (
        session_link::active_member_for(group, context.source_uid),
        session_link::active_member_for(group, context.target_uid),
    ) else {
        return json!({
            "groupId": group.id,
            "title": title,
            "cwd": "",
            "verdict": SyncVerdict::Unknown.as_str(),
            "extraA": 0,
            "extraB": 0,
            "common": 0,
            "defaultChecked": false,
            "availableModes": [],
            "reason": "对应的会话已失效或已被替换，需手动处理",
            "recordCount": {"source": 0, "target": 0, "baseline": null},
            "source": source_summary,
            "target": target_summary,
        });
    };

    let baseline = session_link::load_pair_baseline(
        context.paths,
        group,
        &source_member.member_id,
        &target_member.member_id,
    );
    let source_content = context.content_of(context.source_uid, &source_member.session_id);
    let target_content = context.content_of(context.target_uid, &target_member.session_id);
    let decision = session_link::decide_sync(&source_content, &target_content, &baseline);
    let reason = decision.reason.clone();
    let modes: Vec<&str> = decision
        .verdict
        .available_modes()
        .iter()
        .map(|mode| mode.as_str())
        .collect();
    // Ahead stays unavailable in ordinary sync; group unification needs a verified
    // token for its separate explicit overwrite mode.
    let actionable = !modes.is_empty() || decision.verdict == SyncVerdict::Ahead;

    let mut item = json!({
        "groupId": group.id,
        "title": title,
        "cwd": "",
        "verdict": decision.verdict.as_str(),
        "extraA": decision.extra_a,
        "extraB": decision.extra_b,
        "common": decision.common,
        "defaultChecked": decision.default_checked,
        "availableModes": modes,
        "reason": reason.clone(),
        "recordCount": {
            "source": record_count_of(&source_content),
            "target": record_count_of(&target_content),
            "baseline": baseline.ready().map(|record| record.record_count),
        },
        "source": source_summary,
        "target": target_summary,
    });
    if actionable {
        let binding = live_preview_binding(
            group,
            source_member,
            target_member,
            &source_content,
            &target_content,
            &baseline,
            decision.verdict,
        );
        match session_link::save_preview_token(context.paths, binding) {
            Ok(preview_id) => item["previewToken"] = json!(preview_id),
            Err(error) => {
                // 绑定存不下来就不能让用户勾选：不给出可执行动作，并说明原因。
                item["availableModes"] = json!([]);
                item["defaultChecked"] = json!(false);
                item["reason"] = json!(format!("{reason}（检查结果无法保存：{error}）"));
            }
        }
    }
    item
}

// ---------------------------------------------------------------------------
// R3 / R4：执行同步（design §6）
// ---------------------------------------------------------------------------

/// 空报告（与 WorkBuddy 侧同形）。
pub(super) fn empty_report() -> Value {
    json!({ "synced": [], "skipped": [], "errors": [] })
}
