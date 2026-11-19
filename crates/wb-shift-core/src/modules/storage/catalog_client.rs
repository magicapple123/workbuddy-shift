//! 客户端根（`~/.workbuddy` / `~/.workbuddy-ai`）的分类构建。
//!
//! 从 `catalog.rs` 拆出（单文件规模控制），职责边界：**只处理客户端根的顶层目录**，
//! 工具数据根的分类与「其它 / 未分类」补算口径留在 [`super::catalog`]。
//!
//! 三处口径约定（第三轮第 8 条 8.3）：
//! - 快扫只覆盖 [`protected::client_keeps`] 允许的顶层目录，其余按体积缓存给数字；
//! - 受保护项缓存缺失 / 过期时按 0 计并回传 `pending = true`，**不得**把部分和当完整值；
//! - 每个根末尾必须有一行 `<tag>其它 / 未分类`，使「已列分类之和 == 根总量」。

use super::catalog::{
    human_usage, mtime_ms, now_ms, whole_dir, Category, DeferredJob, Unit, CLIPBOARD_RETENTION_DAYS,
};
use super::protected::{self, DeferredSizes};
use super::roots::{usage_index_deferred, StorageRoots, Usage};
use crate::modules::variant::WbVariant;
use std::path::Path;

/// 构建一个客户端根的分类；返回 `(根总量, 是否 pending, 需要后台补算的任务)`。
pub(super) fn client_categories(
    client_id: &str,
    root: &Path,
    roots: &StorageRoots,
    variant: WbVariant,
    out: &mut Vec<Category>,
) -> (Usage, bool, Option<DeferredJob>) {
    let (index, deferred_names) = usage_index_deferred(root, 2, protected::client_keeps);
    let cache = protected::load(&roots.store, variant);
    let fresh = protected::is_fresh(&cache, protected::now_secs());
    let sizes: DeferredSizes = protected::resolve(&cache, fresh, client_id, &deferred_names);

    // 报告的根总量 = 快扫总量 + 受保护项缓存值（未命中按 0 计，并用 pending 标注）。
    let mut total = index.get("").copied().unwrap_or_default();
    total.add(sizes.total());
    let deferred = |key: &str| sizes.get(key).unwrap_or_default();
    let usage = |key: &str| index.get(key).copied().unwrap_or_else(|| deferred(key));

    let suffix = if client_id == "cn" { "cn" } else { "ai" };
    let tag = if client_id == "cn" {
        "国内版"
    } else {
        "国际版"
    };
    let mut listed = Usage::default();

    for (key, id, label) in [
        ("logs", "logs", "日志"),
        ("traces", "traces", "运行追踪"),
        ("file-tree-manifests", "file-tree-manifests", "文件树清单"),
        ("file-history", "file-history", "文件历史"),
    ] {
        let category = whole_dir(
            &format!("client.{suffix}.{id}"),
            &format!("{tag}{label}"),
            "client",
            root.join(key),
            usage(key),
            "客户端缓存，会按需重建；要求客户端退出",
        );
        listed.add(category.usage);
        out.push(category);
    }

    // incomplete-backup-* 可能有多份，逐个单元；体积直接取顶层遍历已经给出的键，
    // 不再为每个目录单独 usage_of 一遍（第三轮第 6 条 6.5）。
    let mut backup_units = Vec::new();
    for (key, value) in &index {
        let Some(name) = key.strip_prefix("incomplete-backup-") else {
            continue;
        };
        if name.contains('/') {
            continue;
        }
        backup_units.push(Unit {
            path: root.join(key),
            usage: *value,
        });
    }
    backup_units.sort_by(|a, b| a.path.cmp(&b.path));
    let backup_usage = backup_units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
    listed.add(backup_usage);
    out.push(Category {
        id: format!("client.{suffix}.incomplete-backup"),
        group: "client",
        label: format!("{tag}未完成备份"),
        path: root.to_path_buf(),
        usage: backup_usage,
        units: backup_units,
        deletable: true,
        note: "客户端中断的升级备份，可直接清理".into(),
        protected: None,
    });

    // clipboard-images：只清 30 天前的文件（分类体积 = 可清部分，其余进「其它」）。
    let clipboard = root.join("clipboard-images");
    let mut old_units = Vec::new();
    let mut clipboard_total = Usage::default();
    let cutoff = now_ms() - (CLIPBOARD_RETENTION_DAYS as i64) * 24 * 60 * 60 * 1000;
    if let Ok(entries) = std::fs::read_dir(&clipboard) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if super::roots::is_link(&meta) || !meta.is_file() {
                continue;
            }
            let item = Usage {
                bytes: meta.len(),
                files: 1,
            };
            clipboard_total.add(item);
            if mtime_ms(&meta) < cutoff {
                old_units.push(Unit { path, usage: item });
            }
        }
    }
    let old_usage = old_units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
    listed.add(old_usage);
    out.push(Category {
        id: format!("client.{suffix}.clipboard-images"),
        group: "client",
        label: format!("{tag}剪贴板图片"),
        path: clipboard,
        usage: old_usage,
        units: old_units,
        deletable: true,
        note: format!(
            "共 {} · 只清 {CLIPBOARD_RETENTION_DAYS} 天前的文件（其余计入「其它」）",
            human_usage(clipboard_total)
        ),
        protected: None,
    });

    // 不可删：客户端运行时与数据主体。`binaries` / `plugins` / `blobs` 属于受保护大目录，
    // 数字可能来自缓存（未命中为 0 且整体 pending）。
    let protected_dirs = [
        ("binaries", "运行时", "客户端运行时文件，删除会破坏安装"),
        ("plugins", "插件", "已安装插件，删除会丢失扩展"),
        (
            "projects",
            "会话正文目录",
            "会话正文在「会话资产」页签按会话清理",
        ),
        (
            "blobs",
            "图片附件",
            "多会话共享，只随会话按引用计数删零引用文件",
        ),
        (
            "artifact-index",
            "产物索引",
            "索引指向工作区真实文件，只能随会话删索引本身",
        ),
    ];
    for (dir, label, why) in protected_dirs {
        let category = Category::protected(
            &format!("client.{suffix}.{dir}"),
            &format!("{tag}{label}"),
            "client",
            root.join(dir),
            usage(dir),
            why,
        );
        listed.add(category.usage);
        out.push(category);
    }

    let other = Category::other(
        &format!("client.{suffix}.other"),
        &format!("{tag}其它 / 未分类"),
        "client",
        root.to_path_buf(),
        total.saturating_sub(listed),
    );
    out.push(other);

    let job = if sizes.pending {
        Some(DeferredJob {
            root_id: client_id.to_string(),
            root: root.to_path_buf(),
            names: sizes.missing.clone(),
        })
    } else {
        None
    };
    (total, sizes.pending, job)
}
