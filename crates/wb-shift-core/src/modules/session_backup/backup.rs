//! 持久化原语与事务目录：fsync/durable write、事务与生命周期路径、operation id 校验。

#[allow(unused_imports)]
use super::cleanup::*;
#[allow(unused_imports)]
use super::lifecycle::*;
use super::*;

// ---------------------------------------------------------------------------
// 会话专用可靠落盘（design §4）
// ---------------------------------------------------------------------------

/// 持久化目录项更新（父目录 fsync）。
///
/// Unix 上对目录 `fsync` 才能保证「新建/改名/删除的目录项」在断电后仍可见；
/// Windows 无法打开目录句柄，属平台限制（[`DIRECTORY_SYNC_SUPPORTED`]），
/// 不静默假装成功——限制由报告与 spec 披露。
#[cfg(unix)]
pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 持久化单个文件的已写内容。
///
/// Unix 上只读句柄即可 `fsync`；Windows 的 `FlushFileBuffers` 要求句柄具备写权限，
/// 而 `File::open` 只申请 `GENERIC_READ`，对只读句柄 `sync_all` 必定返回
/// `ERROR_ACCESS_DENIED(5)`。备份目录里的文件都是本进程刚写出的副本，故用可写句柄打开。
#[cfg(unix)]
pub fn sync_file(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

/// [`sync_file`] 的 Windows 实现：句柄必须可写（见上方说明）。
#[cfg(not(unix))]
pub fn sync_file(path: &Path) -> std::io::Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

/// 目标文件所在目录的持久化（不存在时视为无需处理）。
pub(super) fn sync_parent_of(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => sync_dir(parent),
        _ => Ok(()),
    }
}

/// 会话专用原子写：写完临时文件先 `sync_all`，原子替换后持久化父目录。
///
/// 普通 `flush`、回读摘要或 rename 成功都不能替代持久化证明；这里只用于会话
/// 业务与生命周期文件（认证/设置沿用 `config::atomic_write` 的既有行为）。
pub fn durable_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{file_name}.tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        sync_parent_of(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// [`durable_write`] 的字符串版本。
pub fn durable_write_str(path: &Path, content: &str) -> std::io::Result<()> {
    durable_write(path, content.as_bytes())
}

/// 递归持久化一个目录内的文件与子目录（备份目录在转 protected 之前调用）。
pub fn sync_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            sync_tree(&path)?;
        } else {
            sync_file(&path)?;
        }
    }
    sync_dir(dir)
}

/// 在写连接上确认 `synchronous` 至少为 FULL（2）；不足则显式设置并核验。
///
/// 只影响当前连接，不改 WorkBuddy 的 `journal_mode`；WAL 模式下的提交持久性
/// 由 SQLite 自己按 `synchronous` 保证，不能只 fsync 主库。
pub fn ensure_full_synchronous(conn: &rusqlite::Connection) -> Result<(), String> {
    let level: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|error| format!("数据库同步级别读取失败：{error}"))?;
    if level < 2 {
        conn.execute_batch("PRAGMA synchronous=FULL")
            .map_err(|error| format!("数据库同步级别设置失败：{error}"))?;
        let after: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(|error| format!("数据库同步级别复核失败：{error}"))?;
        if after < 2 {
            return Err(format!(
                "数据库无法建立持久化保证（synchronous={after}，需要 FULL），已停止写入"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 路径（可信根 + 校验后的 UUID 推导）
// ---------------------------------------------------------------------------

/// 操作专属临时目录根：`backups/session-transactions/{variant}`。
pub fn transactions_root(paths: &SessionPaths, variant: WbVariant) -> PathBuf {
    paths
        .backup_root()
        .join(TRANSACTIONS_DIR_NAME)
        .join(variant.as_str())
}

/// 一次操作的专属临时目录；`operation_id` 必须是合法 UUID。
pub fn transaction_dir(
    paths: &SessionPaths,
    variant: WbVariant,
    operation_id: &str,
) -> Result<PathBuf, String> {
    let id = validated_operation_id(operation_id)?;
    Ok(transactions_root(paths, variant).join(id))
}

/// 维护记录目录：`session-links/backup-lifecycle/{variant}`。
pub fn lifecycle_root(paths: &SessionPaths, variant: WbVariant) -> PathBuf {
    paths
        .session_links_dir()
        .join(LIFECYCLE_DIR_NAME)
        .join(variant.as_str())
}

pub(super) fn lifecycle_file(
    paths: &SessionPaths,
    variant: WbVariant,
    operation_id: &str,
) -> Result<PathBuf, String> {
    let id = validated_operation_id(operation_id)?;
    Ok(lifecycle_root(paths, variant).join(format!("{id}.json")))
}

/// operationId 必须是规范 UUID：路径由它推导，不能接受任意字符串（路径穿越）。
pub(super) fn validated_operation_id(operation_id: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(operation_id)
        .map(|id| id.to_string())
        .map_err(|_| format!("操作标识不是合法 UUID（{operation_id}），已停止删除"))
}
