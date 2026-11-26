//! 跨组安全批量同步：一次确认，把所有可安全快进的副本同步完。
//!
//! 与单组 [`sync_safe_batch`] 的区别只在**编排**：逐组重算安全来源 → 只收
//! `versionStatus == "behind"` 的成员 → 全量就绪后只开一次编辑器生命周期窗口 → 顺序执行 →
//! 统一收尾重开。判定本身仍走 `resolve_pair` / `preview_resolved_pair` / `run_pair_sync`，
//! 这里不重写任何一条判据，也不替用户扩大权限。
//!
//! 存在分歧 / 无法确认来源 / 没有落后副本的组一律**整组跳过**（`skippedGroups` + `reasonCode`），
//! 不猜、也不用「覆盖」替用户决定；单个目标复核后不再属于快进范围时同样只跳过，不影响同组其它目标。

use super::*;

/// 一组的准备结论：整组跳过（附原因），或「有可同步的落后副本」。
///
/// 纯数据（不读存储、不碰编辑器），因此「这个组该不该进批量、进哪些目标」可以被单测钉住。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GroupPlan {
    /// 整组不参与本次批量：`reason_code` 与文案直接回给前端做汇总展示。
    Skip {
        /// 组标题（详情读不出时为空串：前端回退显示 `groupId`）。
        title: String,
        reason_code: &'static str,
        message: String,
    },
    /// 该组有来源与落后副本，可以逐目标复核。
    Ready {
        title: String,
        source_member_id: String,
        targets: Vec<String>,
    },
}

/// 跨组安全批量同步：逐组、逐目标地只同步可安全快进的副本，返回跨组汇总报告。
///
/// 失败语义：
/// - 组级问题（详情读不出 / 没有安全来源 / 没有落后副本）进 `skippedGroups`，继续下一组；
/// - 目标级问题（复核失败 / 复核后不再快进）进 `skipped`，继续下一个目标；
/// - 真写盘时单个目标返回错误 → `errors` 记 `{groupId, error}` 后 **break**（不掩盖半途状态）；
/// - 任一目标报 `needsRecovery` → 立刻 stop，交给既有恢复流程。
pub fn sync_all_safe(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_ids: &[String],
    restart: bool,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    let mut seen = HashSet::new();
    let group_ids = group_ids
        .iter()
        .map(String::as_str)
        .filter(|group_id| seen.insert(*group_id))
        .collect::<Vec<_>>();
    if group_ids.is_empty() {
        return Err("没有需要同步的会话组".to_string());
    }

    let mut report = json!({
        "client": client.as_str(),
        "variantScope": scope.map(WbVariant::as_str),
        "groups": [],
        "skippedGroups": [],
        "synced": [],
        "skipped": [],
        "errors": [],
        "needsRecovery": false,
        "temporaryFiles": [],
    });

    // 准备阶段：把所有组的复核都在关闭编辑器之前做完。为注定失败的请求打断用户是
    // 无谓的，而且编辑器只该关一次（见下方 `prepare_editor_window` 的调用）。
    let mut prepared: Vec<(String, String, PairContext, SyncSelection)> = Vec::new();
    for group_id in group_ids {
        let (title, source_member_id, targets) = match plan_group(client, scope, group_id) {
            GroupPlan::Skip {
                title,
                reason_code,
                message,
            } => {
                push_entry(
                    &mut report,
                    "skippedGroups",
                    json!({
                        "groupId": group_id,
                        "title": title,
                        "reasonCode": reason_code,
                        "message": message,
                    }),
                );
                continue;
            }
            GroupPlan::Ready {
                title,
                source_member_id,
                targets,
            } => (title, source_member_id, targets),
        };
        // 本组真正入队的目标数：全部复核通过才计为「覆盖一个组」，避免前端 toast 把
        // 「一个副本都没同步的组」也算进「覆盖 M 个组」。
        let mut prepared_count = 0;
        for target_member_id in targets {
            match prepare_target(
                client,
                scope,
                group_id,
                &source_member_id,
                &target_member_id,
            ) {
                Ok((pair, selection)) => {
                    prepared.push((group_id.to_string(), title.clone(), pair, selection));
                    prepared_count += 1;
                }
                Err((reason_code, message)) => push_entry(
                    &mut report,
                    "skipped",
                    json!({
                        "groupId": group_id,
                        "targetMemberId": target_member_id,
                        "reasonCode": reason_code,
                        "message": message,
                    }),
                ),
            }
        }
        // 复核之后才写 `groups`：`prepared_count == 0` 的组不出现（只留 `skipped` 里的逐目标原因）。
        if prepared_count > 0 {
            push_entry(
                &mut report,
                "groups",
                json!({
                    "groupId": group_id,
                    "title": title,
                    "targetCount": prepared_count,
                }),
            );
        }
    }

    // 没有任何就绪目标：直接回报告，不关编辑器、也不制造「正在同步」的假象。
    if prepared.is_empty() {
        return Ok(report);
    }

    let target_uids = prepared
        .iter()
        .map(|(_, _, pair, _)| pair.target.uid.as_str())
        .collect::<Vec<_>>();
    let guard = prepare_editor_window(client, restart, false, &target_uids)?;
    let editor_prepared = guard.is_some();
    // 执行段交给 `run_prepared`：它的错误与报告统一经 `settle_editor_window` 收尾，
    // 保证「关了的宿主一定重开」，也不会出现「写入失败但编辑器还关着」的状态。
    settle_editor_window(
        guard,
        run_prepared(client, scope, report, prepared, editor_prepared),
    )
}

