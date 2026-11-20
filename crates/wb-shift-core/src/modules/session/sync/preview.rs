//! 同步预览：只读核对双方成员、基线与正文，生成预览项与一次性预览凭据绑定。
use super::liveness::{group_rows_alive, representative_member};
use super::*;
use crate::modules::session_link::{
    fingerprint_cache, stamp_content, BoundedCache, ContentStamp, FingerprintedContent,
};
use std::sync::OnceLock;

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
    build_binding(
        group,
        source_member,
        target_member,
        member_binding(source_member, source_content),
        member_binding(target_member, target_content),
        baseline,
        verdict,
        archive,
    )
}

/// 指纹口径的预览绑定（R5-16：判定缓存命中时只有摘要，没有正文）。
///
/// 与 [`live_preview_binding`] 装配同一份 `PreviewBinding`，差别只在成员摘要的来源；
/// 两种来源产出的 `raw_digest` / `normalized_digest` / `record_count` 同口径，
/// 因此预览凭据在执行前核对时仍然逐项可比。
#[allow(clippy::too_many_arguments)]
fn fingerprint_preview_binding(
    group: &LinkGroup,
    source_member: &LinkMember,
    target_member: &LinkMember,
    source_content: &FingerprintedContent,
    target_content: &FingerprintedContent,
    baseline: &BaselineState,
    verdict: SyncVerdict,
    archive: &PreviewArchiveState,
) -> PreviewBinding {
    build_binding(
        group,
        source_member,
        target_member,
        fingerprint_binding(source_member, source_content),
        fingerprint_binding(target_member, target_content),
        baseline,
        verdict,
        archive,
    )
}

