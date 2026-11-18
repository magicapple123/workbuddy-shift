//! 关联组模型、校验和主表读写。

use super::super::operation_log::{try_lock_file, FileLock, LockError, Operation};
use crate::modules::session::SessionPaths;
use crate::modules::session_backup;
use crate::modules::variant::WbVariant;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

/// 关联组主表格式版本；读到其它版本一律禁写（保留现场）。
pub const LINK_STORE_VERSION: u32 = 1;
/// 存储锁的最长等待（存储锁只用于短时读改写）。
const STORE_LOCK_RETRY: usize = 25;
const STORE_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------

/// 关联组主表。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStore {
    pub version: u32,
    pub revision: u64,
    #[serde(default)]
    pub groups: Vec<LinkGroup>,
}
impl LinkStore {
    pub fn empty() -> Self {
        Self {
            version: LINK_STORE_VERSION,
            revision: 0,
            groups: Vec::new(),
        }
    }
}

/// 一个逻辑会话的接力组：同一逻辑会话的各账号副本归入同一组。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkGroup {
    pub id: String,
    pub variant: WbVariant,
    pub created_at: i64,
    #[serde(default)]
    pub members: Vec<LinkMember>,
    #[serde(default)]
    pub pair_bases: Vec<PairBase>,
}

/// 组内成员状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberState {
    /// 该账号当前的接力成员。
    Active,
    /// 已判定失效（正文/行缺失）但尚未被替换，保留记录不自动复活。
    Stale,
    /// 已被同账号的更新成员替换，保留记录不自动复活。
    Superseded,
}

impl MemberState {
    /// 稳定字符串（组指纹与上报用；与 serde 输出保持一致，有单测守住）。
    pub fn as_str(self) -> &'static str {
        match self {
            MemberState::Active => "active",
            MemberState::Stale => "stale",
            MemberState::Superseded => "superseded",
        }
    }
}

/// 组内成员：某个账号上的某一个会话副本。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkMember {
    pub member_id: String,
    /// 工具内账号 id（可空：独立入口只有 uid）。身份判定以 uid 为准。
    #[serde(default)]
    pub account_id: Option<String>,
    pub uid: String,
    pub session_id: String,
    /// 该副本所属档位（跨档组成员各自不同）。旧存储没有此字段：
    /// 载入时用组级 `variant` 回填，见 `backfill_member_variants`。
    #[serde(default)]
    pub variant: Option<WbVariant>,
    pub state: MemberState,
    pub linked_at: i64,
    #[serde(default)]
    pub last_synced_at: Option<i64>,
}

/// 成员所属档位：显式保存优先；旧存储（尚未回填）回退组级 `variant`。
pub fn member_variant(group: &LinkGroup, member: &LinkMember) -> WbVariant {
    member.variant.unwrap_or(group.variant)
}

/// 旧存储兼容：成员缺 `variant` 时用组级 `variant` 回填。
///
/// 新建成员一律写入显式档位（跨档组必须如此）；仅在 `load_store` 内调用，
/// 使读取后的存储在身份判定与一致性校验前就具备完整档位信息。
fn backfill_member_variants(store: &mut LinkStore) {
    for group in store.groups.iter_mut() {
        for member in group.members.iter_mut() {
            if member.variant.is_none() {
                member.variant = Some(group.variant);
            }
        }
    }
}

/// 配对基线引用（成员对无序存储，避免同一对出现两条记录）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairBase {
    pub member_ids: [String; 2],
    pub baseline_ref: String,
    pub normalization_version: u32,
}

// ---------------------------------------------------------------------------
// 关联存储读写
// ---------------------------------------------------------------------------
/// 存储读取结果。
#[derive(Debug, Clone)]
pub enum StoreState {
    /// 首次使用：无主文件且无未完成痕迹。
    Missing,
    Ready(LinkStore),
    /// 保留现场并禁止写入。
    Unavailable(String),
}

fn acquire_link_store_lock(paths: &SessionPaths) -> Result<FileLock, String> {
    let path = paths.link_store_lock_file();
    for _ in 0..STORE_LOCK_RETRY {
        match try_lock_file(&path) {
            Ok(lock) => return Ok(lock),
            Err(LockError::Busy) => std::thread::sleep(STORE_LOCK_RETRY_INTERVAL),
            Err(LockError::Unavailable(reason)) => {
                return Err(format!("同步记录暂时不可用：{reason}"));
            }
        }
    }
    Err("同步记录正被其它操作占用，请稍后重试".to_string())
}

// ---------------------------------------------------------------------------
// 关联存储读写
// ---------------------------------------------------------------------------

/// 目录下是否存在任意 `*.json` 条目（目录读不了时按「没有」处理）。
fn has_json_entries(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
}

