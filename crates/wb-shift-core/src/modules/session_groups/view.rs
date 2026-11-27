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
    /// 展示用的「最近活跃」时间，见 [`activity_at_of`]：正文最后一条消息时间优先，
    /// 取不到回退会话行的 `updated_at`。组 summary 的 `latestActivityAt` 取各成员的 max。
    ///
    /// 会话行自己的 `updated_at` 不再单独保留：复制会把目标行写成复制时刻，它只作为
    /// 兜底输入出现在 [`activity_at_of`] 里，留着这个字段只会让后续改动误用。
    pub(super) activity_at: i64,
    pub(super) content: ContentState,
    pub(super) reason: String,
    pub(super) version_status: &'static str,
}

/// 成员的「最近活跃」时间：**正文最后一条消息的时间**优先，拿不到才回退会话行的 `updated_at`。
///
/// 为什么不能直接用 `updated_at`：复制会把目标行的 `updated_at` 写成复制时刻（
/// `session/copy/db.rs` 的刻意设计，见 AGENTS 与任务书——改它会让副本沉到客户端列表底部），
/// 于是「上周的会话今天复制过来」在列表里会显示成「刚刚」，排序也会跳到最前。
///
/// 拿不到正文时间戳（旧记录没有 `timestamp`、正文缺失 / 不可读、插件侧正文口径不同）时
/// 一律回退 `updated_at`：宁可退回改造前的行为，也不能把时间显示成 0 / 1970。
pub(super) fn activity_at_of(content: &ContentState, updated_at: i64) -> i64 {
    match content {
        ContentState::Ready(snapshot) => snapshot.latest_at.unwrap_or(updated_at),
        ContentState::Missing | ContentState::Unavailable(_) => updated_at,
    }
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
                // 列表与详情展示的「最近更新」一律是正文最后一条消息的时间（见该函数的说明）。
                activity_at: activity_at_of(&content, updated_at),
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
    // 组级「最近更新」= 各成员 `activity_at` 的 max（不是复制时刻，见 `activity_at_of`）。
    let latest_activity_at = views.iter().map(|view| view.activity_at).max().unwrap_or(0);
    // 卡片摘要：取「最新有内容的活跃副本」的一条消息（只 1 条，控制列表载荷体积）。
    // 非活跃成员（已被替代 / 已失效）的正文可能早就过时，不能拿来做卡片摘要。
    let excerpt = views
        .iter()
        .filter(|view| view.member.state == MemberState::Active)
        .filter(|view| matches!(view.content, ContentState::Ready(_)))
        .max_by_key(|view| view.activity_at)
        .and_then(|view| excerpt_of(&view.content));
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
        // 卡片摘要（最多两行）：最近一条用户消息，没有则助手消息；无可读消息时为 `null`，
        // 前端不占位。只放 1 条是为了控制列表载荷（上百个组 × 每条 120 字）。
        "excerpt": excerpt,
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
            // 成员详情也展示「正文最后一条消息的时间」，与列表的 `latestActivityAt` 同源，
            // 避免列表说三天前、详情说刚刚。
            "updatedAt": view.activity_at,
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
            let project = workbuddy_project_label(row, cwd);
            let updated_at = row
                .and_then(|row| row.get("updatedAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let content = if row.is_none() {
                ContentState::Missing
            } else {
                // 与 `list_sessions_for_user_at` 用同一份解析结果：行与正文必须同源。
                // 列表/详情只消费摘要、预览与逐行指纹，走轻量缓存；写路径
                // （复制 / 同步）不经过这里，仍读全文（见 graph::read_list_snapshot）。
                match session::find_project_jsonl(&paths, &member.session_id) {
                    Some(path) => super::graph::read_list_snapshot(&path, &member.session_id),
                    None => ContentState::Missing,
                }
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
///
/// WorkBuddy 会话的项目名：优先用 `spaceName`，缺值时回退 `cwd` 文件夹名。
///
/// `list_sessions_for_user_at` 已经把 `spaces.name` JOIN 成 `spaceName` 随行下发；
/// WorkBuddy 会把当前 workspace 目录（如 `D:\WorkBuddy_data\spaces\2026_11_16_13-46-13`）
/// 写进 `cwd`，直接取 basename 会得到一串时间戳，而客户端里真正的项目名在 space 上。
/// 老库没有 `spaces` 表 / 空间行已删除时 `spaceName` 为空，保持原回退行为。
pub(super) fn workbuddy_project_label(row: Option<&Value>, cwd: &str) -> String {
    let space_name = row
        .and_then(|row| row.get("spaceName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    match space_name {
        Some(name) => name.to_string(),
        None => basename(cwd),
    }
}

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

// ---------------------------------------------------------------------------
// 用例：列表/详情的「最近更新」与卡片摘要（T02 / T03）
// ---------------------------------------------------------------------------
//
// 这两条口径只有在**真的走一遍组载荷装配**时才能断言：`latestActivityAt` 与 `excerpt`
// 都由 `group_payload` 从「正文快照 + 会话行」两处输入合成，纯函数用例覆盖不到。
// 装配链路是 `detail_at`（可注入临时数据根），因此这里自带一个最小沙箱：关联存储根、
// `workbuddy.db` 与 `projects/*.jsonl` 全部落在临时目录，绝不读写真实 `~/.workbuddy-shift`
// 或客户端数据（AGENTS 第 8 节）。
//
// 不复用 `session_groups::tests` 里的 `TempStore`：那个 helper 是 `mod tests` 的私有项
// （同模块 include! 的用例专用），跨模块取不到；也不为测试放宽它的可见性。
#[cfg(test)]
mod tests {
    use super::*;

    /// 临时沙箱：关联存储根与会话数据根都在临时目录下。
    struct Sandbox {
        dir: PathBuf,
        paths: SessionPaths,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wb_shift_group_view_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).expect("临时目录应可创建");
            let paths = SessionPaths {
                store_root: dir.join("store"),
                data_root: dir.join("data"),
                auth_file: dir.join("auth.info"),
                link_namespace: crate::modules::session::LinkNamespace::WorkBuddy,
            };
            std::fs::create_dir_all(&paths.store_root).expect("关联存储根应可创建");
            Self { dir, paths }
        }

        /// 建 `sessions` 表（结构与客户端实测库一致的最小列集）。
        fn create_sessions_table(&self) {
            std::fs::create_dir_all(&self.paths.data_root).expect("数据根应可创建");
            let conn =
                rusqlite::Connection::open(self.paths.workbuddy_db()).expect("测试库应可打开");
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    title TEXT,
                    custom_title TEXT,
                    cwd TEXT,
                    created_at INTEGER,
                    updated_at INTEGER,
                    deleted_at INTEGER,
                    is_playground INTEGER
                );",
            )
            .expect("sessions 表应可建");
        }

        /// 插一条会话行：`updated_at` 由用例显式给出（复制路径会把它写成复制时刻）。
        fn add_session_row(&self, id: &str, uid: &str, updated_at: i64) {
            let conn =
                rusqlite::Connection::open(self.paths.workbuddy_db()).expect("测试库应可打开");
            conn.execute(
                "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, deleted_at, is_playground)
                 VALUES (?1, ?2, ?3, NULL, '/ws/a', 1000, ?4, NULL, 0)",
                rusqlite::params![id, uid, format!("标题 {id}"), updated_at],
            )
            .expect("会话行应可写入");
        }

        /// 写正文：`projects/<session id>.jsonl`（`find_project_jsonl` 的一层布局）。
        fn write_body(&self, session_id: &str, lines: &[String]) {
            let projects = self.paths.projects_dir();
            std::fs::create_dir_all(&projects).expect("projects 目录应可创建");
            std::fs::write(
                projects.join(format!("{session_id}.jsonl")),
                format!("{}\n", lines.join("\n")),
            )
            .expect("正文应可写入");
        }

        fn seed_group(&self, group_id: &str, members: Vec<LinkMember>) {
            let store = crate::modules::session_link::LinkStore {
                version: crate::modules::session_link::LINK_STORE_VERSION,
                revision: 1,
                groups: vec![LinkGroup {
                    id: group_id.to_string(),
                    variant: WbVariant::Cn,
                    created_at: 1,
                    members,
                    pair_bases: Vec::new(),
                }],
            };
            std::fs::write(
                self.paths.session_links_file(),
                serde_json::to_string_pretty(&store).expect("关联表应可序列化"),
            )
            .expect("关联表应可写入");
        }

        /// 走生产同一条装配链路（`detail_at` 只把数据根换成临时目录）。
        fn detail(&self, group_id: &str) -> Value {
            let paths = self.paths.clone();
            let resolve = move |_: WbVariant| paths.clone();
            detail_at(
                &self.paths,
                &resolve,
                SessionClient::Workbuddy,
                None,
                group_id,
            )
            .expect("组详情应可读")
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn member(member_id: &str, uid: &str, session_id: &str) -> LinkMember {
        LinkMember {
            member_id: member_id.to_string(),
            account_id: Some(format!("account-{uid}")),
            uid: uid.to_string(),
            session_id: session_id.to_string(),
            variant: Some(WbVariant::Cn),
            state: MemberState::Active,
            linked_at: 1,
            last_synced_at: None,
        }
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|delta| delta.as_millis() as i64)
            .unwrap_or(0)
    }

    /// 复制时刻落在会话行 `updated_at` 上，但列表/详情必须显示正文最后一条消息的时间。
    ///
    /// 现场：用户在三天前聊完的会话，今天复制到另一个账号。复制把目标行 `updated_at`
    /// 写成 `copy_now`（刻意设计，见 `session/copy/db.rs`），改造前列表就显示成「刚刚」，
    /// 排序还会跳到最前。
    #[test]
    fn latest_activity_uses_last_message_time() {
        let sandbox = Sandbox::new("latest-message");
        sandbox.create_sessions_table();
        let now = now_ms();
        let three_days_ago = now - 3 * 24 * 60 * 60 * 1000;
        sandbox.add_session_row("session-a", "uid-a", now);
        sandbox.write_body(
            "session-a",
            &[
                format!(
                    r#"{{"type":"user","text":"三天前的问题","timestamp":{}}}"#,
                    three_days_ago - 1000
                ),
                format!(
                    r#"{{"type":"message","role":"assistant","text":"三天前的回答","timestamp":{three_days_ago}}}"#
                ),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);

        let detail = sandbox.detail("g-1");
        let latest = detail["latestActivityAt"]
            .as_i64()
            .expect("latestActivityAt 必须是整数");
        assert!(
            (latest - three_days_ago).abs() <= 1000,
            "最近更新应取正文最后一条消息的时间（±1s）：期望 {three_days_ago}，实际 {latest}；{detail}"
        );
        // 成员详情与列表同源：不允许列表说三天前、详情说刚刚。
        assert_eq!(
            detail["members"][0]["updatedAt"].as_i64(),
            Some(latest),
            "成员详情的 updatedAt 必须与组级 latestActivityAt 同源：{detail}"
        );
    }

    /// 正文一个可用时间戳都没有时不 panic，回退会话行的 `updated_at`（旧记录的真实形态）。
    #[test]
    fn latest_activity_falls_back_to_updated_at() {
        let sandbox = Sandbox::new("latest-fallback");
        sandbox.create_sessions_table();
        let row_updated_at = 1_700_000_000_000;
        sandbox.add_session_row("session-a", "uid-a", row_updated_at);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"没有时间戳的旧记录"}"#.to_string(),
                // 0 与负数不是真实活动时间（口径同 `record_timestamp_of`），必须同样回退。
                r#"{"type":"message","role":"assistant","text":"时间戳写坏了","timestamp":0}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);

        let detail = sandbox.detail("g-1");
        assert_eq!(
            detail["latestActivityAt"].as_i64(),
            Some(row_updated_at),
            "正文无可用时间戳时必须回退会话行的 updatedAt：{detail}"
        );
        assert_eq!(
            detail["members"][0]["updatedAt"].as_i64(),
            Some(row_updated_at),
            "{detail}"
        );
    }

    /// 摘要取最近一条**用户**消息：工具结果行、助手消息都不该抢占它。
    #[test]
    fn excerpt_prefers_latest_user_message() {
        let sandbox = Sandbox::new("excerpt-user");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"第一问"}"#.to_string(),
                r#"{"type":"message","role":"assistant","text":"第一答"}"#.to_string(),
                r#"{"type":"user","text":"第二问"}"#.to_string(),
                r#"{"type":"message","role":"assistant","text":"第二答"}"#.to_string(),
                r#"{"type":"tool","text":"{\"ok\":true}"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);

        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("第二问"));
    }

    /// 没有用户消息时退到最近一条助手消息；没有任何可读消息时是 `null`（前端不占位）。
    #[test]
    fn excerpt_falls_back_to_assistant_then_null() {
        let sandbox = Sandbox::new("excerpt-assistant");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"message","role":"assistant","text":"只有回答"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("只有回答"));

        // 正文只有不可读记录（工具行）→ 摘要为空，而不是空字符串。
        let sandbox = Sandbox::new("excerpt-none");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"tool","text":"{\"ok\":true}"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], Value::Null);
    }

    /// 注入块（`<system-reminder>` / `<craft_mode>`）不是用户说的话：摘要要先剥掉它们。
    ///
    /// 真实库上首次跑出来的问题：`excerpt` 取到整段
    /// `<system-reminder data-role="user-context">\n<craft_mode>\nYou are now in Agent mode…`，
    /// 卡片上等于没有摘要。这条用例从「正文 → 组载荷」整链断言，别只在纯函数层自证。
    #[test]
    fn excerpt_skips_injected_context_blocks() {
        let sandbox = Sandbox::new("excerpt-injected");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"先问一句"}"#.to_string(),
                // 注入块在前、用户真正的问题在最后（system-reminder 包住 craft_mode）。
                r#"{"type":"user","text":"<system-reminder data-role=\"user-context\">\n<craft_mode>\nYou are now in Agent mode. Continue with the task in the new mode.\n</craft_mode>\n</system-reminder>\n真正的问题"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("真正的问题"));

        // 只有注入块、没有正文 → 这条不算「有内容」，回退到上一条用户消息。
        let sandbox = Sandbox::new("excerpt-injected-only");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"上一条真正的问题"}"#.to_string(),
                // 未闭合到末尾：整条记录都是注入。
                r#"{"type":"user","text":"<system-reminder data-role=\"user-context\">\n只有注入，没有正文"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("上一条真正的问题"));

        // 连一条有内容的用户消息都没有时，仍然按老规则退到最近一条助手消息。
        let sandbox = Sandbox::new("excerpt-injected-assistant");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"message","role":"assistant","text":"助手可见的回答"}"#.to_string(),
                r#"{"type":"user","text":"<system-reminder>注入</system-reminder>"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("助手可见的回答"));
    }

    /// 产品拼进用户消息的 `@image#N:文件名` 是给模型看的图片引用记号，不是用户说的话：
    /// 列表摘要与详情预览走同一条清理链，占位符必须剥掉，只保留后面的正文。
    #[test]
    fn excerpt_strips_image_placeholders() {
        // 1) 开头连续两个占位符 + 正文（用户截图里的真实形态）→ 只剩正文；预览同口径。
        let sandbox = Sandbox::new("excerpt-image-leading");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"@image#1:Clipboard_Screenshot.png @image#2:Clipboard_Screenshot-1.png 为什么在关联会话界面会出现这种可读性比较差的内容？"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        let detail = sandbox.detail("g-1");
        let expected = "为什么在关联会话界面会出现这种可读性比较差的内容？";
        assert_eq!(detail["excerpt"], json!(expected));
        assert_eq!(
            detail["members"][0]["contentPreview"][0]["text"]
                .as_str()
                .unwrap_or_default(),
            expected,
            "详情预览必须与列表摘要同口径剥掉占位符：{detail}"
        );

        // 2) 句中占位符 → 正文保留，删除后留下的连续空格折叠成一个。
        let sandbox = Sandbox::new("excerpt-image-inline");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"根据这张图 @image#1:x.png 帮我分析"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("根据这张图 帮我分析")
        );

        // 3) 只有占位符、没有正文 → 这条不算「有内容」，回退到上一条用户消息。
        let sandbox = Sandbox::new("excerpt-image-only");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"上一条真正的问题"}"#.to_string(),
                r#"{"type":"user","text":"@image#1:a.png @image#2:b.png"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("上一条真正的问题"));

        // 3b) 前面没有用户消息时，按老规则退到最近一条助手消息。
        let sandbox = Sandbox::new("excerpt-image-assistant");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"message","role":"assistant","text":"助手可见的回答"}"#.to_string(),
                r#"{"type":"user","text":"@image#1:a.png"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("助手可见的回答"));

        // 4) 反例：普通 `@某人` 提及不是占位符，必须原样保留。
        let sandbox = Sandbox::new("excerpt-image-mention");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"@某人 请看这个"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("@某人 请看这个"));

        // 5) 真实库残留（group 567ccfbc…）：文件名被引号包住，扩展名后紧跟闭合引号，
        //    引号必须一并剥掉；摘要与详情预览同口径。
        let sandbox = Sandbox::new("excerpt-image-quoted");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"@image#1:\"黄岑果证明·.jpg\" 我在校期间还参加过一次三下乡活动,把这段经历也加到我的资料里,可以适当扩写一下"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        let detail = sandbox.detail("g-1");
        let expected =
            "我在校期间还参加过一次三下乡活动,把这段经历也加到我的资料里,可以适当扩写一下";
        assert_eq!(detail["excerpt"], json!(expected));
        assert_eq!(
            detail["members"][0]["contentPreview"][0]["text"]
                .as_str()
                .unwrap_or_default(),
            expected,
            "带引号文件名的占位符也必须在详情预览里剥掉：{detail}"
        );

        // 6) 弯引号形态（各端拼出来的引号不统一，真实库同样存在）：同样剥掉，正文一个字不动。
        let sandbox = Sandbox::new("excerpt-image-curly-quote");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"@image#1:“黄岑果证明·.jpg” 这条里的数据是否正确？"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("这条里的数据是否正确？")
        );
    }

    /// 整段被单个标签包住时摘要去掉这层包装（真实库上出现过
    /// `excerpt = "<user_query>总结一下简历通项目规模</user_query>"`）。
    ///
    /// 反例同样重要：整段不是「一个标签对」时**保持原样**，不能把正文里中途出现的标签当包装。
    #[test]
    fn excerpt_unwraps_single_wrapper_tag() {
        // 单层包装 → 取内部。
        let sandbox = Sandbox::new("excerpt-wrapper-single");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"<user_query>真实问题</user_query>"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("真实问题"));

        // 内部多行 + 首尾空白 → 剥完再 trim。
        let sandbox = Sandbox::new("excerpt-wrapper-multiline");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"<user_query>\n  多行问题\n</user_query>"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("多行问题"));

        // 两层包装 → 一层层剥到不再变化（上限 4）。
        let sandbox = Sandbox::new("excerpt-wrapper-nested");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"<a><b>x</b></a>"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("x"));

        // 反例：整段不是单个标签对（前后都有正文）→ 原样保留。
        let sandbox = Sandbox::new("excerpt-wrapper-negative");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"前一句 <user_query>不算包装</user_query> 后一句"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("前一句 <user_query>不算包装</user_query> 后一句")
        );

        // 包装内部为空 → 这条视为「没有正文」，继续往前找上一条用户消息。
        let sandbox = Sandbox::new("excerpt-wrapper-empty");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"上一条真正的问题"}"#.to_string(),
                r#"{"type":"user","text":"<user_query>\n\n</user_query>"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("上一条真正的问题"));
    }

    /// 工具注入的包装标签（队友通知、斜杠命令回显、命令输出…）整块丢弃，不能在卡片上留成
    /// 「一串带属性的标签」。
    ///
    /// 现场残留（真实库）：
    /// `<teammate-message teammate_id="system" summary="… completed: 摘要去掉包装标签">已完成</teammate-message>`
    /// 被当成用户消息，卡片摘要直接显示成这条标签。这些标签的**内容**是工具注入的元信息，
    /// 不是用户说的话，所以整块丢掉；丢完没有正文时按「这条没内容」继续往前找。
    ///
    /// 同时守住反例：兜底切标签必须以「能找到配对闭合标签」为前提，`a < b` 这类比较符不能被切坏。
    #[test]
    fn excerpt_drops_tool_injected_wrappers() {
        // 1) 整段就是一个 teammate-message → 丢弃后没有正文，回退到上一条用户消息。
        let sandbox = Sandbox::new("excerpt-teammate-only");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"上一条真正的问题"}"#.to_string(),
                r#"{"type":"user","text":"<teammate-message teammate_id=\"system\" summary=\"software-engineer-s2 completed: 摘要去掉包装标签\">已完成</teammate-message>"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("上一条真正的问题"));

        // 2) 前面没有用户消息时按老规则退到最近一条助手消息。
        let sandbox = Sandbox::new("excerpt-teammate-assistant");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"message","role":"assistant","text":"助手可见的回答"}"#.to_string(),
                r#"{"type":"user","text":"<teammate-message teammate_id=\"system\" summary=\"x\">已完成</teammate-message>"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("助手可见的回答"));

        // 3) 斜杠命令回显：只丢回显，用户真正打的字要留着。
        let sandbox = Sandbox::new("excerpt-command-echo");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"<command-name>/compact</command-name>真正的用户问题"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("真正的用户问题"));

        // 3b) 多条工具块并列（命令回显 + 命令输出）：连着剥干净。
        let sandbox = Sandbox::new("excerpt-command-output");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<local-command-stdout>Cleared conversation</local-command-stdout>\n帮我看看构建失败"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("帮我看看构建失败"));

        // 4) `<user_query>` 不是注入标签：仍然只剥一层包装（原有行为不回归）。
        let sandbox = Sandbox::new("excerpt-user-query-kept");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"<user_query>真实问题</user_query>"}"#.to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("真实问题"));

        // 5) 开头是「元素 + 后续正文」：拆掉元素、**保留元素之后的用户原话**
        //    （`unwrap_leading_element`）。真实库里用户消息常被协议拼成
        //    `<user_query>正文</user_query> <image_local_path>…</image_local_path>`，
        //    只切开标签会把闭合标签留在卡片上；这里连闭合标签一起拆掉，
        //    紧贴的正文（`（补充一句）`）必须一个字都不丢。
        let sandbox = Sandbox::new("excerpt-residual-open-tag");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[r#"{"type":"user","text":"<user_query lang=\"zh\">真实问题</user_query>（补充一句）"}"#
                .to_string()],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("真实问题（补充一句）")
        );

        // 6) 反例：正文里的比较符不是标签，兜底规则不许动它。
        let sandbox = Sandbox::new("excerpt-angle-brackets");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"比较 a < b 与 c > d"}"#.to_string(),
                // 开标签没有配对闭合标签：同样不许切（否则会吃掉后面的正文）。
                r#"{"type":"user","text":"<div>没有闭合的比较 a < b"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("<div>没有闭合的比较 a < b")
        );
    }

    /// 截图里的两类脏内容：用户消息被 `<system-reminder><craft_mode>` 注入块包住，
    /// 以及 `<task-notification>` 工具载荷。详情预览必须与列表摘要同一套清理口径。
    #[test]
    fn preview_strips_injection_and_skips_tool_payload() {
        let sandbox = Sandbox::new("preview-clean");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"<system-reminder data-role=\"user-context\">\n<craft_mode>\nYou are now in Agent mode. Continue with the task in the new mode.\n</craft_mode>\n</system-reminder>\n帮我看看这份文档"}"#.to_string(),
                r#"{"type":"user","text":"<task-notification><task-id>t-1</task-id><tool-use-id>call_1</tool-use-id><status>completed</status><summary>Background command finished</summary></task-notification>"}"#.to_string(),
                r#"{"type":"assistant","text":"文档已核对，共 19 个话题。"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        let detail = sandbox.detail("g-1");
        let previews = detail["members"][0]["contentPreview"]
            .as_array()
            .expect("contentPreview 应为数组");

        assert_eq!(previews.len(), 2, "工具载荷整条应被跳过，只剩 2 条可读内容");
        let texts: Vec<&str> = previews
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect();
        assert!(
            texts.iter().all(
                |text| !text.contains("system-reminder") && !text.contains("task-notification")
            ),
            "预览里不得出现注入标签: {texts:?}"
        );
        assert!(
            texts[0].contains("帮我看看这份文档"),
            "注入块后的用户原话要保留"
        );
    }

    /// 记录里**夹着工具调用结果**时整条不参与摘要（记录级过滤，不再做文本手术）。
    ///
    /// 现场残留（真实库 102 组里 25 组）：最近一条用户记录长这样——标签全在文本中间，整段
    /// 不是单个包装对，`strip_injected_blocks` / `unwrap_wrapper_tags` / 兜底切头标签都碰
    /// 不到，于是卡片上直接显示 `</task-id><tool-use-id>call_…`。这类记录只能整条不参与摘要：
    /// **宁可没有摘要，也不要在卡片上显示标签残片**。
    #[test]
    fn excerpt_skips_tool_payload_records() {
        // 1) 最近的用户记录是工具载荷 → 跳过它，取上一条真正的问题。
        let sandbox = Sandbox::new("excerpt-payload-skip");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"帮我看看这个报错"}"#.to_string(),
                r#"{"type":"user","text":"TeiPX5</task-id>\n<tool-use-id>call_1</tool-use-id>\n<status>failed</status>"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], json!("帮我看看这个报错"));

        // 2) 整组只有载荷记录 → `null`（前端不占位），且不 panic。
        //    两条都算载荷：一条是「文本中间夹标签」，一条是「只剩闭合标签残片」。
        let sandbox = Sandbox::new("excerpt-payload-only");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"user","text":"TeiPX5</task-id>\n<tool-use-id>call_1</tool-use-id>\n<status>failed</status>"}"#
                    .to_string(),
                r#"{"type":"user","text":"</task-id>"}"#.to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(sandbox.detail("g-1")["excerpt"], Value::Null);

        // 3) 干净助手消息 + 载荷用户记录 → 返回助手消息（不与载荷「抢」摘要）。
        let sandbox = Sandbox::new("excerpt-payload-assistant");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        sandbox.write_body(
            "session-a",
            &[
                r#"{"type":"message","role":"assistant","text":"助手看到这次的失败原因"}"#
                    .to_string(),
                r#"{"type":"user","text":"TeiPX5</task-id>\n<tool-use-id>call_1</tool-use-id>"}"#
                    .to_string(),
            ],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);
        assert_eq!(
            sandbox.detail("g-1")["excerpt"],
            json!("助手看到这次的失败原因")
        );
    }

    /// 摘要按字符截断：120 字上限，不把多字节汉字截成半个。
    #[test]
    fn excerpt_truncates_by_characters() {
        let sandbox = Sandbox::new("excerpt-truncate");
        sandbox.create_sessions_table();
        sandbox.add_session_row("session-a", "uid-a", 1_700_000_000_000);
        let long = "会".repeat(200);
        sandbox.write_body(
            "session-a",
            &[format!(r#"{{"type":"user","text":"{long}"}}"#)],
        );
        sandbox.seed_group("g-1", vec![member("m-a", "uid-a", "session-a")]);

        let excerpt = sandbox.detail("g-1")["excerpt"]
            .as_str()
            .expect("摘要应为字符串")
            .to_string();
        assert_eq!(excerpt.chars().count(), 120, "摘要应截到约 120 字");
        assert!(excerpt.chars().all(|ch| ch == '会'), "截断不得破坏字符边界");
    }
}

#[cfg(test)]
mod project_label_tests {
    use super::*;
    use serde_json::json;

    /// 现场数据：`sessions.cwd` 是当前 workspace 时间戳目录，`spaces.name` 才是项目名。
    #[test]
    fn workbuddy_uses_space_name_instead_of_workspace_dir() {
        let cwd = r"D:/WorkBuddy_data/spaces/2026_11_16_13-46-13";
        let row = json!({
            "cwd": cwd,
            "spaceName": "workbuddy-shift",
        });
        assert_eq!(workbuddy_project_label(Some(&row), cwd), "workbuddy-shift");
    }

    /// 老库 / 空间行已删除：没有 `spaceName` 时保持原 basename(cwd) 行为。
    #[test]
    fn workbuddy_falls_back_to_cwd_without_space_name() {
        let cwd = r"D:/WorkBuddy_data/spaces/2026_11_16_13-46-13";
        assert_eq!(workbuddy_project_label(None, cwd), "2026_11_16_13-46-13");
        let row_without_space = json!({"cwd": cwd, "spaceName": ""});
        assert_eq!(
            workbuddy_project_label(Some(&row_without_space), cwd),
            "2026_11_16_13-46-13"
        );
    }
}
