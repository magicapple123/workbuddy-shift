//! 存储扫描的**单快照**：同一 TTL 内 `catalog` / `scan_sessions` / `trash` 各只跑一次，
//! 三个接口（总览、会话资产、回收站）共享同一份结果（第三轮第 6 条 6.2）。
//!
//! 改造前：`storage_scan` 与 `storage_sessions` 各自跑一次 `scan_sessions`，回收站被
//! `tool_categories` / `build_scan` / `trash_payload` 多次走盘 —— 同一棵会话树走两遍、
//! 回收站走三遍，页面首屏还要等 `Promise.all`。
//!
//! 现在的语义：
//! - `ensure()` 命中未过期快照直接返回（`reused = true`，响应里 `cached: true`）；
//! - 未命中时**持锁构建**，把「整棵树的元数据遍历」串行化，慢盘上不会互相抢盘；
//! - 清理 / 恢复 / 彻底删除后由 [`invalidate`] 整份作废，下一次请求重算；
//! - 受保护大目录还在后台补算时，快照不"冻结"数字：补算完成后的下一次请求会重建一次，
//!   页面才能从「计算中」变成完整值。

use super::catalog::{self, Catalog};
use super::protected;
use super::roots::{StorageRoots, Usage};
use super::sessions::{self, Bundle};
use crate::modules::locks::recover;
use crate::modules::variant::WbVariant;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 快照有效期（秒）：与改造前的 60 秒扫描缓存一致，页面顶部显示 `generatedAt`。
pub const SNAPSHOT_TTL_SECS: u64 = 60;

/// 各阶段耗时（毫秒），用于验收「改前 / 改后」的对照与线上诊断。
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub catalog_ms: u64,
    pub sessions_ms: u64,
    pub trash_ms: u64,
    /// 客户端进程探测耗时（Windows 上数百毫秒，与扫描无关，单独列出避免误读）。
    pub probe_ms: u64,
    pub total_ms: u64,
}

impl Timings {
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "totalMs": self.total_ms,
            "catalogMs": self.catalog_ms,
            "sessionsMs": self.sessions_ms,
            "trashMs": self.trash_ms,
            "probeMs": self.probe_ms,
        })
    }
}

/// 一次完整扫描的结果（三个接口共享）。
pub struct Snapshot {
    /// 工具数据根（受保护项缓存文件就在它下面）。
    pub store_root: std::path::PathBuf,
    pub built_at: Instant,
    pub generated_at: i64,
    pub variant: WbVariant,
    pub catalog: Catalog,
    pub bundles: Vec<Bundle>,
    pub trash_usage: Usage,
    pub trash_entries: usize,
    /// 客户端是否运行中（进程探测在 Windows 上要几百毫秒，只探测一次）。
    pub client_running: bool,
    pub timings: Timings,
}

