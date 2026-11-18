//! 存储根与路径工具：数据根候选/探测、uid 数据目录、会话历史根与文件辅助。

#[allow(unused_imports)]
use super::copy::*;
#[allow(unused_imports)]
use super::index_merge::*;
#[allow(unused_imports)]
use super::list::*;
#[allow(unused_imports)]
use super::remap::*;
use super::*;

// ---------------------------------------------------------------------------
// 路径解析
// ---------------------------------------------------------------------------

/// 平台候选的扩展数据根目录列表（按优先级，已去重）。
///
/// - Windows：`%LOCALAPPDATA%\<app_dir>\Data`（插件侧实测）。
/// - macOS / Linux：按同一相对布局推导（**未实测**），仅作 best-effort 兜底。
pub fn store_data_root_candidates(spec: SessionStoreSpec) -> Vec<PathBuf> {
    let bases: Vec<PathBuf> = {
        #[cfg(target_os = "windows")]
        {
            [dirs::data_local_dir(), dirs::cache_dir()]
                .into_iter()
                .flatten()
                .collect()
        }
        #[cfg(target_os = "macos")]
        {
            // ~/Library/Application Support 与 ~/Library/Caches（未实测）。
            [dirs::data_dir(), dirs::cache_dir()]
                .into_iter()
                .flatten()
                .collect()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            // $XDG_DATA_HOME（~/.local/share）与 $XDG_CACHE_HOME（~/.cache）（未实测）。
            [dirs::data_dir(), dirs::cache_dir()]
                .into_iter()
                .flatten()
                .collect()
        }
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for base in bases {
        let candidate = base.join(spec.app_dir).join("Data");
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// 解析扩展数据根目录：返回第一个真实存在的候选目录；都不存在时返回 `None`。
pub fn store_data_root(spec: SessionStoreSpec) -> Option<PathBuf> {
    store_data_root_candidates(spec)
        .into_iter()
        .find(|path| path.is_dir())
}

/// VS Code 插件的数据根候选（历史入口，等价于 [`store_data_root_candidates`] 传入 [`VSCODE_STORE`]）。
pub fn ext_data_root_candidates() -> Vec<PathBuf> {
    store_data_root_candidates(VSCODE_STORE)
}

/// VS Code 插件的数据根（历史入口）。
pub fn ext_data_root() -> Option<PathBuf> {
    store_data_root(VSCODE_STORE)
}

/// 账号 uid 的数据目录：`<root>/<uid>/<client_dir>/<uid>`。
pub fn uid_data_dir_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> PathBuf {
    root.join(uid).join(spec.client_dir).join(uid)
}

/// 账号 uid 的会话历史根：`<root>/<uid>/<client_dir>/<uid>/history`。
pub fn history_root_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> PathBuf {
    uid_data_dir_in(spec, root, uid).join(HISTORY_DIR)
}

/// VS Code 插件侧的历史根（历史入口）。
pub fn history_root(root: &Path, uid: &str) -> PathBuf {
    history_root_in(VSCODE_STORE, root, uid)
}

/// uid 白名单校验：非空、不含路径分隔符 / `..`、且不是 `default` / `Public`。
///
/// 用于杜绝路径穿越与误碰结构不同的兜底目录（`default\` / `Public\`）。
pub(crate) fn is_safe_uid(uid: &str) -> bool {
    let uid = uid.trim();
    !uid.is_empty()
        && uid != "."
        && uid != ".."
        && !uid.contains(['/', '\\'])
        && !uid.eq_ignore_ascii_case("default")
        && !uid.eq_ignore_ascii_case("public")
}

/// 是否为 32 位小写 hex id。
pub(crate) fn is_hex32(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 生成 32 位小写 hex id（对齐扩展 `generateId()` 产物，非带连字符 UUID）。
pub(super) fn gen_hex32() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut out = String::with_capacity(32);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// 生成一个此前未出现的 32 位小写 hex id，并登记进 `used`。
pub(super) fn unique_hex32(used: &mut BTreeSet<String>) -> String {
    loop {
        let candidate = gen_hex32();
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
}

// ---------------------------------------------------------------------------
// 通用小工具
// ---------------------------------------------------------------------------

/// 读取并解析 JSON 文件；文件缺失或内容损坏时返回 `None`（不 panic）。
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 把索引里的时间字段换算成 epoch 毫秒；无法识别时返回 0。
pub(super) fn time_to_ms(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::String(text)) => chrono::DateTime::parse_from_rfc3339(text.trim())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(|raw| {
                if raw > 1_000_000_000_000 {
                    raw
                } else {
                    raw.saturating_mul(1000)
                }
            })
            .unwrap_or(0),
        _ => 0,
    }
}

/// 递归复制目录（保持相对结构与文件名，含附件中文名）。
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// 尽力删除目录树（忽略不存在 / 失败）。
pub(crate) fn remove_dir_all_if_exists(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}