/// 是否存在未完成的残留痕迹（主文件缺失时据此拒绝初始化）。
///
/// 基线文件同样算痕迹：基线代表既有的关联关系，主文件缺失只能是异常现场，
/// 不能当成首次使用重建空表（design §2）。
fn has_unfinished_traces(paths: &SessionPaths) -> bool {
    if has_json_entries(&paths.baselines_dir()) {
        return true;
    }
    let dir = paths.operations_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            // 读不了的残片同样算痕迹：不能当成空表覆盖。
            return true;
        };
        match serde_json::from_str::<Operation>(&text) {
            Ok(operation) => {
                if operation.phase.is_unfinished() {
                    return true;
                }
            }
            Err(_) => return true,
        }
    }
    false
}

/// 读取关联主表；区分首次缺失与不可用。
pub fn load_store(paths: &SessionPaths) -> StoreState {
    let file = paths.session_links_file();
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if has_unfinished_traces(paths) {
                return StoreState::Unavailable(
                    "同步记录主文件缺失但存在未完成的痕迹（操作记录或上次同步的文件），已保留现场"
                        .to_string(),
                );
            }
            return StoreState::Missing;
        }
        Err(error) => return StoreState::Unavailable(format!("同步记录无法读取：{error}")),
    };
    let mut store = match serde_json::from_str::<LinkStore>(&text) {
        Ok(store) => store,
        Err(_) => {
            return StoreState::Unavailable("同步记录已损坏，原文件已保留".to_string());
        }
    };
    if store.version != LINK_STORE_VERSION {
        return StoreState::Unavailable(format!(
            "同步记录版本 {} 不受支持（当前支持 {}），原文件已保留",
            store.version, LINK_STORE_VERSION
        ));
    }
    // 旧存储兼容：成员缺 `variant` 时用组级 `variant` 回填（跨档组成员显式保存）。
    backfill_member_variants(&mut store);
    if let Err(reason) = validate_store(&store) {
        return StoreState::Unavailable(format!("同步记录内容不一致：{reason}"));
    }
    StoreState::Ready(store)
}

/// 不变量校验（写入前与读取后都执行）：
/// - 组 id 唯一；
/// - 同一 (variant, uid, sessionId) 只属于一个组（成员档位取 `member_variant`）；
/// - 每组每个账号至多一个 active 成员；
/// - 配对基线双方都在组内且不重复。
pub fn validate_store(store: &LinkStore) -> Result<(), String> {
    let mut group_ids = std::collections::HashSet::new();
    let mut identities = std::collections::HashSet::new();
    for group in &store.groups {
        if !group_ids.insert(group.id.as_str()) {
            return Err(format!("组记录重复：{}", group.id));
        }
        let mut member_ids = std::collections::HashSet::new();
        let mut active_by_uid = std::collections::HashSet::new();
        for member in &group.members {
            if !member_ids.insert(member.member_id.as_str()) {
                return Err(format!("成员记录重复：{}", member.member_id));
            }
            let identity = (
                member_variant(group, member).as_str(),
                member.uid.as_str(),
                member.session_id.as_str(),
            );
            if !identities.insert(identity) {
                return Err(format!("会话 {} 同时属于多个同步组", member.session_id));
            }
            if member.state == MemberState::Active && !active_by_uid.insert(member.uid.as_str()) {
                return Err(format!(
                    "账号 {} 在同一组内出现多条有效成员记录",
                    member.uid
                ));
            }
        }
        let mut pairs = std::collections::HashSet::new();
        for pair in &group.pair_bases {
            if !pairs.insert(pair_key(&pair.member_ids[0], &pair.member_ids[1])) {
                return Err(format!(
                    "同一对成员重复保存了同步记录：{}",
                    pair.baseline_ref
                ));
            }
            for member_id in pair.member_ids.iter() {
                if !member_ids.contains(member_id.as_str()) {
                    return Err(format!("同步记录引用了不存在的成员：{member_id}"));
                }
            }
        }
    }
    Ok(())
}

/// 规范化的成员对 key（无序成员对唯一）。
pub fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn pair_matches(pair: &PairBase, key: &(String, String)) -> bool {
    pair.member_ids[0] == key.0 && pair.member_ids[1] == key.1
}

