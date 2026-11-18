//! 同步预览：只读核对双方成员、基线与正文，生成预览项与一次性预览凭据绑定。
use super::liveness::{group_rows_alive, representative_member};
use super::*;

/// 内容快照里的记录数（不可验证时为 0，仅用于展示）。
pub(in crate::modules::session) fn record_count_of(content: &ContentState) -> usize {
    match content {
        ContentState::Ready(snapshot) => snapshot.normalized.record_count,
        _ => 0,
    }
}

/// 会话行的展示名与目录（标题优先 custom_title，与侧栏一致）。
///
/// `deleted_at` 列按存在性拼 SQL：历史库没有软删除标记时不能强行套 `deleted_at IS NULL`
/// （prepare 直接失败 → 行被判缺失 → 与存活索引「全部存活」的结论分叉，组会以
/// `(无标题)` 列出）。缺列时与 [`LiveSessionRows`] 同口径：按未删除处理。
pub(in crate::modules::session) fn session_row_info(
    paths: &SessionPaths,
    cid: &str,
) -> Option<(String, String)> {
    let db = paths.workbuddy_db();
    let conn = open_db(&db, true)?;
    if !table_exists(&conn, "sessions") {
        return None;
    }
    let sql = match (
        column_exists(&conn, "sessions", "custom_title"),
        column_exists(&conn, "sessions", "deleted_at"),
    ) {
        (true, true) => {
            "SELECT title, custom_title, cwd FROM sessions WHERE id = ?1 AND deleted_at IS NULL"
        }
        (true, false) => "SELECT title, custom_title, cwd FROM sessions WHERE id = ?1",
        (false, true) => {
            "SELECT title, NULL, cwd FROM sessions WHERE id = ?1 AND deleted_at IS NULL"
        }
        (false, false) => "SELECT title, NULL, cwd FROM sessions WHERE id = ?1",
    };
    conn.query_row(sql, [cid], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })
    .ok()
    .map(|(title, custom_title, cwd)| {
        (
            session_display_title(title, custom_title),
            cwd.unwrap_or_default(),
        )
    })
}

/// 会话行是否仍归属该成员账号（行被删除或改归属即成员失效，design §5）。
pub(in crate::modules::session) fn member_row_owned_by(
    paths: &SessionPaths,
    member: &LinkMember,
) -> bool {
    session_row_owner(paths, &member.session_id).is_some_and(|owner| owner == member.uid)
}

/// 组内是否存在该账号的成员（任意状态）：双方都有成员才谈得上「共同参与」。
pub(in crate::modules::session) fn has_member_for(group: &LinkGroup, uid: &str) -> bool {
    group.members.iter().any(|member| member.uid == uid)
}

/// 成员摘要（只含身份与状态，不含正文）。
pub(in crate::modules::session) fn member_summary(member: Option<&LinkMember>) -> Value {
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
pub(in crate::modules::session) fn member_binding(
    member: &LinkMember,
    content: &ContentState,
) -> PreviewMemberBinding {
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
// 参数都是本次绑定的显式输入（含归档状态），与复制侧同口径不做结构装箱。
#[allow(clippy::too_many_arguments)]
pub(in crate::modules::session) fn live_preview_binding(
    group: &LinkGroup,
    source_member: &LinkMember,
    target_member: &LinkMember,
    source_content: &ContentState,
    target_content: &ContentState,
    baseline: &BaselineState,
    verdict: SyncVerdict,
    archive: &PreviewArchiveState,
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
        source_status: archive.source_status.clone(),
        target_status: archive.target_status.clone(),
        archive_action: archive.action.clone(),
    }
}

/// 预览「当前账号 → 目标账号」可同步的关联组（design §6）。
///
/// 只处理双方共同参与的组，不涉及第三方账号；只读，不写任何会话正文。
/// 来源身份一律取该档位登录态文件，不接受前端传入。
pub fn session_links_preview(variant: WbVariant, target_acc: &Value) -> Result<Value, String> {
    session_links_preview_at(&SessionPaths::for_variant(variant), variant, target_acc)
}

pub(in crate::modules::session) fn session_links_preview_at(
    paths: &SessionPaths,
    variant: WbVariant,
    target_acc: &Value,
) -> Result<Value, String> {
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法同步会话".to_string());
    }
    let source_uid = current_user_uid_at(&paths.auth_file)
        .ok_or_else(|| "未读取到本机登录态，无法确定来源账号".to_string())?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需同步会话".to_string());
    }
    // 能力探测与复制同口径：只对国际版生效（国内版数据根与改造前同构）。
    let supported = variant != WbVariant::Ai || session_copy_supported_at(&paths.data_root);

    let mut report = json!({
        "supported": supported,
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "groups": [],
    });
    if !supported {
        report["storeStatus"] = json!("unsupported");
        return Ok(report);
    }
    match session_link::load_store(paths) {
        StoreState::Missing => report["storeStatus"] = json!("missing"),
        StoreState::Unavailable(reason) => {
            report["storeStatus"] = json!("unavailable");
            report["storeError"] = json!(reason);
        }
        StoreState::Ready(store) => {
            report["storeStatus"] = json!("ready");
            let fixed_paths = |_: &LinkGroup, _: &LinkMember| paths.clone();
            // 只列双方会话行仍存活的组：已删除/不存在的会话不可能同步（判定必然是
            // unknown），继续列出只会让列表堆满 `(无标题)` + 「无法确认」的死组。
            let mut live = LiveSessionRows::new();
            let groups: Vec<Value> = store
                .groups
                .iter()
                .filter(|group| {
                    group.variant == variant
                        && has_member_for(group, &source_uid)
                        && has_member_for(group, &target_uid)
                        && group_rows_alive(
                            &mut live,
                            &fixed_paths,
                            group,
                            &source_uid,
                            &target_uid,
                        )
                })
                .map(|group| {
                    preview_group_item(paths, &fixed_paths, group, &source_uid, &target_uid)
                })
                .collect();
            report["groups"] = json!(groups);
        }
    }
    Ok(report)
}

