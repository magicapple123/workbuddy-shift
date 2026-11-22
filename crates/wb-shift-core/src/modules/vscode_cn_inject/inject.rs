//! secret 读写与注入：safeStorage 行读取、检测与写入 secret。

use std::time::Duration;

#[allow(unused_imports)]
use super::safestorage::*;
use super::write::write_secret_once;
use super::*;

/// `state.vscdb` 单次连接的 `busy_timeout`：与 `session::query::open_db` 的 5 秒同口径
/// （对照 Python `sqlite3.connect(timeout=5)`）。
///
/// **为什么显式写出来**：rusqlite 0.32 在 `sqlite3_open` 之后自己也会设 5000ms
/// （`inner_connection.rs`），所以今天的值恰好等价于库默认。仍然显式设定：
/// ① 依赖升级不该悄悄改掉这里的等待语义；② 这个数字要能被 grep 到、并有注释解释理由。
/// 真正延长容忍度的是 [`STATE_DB_WRITE_ATTEMPTS`]：一次 timeout 用尽后整轮重试。
pub(super) const STATE_DB_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 写库撞上 `SQLITE_BUSY` 时的总尝试次数（含首次）：3 次 × 5 秒 + 2 次退避 ≈ 16 秒上限。
///
/// **为什么单靠一次 timeout 不够**：VS Code 主进程退出后，渲染器 / 扩展宿主可能还在做
/// 最后一次状态落盘，锁能持多久不由我们决定。5 秒用尽就报「写入 state.vscdb 失败」，
/// 用户看到的是失败，而其实再等一两秒就能写进去。重试只在**锁冲突**时发生，
/// 有界（次数 × 单次 timeout），不会无限挂住调用方。
const STATE_DB_WRITE_ATTEMPTS: u32 = 3;
/// 两次尝试之间的退避：让出 CPU，同时给占用方一个明确的收尾窗口。
const STATE_DB_RETRY_BACKOFF: Duration = Duration::from_millis(300);

/// 写库重试策略（可注入：单测用很短的时长复现真实锁冲突，不必等 5 秒）。
#[derive(Clone, Copy, Debug)]
pub(super) struct BusyRetryPolicy {
    /// 单次连接等待写锁的上限（= 该连接的 `busy_timeout`）。
    pub(super) timeout: Duration,
    /// 总尝试次数（含首次）；小于 1 按 1 处理。
    pub(super) attempts: u32,
    /// 两次尝试之间的退避。
    pub(super) backoff: Duration,
}

/// 生产用策略。
pub(super) const DEFAULT_WRITE_POLICY: BusyRetryPolicy = BusyRetryPolicy {
    timeout: STATE_DB_BUSY_TIMEOUT,
    attempts: STATE_DB_WRITE_ATTEMPTS,
    backoff: STATE_DB_RETRY_BACKOFF,
};

// 编译期钉住「生产策略真的会重试」：`attempts` 退化成 1 时，锁只要持有超过一次
// timeout 就必然失败——那等于本次修复没做（rusqlite 本来就有 5 秒默认超时）。
const _: () = assert!(STATE_DB_WRITE_ATTEMPTS > 1);

/// 打开 `state.vscdb` 并设置 `busy_timeout`；本模块**所有**连接都必须走这里。
///
/// 集中成一个 helper（而不是每个调用点各写一次 `conn.busy_timeout(..)`），
/// 是为了让下一个新增的调用点不可能漏掉超时设置——这正是本次故障的形态。
///
/// `read_only` 对应 `SQLITE_OPEN_READ_ONLY`；只读探测（[`has_secret_row_for`]）
/// 依赖它避免在边缘情况下由 SQLite 兜底新建空库。
pub(super) fn open_state_db(path: &Path, read_only: bool) -> rusqlite::Result<Connection> {
    open_state_db_with_timeout(path, read_only, STATE_DB_BUSY_TIMEOUT)
}

/// [`open_state_db`] 的可注入超时版本（单测用短超时在毫秒级复现真实锁冲突）。
pub(super) fn open_state_db_with_timeout(
    path: &Path,
    read_only: bool,
    timeout: Duration,
) -> rusqlite::Result<Connection> {
    let conn = if read_only {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
    } else {
        Connection::open(path)?
    };
    // 设不上只是回到「没有超时」（原地立刻返回 SQLITE_BUSY）的旧行为，
    // 不该让调用方连打开都失败，故忽略错误。
    let _ = conn.busy_timeout(timeout);
    Ok(conn)
}

