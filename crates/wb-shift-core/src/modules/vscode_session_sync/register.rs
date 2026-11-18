//! 复制会话登记：会话定位、上下文、register_copied_sessions 系列与 register_one。
//!
//! 「工作区索引怎么读、读到没有」（三态可用性 + `会话 id → 定位`）在
//! [`super::index`]，这里只消费；不要在登记路径上另写一份目录遍历。

use super::*;

// ---------------------------------------------------------------------------
// 登记上下文
// ---------------------------------------------------------------------------

/// Resolve one explicitly identified session without consulting the client's active login UID.
/// The opaque workspace hash is returned only as a display locator; it is never decoded.
pub(crate) fn session_location_and_content(
    spec: SessionStoreSpec,
    root: &Path,
    uid: &str,
    conversation_id: &str,
) -> Option<(String, String, ContentState)> {
    let index = conversation_index(spec, root, uid);
    let locator = index.get(conversation_id)?;
    let dir = conversation_dir(spec, root, uid, &locator.workspace_hash, conversation_id);
    Some((
        locator.workspace_hash.clone(),
        locator.title.clone(),
        vscode_session_link::read_session_content(&dir, conversation_id),
    ))
}

/// 一次预览 / 同步内不变的输入：数据仓、账号、数据根与两侧会话定位索引。
pub(super) struct SyncContext<'a> {
    pub(super) spec: SessionStoreSpec,
    pub(super) paths: &'a SessionPaths,
    pub(super) root: &'a Path,
    pub(super) source_uid: &'a str,
    pub(super) target_uid: &'a str,
    pub(super) source_index: &'a BTreeMap<String, ConversationLocator>,
    pub(super) target_index: &'a BTreeMap<String, ConversationLocator>,
    /// `Some` 时只处理该档位的关联组。IDE 两侧共用一份表，必须带上；
    /// VS Code 插件跨档位复制，保持 `None`（不过滤）。
    pub(super) variant_filter: Option<WbVariant>,
}

impl SyncContext<'_> {
    /// 账号对应的定位索引（只处理来源与目标两个账号）。
    pub(super) fn index_of(&self, uid: &str) -> &BTreeMap<String, ConversationLocator> {
        if uid == self.source_uid {
            self.source_index
        } else {
            self.target_index
        }
    }

    /// 某账号某会话的目录（索引里找不到该会话时为 `None`）。
    pub(super) fn dir_of(&self, uid: &str, conversation_id: &str) -> Option<PathBuf> {
        self.index_of(uid).get(conversation_id).map(|locator| {
            conversation_dir(
                self.spec,
                self.root,
                uid,
                &locator.workspace_hash,
                conversation_id,
            )
        })
    }

    /// 会话内容状态；索引里找不到该会话（被删除/改归属）时按 `Missing`（判定侧得到 unknown）。
    pub(super) fn content_of(&self, uid: &str, conversation_id: &str) -> ContentState {
        match self.dir_of(uid, conversation_id) {
            Some(dir) => vscode_session_link::read_session_content(&dir, conversation_id),
            None => ContentState::Missing,
        }
    }

    /// 某账号某会话的定位信息（工作区 hash + 标题）。
    pub(super) fn locator_of(
        &self,
        uid: &str,
        conversation_id: &str,
    ) -> Option<&ConversationLocator> {
        self.index_of(uid).get(conversation_id)
    }

    /// 会话标题（取工作区索引里的名字；插件没有会话级 cwd，留空由标题承担区分）。
    pub(super) fn title_of(&self, uid: &str, conversation_id: &str) -> String {
        self.index_of(uid)
            .get(conversation_id)
            .map(|locator| locator.title.clone())
            .unwrap_or_else(|| "(无标题)".to_string())
    }
}

/// Account metadata is display-only; plugin identity and storage remain UID based.
pub(super) fn account_metadata_for_uid(
    paths: &SessionPaths,
    uid: &str,
) -> (Option<String>, Option<WbVariant>) {
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    match accounts
        .iter()
        .find(|account| account.get("uid").and_then(Value::as_str) == Some(uid))
    {
        Some(account) => (
            account.get("id").and_then(Value::as_str).map(String::from),
            Some(account::variant_of(account)),
        ),
        None => (None, None),
    }
}