/// 预览绑定的唯一装配点：成员摘要与判定结果都由调用方给齐。
#[allow(clippy::too_many_arguments)]
fn build_binding(
    group: &LinkGroup,
    source_member: &LinkMember,
    target_member: &LinkMember,
    source: PreviewMemberBinding,
    target: PreviewMemberBinding,
    baseline: &BaselineState,
    verdict: SyncVerdict,
    archive: &PreviewArchiveState,
) -> PreviewBinding {
    PreviewBinding {
        variant: group.variant,
        group_id: group.id.clone(),
        group_fingerprint: session_link::group_fingerprint(group),
        source,
        target,
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

/// 指纹口径的成员绑定（不可验证时摘要留空，判定必然为 unknown）。
///
/// 与 [`member_binding`] 同口径，但**拿不到正文**：`FingerprintedContent` 里就没有
/// `text` 字段，预览缓存因此不可能把正文带进内存。
fn fingerprint_binding(
    member: &LinkMember,
    content: &FingerprintedContent,
) -> PreviewMemberBinding {
    let (raw_digest, normalized_digest, record_count) = match content {
        FingerprintedContent::Ready(fingerprint) => (
            fingerprint.full_digest.clone(),
            fingerprint.normalized_digest.clone(),
            fingerprint.record_count,
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

// ---------------------------------------------------------------------------
// 预览判定的进程内缓存（R5-16：切号弹窗的摘要缓存）
// ---------------------------------------------------------------------------
//
// 背景：切号弹窗打开即调用预览，而预览原本逐组全量读取双方正文（实测某账号 106 个组、
// 双方正文合计约 988 MB），用户看到「点切换、还没开始切就很卡」。
//
// 安全边界（**执行时不得绕过**）：
//   - 只有本文件的只读判定可以命中缓存；
//   - 复制 / 同步的执行路径仍走 `member_content_state` → `read_content_snapshot`，
//     必须重新读取正文（它们要拿真实 `text` 落盘，且预览凭据本来就要求执行前逐项复核）；
//   - 缓存条目不提供正文（`PreviewDecisionEntry` 里只有判定标量与摘要指纹），
//     这条约束由类型保证，不是注释约定。

/// 预览判定缓存的条目上限。
///
/// 每条只含判定标量 + 两份摘要指纹（约 400 字节），2048 条约 0.8 MB，
/// 远低于「打开 106 个组后 RSS 增长 < 30 MB」的验收口径。
pub(in crate::modules::session) const PREVIEW_DECISION_CAPACITY: usize = 2048;

/// 预览判定的缓存键：**两侧内容身份 + 共同基线**，绝不能只按 session_id。
///
/// 两侧身份取 [`ContentStamp`]（规范化绝对路径 + session_id + len + mtime_ns）：
/// `len` 与 `mtime_ns` 任一变化就换了一个键，因此内容真正变了必然 miss，
/// 不会返回陈旧判定；`session_id` 必须在键里——归一化摘要按本副本 session_id 归一，
/// 漏掉它会把不同成员的摘要串味（R5-16 记的最危险错误）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::modules::session) struct PreviewDecisionKey {
    pub(in crate::modules::session) source: ContentStamp,
    pub(in crate::modules::session) target: ContentStamp,
    pub(in crate::modules::session) baseline: BaselineKey,
}

/// 共同基线的身份：可验证基线的内容身份（总摘要 + 记录数），或不可验证的形态。
///
/// 判定只依赖基线的有序行摘要，而总摘要由行摘要按长度编码推导，因此
/// `(total_digest, record_count)` 足以代表同一份基线内容。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::modules::session) enum BaselineKey {
    /// 可验证的共同基线（与 `BaselineRecord.line_digests` 一一对应）。
    Ready {
        total_digest: String,
        record_count: usize,
    },
    /// 该成员对从未建立过基线。
    Missing,
    /// 有基线引用但内容不可验证，携带不可验证原因（原因不同判定文案不同）。
    Unverifiable(String),
}

impl BaselineKey {
    /// 基线状态的身份投影（纯内存，不读文件）。
    fn of(baseline: &BaselineState) -> Self {
        match baseline {
            BaselineState::Ready(record) => Self::Ready {
                total_digest: record.total_digest.clone(),
                record_count: record.record_count,
            },
            BaselineState::Missing => Self::Missing,
            BaselineState::Unverifiable(reason) => Self::Unverifiable(reason.clone()),
        }
    }
}

/// 预览判定缓存的一条：判定标量 + 双方摘要指纹，**没有正文**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::modules::session) struct PreviewDecisionEntry {
    pub(in crate::modules::session) decision: SyncDecision,
    pub(in crate::modules::session) source: FingerprintedContent,
    pub(in crate::modules::session) target: FingerprintedContent,
}

impl PreviewDecisionEntry {
    /// 由判定结果与双方指纹组装条目（不拷贝正文）。
    fn new(
        decision: SyncDecision,
        source: FingerprintedContent,
        target: FingerprintedContent,
    ) -> Self {
        Self {
            decision,
            source,
            target,
        }
    }
}

/// 进程内共享的预览判定缓存（不落盘：缓存与磁盘不一致会导致误判）。
///
/// 预览是顺序执行，单锁的竞争可以忽略；上限由 `BoundedCache` 兜底，超出按插入顺序
/// 淘汰最早的一半，不引入 LRU 依赖。
pub(in crate::modules::session) fn preview_decisions(
) -> &'static BoundedCache<PreviewDecisionKey, PreviewDecisionEntry> {
    static CACHE: OnceLock<BoundedCache<PreviewDecisionKey, PreviewDecisionEntry>> =
        OnceLock::new();
    CACHE.get_or_init(|| BoundedCache::with_capacity(PREVIEW_DECISION_CAPACITY))
}

/// 预览侧的一个成员内容探测结果。
///
/// - `stamp`：缓存键要素（一次 `metadata`，不读正文）；
/// - `fingerprint`：摘要指纹视图，**没有正文**，可直接下发预览绑定；
/// - `state`：仅当本次真的读过文件时存在。判定缓存未命中的一侧要拿它跑 `decide_sync`
///   （判定依赖逐行摘要，指纹只留总摘要与记录数，不足以重算）。
struct ContentProbe {
    stamp: ContentStamp,
    fingerprint: FingerprintedContent,
    state: Option<ContentState>,
}

impl ContentProbe {
    /// 取真实内容状态：命中缓存的一侧没有正文，此时**回落到真实读取**（不返回陈旧判定）。
    fn take_state(&mut self) -> ContentState {
        match self.state.take() {
            Some(state) => state,
            None => state_of_stamp(&self.stamp),
        }
    }
}

/// 按身份探测结果真实读取一次正文（`Missing` 不必读，其余一律实读）。
fn state_of_stamp(stamp: &ContentStamp) -> ContentState {
    match stamp {
        ContentStamp::Stamped {
            path, session_id, ..
        } => session_link::read_content_snapshot(path, session_id),
        // 没有正文文件：与 `member_content_state` 同口径按 Missing 处理（不当作空正文）。
        ContentStamp::Missing => ContentState::Missing,
        // 元数据不可信时没有失效依据：调用方在 `probe_member` 里已经实读过，
        // 正常不会走到这里；真的走到就按不可验证处理，绝不猜测内容未变。
        ContentStamp::Unavailable(reason) => ContentState::Unavailable(reason.clone()),
    }
}

/// 探测成员内容：先看指纹缓存，未命中再真实读取并回填。
///
/// `Missing` / `Unavailable` 一律不写缓存（文件可能马上被创建或修复），
/// 且身份不可信的成员**必须回落到真实读取**，不得沿用任何陈旧指纹。
fn probe_member(paths: &SessionPaths, session_id: &str) -> ContentProbe {
    let Some(path) = find_project_jsonl(paths, session_id) else {
        return ContentProbe {
            stamp: ContentStamp::Missing,
            fingerprint: FingerprintedContent::Missing,
            state: Some(ContentState::Missing),
        };
    };
    let stamp = stamp_content(&path, session_id);
    // 指纹缓存只认 `Stamped`：`Missing` / `Unavailable` 从不入缓存，因此这里必然 miss。
    if let Some(fingerprint) = fingerprint_cache().lookup(&stamp) {
        return ContentProbe {
            stamp,
            fingerprint: FingerprintedContent::Ready(fingerprint),
            state: None,
        };
    }
    let state = session_link::read_content_snapshot(&path, session_id);
    let fingerprint = FingerprintedContent::from_state(&state, session_id, &stamp);
    if let FingerprintedContent::Ready(ready) = &fingerprint {
        fingerprint_cache().store(&stamp, ready.clone());
    }
    ContentProbe {
        stamp,
        fingerprint,
        state: Some(state),
    }
}

/// 判定的缓存键；任一侧身份不可信（`Missing` / `Unavailable`）时不给键，即不缓存。
fn decision_key(
    source: &ContentStamp,
    target: &ContentStamp,
    baseline: &BaselineState,
) -> Option<PreviewDecisionKey> {
    match (source, target) {
        (ContentStamp::Stamped { .. }, ContentStamp::Stamped { .. }) => Some(PreviewDecisionKey {
            source: source.clone(),
            target: target.clone(),
            baseline: BaselineKey::of(baseline),
        }),
        _ => None,
    }
}

/// 身份是否仍与磁盘一致（回填缓存前的最后一次核对）。
///
/// 读取期间被改写的正文不能挂在旧键上：挂上去下次预览就会把这具「过期摘要」
/// 当成最新结果。
fn stamp_still_valid(stamp: &ContentStamp) -> bool {
    match stamp {
        ContentStamp::Stamped {
            path, session_id, ..
        } => stamp_content(path, session_id) == *stamp,
        // 这两类从不入缓存，保守地判为不可信。
        _ => false,
    }
}

/// 预览判定：优先命中进程内缓存，未命中则真实读取双方正文后判定并回填。
///
/// 命中路径**零次正文读取**；未命中路径只读一次每侧，读完正文立即丢弃（条目里只留标量）。
fn cached_decision(
    source: &mut ContentProbe,
    target: &mut ContentProbe,
    baseline: &BaselineState,
) -> PreviewDecisionEntry {
    let key = decision_key(&source.stamp, &target.stamp, baseline);
    if let Some(hit) = key.as_ref().and_then(|key| preview_decisions().get(key)) {
        return hit;
    }
    // 判定依赖逐行摘要：任何一侧没有真实状态就必须补读（缓存只留了总摘要）。
    let source_state = source.take_state();
    let target_state = target.take_state();
    let decision = session_link::decide_sync(&source_state, &target_state, baseline);
    let entry = PreviewDecisionEntry::new(
        decision,
        FingerprintedContent::from_state(
            &source_state,
            &session_id_of(&source.stamp),
            &source.stamp,
        ),
        FingerprintedContent::from_state(
            &target_state,
            &session_id_of(&target.stamp),
            &target.stamp,
        ),
    );
    // 回填前再核一次身份：读取期间被改写的正文不得挂在旧键上。
    if let Some(key) = key {
        if stamp_still_valid(&source.stamp) && stamp_still_valid(&target.stamp) {
            preview_decisions().insert(key, entry.clone());
        }
    }
    entry
}

/// 身份里的 session_id（只有 `Stamped` 才有；其它形态不入缓存，取值仅用于拼指纹）。
fn session_id_of(stamp: &ContentStamp) -> String {
    match stamp {
        ContentStamp::Stamped { session_id, .. } => session_id.clone(),
        _ => String::new(),
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
            // 汇总：让用户一眼看到「有几条会话换号后不能再以原专家身份对话」。
            // 只计数不筛选，前端可据此给出筛选项；0 条时不出现该字段。
            let expert_missing = groups
                .iter()
                .filter(|item| {
                    item.get("expert")
                        .is_some_and(|expert| expert["state"] == "missing")
                })
                .count();
            if expert_missing > 0 {
                report["expertMissing"] = json!(expert_missing);
            }
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
    // R5-16：先做身份探测（一次 metadata），判定走进程内缓存——命中时零次正文读取。
    let mut source_probe = probe_member(&source_paths, &source_member.session_id);
    let mut target_probe = probe_member(&target_paths, &target_member.session_id);
    let rows_match = member_row_owned_by(&source_paths, source_member)
        && member_row_owned_by(&target_paths, target_member);
    let PreviewDecisionEntry {
        decision,
        source: source_content,
        target: target_content,
    } = if rows_match {
        cached_decision(&mut source_probe, &mut target_probe, &baseline)
    } else {
        // 会话行缺失/归属异常：成员实际已失效，与内容不可验证同等对待。
        // 这一路径不复用任何缓存结论（前提条件已经变了）。
        PreviewDecisionEntry::new(
            SyncDecision::unknown("会话记录缺失或归属异常，对应的会话已失效"),
            source_probe.fingerprint,
            target_probe.fingerprint,
        )
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

    // R5-13：专家绑定只做**展示判定**——目标账号不能用该专家时标黄提示，
    // 但默认仍勾选、仍复制（不改写任何 expert_* 列，也不复制专家包）。
    let (expert_id, expert_identity) =
        super::super::copy::read_expert_ref(&source_paths, &source_member.session_id);
    let expert = super::expert_binding::detect(
        &target_paths.data_root,
        target_uid,
        expert_id.as_deref(),
        expert_identity.as_deref(),
    );

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
            "source": source_content.record_count(),
            "target": target_content.record_count(),
            "baseline": baseline.ready().map(|record| record.record_count),
        },
        "source": source_summary,
        "target": target_summary,
    });
    if expert.state != "unknown" {
        item["expert"] = json!({
            "state": expert.state,
            "plugin": expert.plugin,
            "name": expert.name,
        });
    }
    if let Some(action) = archive_action.as_deref() {
        item["archiveAction"] = json!(action);
    }
    if actionable {
        let binding = fingerprint_preview_binding(
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

// ---------------------------------------------------------------------------
// 单测：判定键、探测命中、真实读取回落与容量上限（R5-16）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::session_link::{real_read_count, BaselineRecord};

    /// 两条记录的最小正文（sessionId 归一后两侧内容相同）。
    fn body(cid: &str) -> String {
        format!(
            "{}\n{}\n",
            json!({"type": "user", "sessionId": cid, "text": "hi"}),
            json!({"type": "assistant", "sessionId": cid, "text": "ok"})
        )
    }

    /// 只在测试里用的临时数据根（绝不碰真实 `~/.workbuddy`）。
    fn probe_env(name: &str) -> (PathBuf, SessionPaths) {
        let root = std::env::temp_dir().join(format!(
            "wb_shift_preview_cache_{}_{name}",
            uuid::Uuid::new_v4().simple()
        ));
        let paths = SessionPaths {
            store_root: root.join("store"),
            data_root: root.join("data"),
            auth_file: root.join("auth.info"),
            link_namespace: LinkNamespace::WorkBuddy,
        };
        std::fs::create_dir_all(paths.projects_dir()).unwrap();
        (root, paths)
    }

    fn stamp_of(len: u64, session_id: &str) -> ContentStamp {
        ContentStamp::Stamped {
            path: PathBuf::from(format!("C:/proj/ws/{session_id}.jsonl")),
            session_id: session_id.to_string(),
            len,
            mtime_ns: len as i128,
        }
    }

    fn eviction_key(index: usize) -> PreviewDecisionKey {
        PreviewDecisionKey {
            source: stamp_of(index as u64, "sess-a"),
            target: stamp_of(index as u64, "sess-b"),
            baseline: BaselineKey::Missing,
        }
    }

    /// 键 = 两侧内容身份 + 共同基线：任何一项变化都必须换键，缺身份时不缓存。
    #[test]
    fn decision_key_separates_identity_and_baseline() {
        let baseline = BaselineState::Missing;
        let key = decision_key(&stamp_of(1, "sess-a"), &stamp_of(2, "sess-b"), &baseline)
            .expect("两侧身份完整时应给出键");
        assert_eq!(
            key,
            decision_key(&stamp_of(1, "sess-a"), &stamp_of(2, "sess-b"), &baseline).unwrap(),
            "同样的输入必须给出同样的键"
        );

        // len 变化（= mtime 变化，测试里两者同步推进）→ 换键。
        assert_ne!(
            key,
            decision_key(&stamp_of(9, "sess-a"), &stamp_of(2, "sess-b"), &baseline).unwrap(),
            "来源 len 变化必须换键"
        );
        assert_ne!(
            key,
            decision_key(&stamp_of(1, "sess-a"), &stamp_of(9, "sess-b"), &baseline).unwrap(),
            "目标 len 变化必须换键"
        );
        // session_id 变化 → 换键（归一化摘要按本副本 sessionId 归一，串味最危险）。
        assert_ne!(
            key,
            decision_key(&stamp_of(1, "sess-x"), &stamp_of(2, "sess-b"), &baseline).unwrap(),
            "session_id 必须进键"
        );
        // 路径变化 → 换键（不同文件可能凑巧同长同 mtime）。
        let mut other_path = stamp_of(1, "sess-a");
        if let ContentStamp::Stamped { path, .. } = &mut other_path {
            *path = PathBuf::from("C:/other/ws/sess-a.jsonl");
        }
        assert_ne!(
            key,
            decision_key(&other_path, &stamp_of(2, "sess-b"), &baseline).unwrap(),
            "路径必须进键"
        );
        // 共同基线变化 → 换键。
        let baseline_ready = BaselineState::Ready(BaselineRecord {
            version: 1,
            baseline_ref: "ref".to_string(),
            normalization_version: 1,
            created_at: 1,
            record_count: 2,
            total_digest: "digest".to_string(),
            line_digests: vec!["a".to_string(), "b".to_string()],
        });
        assert_ne!(
            key,
            decision_key(
                &stamp_of(1, "sess-a"),
                &stamp_of(2, "sess-b"),
                &baseline_ready
            )
            .unwrap(),
            "基线变化必须换键"
        );

        // 任一侧身份不可信 → 不给键（Missing / Unavailable 从不入缓存）。
        assert!(decision_key(&ContentStamp::Missing, &stamp_of(2, "sess-b"), &baseline).is_none());
        assert!(decision_key(
            &ContentStamp::Unavailable("permission denied".to_string()),
            &stamp_of(2, "sess-b"),
            &baseline
        )
        .is_none());
    }

    /// 探测命中缓存时不再读正文；正文变化后才真实读取。
    #[test]
    fn probe_member_serves_fingerprint_without_reading_body() {
        let (root, paths) = probe_env("probe");
        std::fs::write(paths.projects_dir().join("sess-a.jsonl"), body("sess-a")).unwrap();

        let first = probe_member(&paths, "sess-a");
        assert_eq!(first.fingerprint.record_count(), 2);
        assert!(
            first.state.is_some(),
            "首次探测必须真实读取（否则无法判定）"
        );
        let reads_after_first = real_read_count();

        let second = probe_member(&paths, "sess-a");
        assert_eq!(
            real_read_count(),
            reads_after_first,
            "第二次探测必须命中指纹缓存、不得再读正文"
        );
        assert!(second.state.is_none(), "命中路径不得携带正文");
        assert_eq!(second.fingerprint, first.fingerprint);
        assert_eq!(second.stamp, first.stamp);

        // 正文不存在 → Missing，且不写缓存。
        let missing = probe_member(&paths, "sess-none");
        assert_eq!(missing.fingerprint, FingerprintedContent::Missing);
        assert_eq!(missing.stamp, ContentStamp::Missing);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 判定：命中时零次正文读取；内容变化（len/mtime）后必须重新读取并重新判定。
    #[test]
    fn cached_decision_hits_then_recomputes_after_append() {
        let (root, paths) = probe_env("decision");
        let source_path = paths.projects_dir().join("sess-a.jsonl");
        std::fs::write(&source_path, body("sess-a")).unwrap();
        std::fs::write(paths.projects_dir().join("sess-b.jsonl"), body("sess-b")).unwrap();
        let baseline = BaselineState::Missing;

        let mut source = probe_member(&paths, "sess-a");
        let mut target = probe_member(&paths, "sess-b");
        let first = cached_decision(&mut source, &mut target, &baseline);
        assert_eq!(first.decision.verdict, SyncVerdict::Identical);
        assert_eq!(first.source.record_count(), 2);
        assert_eq!(first.target.record_count(), 2);

        let reads_before = real_read_count();
        let mut source = probe_member(&paths, "sess-a");
        let mut target = probe_member(&paths, "sess-b");
        let second = cached_decision(&mut source, &mut target, &baseline);
        assert_eq!(real_read_count(), reads_before, "判定命中必须零次正文读取");
        assert_eq!(second, first, "命中返回同一条判定与同一份指纹");

        // 来源追加一条：len 与 mtime 都变了 → 必须重新读取，不得沿用旧判定。
        let mut text = std::fs::read_to_string(&source_path).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"type": "user", "sessionId": "sess-a", "text": "more"})
        ));
        std::fs::write(&source_path, &text).unwrap();
        let mut source = probe_member(&paths, "sess-a");
        let mut target = probe_member(&paths, "sess-b");
        let third = cached_decision(&mut source, &mut target, &baseline);
        assert!(
            real_read_count() > reads_before,
            "内容变化必须回落到真实读取"
        );
        assert_eq!(third.decision.verdict, SyncVerdict::FastForward);
        assert_eq!(third.source.record_count(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 条目上限 2048，超限按插入顺序淘汰最早的一半（本地缓存验证，不污染进程内实例）。
    #[test]
    fn preview_decision_cache_is_bounded() {
        assert_eq!(PREVIEW_DECISION_CAPACITY, 2048);
        let cache: BoundedCache<PreviewDecisionKey, PreviewDecisionEntry> =
            BoundedCache::with_capacity(PREVIEW_DECISION_CAPACITY);
        let entry = PreviewDecisionEntry::new(
            SyncDecision::unknown("capacity"),
            FingerprintedContent::Missing,
            FingerprintedContent::Missing,
        );
        for index in 0..PREVIEW_DECISION_CAPACITY + 1 {
            cache.insert(eviction_key(index), entry.clone());
        }
        assert!(
            cache.len() <= PREVIEW_DECISION_CAPACITY,
            "超过上限必须淘汰，实际 {}",
            cache.len()
        );
        assert!(cache.get(&eviction_key(0)).is_none(), "最早的一批应被淘汰");
        assert!(
            cache
                .get(&eviction_key(PREVIEW_DECISION_CAPACITY))
                .is_some(),
            "最新一条必须留着"
        );
    }
}