/// 一次写入尝试的失败：区分「库被占用（值得重试）」与「其它（重试无意义）」。
#[derive(Debug)]
pub(super) enum InjectFailure {
    /// `SQLITE_BUSY` / `SQLITE_LOCKED`：另一个连接（典型是编辑器的子进程）还持着写锁。
    Busy(rusqlite::Error),
    /// 其它失败（含加密 / 校验 / 表结构），直接报给用户。
    Fatal(String),
}

impl InjectFailure {
    /// 转成用户可读文案；`policy` 用于把「重试了几次、每次等多久」写进提示。
    pub(super) fn into_message(self, policy: BusyRetryPolicy) -> String {
        match self {
            Self::Fatal(message) => message,
            Self::Busy(err) => format!(
                "写入 state.vscdb 失败: {err}\n\n该数据库正被编辑器进程占用（已重试 {} 次、每次最多等待 {} 秒仍未释放写锁）。请确认编辑器已完全退出后重试。",
                policy.attempts.max(1),
                policy.timeout.as_secs_f32()
            ),
        }
    }
}

/// 把一次 rusqlite 失败归类：锁冲突可重试，其余按 `label` 拼成文案。
///
/// `label` 是**完整的**失败标签（如「写入 state.vscdb 失败」），文案最终是
/// `{label}: {err}`——与改造前逐字一致，避免为了复用 helper 而改动用户可见文案。
pub(super) fn classify_db_error(label: &str, err: rusqlite::Error) -> InjectFailure {
    if is_db_busy_error(&err) {
        InjectFailure::Busy(err)
    } else {
        InjectFailure::Fatal(format!("{label}: {err}"))
    }
}

/// 是否「库被别的连接锁住」（`SQLITE_BUSY` / `SQLITE_LOCKED`）。
///
/// 用结构化的 `ErrorCode` 判断，不匹配错误文案（文案随 SQLite 版本 / 语言变化）。
pub(super) fn is_db_busy_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::DatabaseBusy
                || inner.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// 跑一次「可能撞锁」的动作；**只在锁冲突时**重试，其它失败立刻返回。
///
/// 动作接收本次应使用的单次等待时长（= `policy.timeout`），由调用方交给
/// [`open_state_db_with_timeout`]；每次重试都是全新的连接，顺带重做一次 journal 恢复。
pub(super) fn retry_on_busy<F>(policy: BusyRetryPolicy, mut action: F) -> Result<(), InjectFailure>
where
    F: FnMut(Duration) -> Result<(), InjectFailure>,
{
    let attempts = policy.attempts.max(1);
    let mut attempt: u32 = 1;
    loop {
        match action(policy.timeout) {
            Ok(()) => return Ok(()),
            // 用尽前不再往上报：占用方可能下一秒就释放。
            Err(InjectFailure::Busy(_)) if attempt < attempts => {
                attempt += 1;
                std::thread::sleep(policy.backoff);
            }
            // 最后一次仍撞锁 → 把最后的错误原样交出去（`into_message` 负责文案）。
            Err(failure) => return Err(failure),
        }
    }
}

/// 读取并解密 CodeBuddy CN 当前登录 secret（明文 JSON 字符串）；保持既有行为。
pub fn read_codebuddy_cn_secret(user_data_dir: Option<&Path>) -> Result<Option<String>, String> {
    read_secret_for(&CODEBUDDY_CN_TARGET, user_data_dir)
}

/// 读取并解密指定桌面 IDE 档位的 secret（明文 JSON 字符串）。
pub fn read_codebuddy_ide_secret(
    flavor: CodeBuddyIdeFlavor,
    user_data_dir: Option<&Path>,
) -> Result<Option<String>, String> {
    read_secret_for(flavor.target(), user_data_dir)
}

