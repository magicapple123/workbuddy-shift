//! 写入同步：账号写入认证文件、logout 标记与默认值填充，以及凭据写入互斥。
//!
//! # 凭据写入的临界区（issue #202）
//!
//! 两个入口会改动「本机凭据」，它们必须共用同一把跨进程互斥：
//!
//! 1. **切号写回**（[`write_back_credentials`]）：读账号库快照 → 关进程/复制会话 → 把快照
//!    写进官方认证文件；
//! 2. **令牌刷新回写**（`refresh::refresh_account_token`）：读快照 → 打刷新接口 → 写回账号库。
//!
//! 两者交叉会让「切号读到的旧快照」盖掉「刷新刚拿到的新令牌」，官方客户端于是拿着一份
//! 已失效的令牌启动，而且整个切换还返回成功——这是本仓库唯一会**静默写坏凭据**的路径。
//!
//! ## 锁顺序（唯一正解，不得反向）
//!
//! ```text
//! [1] credentials.lock            ← 新增。层级最外：持锁期间**不再获取任何其它锁**
//! [2] session-ops-{variant}.lock  ← session_link 既有顺序
//! [3] session-links.lock          ← session_link 既有顺序（只在 [2] 之内获取）
//! ```
//!
//! 编号就是获取优先级：低编号绝不可以在**持有**高编号锁时获取。本文件只在 [1] 上工作，
//! 且从不嵌套——切号进入写回这一步时，会话复制/同步早已返回，[2]/[3] 都已随作用域释放
//! （见 `switch/execute.rs` 的调用位置）。刷新侧同样只在最外层获取 [1]。
//! 因此既不会与 `session_link` 的既有顺序冲突，也不存在「A→B 与 B→A」的交叉持有。
//!
//! ## 临界区覆盖多长（两侧不同，报时长前先分清是哪一侧）
//!
//! - **切号侧**（[`write_back_credentials`]）：只覆盖「重读 + 校验 + 写回」那几毫秒。不覆盖
//!   关进程/恢复/复制会话（可能持续数分钟，有用户单账号 988MB）；「手里的快照已经过时」由
//!   写回前的 [`ensure_snapshot_is_current`] 兜住，最坏结果是走到最后一步放弃写回并提示重切；
//! - **刷新侧**（`refresh::refresh_account_token`）：覆盖「打接口 + 写回账号库」全程，**最长
//!   30s**（reqwest 客户端总超时，见 `config/http.rs` 的 `.timeout(30s)`）；保活串行刷新多个
//!   账号时，前一个账号持锁的时间还会顺延到后一个账号身上，实际窗口更宽。
//!
//! 代价是真实的：刷新持锁的那段时间（最长 30s、保活时更久）里用户点切号，会在关进程、复制
//! 会话都**做完之后**才拿到 busy——整次切换白做。仍然这样选，是因为反过来「先请求后抢锁」会让
//! 服务端已经轮换的旧令牌留在盘上、账号直接变 `needs_relogin`，比切号最后一步失败严重得多。
//!
//! 抢不到锁一律**立即失败**（不等待、不重试、不降级继续），并保证不写入任何凭据——
//! 产品决策见 issue #202。

use super::read::*;
use super::*;

use crate::modules::config::store_dir;
use crate::modules::session_link::{try_lock_file, FileLock, LockError};

/// 凭据写入锁文件名（落在工具存储根的 `locks/` 下）。
///
/// 与 `session_link` 的档位锁/关联存储锁同目录，便于用户与诊断脚本一眼看全本机的锁。
pub const CREDENTIAL_LOCK_FILE: &str = "credentials.lock";

/// 抢不到凭据写入锁时的统一文案。
///
/// 产品决策（issue #202）：**不等待、不降级继续**，直接告诉用户稍后重试；这条文案同时
/// 说明「没有写入任何凭据」，避免用户以为切号已经部分生效。
pub const CREDENTIAL_LOCK_BUSY_MESSAGE: &str =
    "正在执行其他切换/刷新操作，请稍后重试（未写入任何凭据）";

