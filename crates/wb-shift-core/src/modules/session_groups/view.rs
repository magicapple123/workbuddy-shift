//! 列表/详情视图与分组载荷：可搜索摘要、成员快照与展示字段。

use super::*;
use crate::modules::locks;

#[derive(Debug, Clone)]
pub(super) struct MemberView {
    pub(super) member: LinkMember,
    pub(super) account_key: String,
    pub(super) account_name: String,
    pub(super) title: String,
    pub(super) project_label: String,
    pub(super) updated_at: i64,
    pub(super) content: ContentState,
    pub(super) reason: String,
    pub(super) version_status: &'static str,
}

/// List lightweight searchable summaries. IDE must include its mandatory variant scope.
pub fn list(client: SessionClient, scope: Option<WbVariant>) -> Result<Value, String> {
    list_at(&client.store_paths(), client, scope)
}

/// [`list`] 的可注入版本：只替换**关联存储路径**，插件 / IDE 的会话数据根仍按客户端解析
/// （与 [`visible_groups_at`] 同口径）。
///
/// 载荷里 `deadGroupCount` / `pruneBlocked` 的**唯一**写入点是 [`list_payload`]：用例与
/// 生产走同一份装配，不会出现「纯函数测到了、真实载荷却没接上」的覆盖缺口。
pub(super) fn list_at(
    paths: &SessionPaths,
    client: SessionClient,
    scope: Option<WbVariant>,
) -> Result<Value, String> {
    list_at_with_index_root(paths, client, scope, client.data_root().as_deref())
}

/// [`list_at`] 的可注入版本：同时显式传入插件 / IDE 的会话数据根。
///
/// 与 [`visible_groups_at`] 同构。`index_root = None` 表示「解析不到可用的数据根」：此时
/// 清理会被**整次拒绝**，列表的 `pruneBlocked` 必须给出同一句原因，否则前端又会出现
/// 「入口可点但必然失败」。用例据此覆盖 `Ready` 分支从扫描结论到载荷的最后一跳
/// （`visible.blocked` → [`list_payload`]）。
///
/// 卡片装配（会话正文）仍读客户端自身的会话根、无法在这里注入，所以用例只用「主表里没有
/// 组」的现场走通这条分支：0 组时不装配任何卡片，`client.data_root()` 也不会被调到，
/// 满足 AGENTS.md 第 8 节「测试不读真实客户端数据」。
pub(super) fn list_at_with_index_root(
    paths: &SessionPaths,
    client: SessionClient,
    scope: Option<WbVariant>,
    index_root: Option<&Path>,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    match session_link::load_store(paths) {
        StoreState::Missing => Ok(list_payload(
            client,
            scope,
            "missing",
            None,
            0,
            None,
            Vec::new(),
        )),
        StoreState::Unavailable(reason) => Ok(list_payload(
            client,
            scope,
            "unavailable",
            Some(&reason),
            0,
            // 主表读不出来时 `prune` 一定 `Err`（同一句文案），所以这里也必须是阻塞：
            // 让不变量「`pruneBlocked` 非空 ⇔ `prune` 会拒绝」无条件成立。
            Some(&super::maintenance::store_unavailable_reason(&reason)),
            Vec::new(),
        )),
        StoreState::Ready(store) => {
            let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
            let visible =
                visible_groups_at(&store, client, scope, &variant_member_paths, index_root);
            let filtered = visible.groups;
            let shared = prefetch_session_rows(&filtered);
            let groups = build_group_payloads(client, scope, paths, &accounts, &filtered, &shared);
            Ok(list_payload(
                client,
                scope,
                "ready",
                None,
                visible.dead,
                visible.blocked.as_deref(),
                groups,
            ))
        }
    }
}