/// 读取并解密指定目标的 secret（明文 JSON 字符串）。
///
/// 只读探测：即使 db 不存在也只返回 `Ok(None)`，绝不创建目录或文件（issue #91）。
pub fn read_secret_for(
    target: &VscodeSafeStorageTarget,
    user_data_dir: Option<&Path>,
) -> Result<Option<String>, String> {
    let root = match user_data_dir {
        Some(p) => p.to_path_buf(),
        None => (target.data_dir_resolver)()
            .ok_or_else(|| format!("无法定位 {} 数据目录", target.display_name))?,
    };
    let Some(db_path) = find_existing_state_db_path(&root) else {
        return Ok(None);
    };
    let data_root = data_root_from_db(&db_path)?.to_path_buf();
    let conn = open_state_db(&db_path, false).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
    let key = secret_storage_item_key_for(target);
    let raw_value: Option<String> = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [key.as_str()],
        |row| row.get(0),
    ) {
        Ok(value) => Some(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(err) => return Err(format!("查询 {} secret 失败: {err}", target.display_name)),
    };
    match raw_value {
        Some(value) => decode_secret_storage_value(&value, &data_root, target).map(Some),
        None => Ok(None),
    }
}

/// 目标 `state.vscdb` 中是否存在该 secret 行（只读查询、**不解密**）。
///
/// 供账号页轮询的状态接口使用：macOS 上解密会触发钥匙串授权弹窗，绝不能进轮询路径，
/// 因此这里只判断 key 是否存在（不读值、不解密）。
///
/// 与 [`resolve_state_db_path_for`] 的关键差异：**不创建任何目录或文件**——
/// 三个候选路径都不存在时直接返回 `Ok(false)`（`resolve_state_db_path_for` 会
/// `create_dir_all`，不能用于只读探测）。
pub fn has_secret_row_for(
    target: &VscodeSafeStorageTarget,
    user_data_dir: Option<&Path>,
) -> Result<bool, String> {
    let root = match user_data_dir {
        Some(path) => path.to_path_buf(),
        None => match (target.data_dir_resolver)() {
            Some(dir) => dir,
            None => return Ok(false),
        },
    };
    let Some(db_path) = find_existing_state_db_path(&root) else {
        return Ok(false);
    };
    // 只读打开：避免在「文件不存在」等边缘情况下由 SQLite 兜底新建空库。
    let conn = open_state_db(&db_path, true).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
    let key = secret_storage_item_key_for(target);
    match conn.query_row(
        "SELECT 1 FROM ItemTable WHERE key = ?1 LIMIT 1",
        [key.as_str()],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        // 空库 / 尚未初始化 ItemTable：按「无该行」处理，不是失败。
        Err(err) if is_missing_table_error(&err) => Ok(false),
        Err(err) => Err(format!("查询 {} secret 行失败: {err}", target.display_name)),
    }
}

/// SQLite「表不存在」（`no such table`）：空库或尚未初始化 ItemTable。
pub(super) fn is_missing_table_error(err: &rusqlite::Error) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("no such table")
}

/// 加密并写入 CodeBuddy CN secret；保持既有行为。
pub fn inject_codebuddy_cn_secret(
    plaintext: &str,
    user_data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    inject_secret_for(&CODEBUDDY_CN_TARGET, plaintext, user_data_dir)
}

/// 加密并写入指定桌面 IDE 档位的 secret。
pub fn inject_codebuddy_ide_secret(
    flavor: CodeBuddyIdeFlavor,
    plaintext: &str,
    user_data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    inject_secret_for(flavor.target(), plaintext, user_data_dir)
}

/// 加密并写入指定目标 secret。
///
/// 撞上 `SQLITE_BUSY`（编辑器进程还持着写锁）时会整轮重试，见 [`DEFAULT_WRITE_POLICY`]；
/// 路径解析与目录准备只做一次，不在重试里重复。
pub fn inject_secret_for(
    target: &VscodeSafeStorageTarget,
    plaintext: &str,
    user_data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    inject_secret_for_with_policy(target, plaintext, user_data_dir, DEFAULT_WRITE_POLICY)
}

/// [`inject_secret_for`] 的可注入策略版本（单测用很短的 timeout / 退避覆盖真实锁冲突）。
pub(super) fn inject_secret_for_with_policy(
    target: &VscodeSafeStorageTarget,
    plaintext: &str,
    user_data_dir: Option<&Path>,
    policy: BusyRetryPolicy,
) -> Result<PathBuf, String> {
    let db_path = resolve_state_db_path_for(target, user_data_dir)?;
    let data_root = data_root_from_db(&db_path)?.to_path_buf();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 state.vscdb 父目录失败: {e}"))?;
    }
    retry_on_busy(policy, |timeout| {
        write_secret_once(target, plaintext, &db_path, &data_root, timeout)
    })
    .map_err(|failure| failure.into_message(policy))?;
    Ok(db_path)
}