/// 无法建立互斥（不是「被别人占用」，而是锁文件不可用）时的文案前缀。
pub const CREDENTIAL_LOCK_UNAVAILABLE_PREFIX: &str = "无法建立凭据写入互斥";

/// 锁失败 → 面向用户的文案。`Busy` 与 `Unavailable` 必须能区分：前者等一会儿再试即可，
/// 后者是权限/磁盘问题（继续重试也没有意义）。
fn credential_lock_failure(error: LockError) -> String {
    match error {
        LockError::Busy => CREDENTIAL_LOCK_BUSY_MESSAGE.to_string(),
        LockError::Unavailable(reason) => {
            format!("{CREDENTIAL_LOCK_UNAVAILABLE_PREFIX}：{reason}（未写入任何凭据）")
        }
    }
}

/// 获取凭据写入互斥（生产路径：工具存储根 `~/.workbuddy-shift`）。
///
/// 守卫是 RAII：任何提前返回（含 `?`）都会在作用域结束时释放，不存在漏放。
pub fn try_acquire_credential_write_lock() -> Result<FileLock, String> {
    try_acquire_credential_write_lock_at(&store_dir())
}

/// [`try_acquire_credential_write_lock`] 的可测实现：显式传入工具存储根。
pub(super) fn try_acquire_credential_write_lock_at(store_root: &Path) -> Result<FileLock, String> {
    try_lock_file(&store_root.join("locks").join(CREDENTIAL_LOCK_FILE))
        .map_err(credential_lock_failure)
}

// ---------------------------------------------------------------------------
// 「手上的快照是否已经过时」判定
// ---------------------------------------------------------------------------

/// 秒/毫秒混写分界：10 位秒必然小于它，13 位毫秒必然大于它。
///
/// 与 `read.rs` 的 `SECONDS_UPPER_BOUND` 同一条分界（那里是私有常量）。官方认证文件历史上
/// 两种写法都出现过；不先分数量级就比大小，秒级快照会被毫秒级磁盘记录「无条件判旧」。
const SECONDS_UPPER_BOUND_MS: i64 = 100_000_000_000;

/// 把「数字 / 数字字符串 / 秒级 / 毫秒级」的时间戳归一成毫秒；不可用值返回 `None`。
///
/// 与 `normalize_refresh_ts` 的差别：**不**丢弃未来时刻。`expiresAt` 天然在未来，
/// 而「磁盘上的到期时间比快照更晚」正是「有人接着刷新过」的信号。
fn timestamp_ms(value: &Value) -> Option<i64> {
    let raw = match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f as i64))?,
        Value::String(text) => text.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    if raw <= 0 {
        return None;
    }
    let ms = if raw < SECONDS_UPPER_BOUND_MS {
        raw.checked_mul(1000)?
    } else {
        raw
    };
    (ms > 0).then_some(ms)
}

/// 一份凭据记录的「新鲜度」：只有两个时间戳能证明「谁更新」。
///
/// 刻意不用文件 mtime：本工具自己的写入也会刷新 mtime（`accounts.json` 在写备注、记录
/// 最后使用时间时都会被重写），按 mtime 判定会把纯展示类改动误判成「凭据已更新」；
/// 内容摘要同理——`upsert_account` 会重排 JSON，摘要一定变。只有续期会推进
/// `refreshedAt` / `expiresAt`，因此它们是唯一不会误伤正常流程的判据。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialFreshness {
    /// 最近一次**真正发生过**的续期时刻（毫秒）；未知为 `None`。
    refreshed_at: Option<i64>,
    /// access token 到期时刻（毫秒）；未知为 `None`。
    expires_at: Option<i64>,
}