/// 列表载荷装配（[`list_at`] 与用例共用的**唯一**写入点）。
///
/// `pruneBlocked` 无阻塞时是 `null`（键必须存在）：前端要区分「后端说可以清理」与
/// 「老后端根本没有这个字段」，前者禁用按钮，后者退回旧行为。
pub(super) fn list_payload(
    client: SessionClient,
    scope: Option<WbVariant>,
    store_status: &str,
    store_error: Option<&str>,
    dead: usize,
    blocked: Option<&str>,
    groups: Vec<Value>,
) -> Value {
    let mut result = prune_status_payload(dead, blocked);
    result["client"] = json!(client.as_str());
    result["variantScope"] = json!(scope.map(WbVariant::as_str));
    result["storeStatus"] = json!(store_status);
    result["groups"] = json!(groups);
    if let Some(reason) = store_error {
        result["storeError"] = json!(reason);
    }
    result
}

/// 列表载荷里的「清理入口状态」两个键：被隐藏的失效组数 + 清理是否被阻塞的原因。
///
/// 单独成函数是为了钉住载荷契约：`pruneBlocked` **无阻塞时是 `null` 而不是省略**——前端
/// 要区分「后端说可以清理」与「老后端根本没有这个字段」，前者禁用按钮，后者退回旧行为。
pub(super) fn prune_status_payload(dead: usize, blocked: Option<&str>) -> Value {
    json!({
        // 前端据此决定是否显示「清理失效关联组」入口（列表为空时仍要能触达）。
        "deadGroupCount": dead,
        // 非 `null` = 清理必然被拒绝：前端禁用入口并展示这句原因。
        "pruneBlocked": blocked,
    })
}

/// 管理页要展示的组，以及被隐藏的失效组数量。
pub(super) struct VisibleGroups<'a> {
    /// 要展示的组（顺序与存储一致，供列表与预取共用）。
    pub(super) groups: Vec<&'a LinkGroup>,
    /// 被隐藏的组数：组内所有成员指向的会话行都已失效。
    ///
    /// 必须回报给前端：全部组都失效时列表为空，清理入口再按「列表非空才显示」就会
    /// 永远无法触达——正是最需要清理的时候。
    pub(super) dead: usize,
    /// 清理会被拒绝的原因（`None` = 可清理）。
    ///
    /// 与批量清理**同一份判定**产出：`dead_groups` 拿到的 `Err` 就是这个字符串。前端据此
    /// 禁用「清理失效关联组」并展示原因，而不是让用户点一次必然失败的按钮。
    pub(super) blocked: Option<String>,
}

/// 管理页要展示的组：scope 内、且组内还有存活成员的组。
///
/// 组里所有成员指向的会话行都已**确认失效**（会话被删除、行缺失或归属变更）时，这一组在
/// 管理页没有任何可用内容——预览/同步入口也一律判 unknown，列表再列出来只会是死条目。
/// 部分成员缺失的组仍然返回：卡片继续按既有文案提示「内容缺失」，用户可自行修复。
///
/// 存活判定**按客户端分派**，口径与批量清理完全一致（隐藏的 = 可清理的）：
///
/// - `Workbuddy`：`workbuddy.db` 的 `sessions` 表（[`group_any_alive`] + [`LiveSessionRows`]）；
/// - `VscodeExt` / `CodebuddyIde`：工作区索引条目**或**会话目录
///   （[`super::liveness::IndexLiveness`]，与清理共用同一份判定）。
///
/// 插件 / IDE 侧**数据根解析不到**（没有判定依据）时 fail-open 全部展示：读路径不因为缺
/// 依据而隐藏用户的组。
///
/// 成员可能跨档：WorkBuddy 的数据根必须按**成员自身档位**解析（源读源档、目标读目标档）。
///
/// `index_root` 由调用方注入：生产路径（`list_at`）传 `client.data_root()`，用例传临时目录
/// ——**不得**在测试里走真实数据根（AGENTS.md 第 8 节）。
pub(super) fn visible_groups_at<'a>(
    store: &'a session_link::LinkStore,
    client: SessionClient,
    scope: Option<WbVariant>,
    member_paths: &dyn Fn(&LinkGroup, &LinkMember) -> SessionPaths,
    index_root: Option<&Path>,
) -> VisibleGroups<'a> {
    // 与批量清理**同一趟**扫描：`dead` 决定隐藏哪些组，`blocked` 决定清理入口是否可用。
    let scan = scan_groups(store, client, scope, member_paths, index_root);
    let dead = scan.dead.len();
    let dead_ids: HashSet<&str> = scan.dead.iter().map(|group| group.id.as_str()).collect();
    let groups = store
        .groups
        .iter()
        .filter(|group| group_is_in_scope(client, scope, group))
        .filter(|group| !dead_ids.contains(group.id.as_str()))
        .collect();
    VisibleGroups {
        groups,
        dead,
        blocked: scan.blocked,
    }
}

