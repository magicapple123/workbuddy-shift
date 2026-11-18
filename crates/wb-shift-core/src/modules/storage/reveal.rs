//! 在系统文件管理器中定位：Windows `explorer /select,`、macOS `open -R`、Linux `xdg-open`。
//!
//! 路径必须 canonicalize 后仍在白名单根内；目标不存在时回退到最近的已存在父目录
//! （删完文件后按钮仍可用）。不等待子进程退出——explorer 自身常驻。

use super::roots::{ensure_no_symlink_components, ensure_within, StorageRoots};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn reveal(roots: &StorageRoots, path: &str, mode: &str) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("缺少路径".to_string());
    }
    if mode != "select" && mode != "open" {
        return Err("mode 只支持 select / open".to_string());
    }
    let requested = PathBuf::from(path);
    let root = roots
        .root_containing(&requested)
        .ok_or_else(|| "路径不在白名单根内".to_string())?;
    ensure_within(&root, &requested)?;
    ensure_no_symlink_components(&root, &requested)?;

    let (target, fell_back) = existing_ancestor(&requested);
    spawn_reveal(&target, if fell_back { "open" } else { mode })?;
    Ok(json!({
        "ok": true,
        "path": target.to_string_lossy(),
        "fellBack": fell_back,
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