impl CredentialFreshness {
    /// 账号库记录的新鲜度：`refreshedAt` 与 `auth_raw` 里两种 `lastRefreshTime` 结构取最大。
    ///
    /// `auth_raw` 存的是导入时的完整认证文件根，历史上平铺与嵌套两种结构都出现过
    /// （与 `export_last_refresh_time` 同一口径）。
    pub fn of_account(acc: &Value) -> Self {
        let now = now_ms();
        let raw = acc.get("auth_raw");
        let refreshed_at = [
            acc.get("refreshedAt")
                .and_then(|value| normalize_refresh_ts(value, now)),
            raw.and_then(|root| root.get("lastRefreshTime"))
                .and_then(|value| normalize_refresh_ts(value, now)),
            raw.and_then(|root| root.get("auth"))
                .and_then(|auth| auth.get("lastRefreshTime"))
                .and_then(|value| normalize_refresh_ts(value, now)),
        ]
        .into_iter()
        .flatten()
        .max();
        Self {
            refreshed_at,
            expires_at: acc.get("expiresAt").and_then(timestamp_ms),
        }
    }

    /// 官方认证文件根的新鲜度：`auth.lastRefreshTime` / `auth.expiresAt`（平铺写法也认）。
    pub fn of_auth_file(root: &Value) -> Self {
        let now = now_ms();
        let auth = root.get("auth");
        let refreshed_at = [
            auth.and_then(|inner| inner.get("lastRefreshTime"))
                .and_then(|value| normalize_refresh_ts(value, now)),
            root.get("lastRefreshTime")
                .and_then(|value| normalize_refresh_ts(value, now)),
        ]
        .into_iter()
        .flatten()
        .max();
        Self {
            refreshed_at,
            expires_at: auth
                .and_then(|inner| inner.get("expiresAt"))
                .or_else(|| root.get("expiresAt"))
                .and_then(timestamp_ms),
        }
    }

    /// 最近一次续期时刻（毫秒，未知为 `None`）。
    pub fn refreshed_at(self) -> Option<i64> {
        self.refreshed_at
    }

    /// access token 到期时刻（毫秒，未知为 `None`）。
    pub fn expires_at(self) -> Option<i64> {
        self.expires_at
    }

    /// `self`（磁盘）是否**严格新于** `snapshot`（本次切号手上那份）。
    ///
    /// 返回 `Some(原因)` 表示必须放弃写回。两条判据都只在**双方都有值且磁盘更晚**时成立：
    ///
    /// - 任一侧缺值就不比较：旧记录没有 `refreshedAt`，硬比会把「未知」判成「更旧」，
    ///   正常切号会被拦住；
    /// - 相等不算新：正常切号读到的快照与磁盘完全一致，必须放行。
    pub fn newer_than(self, snapshot: &Self) -> Option<String> {
        if let (Some(disk), Some(known)) = (self.refreshed_at, snapshot.refreshed_at) {
            if disk > known {
                return Some(format!("磁盘刷新时间 {disk} 晚于快照 {known}"));
            }
        }
        if let (Some(disk), Some(known)) = (self.expires_at, snapshot.expires_at) {
            if disk > known {
                return Some(format!("磁盘到期时间 {disk} 晚于快照 {known}"));
            }
        }
        None
    }
}

/// 目标账号与「官方认证文件当前登录的账号」是否同一个（按 uid，回退 id）。
///
/// 只有同一个账号才比较新鲜度：官方文件里通常是**源账号**，拿它的时间戳去卡目标账号的
/// 快照会把「切到一个很久没刷新的账号」误判成「磁盘更新」。任一侧取不到身份时按「不是
/// 同一个」处理，宁可不拦也不误伤。
pub(super) fn same_account(acc: &Value, root: &Value) -> bool {
    let target = get_str(acc, "uid").or_else(|| get_str(acc, "id"));
    let current = root
        .get("account")
        .and_then(|inner| get_str(inner, "uid"))
        .or_else(|| get_str(root, "uid"))
        .or_else(|| root.get("account").and_then(|inner| get_str(inner, "id")));
    match (target, current) {
        (Some(target), Some(current)) => target == current,
        _ => false,
    }
}