/// 生产路径的成员数据根解析：成员的档位决定数据根（跨档组按成员自身档位读）。
fn variant_member_paths(group: &LinkGroup, member: &LinkMember) -> SessionPaths {
    SessionPaths::for_variant(session_link::member_variant(group, member))
}

/// 预取本次列表涉及的所有账号会话索引。
///
/// `group_payload` 内的缓存是「按分组」的；若不做这一步，并行后每个分组都会把同一
/// 账号的会话列表重查一遍（160 组 × 每组 4 个成员）。共享成 `Arc` 后各线程只克隆指针。
pub(super) fn prefetch_session_rows(
    groups: &[&LinkGroup],
) -> HashMap<(String, String), Arc<Vec<Value>>> {
    let mut shared: HashMap<(String, String), Arc<Vec<Value>>> = HashMap::new();
    for group in groups {
        for member in &group.members {
            let variant = session_link::member_variant(group, member);
            let key = (variant.as_str().to_string(), member.uid.clone());
            if let std::collections::hash_map::Entry::Vacant(slot) = shared.entry(key) {
                let rows = session::list_sessions_for_user(variant, &member.uid)
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                slot.insert(Arc::new(rows));
            }
        }
    }
    shared
}

/// 分组列表的并行构建。
///
/// 首屏耗时几乎全在「读 600+ 份会话正文并逐行摘要」上，单线程在开发构建下要几十秒。
/// 分组之间没有共享可变状态（会话索引只读、基线只读），按分组切片即可安全并行；
/// 每个线程内部仍是顺序处理，内存峰值只与「线程数 × 单组成员数 × 正文大小」相关。
pub(super) fn build_group_payloads(
    client: SessionClient,
    scope: Option<WbVariant>,
    paths: &SessionPaths,
    accounts: &[Value],
    groups: &[&LinkGroup],
    shared: &HashMap<(String, String), Arc<Vec<Value>>>,
) -> Vec<Value> {
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 12);
    // 列表载荷固定按成员档位解析数据根（生产口径）；注入版本只在 `detail_at` 里用。
    let context = PayloadContext {
        client,
        scope,
        paths,
        member_paths: &SessionPaths::for_variant,
    };
    if workers <= 1 || groups.len() <= 1 {
        return groups
            .iter()
            .map(|group| {
                let mut cache = shared.clone();
                group_payload(&context, group, accounts, &mut cache, false)
            })
            .collect();
    }
    // 分组之间的正文大小差异很大（有的组 4 个成员都有几 MB 正文，有的全缺失），
    // 按切片静态分派会让部分线程早早就空闲。改成原子游标抢任务，先完成的线程继续取下一个。
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let collected = std::sync::Mutex::new(Vec::with_capacity(groups.len()));
    std::thread::scope(|s| {
        for _ in 0..workers {
            let cursor = &cursor;
            let collected = &collected;
            // 与 `cursor` / `collected` 同法：按引用捕获，避免 `move` 把上下文搬进第一个线程。
            let context = &context;
            s.spawn(move || loop {
                let index = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(group) = groups.get(index) else {
                    break;
                };
                let mut cache = shared.clone();
                // `context` 在本循环里已是引用（见上一行），直接用。
                let value = group_payload(context, group, accounts, &mut cache, false);
                // 收集器只做排序前暂存：即便有线程 panic 导致中毒，thread::scope 也会在
                // 退出时把 panic 抛出；这里取回内部 Vec 继续 push，避免二次 panic。
                locks::recover(collected).push((index, value));
            });
        }
    });
    let mut items = collected
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    items.sort_by_key(|(index, _)| *index);
    items.into_iter().map(|(_, value)| value).collect()
}