impl Snapshot {
    /// 受保护项还没算完的 `(root_id, 顶层目录名)` 列表。
    pub fn pending_names(&self) -> Vec<(String, String)> {
        self.catalog
            .deferred
            .iter()
            .flat_map(|job| {
                job.names
                    .iter()
                    .map(|name| (job.root_id.clone(), name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// 会话资产包的合计（与 `bundle.total()` 同口径，逐项相加）。
    pub fn session_usage(&self) -> Usage {
        self.bundles
            .iter()
            .fold(Usage::default(), |mut acc, bundle| {
                acc.add(bundle.total());
                acc
            })
    }
}

/// 进程内快照缓存。
///
/// 用 `recover` 而不是 `guard`：这是可重建缓存，中毒时下一次请求会整体覆盖。
static SNAPSHOT: Mutex<Option<(String, Arc<Snapshot>)>> = Mutex::new(None);

/// 作废整份快照（清理 / 恢复 / 彻底删除 / 强制刷新后调用）。
pub fn invalidate() {
    let mut cache = recover(&SNAPSHOT);
    *cache = None;
}

/// 取当前快照；返回 `(快照, 是否复用缓存)`。
///
/// `data_root` 参与缓存键：不同档位（国内 / 国际）与不同沙箱的数据根不能混用。
pub fn ensure(roots: &StorageRoots, variant: WbVariant, data_root: &Path) -> (Arc<Snapshot>, bool) {
    let key = snapshot_key(roots, variant, data_root);
    let ttl = Duration::from_secs(SNAPSHOT_TTL_SECS);

    {
        let cache = recover(&SNAPSHOT);
        if let Some(snapshot) = reusable(cache.as_ref(), &key, ttl) {
            return (snapshot, true);
        }
    }

    // 持锁构建：同一时刻只允许一次全量遍历（慢盘上并发扫描会互相拖慢）。
    let mut cache = recover(&SNAPSHOT);
    // 二次检查：等锁期间可能已有别的请求构建好了。
    if let Some(snapshot) = reusable(cache.as_ref(), &key, ttl) {
        return (snapshot, true);
    }
    let snapshot = Arc::new(build(roots, variant, data_root));
    *cache = Some((key, Arc::clone(&snapshot)));
    (snapshot, false)
}

/// 缓存是否可用：键相同、未过期，且受保护项已补齐（否则要重建一次拿完整数字）。
fn reusable(
    entry: Option<&(String, Arc<Snapshot>)>,
    key: &str,
    ttl: Duration,
) -> Option<Arc<Snapshot>> {
    let (cached_key, snapshot) = entry?;
    if cached_key != key || snapshot.built_at.elapsed() >= ttl {
        return None;
    }
    let pending = snapshot.pending_names();
    if pending.is_empty() {
        return Some(Arc::clone(snapshot));
    }
    // 后台补算已把数字写进缓存 → 重建一次，让页面拿到完整总量。
    if protected::pending_resolved(&snapshot.store_root, snapshot.variant, &pending) {
        None
    } else {
        Some(Arc::clone(snapshot))
    }
}

fn snapshot_key(roots: &StorageRoots, variant: WbVariant, data_root: &Path) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        roots.store.to_string_lossy(),
        roots.client_cn.to_string_lossy(),
        roots.client_ai.to_string_lossy(),
        variant.as_str(),
        data_root.to_string_lossy()
    )
}

/// 真正跑一遍全量扫描（由 [`ensure`] 持锁调用）。
fn build(roots: &StorageRoots, variant: WbVariant, data_root: &Path) -> Snapshot {
    let started = Instant::now();

    // 关联主表只读一次：`catalog` 用它判无引用基线，`sessions` 用它解析关联组。
    let links = sessions::read_session_links(&roots.store);

    let catalog_started = Instant::now();
    let catalog = catalog::catalog(roots, variant, &links);
    let catalog_ms = millis(catalog_started.elapsed());

    let sessions_started = Instant::now();
    let bundles = sessions::scan_sessions_with_links(
        variant,
        data_root,
        &roots.store,
        read_rows(data_root),
        &links,
    );
    let sessions_ms = millis(sessions_started.elapsed());

    // 回收站不再单独走盘：体积与条目数都来自 `catalog` 那一趟遍历的 `trash` 键，
    // 因此「总览卡」与「分类明细里的回收站行」由构造保证是同一个数字
    // （第三轮第 6 条 6.5、第 8 条口径一致）。`trash_ms` 因此恒为 0，保留字段是为了
    // 契约稳定与线上诊断。
    let trash_usage = catalog.trash_usage;
    let trash_entries = catalog.trash_entries;
    let trash_ms = 0;

    // 进程探测（Windows 上数百毫秒）原本每个接口各做一次，这里同一份快照只做一次；
    // 破坏性操作仍在 `CleanContext::for_variant` 里实时探测，判定不受缓存影响。
    let probe_started = Instant::now();
    let client_running = super::client_running(variant);
    let probe_ms = millis(probe_started.elapsed());

    // 受保护项缺失 / 过期 → 起一次后台补算（单飞，失败静默）。
    for job in &catalog.deferred {
        protected::fill_in_background(
            roots.store.clone(),
            variant,
            job.root_id.clone(),
            job.root.clone(),
            job.names.clone(),
        );
    }

    Snapshot {
        store_root: roots.store.clone(),
        built_at: Instant::now(),
        generated_at: catalog::now_ms(),
        variant,
        catalog,
        bundles,
        trash_usage,
        trash_entries,
        client_running,
        timings: Timings {
            catalog_ms,
            sessions_ms,
            trash_ms,
            probe_ms,
            total_ms: millis(started.elapsed()),
        },
    }
}

/// 读一个档位的会话行（快照构建用；`sessions::scan_sessions` 的可复用拆解）。
fn read_rows(data_root: &Path) -> Vec<serde_json::Map<String, serde_json::Value>> {
    sessions::read_rows_at(data_root)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
