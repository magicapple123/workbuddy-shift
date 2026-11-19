//! 存储根、白名单与用量统计。
//!
//! 所有破坏性路径都必须经过本模块的校验：先按**相对路径**拼接（拒绝绝对路径与 `..`），
//! 再逐级检查符号链接 / junction，最后按 canonicalize 结果复核仍在白名单根内。
//! 只读统计跳过链接，不跟随，也不读写文件内容。

use crate::modules::config::home_dir;
use std::path::{Component, Path, PathBuf};

mod index;

pub use index::{usage_index, usage_index_deferred};

/// 三个受管根：工具自身存储、国内版客户端、国际版客户端。
///
/// 白名单之外一律拒绝；`~/.workbuddy` 只允许其子目录，`binaries` / `plugins` / `app` /
/// `storage` / `blobs` 等由上层分类标成不可删（本模块不负责业务分类）。
#[derive(Clone, Debug)]
pub struct StorageRoots {
    pub store: PathBuf,
    pub client_cn: PathBuf,
    pub client_ai: PathBuf,
}

impl StorageRoots {
    /// 生产入口：`~/.workbuddy-shift`、`~/.workbuddy`、`~/.workbuddy-ai`。
    pub fn resolve() -> Self {
        let home = home_dir();
        Self {
            store: home.join(".workbuddy-shift"),
            client_cn: home.join(".workbuddy"),
            client_ai: home.join(".workbuddy-ai"),
        }
    }

    /// 单测 / 沙箱入口：显式给出三个根。
    pub fn new(store: PathBuf, client_cn: PathBuf, client_ai: PathBuf) -> Self {
        Self {
            store,
            client_cn,
            client_ai,
        }
    }

    /// 路径落在哪个白名单根内（`store` / `cn` / `ai`）；不在任何根内返回 `None`。
    pub fn root_containing(&self, path: &Path) -> Option<PathBuf> {
        [
            self.store.clone(),
            self.client_cn.clone(),
            self.client_ai.clone(),
        ]
        .into_iter()
        .find(|root| path.starts_with(root))
    }

    /// 按白名单 id 取根：`store` / `cn` / `ai`。
    pub fn by_id(&self, id: &str) -> Option<PathBuf> {
        match id {
            "store" => Some(self.store.clone()),
            "cn" => Some(self.client_cn.clone()),
            "ai" => Some(self.client_ai.clone()),
            _ => None,
        }
    }

    /// 回收站根：`~/.workbuddy-shift/trash`。
    pub fn trash_root(&self) -> PathBuf {
        self.store.join("trash")
    }

    /// 分类清理前的整库 / 配置备份根：`~/.workbuddy-shift/backups/storage`。
    pub fn storage_backup_root(&self) -> PathBuf {
        self.store.join("backups").join("storage")
    }

    /// 客户端根列表（只返回存在的，避免 UI 展示空目录）。
    pub fn client_roots(&self) -> Vec<(&'static str, PathBuf)> {
        [("cn", &self.client_cn), ("ai", &self.client_ai)]
            .into_iter()
            .filter(|(_, path)| path.is_dir())
            .map(|(id, path)| (id, path.clone()))
            .collect()
    }
}

/// 是否链接（符号链接 / Windows junction / 其它 reparse point）。
///
/// Windows 上 junction 是最常见的越界手法，`FileType::is_symlink()` 对部分 reparse
/// point 不返回 true，所以按 `FILE_ATTRIBUTE_REPARSE_POINT` 判定；其它平台用 `is_symlink`。
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// 用量：字节数与文件数（目录不计入文件数）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.files = self.files.saturating_add(other.files);
    }

    /// 补集用：`self - other`，减到 0 为止（只用于「其它 / 未分类」展示，不能出现负数）。
    pub fn saturating_sub(&self, other: Usage) -> Usage {
        Usage {
            bytes: self.bytes.saturating_sub(other.bytes),
            files: self.files.saturating_sub(other.files),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.bytes == 0 && self.files == 0
    }
}

/// 统计一个路径（文件或目录）的用量；不跟随符号链接，读不到的子项按 0 计。
pub fn usage_of(path: &Path) -> Usage {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Usage::default();
    };
    if is_link(&meta) {
        return Usage::default();
    }
    if meta.is_file() {
        return Usage {
            bytes: meta.len(),
            files: 1,
        };
    }
    let mut usage = Usage::default();
    let Ok(entries) = std::fs::read_dir(path) else {
        return usage;
    };
    for entry in entries.flatten() {
        usage.add(usage_of(&entry.path()));
    }
    usage
}

/// 把相对路径拼到白名单根下，并拒绝任何越界写法。
///
/// 拒绝：绝对路径、`..`、盘符前缀、空串；`rel` 里的分隔符统一按平台处理（`/` 与 `\` 都可）。
/// 生产路径用 [`ensure_within`]（canonicalize 复核），本函数只服务单测与后续「相对路径删除」入口。
#[cfg(test)]
pub fn resolve_inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        return Err("路径为空".to_string());
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err("不允许绝对路径".to_string());
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => return Err(format!("路径包含不允许的片段：{trimmed}")),
        }
    }
    let joined = root.join(candidate);
    ensure_within(root, &joined)?;
    Ok(joined)
}