/// 成员会话行 / 正文的路径解析器（`variant → SessionPaths`）。
///
/// 生产传 [`SessionPaths::for_variant`]（按成员档位解析数据根），用例可传同一份临时根，
/// 从而**不读写真实 `~/.workbuddy-shift`** 也能覆盖组载荷的合成链路（见 [`detail_at`]）。
/// 这里只替换「从哪里读成员内容」，判定与徽标合成（`aggregate_group_state` /
/// `common_base_branches` / 组级 `summaryStatus`）用的仍是同一份实现。
///
/// 带 `Sync`：列表载荷会按分组切到多个线程并行构建（`build_group_payloads`），解析器要被
/// 多个线程共享。可见性与 [`detail_at`] 对齐（`pub(crate)`），避免 `private_interfaces`
/// 这类「类型比使用它的函数更私有」的告警。
pub(crate) type MemberPaths<'a> = &'a (dyn Fn(WbVariant) -> SessionPaths + Sync);

/// 组载荷装配的上下文：客户端、作用域、关联存储根与成员路径解析器。
///
/// 打包成一个结构而不是继续加参数：`group_payload` 的参数已经到 clippy 的上限
/// （8/7），且这四项在整次装配里是**不变**的（只有 group / cache 会变）。
pub(super) struct PayloadContext<'a> {
    pub(super) client: SessionClient,
    pub(super) scope: Option<WbVariant>,
    /// 关联存储根（关联表 / 基线 / 预览凭据都在它下面）。
    pub(super) paths: &'a SessionPaths,
    /// 成员会话行与正文的解析器（生产 = [`SessionPaths::for_variant`]）。
    pub(super) member_paths: MemberPaths<'a>,
}

/// Load one group and all saved members, including stale/superseded entries.
pub fn detail(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
) -> Result<Value, String> {
    detail_at(
        &client.store_paths(),
        &SessionPaths::for_variant,
        client,
        scope,
        group_id,
    )
}

/// [`detail`] 的显式路径版本：关联存储根与成员档位解析都由调用方给出。
///
/// 生产仍走 [`detail`]（`client.store_paths()` + [`SessionPaths::for_variant`]）；用例用它把
/// 临时数据根注入进来，从而能断言 `summaryStatus` / `divergence` 这类**组级字段**——它们是
/// 用户在列表里看到「存在分歧」徽标的来源，纯函数用例覆盖不到
/// （与 `list_at` / `list_at_with_index_root` 同一套做法）。
pub(crate) fn detail_at(
    paths: &SessionPaths,
    member_paths: MemberPaths<'_>,
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    let store = match session_link::load_store(paths) {
        StoreState::Ready(store) => store,
        StoreState::Missing => return Err("会话关联组不存在".to_string()),
        // 与列表载荷的 `pruneBlocked`、批量清理的 `Err` 共用同一句（单一来源）。
        StoreState::Unavailable(reason) => {
            return Err(super::maintenance::store_unavailable_reason(&reason));
        }
    };
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id && group_is_in_scope(client, scope, group))
        .ok_or_else(|| "会话关联组不存在或不属于当前客户端".to_string())?;
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    let context = PayloadContext {
        client,
        scope,
        paths,
        member_paths,
    };
    let mut cache = HashMap::new();
    Ok(group_payload(&context, group, &accounts, &mut cache, true))
}

pub(super) fn group_is_in_scope(
    client: SessionClient,
    scope: Option<WbVariant>,
    group: &LinkGroup,
) -> bool {
    match client {
        SessionClient::CodebuddyIde => Some(group.variant) == scope,
        SessionClient::Workbuddy | SessionClient::VscodeExt | SessionClient::Jetbrains => true,
    }
}