/// 预览「显式来源账号 → 显式目标账号」可同步的关联组（会话管理页用；跨档支持）。
///
/// 与 [`session_links_preview`] 的差异只在来源判定：源 uid 取自 `source_acc`，
/// 不再从登录态读取；成员内容与行按**成员自身档位**读取（跨档组：源读源档、
/// 目标读目标档），关联存储与基线共享。
pub fn session_links_preview_cross(
    source_acc: &Value,
    target_acc: &Value,
) -> Result<Value, String> {
    let source_variant = account::variant_of(source_acc);
    let target_variant = account::variant_of(target_acc);
    session_links_preview_cross_at(
        &SessionPaths::for_variant(source_variant),
        source_variant,
        source_acc,
        &SessionPaths::for_variant(target_variant),
        target_variant,
        target_acc,
        SessionPaths::for_variant,
    )
}

/// [`session_links_preview_cross`] 的可注入版本（单测注入临时双档目录与解析器）。
pub(in crate::modules::session) fn session_links_preview_cross_at(
    source_paths: &SessionPaths,
    source_variant: WbVariant,
    source_acc: &Value,
    target_paths: &SessionPaths,
    target_variant: WbVariant,
    target_acc: &Value,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    let source_uid = account_uid(source_acc);
    if source_uid.is_empty() {
        return Err("来源账号缺少 uid，无法预览会话".to_string());
    }
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法同步会话".to_string());
    }
    if source_variant == target_variant && source_uid == target_uid {
        return Err("来源账号与目标账号相同，无需同步会话".to_string());
    }
    // 能力探测与复制同口径：成员数据根只有国际版需要同构检查（读也依赖）。
    let mut supported = true;
    for (paths, variant) in [
        (source_paths, source_variant),
        (target_paths, target_variant),
    ] {
        if variant == WbVariant::Ai && !session_copy_supported_at(&paths.data_root) {
            supported = false;
        }
    }

    let mut report = json!({
        "supported": supported,
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "groups": [],
    });
    // 跨档预览在报告里带两侧档位；同档报告保持原字段（与现有入口逐字兼容）。
    if source_variant != target_variant {
        report["sourceVariant"] = json!(source_variant.as_str());
        report["targetVariant"] = json!(target_variant.as_str());
    }
    if !supported {
        report["storeStatus"] = json!("unsupported");
        return Ok(report);
    }
    // 关联存储与基线共享；用目标档路径承载（同档时两者相同）。
    match session_link::load_store(target_paths) {
        StoreState::Missing => report["storeStatus"] = json!("missing"),
        StoreState::Unavailable(reason) => {
            report["storeStatus"] = json!("unavailable");
            report["storeError"] = json!(reason);
        }
        StoreState::Ready(store) => {
            report["storeStatus"] = json!("ready");
            // 成员数据根：与请求两侧同档的成员直接用请求路径（含单测注入），
            // 其它档位（历史组）按解析器解析，避免误读。
            let member_paths = |group: &LinkGroup, member: &LinkMember| -> SessionPaths {
                let variant = session_link::member_variant(group, member);
                if variant == source_variant {
                    source_paths.clone()
                } else if variant == target_variant {
                    target_paths.clone()
                } else {
                    resolve_source_paths(variant)
                }
            };
            // 成员会话行的存活索引：按数据根缓存，跨档时源/目标各查自己的库一次。
            let mut live = LiveSessionRows::new();
            let groups: Vec<Value> = store
                .groups
                .iter()
                .filter(|group| {
                    has_member_for(group, &source_uid)
                        && has_member_for(group, &target_uid)
                        && group_rows_alive(
                            &mut live,
                            &member_paths,
                            group,
                            &source_uid,
                            &target_uid,
                        )
                })
                .map(|group| {
                    preview_group_item(target_paths, &member_paths, group, &source_uid, &target_uid)
                })
                .collect();
            report["groups"] = json!(groups);
        }
    }
    Ok(report)
}