/// 取报告里的账号 uid（空串按缺失处理）。
pub(super) fn uid_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// R1：复制成功后登记关联（design §5）
// ---------------------------------------------------------------------------

/// 为本次复制成功的会话登记「源 ↔ 副本」关联，返回逐条失败原因。
///
/// **登记失败不回滚复制**：复制已完成、文件已落盘，删除副本只会让用户两头落空；
/// 失败原因由调用方并入报告 `linkErrors[]`，前端提示「已复制但未建立关联」。
pub fn register_copied_sessions(
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    report: &Value,
) -> Vec<Value> {
    register_copied_sessions_in(VSCODE_STORE, root, paths, variant, report)
}

/// [`register_copied_sessions`] 的数据仓参数化版本（VS Code 插件用：不按 variant 拆组）。
pub fn register_copied_sessions_in(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    report: &Value,
) -> Vec<Value> {
    // 插件侧同一扩展数据仓可跨档位复制，登记时不按 variant 拆组。
    register_copied_sessions_inner(spec, root, paths, variant, report, false)
}

/// IDE 两侧共用一份关联表时的登记：只复用同 `variant` 的组，不并入另一档已有的组。
pub(crate) fn register_copied_sessions_isolated(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    report: &Value,
) -> Vec<Value> {
    register_copied_sessions_inner(spec, root, paths, variant, report, true)
}

pub(super) fn register_copied_sessions_inner(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    report: &Value,
    isolate_variant: bool,
) -> Vec<Value> {
    let source_uid = uid_of(report, "sourceUid");
    let target_uid = uid_of(report, "targetUid");
    let (Some(source_uid), Some(target_uid)) = (source_uid, target_uid) else {
        return vec![json!({ "error": "复制报告缺少账号信息，未能建立会话关联" })];
    };
    let source_index = conversation_index(spec, root, &source_uid);
    let target_index = conversation_index(spec, root, &target_uid);
    let context = SyncContext {
        spec,
        paths,
        root,
        source_uid: &source_uid,
        target_uid: &target_uid,
        source_index: &source_index,
        target_index: &target_index,
        variant_filter: None,
    };
    let mut errors: Vec<Value> = Vec::new();
    let copied = report
        .get("copied")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for item in copied {
        let workspace_hash = item.get("workspaceHash").and_then(Value::as_str);
        let source_id = item.get("oldId").and_then(Value::as_str);
        let target_id = item.get("newId").and_then(Value::as_str);
        let (Some(workspace_hash), Some(source_id), Some(target_id)) =
            (workspace_hash, source_id, target_id)
        else {
            errors.push(json!({ "error": "复制报告条目缺少会话信息，未能建立会话关联" }));
            continue;
        };
        let outcome = register_one(&context, variant, isolate_variant, source_id, target_id);
        if let Err(reason) = outcome {
            errors.push(json!({
                "workspaceHash": workspace_hash,
                "conversationId": source_id,
                "error": reason,
            }));
        }
    }
    errors
}