pub(super) fn account_by_uid(accounts: &[Value], uid: &str) -> Option<Value> {
    accounts
        .iter()
        .find(|account| account::get_str(account, "uid").as_deref() == Some(uid))
        .cloned()
}

pub(super) fn ide_flavor(variant: WbVariant) -> codebuddy_ide_session::IdeFlavor {
    match variant {
        WbVariant::Cn => codebuddy_ide_session::IdeFlavor::Cn,
        WbVariant::Ai => codebuddy_ide_session::IdeFlavor::Intl,
    }
}

pub(super) fn group_payload(
    context: &PayloadContext<'_>,
    group: &LinkGroup,
    accounts: &[Value],
    cache: &mut HashMap<(String, String), Arc<Vec<Value>>>,
    include_detail: bool,
) -> Value {
    let PayloadContext {
        client,
        scope,
        paths,
        member_paths,
    } = *context;
    let root = client.data_root();
    let spec = client.store_spec();
    let mut views = group
        .members
        .iter()
        .map(|member| {
            let account = account_by_uid(accounts, &member.uid);
            let account_name = account
                .as_ref()
                .map(account::account_display_name)
                .unwrap_or_else(|| member.uid.clone());
            let account_key = account
                .as_ref()
                .and_then(|account| account::get_str(account, "id"))
                .or_else(|| member.account_id.clone())
                .unwrap_or_else(|| member.uid.clone());
            let (title, project_label, updated_at, content) =
                member_snapshot(client, &root, spec, group, member, cache, member_paths);
            let mut member = member.clone();
            // Correct legacy plugin labels from the account library; region never selects its data root.
            // 插件型客户端（VS Code / JetBrains）的存储与地区无关，档位一律取账号库。
            if client.is_editor_plugin() {
                if let Some(account) = account.as_ref() {
                    member.variant = Some(account::variant_of(account));
                }
            }
            MemberView {
                member,
                account_key,
                account_name,
                title,
                project_label,
                updated_at,
                content,
                reason: String::new(),
                version_status: "unknown",
            }
        })
        .collect::<Vec<_>>();
    views.sort_by(|left, right| {
        left.account_key
            .cmp(&right.account_key)
            .then_with(|| left.member.uid.cmp(&right.member.uid))
            .then_with(|| left.member.member_id.cmp(&right.member.member_id))
    });

    let active = views
        .iter()
        .enumerate()
        .filter(|(_, view)| view.member.state == MemberState::Active)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut matrix: HashMap<(usize, usize), (SyncVerdict, String)> = HashMap::new();
    for &source_index in &active {
        for &target_index in &active {
            if source_index == target_index {
                continue;
            }
            let source = &views[source_index];
            let target = &views[target_index];
            let baseline = session_link::load_pair_baseline(
                paths,
                group,
                &source.member.member_id,
                &target.member.member_id,
            );
            let decision = session_link::decide_sync(&source.content, &target.content, &baseline);
            matrix.insert(
                (source_index, target_index),
                (decision.verdict, decision.reason),
            );
        }
    }
    let (safe_source, aggregate_status) = aggregate_group_state(&mut views, &active, &matrix);
    let divergence = common_base_branches(&views, &active);
    let summary_status = if divergence.is_some() {
        "diverge"
    } else {
        aggregate_status
    };
    let title = views
        .iter()
        .find(|view| !view.title.trim().is_empty())
        .map(|view| view.title.clone())
        .unwrap_or_else(|| "会话组".to_string());
    let project_label = views
        .iter()
        .find(|view| !view.project_label.is_empty())
        .map(|view| view.project_label.clone())
        .unwrap_or_default();
    let latest_activity_at = views.iter().map(|view| view.updated_at).max().unwrap_or(0);
    let account_names = views
        .iter()
        .map(|view| view.account_name.clone())
        .collect::<Vec<_>>();
    let group_key = format!(
        "{}:{}:{}",
        client.as_str(),
        scope.map(WbVariant::as_str).unwrap_or("all"),
        group.id
    );
    let mut result = json!({
        "key": group_key,
        "client": client.as_str(),
        "variantScope": scope.map(WbVariant::as_str),
        "groupId": group.id,
        "groupVariant": group.variant.as_str(),
        "title": title,
        "projectLabel": project_label,
        "latestActivityAt": latest_activity_at,
        "memberCount": group.members.len(),
        "activeMemberCount": active.len(),
        // 「跨组安全批量同步」的候选计数：前端据此显示「同步 N 个待同步组」并汇总副本数，
        // 不必为了算一个数字再把每个组详情都拉一遍。
        "behindMemberCount": views
            .iter()
            .filter(|view| view.version_status == "behind")
            .count(),
        "accountNames": account_names,
        "summaryStatus": summary_status,
        "summaryText": summary_text(summary_status),
        "safeSourceMemberId": safe_source.map(|index| views[index].member.member_id.clone()),
        "hasSafeSource": safe_source.is_some(),
    });
    if include_detail {
        if let Some(divergence) = divergence {
            result["divergence"] = json!({
                "commonMemberIds": divergence.common.iter().map(|&index| &views[index].member.member_id).collect::<Vec<_>>(),
                "branches": divergence.branches.iter().map(|branch| branch.iter().map(|&index| &views[index].member.member_id).collect::<Vec<_>>()).collect::<Vec<_>>(),
            });
        }
        let member_payloads = views.iter().map(|view| json!({
            "memberId": view.member.member_id,
            "accountId": view.member.account_id,
            "uid": view.member.uid,
            "sessionId": view.member.session_id,
            "accountName": view.account_name,
            "variant": session_link::member_variant(group, &view.member).as_str(),
            "linkState": view.member.state.as_str(),
            "versionStatus": view.version_status,
            "title": view.title,
            "projectLabel": view.project_label,
            "updatedAt": view.updated_at,
            "recordCount": content_count(&view.content),
            "contentPreview": content_preview(&view.content),
            "contentState": content_state_name(&view.content),
            "reason": view.reason,
            "canBeSource": view.member.state == MemberState::Active && matches!(view.content, ContentState::Ready(_)),
        })).collect::<Vec<_>>();
        let target_options = compatible_accounts(client, scope, group, accounts);
        result["members"] = json!(member_payloads);
        result["addTargets"] = json!(target_options);
    }
    result
}