/// 关联存储读改写：在存储锁内重读、修改、校验、原子写回并递增 revision。
///
/// 存储不可用（损坏/未知版本/权限失败）时直接失败：不得降级成空表覆盖。
pub fn with_link_store_write<T>(
    paths: &SessionPaths,
    mutate: impl FnOnce(&mut LinkStore) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = acquire_link_store_lock(paths)?;
    let (mut store, existed) = match load_store(paths) {
        StoreState::Missing => (LinkStore::empty(), false),
        StoreState::Ready(store) => (store, true),
        StoreState::Unavailable(reason) => return Err(reason),
    };
    let outcome = mutate(&mut store)?;
    validate_store(&store).map_err(|reason| format!("拒绝保存不一致的同步记录：{reason}"))?;
    store.version = LINK_STORE_VERSION;
    store.revision = store.revision.saturating_add(1);
    let content = serde_json::to_string_pretty(&store).map_err(|error| error.to_string())?;
    if let Some(parent) = paths.session_links_file().parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    // 共享关联表属于业务完成门禁：走会话专用持久化写（design §4）。
    session_backup::durable_write_str(&paths.session_links_file(), &content).map_err(|error| {
        if !existed {
            format!("同步记录首次保存失败：{error}")
        } else {
            format!("同步记录保存失败：{error}")
        }
    })?;
    Ok(outcome)
}

/// 按 (variant, uid, sessionId) 查找所属组（身份唯一归组；成员档位取 `member_variant`）。
pub fn find_group_for_identity<'a>(
    store: &'a LinkStore,
    variant: WbVariant,
    uid: &str,
    session_id: &str,
) -> Option<&'a LinkGroup> {
    store.groups.iter().find(|group| {
        group.members.iter().any(|member| {
            member_variant(group, member) == variant
                && member.uid == uid
                && member.session_id == session_id
        })
    })
}

/// 组内某账号的 active 成员。
pub fn active_member_for<'a>(group: &'a LinkGroup, uid: &str) -> Option<&'a LinkMember> {
    group
        .members
        .iter()
        .find(|member| member.uid == uid && member.state == MemberState::Active)
}

/// 组内某账号的**代表成员**：标题、摘要与存活判定共用同一口径。
///
/// 优先 active（该账号当前的接力成员）；全为 stale/superseded 时退化为 `linked_at`
/// 最新的成员。不能取「第一条该 uid 的成员」：[`add_active_member`] 把旧 active **原位**
/// 转 superseded、新成员 push 到末尾，所以真实顺序是 `[旧 superseded, 新 active]`，
/// 取第一条会拿到已删除旧副本的 `(无标题)` 与 `superseded` 状态，而该组本身完全可同步。
///
/// WorkBuddy 预览（`session::sync::liveness`）与插件预览（`vscode_session_sync`）都走这里，
/// 两侧口径必须一致，不要再各自实现一份。
pub fn representative_member<'a>(group: &'a LinkGroup, uid: &str) -> Option<&'a LinkMember> {
    active_member_for(group, uid).or_else(|| {
        group
            .members
            .iter()
            .filter(|member| member.uid == uid)
            .max_by_key(|member| member.linked_at)
    })
}

/// 组内按 (uid, sessionId) 定位成员（任意状态）。
pub fn find_member<'a>(
    group: &'a LinkGroup,
    uid: &str,
    session_id: &str,
) -> Option<&'a LinkMember> {
    group
        .members
        .iter()
        .find(|member| member.uid == uid && member.session_id == session_id)
}

/// 加入一个 active 成员：同账号已有的 active 成员显式转 superseded。
///
/// 旧成员保留记录且不再有效；正文之后被恢复也不会自动复活（design §2）。
pub fn add_active_member(group: &mut LinkGroup, member: LinkMember) {
    for existing in group.members.iter_mut() {
        if existing.uid == member.uid && existing.state == MemberState::Active {
            existing.state = MemberState::Superseded;
        }
    }
    group.members.push(member);
}

/// 显式改变成员状态（失效标记/恢复记录用；不做自动复活）。
pub fn set_member_state(group: &mut LinkGroup, member_id: &str, state: MemberState) -> bool {
    match group
        .members
        .iter_mut()
        .find(|member| member.member_id == member_id)
    {
        Some(member) => {
            member.state = state;
            true
        }
        None => false,
    }
}

/// 定位成员对的基线引用（无序）。
pub fn find_pair_base<'a>(
    group: &'a LinkGroup,
    member_a: &str,
    member_b: &str,
) -> Option<&'a PairBase> {
    let key = pair_key(member_a, member_b);
    group
        .pair_bases
        .iter()
        .find(|pair| pair_matches(pair, &key))
}

/// 定向写入成员对的基线：只影响这一对，不触碰同组成员的其它配对。
pub fn set_pair_base(
    group: &mut LinkGroup,
    member_a: &str,
    member_b: &str,
    baseline_ref: &str,
    normalization_version: u32,
) {
    let key = pair_key(member_a, member_b);
    let entry = PairBase {
        member_ids: [key.0.clone(), key.1.clone()],
        baseline_ref: baseline_ref.to_string(),
        normalization_version,
    };
    match group
        .pair_bases
        .iter_mut()
        .find(|pair| pair_matches(pair, &key))
    {
        Some(existing) => *existing = entry,
        None => group.pair_bases.push(entry),
    }
}