/// 登记单个副本：比对双方归一化摘要，写组 / 成员 / 配对基线（全部在存储锁内）。
pub(super) fn register_one(
    context: &SyncContext<'_>,
    variant: WbVariant,
    isolate_variant: bool,
    source_id: &str,
    target_id: &str,
) -> Result<(), String> {
    let paths = context.paths;
    let source_uid = context.source_uid;
    let target_uid = context.target_uid;
    let source_content = ready_content(context, source_uid, source_id, "来源")?;
    let target_content = ready_content(context, target_uid, target_id, "副本")?;
    // 基线只能建立在「双方内容确实一致」之上：不一致时不登记，
    // 让该会话以「未关联」呈现，而不是留下一条错误的共同基线。
    if source_content.normalized.line_digests != target_content.normalized.line_digests {
        return Err("已复制但副本内容与来源不一致，未建立关联".to_string());
    }
    let normalized = source_content.normalized.clone();
    let (source_account_id, source_variant) = account_metadata_for_uid(paths, source_uid);
    let (target_account_id, target_variant) = account_metadata_for_uid(paths, target_uid);
    let source_variant = if isolate_variant {
        Some(variant)
    } else {
        source_variant
    };
    let target_variant = if isolate_variant {
        Some(variant)
    } else {
        target_variant
    };
    let (source_uid, target_uid) = (source_uid.to_string(), target_uid.to_string());
    let (source_id, target_id) = (source_id.to_string(), target_id.to_string());
    session_link::with_link_store_write(paths, move |store| {
        // 同一逻辑会话复用同一组：按「源身份」找组，找不到才新建。
        // 插件路径不按 variant 过滤（同一扩展数据仓可跨档位复制）。
        // IDE 路径 `isolate_variant`：只复用同档位的组，避免国内版 / 国际版并成一组。
        let index = match store.groups.iter().position(|group| {
            let same_variant = !isolate_variant || group.variant == variant;
            same_variant
                && group
                    .members
                    .iter()
                    .any(|member| member.uid == source_uid && member.session_id == source_id)
        }) {
            Some(index) => index,
            None => {
                store.groups.push(LinkGroup {
                    id: uuid::Uuid::new_v4().to_string(),
                    variant,
                    created_at: now_ms(),
                    members: Vec::new(),
                    pair_bases: Vec::new(),
                });
                store.groups.len() - 1
            }
        };
        let group = &mut store.groups[index];
        let source_member_id = match session_link::find_member(group, &source_uid, &source_id)
            .map(|m| m.member_id.clone())
        {
            Some(member_id) => {
                session_link::set_member_state(group, &member_id, MemberState::Active);
                if let Some(value) = source_variant {
                    group
                        .members
                        .iter_mut()
                        .find(|member| member.member_id == member_id)
                        .unwrap()
                        .variant = Some(value);
                }
                member_id
            }
            None => {
                let member_id = uuid::Uuid::new_v4().to_string();
                session_link::add_active_member(
                    group,
                    LinkMember {
                        member_id: member_id.clone(),
                        account_id: source_account_id,
                        uid: source_uid.clone(),
                        session_id: source_id.clone(),
                        variant: Some(source_variant.unwrap_or(variant)),
                        state: MemberState::Active,
                        linked_at: now_ms(),
                        last_synced_at: None,
                    },
                );
                member_id
            }
        };
        let target_member_id = match session_link::find_member(group, &target_uid, &target_id)
            .map(|m| m.member_id.clone())
        {
            Some(member_id) => {
                session_link::set_member_state(group, &member_id, MemberState::Active);
                if let Some(value) = target_variant {
                    group
                        .members
                        .iter_mut()
                        .find(|member| member.member_id == member_id)
                        .unwrap()
                        .variant = Some(value);
                }
                member_id
            }
            None => {
                let member_id = uuid::Uuid::new_v4().to_string();
                // 同账号上一次的 active 成员在这里被显式 supersede：保留记录、不再有效。
                session_link::add_active_member(
                    group,
                    LinkMember {
                        member_id: member_id.clone(),
                        account_id: target_account_id,
                        uid: target_uid.clone(),
                        session_id: target_id.clone(),
                        variant: Some(target_variant.unwrap_or(variant)),
                        state: MemberState::Active,
                        linked_at: now_ms(),
                        last_synced_at: None,
                    },
                );
                member_id
            }
        };
        // 复制完成时的双方内容即共同基线（design §5：复制时即打基线）。
        let baseline_ref = uuid::Uuid::new_v4().to_string();
        session_link::save_baseline(paths, &baseline_ref, &normalized)?;
        session_link::set_pair_base(
            group,
            &source_member_id,
            &target_member_id,
            &baseline_ref,
            NORMALIZATION_VERSION,
        );
        Ok(())
    })
}

/// 登记时读一侧内容：定位失败或内容不可验证都按可读原因拒绝登记。
pub(super) fn ready_content(
    context: &SyncContext<'_>,
    uid: &str,
    conversation_id: &str,
    what: &str,
) -> Result<ContentSnapshot, String> {
    match context.content_of(uid, conversation_id) {
        ContentState::Ready(snapshot) => Ok(snapshot),
        ContentState::Missing => Err(format!("已复制但找不到{what}内容，未建立关联")),
        ContentState::Unavailable(reason) => Err(format!("内容无法确认（{reason}），未建立关联")),
    }
}
