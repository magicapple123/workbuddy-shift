//! 分类目录：把三个根下的可清理单元算成「分类 → 单元」。
//!
//! 单元是删除的最小粒度（一个目录或一个文件）。不可删分类同样出现在扫描结果里，
//! 让用户看到体积构成；`units` 为空且 `protected` 有值。
//!
//! 用量口径（第三轮第 8 条 8.3-1「分类 + 其它 == 根总量」）：
//! - 每个根都有一行 `*.other`「其它 / 未分类」，值 = **根总量 − 已列分类**，
//!   只读、不提供清理；这样分类表不会再出现「只是子集、加起来对不上」的情况；
//! - 分类数字来自同一次遍历（[`usage_index`] / [`usage_index_deferred`]），
//!   只有需要逐文件时间过滤（`clipboard-images`）或引用过滤（无引用基线）的分类才单独 stat；
//! - 回收站体积与条目数也来自这趟遍历的 `trash` 键：总览卡与分类行**不可能对不上**；
//! - 客户端根的分类在 [`super::catalog_client`] 里构建（单文件规模控制），受保护大目录
//!   （`binaries` / `plugins` / `blobs` 等）走 [`super::protected`] 的 1 小时缓存：
//!   命中给数字，未命中按 0 计并标记 `protectedPending`，由调用方在后台补算。

use super::roots::{usage_index, usage_of, StorageRoots, Usage};
use crate::modules::variant::WbVariant;
use std::path::PathBuf;

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
    pub(super) fn protected(
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
            usage,
            units: Vec::new(),
            deletable: false,
            note: why.to_string(),
            protected: Some(why.to_string()),
            path,
        }
    }

    /// 「其它 / 未分类」：只读补集，让分类之和等于根总量。
    pub(super) fn other(id: &str, label: &str, group: &'static str, path: PathBuf, usage: Usage) -> Self {
        let why = "根总量减去已列分类，含不可清理的大目录；只读展示，不提供清理";
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

/// 需要后台补算的受保护项（每个客户端根一条，`names` 是该根下要计量的顶层目录名）。
#[derive(Clone, Debug)]
pub struct DeferredJob {
    pub root_id: String,
    pub root: PathBuf,
    pub names: Vec<String>,
}

/// 一次扫描的结果：分类明细 + 两个根的总量（分类之和与总量由同一趟遍历给出）。
#[derive(Clone, Debug)]
pub struct Catalog {
    pub categories: Vec<Category>,
    pub store_total: Usage,
    /// 各客户端根（`cn` / `ai`）的用量，按存在顺序；页面按根分别展示。
    pub client_roots: Vec<(String, Usage)>,
    pub client_total: Usage,
    /// 回收站体积与条目数：与 `tool.trash` 分类**同一趟遍历**得出，因此总览卡与分类行
    /// 不可能对不上（第三轮第 6 条 6.5「回收站只走一次」+ 第 8 条口径一致）。
    pub trash_usage: Usage,
    pub trash_entries: usize,
    /// 受保护项还有没算完的（数字为部分和，页面必须标注「计算中」）。
    pub protected_pending: bool,
    /// 需要后台补算的受保护项。
    pub deferred: Vec<DeferredJob>,
}

/// 构建全部分类；客户端根按实际存在与否展开（国内 / 国际）。
///
/// `links` 是 `session_links.json` 的原文，由快照统一读一次后同时交给本模块
/// （无引用基线判定）与 [`super::sessions`]（关联组解析），避免同一文件读两遍。
pub fn catalog(roots: &StorageRoots, variant: WbVariant, links: &str) -> Catalog {
    let mut categories = Vec::new();
    let (store_total, trash_usage, trash_entries) =
        tool_categories(roots, links, &mut categories);
    let mut client_total = Usage::default();
    let mut client_roots = Vec::new();
    let mut protected_pending = false;
    let mut deferred = Vec::new();
    for (client_id, root) in roots.client_roots() {
        let (usage, pending, job) = super::catalog_client::client_categories(
            client_id,
            &root,
            roots,
            variant,
            &mut categories,
        );
        client_total.add(usage);
        client_roots.push((client_id.to_string(), usage));
        protected_pending |= pending;
        if let Some(job) = job {
            deferred.push(job);
        }
    }
    Catalog {
        categories,
        store_total,
        client_roots,
        client_total,
        trash_usage,
        trash_entries,
        protected_pending,
        deferred,
    }
}

/// 工具数据根的分类；返回 `(根总量, 回收站体积, 回收站条目数)`。
fn tool_categories(
    roots: &StorageRoots,
    links: &str,
    out: &mut Vec<Category>,
) -> (Usage, Usage, usize) {
    let store = &roots.store;
    if !store.is_dir() {
        return (Usage::default(), Usage::default(), 0);
    }
    let index = usage_index(store, 2);
    let total = index.get("").copied().unwrap_or_default();
    let usage = |key: &str| index.get(key).copied().unwrap_or_default();
    let mut listed = Usage::default();

    // 无引用基线：文件名（去扩展名）没出现在关联主表里即可删。
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
            if links.contains(stem) {
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
    listed.add(unreferenced);
    out.push(Category {
        id: "tool.baselines".into(),
        group: "tool",
        label: "无引用基线".into(),
        path: baselines_dir,
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

    let previews = whole_dir(
        "tool.previews",
        "预览凭据",
        "tool",
        store.join("session-links").join("previews"),
        usage("session-links/previews"),
        "一次性预览凭据，可安全重建",
    );
    listed.add(previews.usage);
    out.push(previews);

    let session_backups = whole_dir(
        "tool.session-backups",
        "旧会话备份",
        "tool",
        store.join("backups").join("sessions"),
        usage("backups/sessions"),
        "旧版按时间戳留存，保留期已过",
    );
    listed.add(session_backups.usage);
    out.push(session_backups);

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
    listed.add(login_usage);
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
    listed.add(cache_usage);
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

    let transactions = Category::protected(
        "tool.session-transactions",
        "会话操作备份",
        "tool",
        store.join("backups").join("session-transactions"),
        usage("backups/session-transactions"),
        "进行中/待恢复的会话操作材料，由备份生命周期自动清理",
    );
    listed.add(transactions.usage);
    out.push(transactions);

    // 回收站体积直接取同一次遍历的 `trash` 键：不再对回收站目录单独走一遍盘
    // （第三轮第 6 条 6.5「回收站只走一次」）。条目数也从同一份索引里数
    // （`trash/<时间戳>` 是深度 2 的目录键），总览卡与分类行因此不可能对不上。
    let trash_usage = usage("trash");
    let trash_entries = index
        .keys()
        .filter(|key| {
            key.strip_prefix("trash/")
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
        })
        .count();
    let trash = Category::protected(
        "tool.trash",
        "回收站",
        "tool",
        roots.trash_root(),
        trash_usage,
        "在「回收站」页签里恢复或彻底删除",
    );
    listed.add(trash.usage);
    out.push(trash);

    out.push(Category::other(
        "tool.other",
        "其它 / 未分类",
        "tool",
        store.clone(),
        total.saturating_sub(listed),
    ));
    (total, trash_usage, trash_entries)
}

pub(super) fn whole_dir(
    id: &str,
    label: &str,
    group: &'static str,
    path: PathBuf,
    usage: Usage,
    note: &str,
) -> Category {
    // 目录存在就给一个单元（体积可能是 0：空目录也算「可清理」的目标）。
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

pub(super) fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
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