/// 写回前的新鲜度校验：手上快照是否已被磁盘上的记录超越。
///
/// - `store_record`：**紧贴写回之前**从账号库重读的同一条记录（调用方在锁内取）；
/// - `auth_root`：**紧贴写回之前**读到的官方认证文件（可能属于别的账号）。
///
/// 返回 `Err` = 放弃写回，**不得**返回成功（issue #202 的失败语义）。
pub fn ensure_snapshot_is_current(
    acc: &Value,
    store_record: Option<&Value>,
    auth_root: Option<&Value>,
) -> Result<(), String> {
    let snapshot = CredentialFreshness::of_account(acc);
    if let Some(record) = store_record {
        if let Some(reason) = CredentialFreshness::of_account(record).newer_than(&snapshot) {
            return Err(format!(
                "账号凭据在本次切换期间已被刷新（{reason}），已放弃写回登录态：\
                 用旧令牌覆盖刚续期的新令牌会让 WorkBuddy 一启动就掉线；请重新切换一次"
            ));
        }
    }
    if let Some(root) = auth_root {
        if same_account(acc, root) {
            if let Some(reason) = CredentialFreshness::of_auth_file(root).newer_than(&snapshot) {
                return Err(format!(
                    "官方登录态在本次切换期间已被刷新（{reason}），已放弃写回登录态：\
                     用旧令牌覆盖刚续期的新令牌会让 WorkBuddy 一启动就掉线；请重新切换一次"
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 切号写回：抢锁 → 锁内重读 → 校验 → 原子写
// ---------------------------------------------------------------------------

/// 把账号写回官方认证文件的**唯一入口**（切号路径）。
///
/// 临界区只覆盖「重读 + 校验 + 写入」这几毫秒（**本函数的切号侧口径**；刷新侧的临界区覆盖
/// 「打接口 + 写回账号库」全程、最长 30s，两侧不同，见文件头）：
///
/// - 为什么不从切号一开始就持锁：关进程 + 恢复/复制会话可能持续数分钟（有用户单账号
///   988MB），把令牌刷新一起挡在外面会让积分页/保活在这段时间里全部报「稍后重试」。
///   快照过时这件事由 [`ensure_snapshot_is_current`] 兜住，判据见该函数；
/// - 为什么仍然必须持锁：刷新侧在同一把锁内完成「打接口 + 写回账号库」，因此本函数
///   「重读 → 写入」之间不可能夹进一次刷新，校验结论在写入时仍然成立。
pub fn write_back_credentials(acc: &Value, variant: WbVariant) -> Result<(), String> {
    let auth_path = variant.auth_file_path();
    write_back_credentials_at(acc, &auth_path, &store_dir(), || {
        account_identity(acc).and_then(|identity| crate::modules::account::find_account(&identity))
    })
}

/// 账号的稳定身份（`id` 优先，历史数据回退 `uid`）——用于锁内重读同一条账号库记录。
fn account_identity(acc: &Value) -> Option<String> {
    get_str(acc, "id").or_else(|| get_str(acc, "uid"))
}

/// [`write_back_credentials`] 的可测实现：锁目录、认证文件路径与「锁内重读」都可注入。
///
/// `fresh_record` 是 `FnOnce`（而不是先算好的值）：它必须在**抢到锁之后**才执行，
/// 单测用「抢锁失败时闭包 panic」来固定这个顺序。
pub(super) fn write_back_credentials_at<F>(
    acc: &Value,
    auth_path: &Path,
    store_root: &Path,
    fresh_record: F,
) -> Result<(), String>
where
    F: FnOnce() -> Option<Value>,
{
    // ① 抢锁。抢不到立即失败：此时**不读账号库、不读认证文件、不写任何文件**。
    let _lock = try_acquire_credential_write_lock_at(store_root)?;
    // ② 锁内重读磁盘真身：此后直到写入之间不会再有本工具的刷新插进来。
    let fresh = fresh_record();
    let current = read_auth_file_at(auth_path);
    // ③ 校验手上的快照没有被磁盘超越；被超越就放弃写回（不覆盖、不返回成功）。
    ensure_snapshot_is_current(acc, fresh.as_ref(), current.as_ref())?;
    // ④ 原子写 + 写后校验 + 清理退出标记（与改造前同一实现）。
    write_account_to_auth_file_at(acc, auth_path)
}

/// 把账号写入官方认证文件（原子写 + 写后校验）。对照 server.py `write_account_to_auth_file`。
///
/// **这是不带锁、不带新鲜度校验的底层原语**：切号必须走 [`write_back_credentials`]，
/// 它是唯一「抢锁 → 锁内重读 → 校验快照未过时 → 写入」的入口。直接调用本函数等于放弃
/// issue #202 的两道保护；保留它只为「已经处在临界区内」的调用方与单测。
pub fn write_account_to_auth_file(acc: &Value, variant: WbVariant) -> Result<(), String> {
    write_account_to_auth_file_at(acc, &variant.auth_file_path())
}

pub(super) fn write_account_to_auth_file_at(acc: &Value, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let existing = read_auth_file_at(path).unwrap_or_else(|| json!({}));
    eprintln!(
        "[auth] write_account: existing is_object={} allAccounts_len={}",
        existing.is_object(),
        existing
            .get("allAccounts")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    );
    let all_accounts = existing
        .get("allAccounts")
        .cloned()
        .or_else(|| existing.get("accounts").cloned())
        .filter(|v| v.is_array())
        .unwrap_or_else(|| json!([]));
    let account_obj = build_account_obj(acc);
    let auth_obj = build_auth_obj(acc);

    // 把目标账号并入 allAccounts（去重：按 uid 或 id）
    let target_uid = get_str(acc, "uid").unwrap_or_default();
    let mut all: Vec<Value> = all_accounts.as_array().cloned().unwrap_or_default();
    all.retain(|a| {
        let primary = a
            .get("uid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                a.get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("");
        primary != target_uid
    });
    all.push(account_obj.clone());
    eprintln!("[auth] write_account: merged allAccounts len={}", all.len());

    let session = json!({
        "account": &account_obj,
        "auth": &auth_obj,
        "accounts": &all,
        "allAccounts": &all,
    });
    let content = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
    if let Err(e) = atomic_write(path, &content) {
        eprintln!("[auth] atomic_write FAILED: {e}");
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(
                "无权限写入认证文件：请打开 系统设置→隐私与安全性→App 管理，允许本 App 控制 WorkBuddy 的数据（或为其开启『完全磁盘访问』后重试）"
                    .to_string(),
            );
        }
        return Err(e.to_string());
    }

    // 写后校验：按值比较（token 可能是明文字符串，也可能是 WorkBuddy 5.6 加密信封对象）
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let written_token = written
        .get("auth")
        .and_then(|a| a.get("accessToken"))
        .cloned()
        .unwrap_or(Value::Null);
    let expect_token = auth_obj.get("accessToken").cloned().unwrap_or(Value::Null);
    if written_token != expect_token {
        return Err("认证文件写后校验失败，未写入目标账号".to_string());
    }

    // WorkBuddy 优先读取退出标记：即使认证文件已有凭据，标记仍存在时也视为未登录。
    // 必须在写入及校验成功后清理；失败时保留退出状态，并阻止调用方报告切换成功。
    let marker = logout_marker_path(path);
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "清理 WorkBuddy 退出标记失败（{}）：{error}",
                marker.display()
            ));
        }
    }
    Ok(())
}

/// 官方退出标记是在完整认证文件名后追加后缀，不能替换 `.info` 扩展名。
pub(super) fn logout_marker_path(auth_path: &Path) -> PathBuf {
    let mut marker = auth_path.as_os_str().to_os_string();
    marker.push(".logged-out");
    PathBuf::from(marker)
}

pub(super) fn setdefault(map: &mut Map<String, Value>, key: &str, value: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), value);
    }
}
