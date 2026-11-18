//! 分类目录：把三个根下的可清理单元算成「分类 → 单元」。
//!
//! 单元是删除的最小粒度（一个目录或一个文件）。不可删分类同样出现在扫描结果里，
//! 让用户看到体积构成；`units` 为空且 `protected` 有值。
//!
//! 用量口径：分类数字来自 [`usage_index`] 的同一次遍历（分类之和 = 根总量），
//! 只有需要逐文件时间过滤（`clipboard-images`）或引用过滤（无引用基线）的分类才单独 stat。

use super::roots::{usage_index, usage_of, StorageRoots, Usage};
use std::path::{Path, PathBuf};

/// 一个可删除单元（文件或目录）。
#[derive(Clone, Debug)]
pub struct Unit {
    pub path: PathBuf,
    pub usage: Usage,
}

/// 扫描结果里的一个分类。
#[derive(Clone, Debug)]
pub struct Category {
    pub id: String,
    pub group: &'static str,
    pub label: String,
    pub path: PathBuf,
    pub usage: Usage,
    pub units: Vec<Unit>,
    pub deletable: bool,
    pub note: String,
    pub protected: Option<String>,
}

impl Category {
    fn protected(
        id: &str,
        label: &str,
        group: &'static str,
        path: PathBuf,
        usage: Usage,
        why: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            group,
            label: label.to_string(),
            path,
            usage,
            units: Vec::new(),
            deletable: false,
            note: why.to_string(),
            protected: Some(why.to_string()),
        }
    }
}

/// `clipboard-images` 的最短保留期：只清 30 天前的文件。
pub const CLIPBOARD_RETENTION_DAYS: u64 = 30;

/// 一次扫描的结果：分类明细 + 两个根的总量（分类之和与总量由同一趟遍历给出）。
pub struct Catalog {
    pub categories: Vec<Category>,
    pub store_total: Usage,
    /// 各客户端根（`cn` / `ai`）的用量，按存在顺序；页面按根分别展示。
    pub client_roots: Vec<(String, Usage)>,
    pub client_total: Usage,
}

/// 构建全部分类；客户端根按实际存在与否展开（国内 / 国际）。
pub fn catalog(roots: &StorageRoots) -> Catalog {
    let mut categories = Vec::new();
    let store_total = tool_categories(roots, &mut categories);
    let mut client_total = Usage::default();
    let mut client_roots = Vec::new();
    for (client_id, root) in roots.client_roots() {
        let usage = client_categories(client_id, &root, &mut categories);
        client_total.add(usage);
        client_roots.push((client_id.to_string(), usage));
    }
    Catalog {
        categories,
        store_total,
        client_roots,
        client_total,
    }
}

fn tool_categories(roots: &StorageRoots, out: &mut Vec<Category>) -> Usage {
    let store = &roots.store;
    if !store.is_dir() {
        return Usage::default();
    }
    let index = usage_index(store, 2);
    let total = index.get("").copied().unwrap_or_default();
    let usage = |key: &str| index.get(key).copied().unwrap_or_default();

    // 无引用基线：文件名（去扩展名）没出现在关联主表里即可删。
    let references = std::fs::read_to_string(store.join("session_links.json")).unwrap_or_default();
    let baselines_dir = store.join("session-links").join("baselines");
    let mut units = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&baselines_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if references.contains(stem) {
                continue;
            }
            units.push(Unit {
                usage: usage_of(&path),
                path,
            });
        }
    }
    let unreferenced = units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
    out.push(Category {
        id: "tool.baselines".into(),
        group: "tool",
        label: "无引用基线".into(),
        path: baselines_dir.clone(),
        usage: unreferenced,
        units,
        deletable: true,
        note: format!(
            "共 {} · 可删 {} · 仅删未被关联主表引用的快照",
            human_usage(usage("session-links/baselines")),
            human_usage(unreferenced)
        ),
        protected: None,
    });

    out.push(whole_dir(
        "tool.previews",
        "预览凭据",
        "tool",
        store.join("session-links").join("previews"),
        usage("session-links/previews"),
        "一次性预览凭据，可安全重建",
    ));

    out.push(whole_dir(
        "tool.session-backups",
        "旧会话备份",
        "tool",
        store.join("backups").join("sessions"),
        usage("backups/sessions"),
        "旧版按时间戳留存，保留期已过",
    ));

    // 登录态备份：逐个文件进回收站。
    let mut login_units = Vec::new();
    if let Ok(entries) = std::fs::read_dir(store.join("backups")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_login_backup = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name.starts_with("workbuddy-desktop") && name.ends_with(".info")
                });
            if is_login_backup && path.is_file() {
                login_units.push(Unit {
                    usage: usage_of(&path),
                    path,
                });
            }
        }
    }
    let login_usage = login_units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
    out.push(Category {
        id: "tool.login-backups".into(),
        group: "tool",
        label: "登录态备份".into(),
        path: store.join("backups"),
        usage: login_usage,
        units: login_units,
        deletable: true,
        note: "每次切换前留下的登录态副本，保留最新一份即可".into(),
        protected: None,
    });

    let cache_files = [
        "credit_usage_snapshots.json",
        "official_usage_cache.json",
        "travel_cache.json",
    ];
    let mut cache_units = Vec::new();
    for name in cache_files {
        let path = store.join(name);
        if path.is_file() {
            cache_units.push(Unit {
                usage: usage_of(&path),
                path,
            });
        }
    }
    let cache_usage = cache_units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
    out.push(Category {
        id: "tool.cache".into(),
        group: "tool",
        label: "统计缓存".into(),
        path: store.clone(),
        usage: cache_usage,
        units: cache_units,
        deletable: true,
        note: "下次刷新会重新生成".into(),
        protected: None,
    });

    out.push(Category::protected(
        "tool.session-transactions",
        "会话操作备份",
        "tool",
        store.join("backups").join("session-transactions"),
        usage("backups/session-transactions"),
        "进行中/待恢复的会话操作材料，由备份生命周期自动清理",
    ));

    out.push(Category::protected(
        "tool.trash",
        "回收站",
        "tool",
        roots.trash_root(),
        usage_of(&roots.trash_root()),
        "在「回收站」页签里恢复或彻底删除",
    ));
    total
}