/// 逐级检查 `path` 相对 `root` 的每一段都不是符号链接 / junction。
///
/// 与 `session_backup/cleanup.rs` 同一思路：可信根之上的部分不检查（用户家目录本身可能是
/// 链接），根之下的每一级（含目标自身）都必须是非链接目录或普通文件。
pub fn ensure_no_symlink_components(root: &Path, path: &Path) -> Result<(), String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| "路径不在白名单根内".to_string())?;
    let mut current = root.to_path_buf();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            return Err("路径包含不允许的片段".to_string());
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if is_link(&meta) => {
                return Err(format!("路径包含符号链接：{}", current.display()));
            }
            Ok(_) => {}
            // 目标不存在是允许的（state 由调用方判定存在性）；后续级也必然不存在。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("路径状态无法确认：{error}")),
        }
    }
    Ok(())
}

/// canonicalize 后复核 `path` 仍在 `root` 内（防 8.3 短名 / 大小写 / 链接绕过）。
///
/// 目标不存在时用**最近的存在祖先**比较：`root` 本身可能尚未创建（回收站条目目录），
/// 此时两侧会解析到同一个祖先，判定仍然正确；反过来，路径若经链接逃到根外，解析结果
/// 不会以根开头，会被拒绝。
pub fn ensure_within(root: &Path, path: &Path) -> Result<(), String> {
    if !path.starts_with(root) {
        return Err(format!("路径不在白名单根内：{}", path.display()));
    }
    let root_key = existing_ancestor_key(root);
    let path_key = existing_ancestor_key(path);
    if path_key == root_key || path_key.starts_with(&format!("{}{}", root_key, sep())) {
        Ok(())
    } else {
        Err(format!("路径不在白名单根内：{}", path.display()))
    }
}

/// 最近存在祖先的 canonical 键；整条链都不存在时退回规范化后的原路径。
fn existing_ancestor_key(path: &Path) -> String {
    let mut probe = path.to_path_buf();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(&probe) {
            return normalize_key(&resolved);
        }
        match probe.parent() {
            Some(parent) if parent != probe => probe = parent.to_path_buf(),
            _ => return normalize_key(path),
        }
    }
}

fn sep() -> char {
    if cfg!(windows) {
        '\\'
    } else {
        '/'
    }
}

fn normalize_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', &sep().to_string());
    let trimmed = text.trim_end_matches(sep()).to_string();
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wb-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建夹具目录");
        dir
    }

    #[test]
    fn resolve_inside_rejects_escape_attempts() {
        let root = temp_dir("roots-escape");
        for rel in ["../etc", "a/../../b", "/abs", ""] {
            assert!(resolve_inside(&root, rel).is_err(), "必须拒绝 {rel}");
        }
        // 盘符路径只在 Windows 上有「绝对路径」语义；在 Unix 上它只是普通文件名。
        #[cfg(windows)]
        assert!(
            resolve_inside(&root, r"C:\Windows").is_err(),
            "必须拒绝 C:\\Windows"
        );
        let ok = resolve_inside(&root, "logs/app.log").expect("正常相对路径");
        assert!(ok.starts_with(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_counts_files_and_skips_symlinks() {
        let root = temp_dir("roots-usage");
        std::fs::create_dir_all(root.join("a/b")).expect("建目录");
        std::fs::write(root.join("a/one.txt"), b"12345").expect("写文件");
        std::fs::write(root.join("a/b/two.txt"), b"123").expect("写文件");

        let usage = usage_of(&root);
        assert_eq!(usage.files, 2);
        assert_eq!(usage.bytes, 8);

        let index = usage_index(&root, 2);
        // 文件计入父目录并逐级上卷：`a/one.txt` → "a"，`a/b/two.txt` → "a/b" 与 "a"。
        assert_eq!(index.get("a").map(|u| u.files), Some(2));
        assert_eq!(index.get("a").map(|u| u.bytes), Some(8));
        assert_eq!(index.get("a/b").map(|u| u.files), Some(1));
        assert_eq!(index.get("").map(|u| u.files), Some(2));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn junction_component_is_rejected() {
        let root = temp_dir("roots-junction");
        let outside = temp_dir("roots-junction-outside");
        std::fs::write(outside.join("secret.txt"), b"x").expect("写文件");
        let link = root.join("junction");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .status();
        let status = status.expect("mklink /J 必须可执行（junction 不需要管理员权限）");
        assert!(status.success(), "创建 junction 失败：{status}");
        let target = root.join("junction/secret.txt");
        let error = ensure_no_symlink_components(&root, &target).expect_err("必须拒绝 junction");
        assert!(error.contains("符号链接"), "{error}");
        // 用量统计同样不得跟随 junction。
        assert_eq!(usage_of(&root).files, 0, "junction 不得计入用量");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(windows)]
    #[test]
    fn symlink_component_is_rejected() {
        let root = temp_dir("roots-link");
        let outside = temp_dir("roots-link-outside");
        std::fs::write(outside.join("secret.txt"), b"x").expect("写文件");
        let link = root.join("link");
        // 无权限创建链接时跳过（CI 普通用户可能没有 SeCreateSymbolicLinkPrivilege）。
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        let target = root.join("link/secret.txt");
        let error = ensure_no_symlink_components(&root, &target).expect_err("必须拒绝链接");
        assert!(error.contains("符号链接"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