/// 执行段：顺序跑完所有就绪目标，把每份子报告的条目补上组号后并入跨组汇总。
///
/// 单个目标写盘报错即 `break` 并记一条 `{groupId, error}`（`error` 前缀组标题，用户能定位到组）；
/// 任何目标报 `needsRecovery` 也立刻停，交给既有恢复流程，不继续写别的组。
fn run_prepared(
    client: SessionClient,
    scope: Option<WbVariant>,
    mut report: Value,
    prepared: Vec<(String, String, PairContext, SyncSelection)>,
    editor_prepared: bool,
) -> Result<Value, String> {
    for (group_id, title, pair, selection) in prepared {
        match run_pair_sync(client, scope, &pair, &[selection], editor_prepared) {
            Ok(item) => {
                let mut item = item;
                stamp_group_id(&mut item, &group_id);
                merge_sync_report(&mut report, item);
                if report["needsRecovery"] == true {
                    break;
                }
            }
            Err(error) => {
                push_entry(
                    &mut report,
                    "errors",
                    json!({
                        "groupId": group_id,
                        "error": format!("{title}：{error}"),
                    }),
                );
                break;
            }
        }
    }
    Ok(report)
}

/// 读一次组详情并决定该组怎么参与本次批量（不写盘、不碰编辑器）。
fn plan_group(client: SessionClient, scope: Option<WbVariant>, group_id: &str) -> GroupPlan {
    match detail(client, scope, group_id) {
        Ok(detail) => plan_from_detail(&detail),
        // 详情失败（组已删除 / 存储不可用 / 索引读不全）用原错误：用户需要看到真实原因。
        Err(error) => GroupPlan::Skip {
            title: String::new(),
            reason_code: "detailFailed",
            message: error,
        },
    }
}

/// 纯函数：把一份组详情折成「该组怎么参与本次批量」。
///
/// 判据全部来自既有载荷（`safeSourceMemberId` + 成员 `versionStatus`），不重算任何内容比较：
/// 有安全来源且存在 `behind` 成员才进批量，否则整组跳过并给出可读原因。
pub(super) fn plan_from_detail(detail: &Value) -> GroupPlan {
    let title = detail
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(source_member_id) = detail
        .get("safeSourceMemberId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return GroupPlan::Skip {
            title,
            reason_code: "noSafeSource",
            message: "该组无法确认安全同步来源（存在分歧），请打开该组手动选择保留哪一份"
                .to_string(),
        };
    };
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
    if targets.is_empty() {
        return GroupPlan::Skip {
            title,
            reason_code: "noBehind",
            message: "该组没有待同步的副本（可能已经是最新）".to_string(),
        };
    }
    GroupPlan::Ready {
        title,
        source_member_id,
        targets,
    }
}

/// 单个目标的复核：先 `resolve_pair`，再 `preview_resolved_pair` 确认仍是安全快进。
///
/// 只有 `verdict == "fastForward"` 才用 `parse_selection(.., "fastForward")` 入队：
/// 复核结果变了（别人刚写了这个副本）就只跳过，不给这条目标任何其它写入模式。
fn prepare_target(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    target_member_id: &str,
) -> Result<(PairContext, SyncSelection), (&'static str, String)> {
    let pair = resolve_pair(client, scope, group_id, source_member_id, target_member_id)
        .map_err(|error| ("previewFailed", error))?;
    let preview =
        preview_resolved_pair(client, scope, &pair).map_err(|error| ("previewFailed", error))?;
    if preview.get("verdict").and_then(Value::as_str) != Some("fastForward") {
        // 与 `sync_safe_batch` 同一句文案（单一来源口径：用户两次看到的解释必须一致）。
        return Err((
            "recheckNotFastForward",
            "重新检查后该副本不再属于安全快进范围".to_string(),
        ));
    }
    let token = preview
        .get("previewToken")
        .and_then(Value::as_str)
        .ok_or(("previewFailed", "重新预览未返回可执行凭据".to_string()))?;
    let selection = parse_selection(group_id, token, "fastForward")
        .map_err(|error| ("previewFailed", error))?;
    Ok((pair, selection))
}

/// 给子报告的每条 `synced` / `skipped` / `errors` 补上 `groupId`。
///
/// 跨组汇总后必须还能回溯到具体会话组：内核已经带 `groupId` 的条目不覆盖，只补缺失的。
pub(super) fn stamp_group_id(item: &mut Value, group_id: &str) {
    for key in ["synced", "skipped", "errors"] {
        let Some(items) = item.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        for entry in items {
            if let Value::Object(map) = entry {
                map.entry("groupId").or_insert_with(|| json!(group_id));
            }
        }
    }
}

/// 往报告的数组槽位追加一条记录（槽位缺失或不是数组时补成数组，与 `merge_sync_report` 同风格）。
fn push_entry(report: &mut Value, key: &str, entry: Value) {
    let Value::Object(map) = report else {
        return;
    };
    match map.entry(key.to_owned()).or_insert_with(|| json!([])) {
        Value::Array(items) => items.push(entry),
        slot => *slot = json!([entry]),
    }
}
