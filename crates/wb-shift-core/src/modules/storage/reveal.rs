//! 在系统文件管理器中定位：Windows `explorer /select,`、macOS `open -R`、Linux `xdg-open`。
//!
//! 两种 scope（第三轮第 5 条方案 A）：
//!
//! - [`SCOPE_STORAGE`]（缺省）：只能在白名单根内定位，供类别行 / 回收站行使用。
//!   这是**破坏性操作**的边界，不能为了「打开项目目录」而放松。
//! - [`SCOPE_CWD`]：只读地打开任意**绝对存在的目录**（会话的「工作目录」按钮）。
//!   安全约束：拒绝相对路径、UNC（`\\server\share`）、设备路径（`\\.\`、`\\?\`）与
//!   `..` 穿越；目标不存在时回退到最近的已存在父目录；只交给文件管理器，**不执行**
//!   目标文件（解析阶段把文件归一成它所在的目录）。
//!
//! 校验与执行分开：[`resolve_target`] 是纯函数（单测不启动 explorer），[`reveal`]
//! 在它之上 `spawn`。

use super::roots::{ensure_no_symlink_components, ensure_within, StorageRoots};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// 缺省 scope：白名单内定位（类别行 / 回收站行）。
pub const SCOPE_STORAGE: &str = "storage";
/// 只读定位：任意绝对目录（会话「工作目录」）。
pub const SCOPE_CWD: &str = "cwd";

/// 校验后的定位目标。
#[derive(Clone, Debug)]
pub struct RevealTarget {
    /// 真正交给文件管理器的路径。
    pub path: PathBuf,
    /// 目标不存在时回退到了上层目录。
    pub fell_back: bool,
    /// 实际使用的模式（回退时强制 `open`）。
    pub mode: &'static str,
    /// 实际生效的 scope。
    pub scope: &'static str,
}

/// 解析 `mode`（`select` / `open`），非法值返回稳定错误文案。
fn parse_mode(mode: &str) -> Result<&'static str, String> {
    match mode {
        "select" => Ok("select"),
        "open" => Ok("open"),
        _ => Err("mode 只支持 select / open".to_string()),
    }
}

/// 解析 `scope`；缺省为 [`SCOPE_STORAGE`]（旧调用不传 scope 时行为完全不变）。
pub fn parse_scope(scope: Option<&str>) -> Result<&'static str, String> {
    match scope.map(str::trim).unwrap_or_default() {
        "" | SCOPE_STORAGE => Ok(SCOPE_STORAGE),
        SCOPE_CWD => Ok(SCOPE_CWD),
        other => Err(format!("scope 只支持 storage / cwd：{other}")),
    }
}

/// `scope="cwd"` 的路径约束：绝对路径、非 UNC / 设备 / 长路径前缀、无 `..` 穿越。
fn validate_cwd_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("scope=cwd 只接受绝对路径".to_string());
    }
    let text = path.to_string_lossy();
    // UNC（`\\server\share`）会带上 Windows 凭据；设备与长路径前缀（`\\.\`、`\\?\`）
    // 可以绕过常规路径语义，两者都拒绝。
    if text.starts_with(r"\\") || text.starts_with("//") {
        return Err("scope=cwd 不支持 UNC 或设备路径".to_string());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::RootDir => {}
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                if !matches!(prefix.kind(), std::path::Prefix::Disk(_)) {
                    return Err("scope=cwd 不支持网络 / 设备路径".to_string());
                }
            }
            _ => {
                return Err(format!("scope=cwd 拒绝包含 `..` 或空前缀的路径：{text}"));
            }
        }
    }
    Ok(())
}

/// 解析定位目标（纯函数，不启动任何进程）。
///
/// - `storage` scope：沿用白名单 + canonicalize 复核 + 逐级链接检查；
/// - `cwd` scope：见模块文档的安全约束，文件归一成所在目录（绝不执行目标文件）。
pub fn resolve_target(
    roots: &StorageRoots,
    path: &str,
    mode: &str,
    scope: Option<&str>,
) -> Result<RevealTarget, String> {
    if path.trim().is_empty() {
        return Err("缺少路径".to_string());
    }
    let mode = parse_mode(mode)?;
    let scope = parse_scope(scope)?;
    let requested = PathBuf::from(path);

    if scope == SCOPE_STORAGE {
        let root = roots
            .root_containing(&requested)
            .ok_or_else(|| "路径不在白名单根内".to_string())?;
        ensure_within(&root, &requested)?;
        ensure_no_symlink_components(&root, &requested)?;
        let (target, fell_back) = existing_ancestor(&requested);
        return Ok(RevealTarget {
            path: target,
            fell_back,
            mode: if fell_back { "open" } else { mode },
            scope,
        });
    }

    validate_cwd_path(&requested)?;
    let (mut target, fell_back) = existing_ancestor(&requested);
    // 只读定位：目标是文件时打开它所在的目录，绝不让 explorer 用默认程序打开它。
    if target.is_file() {
        if let Some(parent) = target.parent() {
            target = parent.to_path_buf();
        }
    }
    Ok(RevealTarget {
        path: target,
        fell_back,
        mode: "open",
        scope,
    })
}

/// `POST /api/storage/reveal`：解析目标并在系统文件管理器中打开。
pub fn reveal(
    roots: &StorageRoots,
    path: &str,
    mode: &str,
    scope: Option<&str>,
) -> Result<Value, String> {
    let target = resolve_target(roots, path, mode, scope)?;
    spawn_reveal(&target.path, target.mode)?;
    Ok(json!({
        "ok": true,
        "path": target.path.to_string_lossy(),
        "fellBack": target.fell_back,
        "scope": target.scope,
    }))
}

/// 目标不存在时向上找最近的存在路径（用于删完文件后定位父目录）。
fn existing_ancestor(path: &Path) -> (PathBuf, bool) {
    if path.exists() {
        return (path.to_path_buf(), false);
    }
    let mut current = path.to_path_buf();
    while let Some(parent) = current.parent() {
        if parent.exists() {
            return (parent.to_path_buf(), true);
        }
        current = parent.to_path_buf();
    }
    (path.to_path_buf(), true)
}

#[cfg(target_os = "windows")]
fn spawn_reveal(path: &Path, mode: &str) -> Result<(), String> {
    let mut command = std::process::Command::new("explorer");
    if mode == "select" {
        command.arg(format!("/select,{}", path.to_string_lossy()));
    } else {
        command.arg(path);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打开资源管理器失败：{error}"))
}

#[cfg(target_os = "macos")]
fn spawn_reveal(path: &Path, mode: &str) -> Result<(), String> {
    let mut command = std::process::Command::new("open");
    if mode == "select" {
        command.arg("-R");
    }
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打开访达失败：{error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_reveal(path: &Path, _mode: &str) -> Result<(), String> {
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打开文件管理器失败：{error}"))
}
