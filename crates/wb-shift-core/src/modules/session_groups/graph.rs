//! 组状态图：共同基线/独立分支识别、成员状态归约与统一同步批次编排。

use super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUnifyTarget {
    pub target_member_id: String,
    pub preview_token: String,
    pub mode: String,
}

/// Apply an explicit group choice in one lifecycle window. WorkBuddy closes only a running
/// variant whose current login is a write target; the VS Code plugin closes the editor when the
/// caller was authorized (`restart`). Only clients closed here are reopened. Pair kernels still
/// validate every preview token.
pub fn sync_unify_batch(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
    restart: bool,
) -> Result<Value, String> {
    match client {
        SessionClient::Workbuddy => {
            sync_unify_batch_workbuddy(scope, group_id, source_member_id, targets)
        }
        SessionClient::VscodeExt => {
            sync_unify_batch_vscode(scope, group_id, source_member_id, targets, restart)
        }
        SessionClient::CodebuddyIde => {
            Err("仅 WorkBuddy 与 CodeBuddy 插件会话支持整组统一".to_string())
        }
    }
}

/// Resolve + validate every target before anything is closed or written.
pub(super) fn prepare_unify_targets(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
) -> Result<Vec<(PairContext, SyncSelection)>, String> {
    if targets.is_empty() {
        return Err("没有需要同步的目标副本".to_string());
    }
    let mut seen = HashSet::new();
    let mut prepared = Vec::with_capacity(targets.len());
    for target in targets {
        if !seen.insert(target.target_member_id.as_str()) {
            return Err("目标副本重复，请重新检查会话".to_string());
        }
        let pair = resolve_pair(
            client,
            scope,
            group_id,
            source_member_id,
            &target.target_member_id,
        )?;
        let selection = parse_selection(group_id, &target.preview_token, &target.mode)?;
        prepared.push((pair, selection));
    }
    Ok(prepared)
}

/// WorkBuddy：只关闭「运行中且当前登录是写入目标」的档位，只重开本次关闭的档位。
pub(super) fn sync_unify_batch_workbuddy(
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
) -> Result<Value, String> {
    let client = SessionClient::Workbuddy;
    let prepared = prepare_unify_targets(client, scope, group_id, source_member_id, targets)?;

    let restart_variants = WbVariant::ALL
        .into_iter()
        .filter(|variant| {
            prepared.iter().any(|(pair, _)| {
                account::variant_of(&pair.target_account) == *variant
                    && running_target_needs_restart(*variant, &pair.target.uid)
            })
        })
        .collect::<Vec<_>>();
    let mut closed = Vec::new();
    for variant in restart_variants {
        if let Err(error) = process::close_workbuddy(variant, 20) {
            let reopen_errors = reopen_workbuddy_variants(&closed);
            let suffix = if reopen_errors.is_empty() {
                String::new()
            } else {
                format!(
                    "；此前关闭的客户端重新打开失败：{}",
                    reopen_errors.join("；")
                )
            };
            return Err(format!(
                "关闭 {}失败：{error}{suffix}",
                workbuddy_variant_label(variant)
            ));
        }
        closed.push(variant);
    }

    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [], "restartedVariants": [] });
    for (pair, selection) in prepared {
        match run_pair_sync(client, scope, &pair, &[selection], false) {
            Ok(report) => {
                merge_sync_report(&mut combined, report);
                if combined["needsRecovery"] == true {
                    break;
                }
            }
            Err(error) => {
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}：{error}", pair.target.uid),
                }));
                break;
            }
        }
    }

    if combined["needsRecovery"] == true {
        combined["errors"].as_array_mut().unwrap().push(json!({
            "error": "会话写入待恢复，已暂停重新打开客户端；请先处理恢复提示",
        }));
        return Ok(combined);
    }
    for variant in &closed {
        match session::recover_pending_session_operations(*variant) {
            Ok(recovery) if !crate::modules::switch::recovery_blocks_startup(&recovery) => {}
            Ok(recovery) => {
                combined["needsRecovery"] = json!(true);
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}仍有待恢复的会话写入（{}），已暂停重新打开客户端", workbuddy_variant_label(*variant), crate::modules::switch::recovery_blocking_detail(&recovery)),
                }));
            }
            Err(error) => {
                combined["needsRecovery"] = json!(true);
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}无法检查会话恢复状态（{error}），已暂停重新打开客户端", workbuddy_variant_label(*variant)),
                }));
            }
        }
    }
    if combined["needsRecovery"] == true {
        return Ok(combined);
    }
    let mut reopened = Vec::new();
    for variant in closed {
        if let Err(error) = process::launch_workbuddy(variant, None) {
            combined["errors"].as_array_mut().unwrap().push(json!({
                "error": format!("{}会话已处理，但客户端重新打开失败：{error}", workbuddy_variant_label(variant)),
            }));
        } else {
            reopened.push(variant.as_str());
        }
    }
    combined["restartedVariants"] = json!(reopened);
    Ok(combined)
}

