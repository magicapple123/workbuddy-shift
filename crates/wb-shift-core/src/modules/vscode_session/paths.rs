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
///
/// 只在 [`ScopeSegment::Uid`] 的数据仓里等于「客户端段之后那一级」；JetBrains 那里是
/// **项目 key**，因此该函数的唯一用途是「无项目桶」兜底路径，不要拿它拼真实路径。
pub fn uid_data_dir_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> PathBuf {
    root.join(uid).join(spec.client_dir).join(uid)
}

/// 账号在数据仓下的客户端段：`<root>/<uid>/<client_dir>`（其下一级是 uid 或 projectKey）。
pub fn account_client_dir_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> PathBuf {
    root.join(uid).join(spec.client_dir)
}

/// 某个「项目段」的会话历史根：`<root>/<uid>/<client_dir>/<project_key>/history`。
///
/// `project_key` 的语义由 [`SessionStoreSpec::scope`] 决定：`Uid` 语义下必须是账号 uid
/// （否则会拼出一个客户端永远不会读的目录）；`ProjectKey` 语义下是项目 key。
pub fn scope_history_root_in(
    spec: SessionStoreSpec,
    root: &Path,
    uid: &str,
    project_key: &str,
) -> PathBuf {
    account_client_dir_in(spec, root, uid)
        .join(project_key)
        .join(HISTORY_DIR)
}

/// 账号 uid 的会话历史根：`<root>/<uid>/<client_dir>/<uid>/history`。
///
/// 等价于 [`scope_history_root_in`] 传入 `project_key = uid`，即两种语义下都成立：
/// VS Code 系这是唯一的历史根；JetBrains 这是「没有打开项目」时的退化项目段。
pub fn history_root_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> PathBuf {
    scope_history_root_in(spec, root, uid, uid)
}

/// VS Code 插件侧的历史根（历史入口）。
pub fn history_root(root: &Path, uid: &str) -> PathBuf {
    history_root_in(VSCODE_STORE, root, uid)
}

/// 一个「账号 × 项目」的历史根。
///
/// 读路径据 [`Self::history_root`] 枚举会话桶；写路径据 [`Self::project_key`] 决定目标
/// 目录（项目 key 由项目路径决定，**跨账号同值**，因此可以沿用源侧的 key）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryScope {
    /// 客户端段之后那一级的目录名（`Uid` 语义 = 账号 uid；`ProjectKey` 语义 = 项目 key）。
    pub project_key: String,
    /// `<root>/<uid>/<client_dir>/<project_key>/history`。
    pub history_root: PathBuf,
}

/// 账号在数据仓下的全部历史根。
///
/// - [`ScopeSegment::Uid`]：恒定返回 1 个（`project_key = uid`），目录是否存在由调用方判；
/// - [`ScopeSegment::ProjectKey`]：枚举 `<root>/<uid>/<client_dir>/*` 中**确实带 `history/`**
///   子目录的那些（多项目并列；「未打开项目」时项目段退化成 uid，同样被枚举到）。
///
/// 枚举失败（权限 / 并发删除）返回空表：读路径据此判「没有判定依据」，不会把读不到
/// 当成「没有会话」而误删关联组。
pub fn history_scopes_in(spec: SessionStoreSpec, root: &Path, uid: &str) -> Vec<HistoryScope> {
    match spec.scope {
        ScopeSegment::Uid => vec![HistoryScope {
            project_key: uid.to_string(),
            history_root: history_root_in(spec, root, uid),
        }],
        ScopeSegment::ProjectKey => {
            let client_dir = account_client_dir_in(spec, root, uid);
            let Ok(entries) = std::fs::read_dir(&client_dir) else {
                return Vec::new();
            };
            let mut scopes: Vec<HistoryScope> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| {
                    let project_key = entry.file_name().to_str()?.to_string();
                    if !is_safe_segment(&project_key) {
                        return None;
                    }
                    let history_root = entry.path().join(HISTORY_DIR);
                    history_root.is_dir().then_some(HistoryScope {
                        project_key,
                        history_root,
                    })
                })
                .collect();
            scopes.sort_by(|left, right| left.project_key.cmp(&right.project_key));
            scopes
        }
    }
}

/// 反解 JetBrains 项目 key 为项目路径（仅供展示；解码不出来返回空串）。
///
/// 插件写盘用的是项目路径的 base64url（无 padding）；`recentProjects.xml` 里是同一路径的
/// 原生形态（Windows 盘符大写），因此展示时把盘符统一成大写，与用户在 IDE 里看到的一致。
/// 解码结果必须**看起来像路径**（含分隔符、无控制字符）才采信：账号 uid 这类 32 位 hex
/// 偶尔也能被 base64 解成字节，但它不是项目。
pub fn decode_project_key(project_key: &str) -> String {
    use base64::Engine as _;
    let trimmed = project_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    for decoded in [
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed),
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed),
        base64::engine::general_purpose::STANDARD.decode(trimmed),
    ] {
        let Ok(bytes) = decoded else { continue };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let path = normalize_project_path(&text);
        if !path.is_empty() {
            return path;
        }
    }
    String::new()
}

/// 把解码出的原文规整成可展示的项目路径：拒绝控制字符与「不像路径」的文本。
fn normalize_project_path(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) || !trimmed.contains(['/', '\\'])
    {
        return String::new();
    }
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next(), chars.next()) {
        // Windows 盘符大小写无关，统一按大写展示（与 recentProjects.xml 一致）。
        (Some(drive), Some(':'), Some(sep))
            if drive.is_ascii_alphabetic() && (sep == '/' || sep == '\\') =>
        {
            format!("{}:{}{}", drive.to_ascii_uppercase(), sep, chars.as_str())
        }
        _ => trimmed.to_string(),
    }
}

/// 目录名白名单校验（项目 key / 项目段）：非空、不含路径分隔符、不是 `.` / `..`。
pub(crate) fn is_safe_segment(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
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
