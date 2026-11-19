//! 一个根下的用量归集：一次遍历同时给出总量与各分类的分子。
//!
//! 从 `roots.rs` 拆出（单文件规模控制，职责边界是「只读统计」）：
//! 白名单校验与破坏性路径仍在 `roots.rs`。

use super::{is_link, Usage};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// 按「相对路径前 N 级」归集一个根下的用量。
///
/// 一次遍历同时给出总量与各分类的分子，分类之和恒等于总量（不会出现两个数字对不上）。
/// 键是 `/` 分隔的相对目录路径（深度 1 与 2 都记），例如 `session-links` 与
/// `session-links/baselines`；根自身是空串键。文件计入**父目录**的键。
/// 链接与非普通文件（管道 / 设备）跳过。
pub fn usage_index(root: &Path, depth: usize) -> HashMap<String, Usage> {
    index_root(root, depth, &|_: &str| true).0
}

/// 快扫版：`keep(name)` 返回 false 的**顶层目录**整棵跳过，名字按字典序返回给调用方
/// 延迟计量（受保护项 / 账号目录等大目录）。
///
/// 为什么要单独一个入口：客户端根下的 `app` / `binaries` / 几十个账号 UUID 目录占绝大
/// 部分体积，但它们不可清理；首屏只需要「可清理 + 会话资产」的精确数字（第三轮第 6 条
/// 的 6.6 决策）。顶层文件始终计入总量（单次 stat，代价可忽略）。
pub fn usage_index_deferred<F>(
    root: &Path,
    depth: usize,
    keep: F,
) -> (HashMap<String, Usage>, Vec<String>)
where
    F: Fn(&str) -> bool + Sync,
{
    index_root(root, depth, &keep)
}

fn index_root<F>(root: &Path, depth: usize, keep: &F) -> (HashMap<String, Usage>, Vec<String>)
where
    F: Fn(&str) -> bool + Sync,
{
    let mut index: HashMap<String, Usage> = HashMap::new();
    let mut total = Usage::default();
    let mut deferred: Vec<String> = Vec::new();

    let Ok(entries) = std::fs::read_dir(root) else {
        index.insert(String::new(), total);
        return (index, deferred);
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_link(&meta) {
            continue;
        }
        if meta.is_file() {
            // 顶层文件没有分类键，只计入总量（与改造前口径一致）。
            total.add(Usage {
                bytes: meta.len(),
                files: 1,
            });
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if keep(&name) {
            children.push(path);
        } else {
            deferred.push(name);
        }
    }
    deferred.sort();

    // 顶层并行：10 万级小文件在 Windows 上主要是元数据等待，多线程能把首次扫描从
    // 二十多秒降到几秒；分块避免目录项很多时线程数失控。
    let threads = children.len().clamp(1, 12);
    let chunk = children.len().div_ceil(threads.max(1));
    let mut partials: Vec<(HashMap<String, Usage>, Usage)> = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for slice in children.chunks(chunk.max(1)) {
            handles.push(scope.spawn(move || {
                let mut local: HashMap<String, Usage> = HashMap::new();
                let mut local_total = Usage::default();
                for child in slice {
                    let Ok(meta) = std::fs::symlink_metadata(child) else {
                        continue;
                    };
                    if is_link(&meta) {
                        continue;
                    }
                    if !meta.is_dir() {
                        continue;
                    }
                    for key in ancestor_keys(root, child, depth) {
                        local.entry(key).or_default();
                    }
                    walk_index(root, child, depth, &mut local, &mut local_total);
                }
                (local, local_total)
            }));
        }
        for handle in handles {
            if let Ok(partial) = handle.join() {
                partials.push(partial);
            }
        }
    });
    for (local, local_total) in partials {
        total.add(local_total);
        for (key, usage) in local {
            let slot = index.entry(key).or_default();
            slot.add(usage);
        }
    }
    index.insert(String::new(), total);
    (index, deferred)
}

fn walk_index(
    root: &Path,
    dir: &Path,
    depth: usize,
    index: &mut HashMap<String, Usage>,
    total: &mut Usage,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_link(&meta) {
            continue;
        }
        if meta.is_file() {
            let usage = Usage {
                bytes: meta.len(),
                files: 1,
            };
            for key in ancestor_keys(root, dir, depth) {
                index.entry(key).or_default().add(usage);
            }
            total.add(usage);
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        for key in ancestor_keys(root, &path, depth) {
            index.entry(key).or_default();
        }
        walk_index(root, &path, depth, index, total);
    }
}

fn ancestor_keys(root: &Path, dir: &Path, depth: usize) -> Vec<String> {
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        if let Component::Normal(text) = component {
            parts.push(text.to_string_lossy().to_string());
            if parts.len() == depth {
                break;
            }
        }
    }
    (1..=parts.len())
        .map(|count| parts[..count].join("/"))
        .collect()
}