/// VS Code 插件：全部校验通过后打开编辑器生命周期窗口（`restart=true` 时关闭），
/// 窗口内跑完全部目标再收尾重开；插件侧没有 WorkBuddy 的 `needsRecovery` 语义。
pub(super) fn sync_unify_batch_vscode(
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
    restart: bool,
) -> Result<Value, String> {
    let client = SessionClient::VscodeExt;
    let prepared = prepare_unify_targets(client, scope, group_id, source_member_id, targets)?;
    let target_uids: Vec<&str> = prepared
        .iter()
        .map(|(pair, _)| pair.target.uid.as_str())
        .collect();
    let guard = prepare_editor_window(client, restart, VSCODE_SYNC_RUNNING_HINT, &target_uids)?;
    let editor_prepared = guard.is_some();
    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [] });
    for (pair, selection) in prepared {
        match run_pair_sync(client, scope, &pair, &[selection], editor_prepared) {
            Ok(report) => merge_sync_report(&mut combined, report),
            Err(error) => {
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}：{error}", pair.target.uid),
                }));
                break;
            }
        }
    }
    settle_editor_window(guard, Ok(combined))
}

/// Recompute the group source on the server and sync only targets proven fast-forward safe.
pub fn sync_safe_batch(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    restart: bool,
) -> Result<Value, String> {
    let detail = detail(client, scope, group_id)?;
    let source_member_id = detail
        .get("safeSourceMemberId")
        .and_then(Value::as_str)
        .ok_or_else(|| "该组无法确认安全同步来源".to_string())?
        .to_string();
    let targets = detail
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|member| {
            member.get("memberId").and_then(Value::as_str) != Some(source_member_id.as_str())
                && member.get("versionStatus").and_then(Value::as_str) == Some("behind")
        })
        .filter_map(|member| {
            member
                .get("memberId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [] });
    // 准备阶段：全部目标的复核与凭据都在关闭编辑器之前完成（复核失败/不再安全快进的项照旧跳过），
    // 避免为注定失败的请求打断用户。
    let mut prepared = Vec::new();
    for target_member_id in targets {
        let pair = resolve_pair(
            client,
            scope,
            group_id,
            &source_member_id,
            &target_member_id,
        )?;
        let preview = preview_resolved_pair(client, scope, &pair)?;
        if preview.get("verdict").and_then(Value::as_str) != Some("fastForward") {
            combined["skipped"].as_array_mut().unwrap().push(json!({
                "groupId": group_id,
                "targetMemberId": target_member_id,
                "reasonCode": "recheckNotFastForward",
                "message": "重新检查后该副本不再属于安全快进范围",
            }));
            continue;
        }
        let token = preview
            .get("previewToken")
            .and_then(Value::as_str)
            .ok_or_else(|| "重新预览未返回可执行凭据".to_string())?;
        let selection = parse_selection(group_id, token, "fastForward")?;
        prepared.push((pair, selection));
    }
    let target_uids: Vec<&str> = prepared
        .iter()
        .map(|(pair, _)| pair.target.uid.as_str())
        .collect();
    let guard = prepare_editor_window(client, restart, VSCODE_SYNC_RUNNING_HINT, &target_uids)?;
    let editor_prepared = guard.is_some();
    let result = (|| -> Result<Value, String> {
        for (pair, selection) in prepared {
            let report = run_pair_sync(client, scope, &pair, &[selection], editor_prepared)?;
            merge_sync_report(&mut combined, report);
        }
        Ok(combined)
    })();
    settle_editor_window(guard, result)
}

/// Recognize one present shared snapshot followed by independent append-only branches.
/// This uses normalized ordered content, so timestamps and record counts cannot invent ancestry.
/// A more complex history stays in the generic divergence state.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CommonBaseBranches {
    pub(super) common: Vec<usize>,
    pub(super) branches: Vec<Vec<usize>>,
}

pub(super) fn common_base_branches(
    views: &[MemberView],
    active: &[usize],
) -> Option<CommonBaseBranches> {
    if active.len() < 3 || active.len() != views.len() {
        return None;
    }
    let lines = |index: usize| match &views[index].content {
        ContentState::Ready(snapshot) => Some(snapshot.normalized.line_digests.as_slice()),
        _ => None,
    };
    let mut versions: Vec<Vec<usize>> = Vec::new();
    for &index in active {
        let content = lines(index)?;
        if let Some(version) = versions
            .iter_mut()
            .find(|version| lines(version[0]) == Some(content))
        {
            version.push(index);
        } else {
            versions.push(vec![index]);
        }
    }
    if versions.len() < 3 {
        return None;
    }
    let common_index = versions.iter().position(|version| {
        let common = lines(version[0]).expect("active members have readable content");
        versions
            .iter()
            .filter(|other| *other != version)
            .all(|other| {
                let branch = lines(other[0]).expect("active members have readable content");
                branch.len() > common.len() && branch.starts_with(common)
            })
    })?;
    let common = versions.remove(common_index);
    // Each remaining version must be independently advanced from the shared snapshot.
    if versions.iter().enumerate().any(|(index, branch)| {
        versions.iter().skip(index + 1).any(|other| {
            let left = lines(branch[0]).expect("active members have readable content");
            let right = lines(other[0]).expect("active members have readable content");
            left.starts_with(right) || right.starts_with(left)
        })
    }) {
        return None;
    }
    Some(CommonBaseBranches {
        common,
        branches: versions,
    })
}

/// Reduce pairwise decisions into stable member states and a group summary.
///
/// `views` are already ordered by account key, UID, and member ID. The reducer is deliberately
/// independent of storage and process state so the conservative source-selection contract can be
/// covered with deterministic unit tests.
pub(super) fn aggregate_group_state(
    views: &mut [MemberView],
    active: &[usize],
    matrix: &HashMap<(usize, usize), (SyncVerdict, String)>,
) -> (Option<usize>, &'static str) {
    let mut safe_candidates = active
        .iter()
        .copied()
        .filter(|&candidate| {
            if !matches!(&views[candidate].content, ContentState::Ready(_)) {
                return false;
            }
            active
                .iter()
                .copied()
                .filter(|&other| other != candidate)
                .all(|other| {
                    matrix.get(&(candidate, other)).is_some_and(|(verdict, _)| {
                        matches!(verdict, SyncVerdict::Identical | SyncVerdict::FastForward)
                    })
                })
        })
        .collect::<Vec<_>>();
    safe_candidates.sort_by(|&left, &right| {
        views[left]
            .account_key
            .cmp(&views[right].account_key)
            .then_with(|| views[left].member.uid.cmp(&views[right].member.uid))
            .then_with(|| {
                views[left]
                    .member
                    .member_id
                    .cmp(&views[right].member.member_id)
            })
    });
    let safe_source = safe_candidates.first().copied();

    let mut has_behind = false;
    let mut has_diverge = false;
    let mut has_missing = false;
    for (index, view) in views.iter_mut().enumerate() {
        match view.member.state {
            MemberState::Stale => {
                view.version_status = "stale";
                view.reason = "该副本已标记为失效".to_string();
            }
            MemberState::Superseded => {
                view.version_status = "superseded";
                view.reason = "该副本已被更新成员替代".to_string();
            }
            MemberState::Active => match &view.content {
                ContentState::Missing => {
                    view.version_status = "missing";
                    view.reason = "会话内容不存在，无法同步".to_string();
                    has_missing = true;
                }
                ContentState::Unavailable(reason) => {
                    view.version_status = "unknown";
                    view.reason = reason.clone();
                }
                ContentState::Ready(_) => {
                    if let Some(source_index) = safe_source {
                        if index == source_index
                            || matrix
                                .get(&(source_index, index))
                                .is_some_and(|(verdict, _)| *verdict == SyncVerdict::Identical)
                        {
                            view.version_status = "latest";
                            if index != source_index {
                                view.reason = "内容与操作来源一致".to_string();
                            }
                        } else if let Some((SyncVerdict::FastForward, reason)) =
                            matrix.get(&(source_index, index))
                        {
                            view.version_status = "behind";
                            view.reason = reason.clone();
                            has_behind = true;
                        } else {
                            view.version_status = "unknown";
                            view.reason = "重新检查后无法确认此成员状态".to_string();
                        }
                    } else {
                        let reasons = active
                            .iter()
                            .filter_map(|other| matrix.get(&(*other, index)))
                            .collect::<Vec<_>>();
                        if reasons
                            .iter()
                            .any(|(verdict, _)| *verdict == SyncVerdict::Diverge)
                        {
                            view.version_status = "diverge";
                            view.reason = reasons
                                .iter()
                                .find(|(verdict, _)| *verdict == SyncVerdict::Diverge)
                                .map(|(_, reason)| reason.clone())
                                .unwrap_or_default();
                            has_diverge = true;
                        } else {
                            view.version_status = "unknown";
                            view.reason = reasons
                                .first()
                                .map(|(_, reason)| reason.clone())
                                .unwrap_or_else(|| "缺少可比较的关联成员".to_string());
                        }
                    }
                }
            },
        }
    }

    let summary_status = if safe_source.is_some() {
        if has_behind {
            "behind"
        } else {
            "latest"
        }
    } else if has_diverge {
        "diverge"
    } else if has_missing {
        "missing"
    } else {
        // 无任何可判定的状态标记时同样回退为「无法确认」（原 `has_unknown` 分支与 else 同体）。
        "unknown"
    };

    (safe_source, summary_status)
}

pub(super) fn content_preview(content: &ContentState) -> Vec<Value> {
    let ContentState::Ready(snapshot) = content else {
        return Vec::new();
    };
    let mut recent = snapshot
        .text
        .lines()
        .rev()
        .filter_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            let text = record_text(&record)?;
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            let speaker = match record
                .get("role")
                .or_else(|| record.get("type"))
                .and_then(Value::as_str)
            {
                Some("user") => "用户",
                Some("assistant") => "助手",
                _ => "记录",
            };
            Some(json!({ "speaker": speaker, "text": text.chars().take(240).collect::<String>() }))
        })
        .take(6)
        .collect::<Vec<_>>();
    recent.reverse();
    recent
}
