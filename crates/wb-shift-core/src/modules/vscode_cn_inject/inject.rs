//! secret 读写与注入：safeStorage 行读取、检测与写入 secret。

#[allow(unused_imports)]
use super::safestorage::*;
use super::*;

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
    let conn = Connection::open(&db_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
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
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
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
pub fn inject_secret_for(
    target: &VscodeSafeStorageTarget,
    plaintext: &str,
    user_data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let db_path = resolve_state_db_path_for(target, user_data_dir)?;
    let data_root = data_root_from_db(&db_path)?.to_path_buf();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 state.vscdb 父目录失败: {e}"))?;
    }
    let conn = Connection::open(&db_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .map_err(|e| format!("初始化 ItemTable 失败: {e}"))?;

    let db_key = secret_storage_item_key_for(target);
    let existing_prefix: Option<String> = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?",
        [db_key.as_str()],
        |row| row.get::<_, String>(0),
    ) {
        Ok(val) => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&val) {
                if let Ok(bytes) = decode_buffer_data(&parsed) {
                    detect_prefix(&bytes).map(|s| s.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        }
        Err(_) => None,
    };

    let encrypted = encrypt_secret_payload(
        plaintext.as_bytes(),
        existing_prefix.as_deref(),
        &data_root,
        target,
    )?;
    let buffer_str = encode_secret_buffer(encrypted)?;
    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
        rusqlite::params![db_key, buffer_str],
    )
    .map_err(|e| format!("写入 state.vscdb 失败: {e}"))?;

    // 写后校验：行存在且为 Buffer JSON
    let written: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            [db_key.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| format!("写后校验失败: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&written).map_err(|e| format!("写后校验 JSON 失败: {e}"))?;
    if parsed.get("type").and_then(|v| v.as_str()) != Some("Buffer") {
        return Err("写后校验失败：value 不是 Buffer".to_string());
    }
    Ok(db_path)
}