/// 单个关联组的预览项。
///
/// 记录数与差集只用于向用户解释；能否勾选只由判定结果决定（design §3.2）。
/// `member_paths` 解析成员所在档位的数据根：同档恒为同一份，跨档按成员档位
/// （源读源档、目标读目标档）；关联存储与基线与档位无关，统一走 `store_paths`。
pub(in crate::modules::session) fn preview_group_item(
    store_paths: &SessionPaths,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    group: &LinkGroup,
    source_uid: &str,
    target_uid: &str,
) -> Value {
    // 代表成员口径与存活判定共用：取该账号当前的副本，而不是第一条（旧 superseded 成员
    // 可能排在前面且行已删除，会把可同步的组显示成 `(无标题)`）。
    let source_any = representative_member(group, source_uid);
    let target_any = representative_member(group, target_uid);
    // 组展示名取来源会话（同步保留目标的 sessionId、标题与自定义标题）。
    let (title, cwd) = source_any
        .and_then(|member| {
            let paths = member_paths(group, member);
            session_row_info(&paths, &member.session_id)
        })
        .unwrap_or_else(|| ("(无标题)".to_string(), String::new()));
    let source_summary = member_summary(source_any);
    let target_summary = member_summary(target_any);

    let (Some(source_member), Some(target_member)) = (
        session_link::active_member_for(group, source_uid),
        session_link::active_member_for(group, target_uid),
    ) else {
        // 任一方的有效成员缺失（失效/已被替换）→ 关联不确定，不提供任何写入动作。
        return json!({
            "groupId": group.id,
            "title": title,
            "cwd": cwd,
            "verdict": SyncVerdict::Unknown.as_str(),
            "extraA": 0,
            "extraB": 0,
            "common": 0,
            "defaultChecked": false,
            "availableModes": [],
            "reason": "对应的会话已失效或已被替换，需手动处理",
            // 记录数契约与其它不可验证路径一致：source/target 为 0、baseline 为 null
            // （前端的 `recordCount` 类型按此声明，不能发 null）。
            "recordCount": {"source": 0, "target": 0, "baseline": null},
            "source": source_summary,
            "target": target_summary,
        });
    };

    let source_paths = member_paths(group, source_member);
    let target_paths = member_paths(group, target_member);
    let baseline = session_link::load_pair_baseline(
        store_paths,
        group,
        &source_member.member_id,
        &target_member.member_id,
    );
    let source_content = member_content_state(&source_paths, &source_member.session_id);
    let target_content = member_content_state(&target_paths, &target_member.session_id);
    let rows_match = member_row_owned_by(&source_paths, source_member)
        && member_row_owned_by(&target_paths, target_member);
    let decision = if rows_match {
        session_link::decide_sync(&source_content, &target_content, &baseline)
    } else {
        // 会话行缺失/归属异常：成员实际已失效，与内容不可验证同等对待。
        SyncDecision::unknown("会话记录缺失或归属异常，对应的会话已失效")
    };
    let reason = decision.reason.clone();
    let modes: Vec<&str> = decision
        .verdict
        .available_modes()
        .iter()
        .map(|mode| mode.as_str())
        .collect();
    // 归档资格：独立于正文判定，只看双方会话行的状态。来源已归档 + 目标处于非活跃
    // 终态才授权；缺列/NULL/活跃态一律不授权，也不因此影响既有正文判定的结果。
    // 不可验证的判定不提供任何动作：此时既没有正文模式也不会下发凭据，若仍写出
    // archiveAction，前端摘要会显示「仅同步归档」而用户实际勾不了，等于误导。
    let mut archive = preview_archive_state(
        &source_paths,
        &source_member.session_id,
        &target_paths,
        &target_member.session_id,
    );
    if decision.verdict == SyncVerdict::Unknown {
        archive.action = None;
    }
    let archive_action = archive.action.clone();
    // 归档状态同步作为可选操作，默认不勾选（防误操作），由用户主动勾选授权；正文动作沿用自身默认值。
    let default_checked = decision.default_checked;
    // Keep ordinary sync unavailable for Ahead, but bind its verified snapshot so the
    // session-group "use this copy" flow can explicitly request UnifyOverwrite.
    // 内容一致本身不可勾选，但具备归档资格时要发凭据，否则归档动作无从授权。
    let actionable = !modes.is_empty()
        || decision.verdict == SyncVerdict::Ahead
        || (archive_action.is_some() && decision.verdict == SyncVerdict::Identical);

    let mut item = json!({
        "groupId": group.id,
        "title": title,
        "cwd": cwd,
        "verdict": decision.verdict.as_str(),
        "extraA": decision.extra_a,
        "extraB": decision.extra_b,
        "common": decision.common,
        "defaultChecked": default_checked,
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
    if let Some(action) = archive_action.as_deref() {
        item["archiveAction"] = json!(action);
    }
    if actionable {
        let binding = live_preview_binding(
            group,
            source_member,
            target_member,
            &source_content,
            &target_content,
            &baseline,
            decision.verdict,
            &archive,
        );
        match session_link::save_preview_token(store_paths, binding) {
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