pub(super) fn member_snapshot(
    client: SessionClient,
    root: &Option<PathBuf>,
    spec: Option<SessionStoreSpec>,
    group: &LinkGroup,
    member: &LinkMember,
    cache: &mut HashMap<(String, String), Arc<Vec<Value>>>,
    member_paths: MemberPaths<'_>,
) -> (String, String, i64, ContentState) {
    match client {
        SessionClient::Workbuddy => {
            let variant = session_link::member_variant(group, member);
            // 生产 = `SessionPaths::for_variant(variant)`（按成员档位）；用例注入同一份临时根。
            let paths = member_paths(variant);
            let rows = cache
                .entry((variant.as_str().to_string(), member.uid.clone()))
                .or_insert_with(|| {
                    // 与 `prefetch_session_rows` 同源：都落在 `list_sessions_for_user_at` 上，
                    // 只是这里按注入的路径解析，避免用例读写真实数据目录。
                    Arc::new(
                        session::list_sessions_for_user_at(&paths, &member.uid)
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    )
                });
            let row = rows.iter().find(|row| {
                row.get("id").and_then(Value::as_str) == Some(member.session_id.as_str())
            });
            let title = row
                .and_then(|row| row.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("会话标题暂不可用")
                .to_string();
            let cwd = row
                .and_then(|row| row.get("cwd"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let project = basename(cwd);
            let updated_at = row
                .and_then(|row| row.get("updatedAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let content = if row.is_none() {
                ContentState::Missing
            } else {
                // 与 `list_sessions_for_user_at` 用同一份解析结果：行与正文必须同源。
                session::member_content_state(&paths, &member.session_id)
            };
            (
                title,
                if project.is_empty() {
                    String::new()
                } else {
                    format!("项目 {project}")
                },
                updated_at,
                content,
            )
        }
        SessionClient::CodebuddyIde | SessionClient::VscodeExt | SessionClient::Jetbrains => {
            let variant = session_link::member_variant(group, member);
            let rows = cache
                .entry((variant.as_str().to_string(), member.uid.clone()))
                .or_insert_with(|| {
                    let Some(root) = root.as_ref() else {
                        return Arc::new(Vec::new());
                    };
                    Arc::new(
                        vscode_session::list_sessions_in_store(spec.unwrap(), root, &member.uid)
                            .get("sessions")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    )
                });
            let row = rows.iter().find(|row| {
                row.get("id").and_then(Value::as_str) == Some(member.session_id.as_str())
            });
            let title = row
                .and_then(|row| row.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("会话标题暂不可用")
                .to_string();
            let project = project_label_of(client, row);
            let updated_at = row
                .and_then(|row| row.get("updatedAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let content = match (root.as_ref(), spec) {
                (Some(root), Some(spec)) => vscode_session_sync::session_location_and_content(
                    spec,
                    root,
                    &member.uid,
                    &member.session_id,
                )
                .map(|location| location.content)
                .unwrap_or(ContentState::Missing),
                _ => ContentState::Unavailable("未找到客户端会话数据目录".to_string()),
            };
            (title, project, updated_at, content)
        }
    }
}

/// 会话行的项目标签（卡片顶端那一行；也是「同一会话出现在两个项目桶」的唯一区分依据）。
///
/// - JetBrains：`项目：<项目路径>`（项目 key 是路径的 base64url）；项目段退化成 uid
///   （插件在「没有打开项目」时写入）时显示 `项目：未打开项目`——不能落回「工作区 xxxxxxxx」，
///   因为**两个桶的 workspaceHash 是同一个安装级常量**，那样两行仍然一模一样；
/// - VS Code 插件 / CodeBuddy IDE：沿用既有「工作区 <md5 前 8 位>」（行为不变）。
fn project_label_of(client: SessionClient, row: Option<&Value>) -> String {
    if client != SessionClient::Jetbrains {
        let hash = row
            .and_then(|row| row.get("workspaceHash"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return if hash.is_empty() {
            String::new()
        } else {
            format!("工作区 {}", hash.chars().take(8).collect::<String>())
        };
    }
    // 行缺失（会话已被删除）时不给项目标签：此时说「未打开项目」是错的。
    let Some(row) = row else {
        return String::new();
    };
    let path = row
        .get("projectPath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty());
    match path {
        Some(path) => format!("项目：{path}"),
        None => "项目：未打开项目".to_string(),
    }
}

pub(super) fn content_count(content: &ContentState) -> Option<usize> {
    match content {
        ContentState::Ready(snapshot) => Some(snapshot.normalized.record_count),
        _ => None,
    }
}

pub(super) fn record_text(record: &Value) -> Option<String> {
    match record {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items.iter().filter_map(record_text).collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        Value::Object(fields) => ["text", "content", "message", "parts"]
            .iter()
            .find_map(|key| fields.get(*key).and_then(record_text)),
        _ => None,
    }
}

pub(super) fn content_state_name(content: &ContentState) -> &'static str {
    match content {
        ContentState::Ready(_) => "ready",
        ContentState::Missing => "missing",
        ContentState::Unavailable(_) => "unavailable",
    }
}

pub(super) fn summary_text(status: &str) -> &'static str {
    match status {
        "latest" => "关联副本内容一致",
        "behind" => "有副本落后，可安全同步",
        "diverge" => "多个副本有不同更新，需要选择来源",
        "missing" => "有副本内容缺失",
        _ => "暂时无法确认副本状态",
    }
}

pub(super) fn basename(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("")
        .to_string()
}