fn whole_dir(
    id: &str,
    label: &str,
    group: &'static str,
    path: PathBuf,
    usage: Usage,
    note: &str,
) -> Category {
    let units = if path.exists() {
        vec![Unit {
            path: path.clone(),
            usage,
        }]
    } else {
        Vec::new()
    };
    Category {
        id: id.to_string(),
        group,
        label: label.to_string(),
        path,
        usage: if units.is_empty() {
            Usage::default()
        } else {
            usage
        },
        units,
        deletable: true,
        note: note.to_string(),
        protected: None,
    }
}

fn client_categories(client_id: &str, root: &Path, out: &mut Vec<Category>) -> Usage {
    let index = usage_index(root, 2);
    let total = index.get("").copied().unwrap_or_default();
    let usage = |key: &str| index.get(key).copied().unwrap_or_default();
    let suffix = if client_id == "cn" { "cn" } else { "ai" };
    let tag = if client_id == "cn" {
        "国内版"
    } else {
        "国际版"
    };

    for (key, id, label) in [
        ("logs", "logs", "日志"),
        ("traces", "traces", "运行追踪"),
        ("file-tree-manifests", "file-tree-manifests", "文件树清单"),
        ("file-history", "file-history", "文件历史"),
    ] {
        out.push(whole_dir(
            &format!("client.{suffix}.{id}"),
            &format!("{tag}{label}"),
            "client",
            root.join(key),
            usage(key),
            "客户端缓存，会按需重建；要求客户端退出",
        ));
    }

    // incomplete-backup-* 可能有多份，逐个单元。
    let mut backup_units = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("incomplete-backup-") && entry.path().is_dir() {
                let path = entry.path();
                backup_units.push(Unit {
                    usage: usage_of(&path),
                    path,
                });
            }
        }
    }
    let backup_usage = backup_units.iter().fold(Usage::default(), |mut acc, unit| {
        acc.add(unit.usage);
        acc
    });
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

    // clipboard-images：只清 30 天前的文件。
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
    out.push(Category {
        id: format!("client.{suffix}.clipboard-images"),
        group: "client",
        label: format!("{tag}剪贴板图片"),
        path: clipboard,
        usage: old_usage,
        units: old_units,
        deletable: true,
        note: format!(
            "共 {} · 只清 {CLIPBOARD_RETENTION_DAYS} 天前的文件",
            human_usage(clipboard_total)
        ),
        protected: None,
    });

    // 不可删：客户端运行时与数据主体。
    out.push(Category::protected(
        &format!("client.{suffix}.binaries"),
        &format!("{tag}运行时"),
        "client",
        root.join("binaries"),
        usage("binaries"),
        "客户端运行时文件，删除会破坏安装",
    ));
    out.push(Category::protected(
        &format!("client.{suffix}.plugins"),
        &format!("{tag}插件"),
        "client",
        root.join("plugins"),
        usage("plugins"),
        "已安装插件，删除会丢失扩展",
    ));
    out.push(Category::protected(
        &format!("client.{suffix}.projects"),
        &format!("{tag}会话正文目录"),
        "client",
        root.join("projects"),
        usage("projects"),
        "会话正文在「会话资产」页签按会话清理",
    ));
    out.push(Category::protected(
        &format!("client.{suffix}.blobs"),
        &format!("{tag}图片附件"),
        "client",
        root.join("blobs"),
        usage("blobs"),
        "多会话共享，只随会话按引用计数删零引用文件",
    ));
    out.push(Category::protected(
        &format!("client.{suffix}.artifact-index"),
        &format!("{tag}产物索引"),
        "client",
        root.join("artifact-index"),
        usage("artifact-index"),
        "索引指向工作区真实文件，只能随会话删索引本身",
    ));
    total
}

/// 按分类 id 找分类（清理入口用；找不到即非法 id）。
pub fn find<'a>(categories: &'a [Category], id: &str) -> Option<&'a Category> {
    categories.iter().find(|category| category.id == id)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 人类可读体积（扫描结果里的 note 用；前端另有格式化）。
pub fn human_usage(usage: Usage) -> String {
    let bytes = usage.bytes as f64;
    let text = if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", bytes / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 {
        format!("{:.0} KB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    };
    format!("{text} / {} 文件", usage.files)
}
