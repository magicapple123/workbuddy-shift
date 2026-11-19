//! 受保护 / 大目录的**延迟计量**与 **1 小时持久化缓存**（第三轮第 6 条 6.6）。
//!
//! 为什么需要：客户端根（`~/.workbuddy`、`~/.workbuddy-ai`）下 `app` / `binaries` /
//! `plugins` / `blobs` 与几十个账号 UUID 目录占了绝大部分体积，但它们**不可清理**；
//! 首屏只需要「可清理 + 会话资产」的精确数字。全量遍历这棵树要数秒到数十秒，是当前
//! 存储页最主要的成本。
//!
//! 口径规则（不得违反）：
//! - 快扫结果只包含 [`client_keeps`] 允许的顶层目录 + 顶层文件；
//! - 其它顶层目录按体积缓存给出数字；缓存缺失或超过 [`CACHE_TTL_SECS`] 时返回
//!   `pending = true`，数值先按 0 计，**不得**把部分和伪装成完整总量（响应里带
//!   `protectedPending`，页面据此标注「受保护项计算中」）；
//! - 后台补算单飞（[`fill_in_background`]），App 退出时随进程结束；
//! - 写缓存先写临时文件再原子替换，进程被杀不会留下半截 JSON。
//!
//! 缓存文件：`~/.workbuddy-shift/storage_size_cache.json`

use super::roots::{usage_of, Usage};
use crate::modules::variant::WbVariant;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 受保护项体积缓存的有效期：1 小时。
pub const CACHE_TTL_SECS: u64 = 3600;

/// 缓存文件名（工具数据目录下，属于稳定契约的一部分，改名 = 用户数据迁移问题）。
pub const CACHE_FILE: &str = "storage_size_cache.json";

/// 后台补算线程名（诊断日志与任务管理器里能认出来）。
const FILL_THREAD_NAME: &str = "storage-protected-fill";

/// 客户端根下**留在快扫里**的顶层目录：这些要么可清理，要么参与会话资产口径。
///
/// 不在这个列表里的顶层目录（`app` / `binaries` / `plugins` / `blobs` / `cache` /
/// `browser-runtime` / `connectors` / `credentials` / `extension-registry` /
/// `external-packages` / `im-channels` / 账号 UUID 目录……）一律延迟计量。
pub const CLIENT_FAST_TOP_LEVEL: &[&str] = &[
    "logs",
    "traces",
    "file-tree-manifests",
    "file-history",
    "clipboard-images",
    "projects",
    "artifact-index",
    "changes-index",
    "changes-detail",
];

/// 顶层目录是否留在快扫里（`incomplete-backup-*` 可能有多份，按前缀放行）。
pub fn client_keeps(name: &str) -> bool {
    CLIENT_FAST_TOP_LEVEL.contains(&name) || name.starts_with("incomplete-backup-")
}

/// 单个受保护目录的体积。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SizeEntry {
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub files: u64,
}

impl SizeEntry {
    pub fn from_usage(usage: Usage) -> Self {
        Self {
            bytes: usage.bytes,
            files: usage.files,
        }
    }

    pub fn usage(&self) -> Usage {
        Usage {
            bytes: self.bytes,
            files: self.files,
        }
    }
}

/// 缓存文件内容；`at` 是写入时刻（Unix 秒），`variant` 变化视为整体失效。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SizeCache {
    #[serde(default)]
    pub at: u64,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub sizes: BTreeMap<String, SizeEntry>,
}

/// 一次快扫给出的受保护项计量结果。
#[derive(Clone, Debug, Default)]
pub struct DeferredSizes {
    /// 已命中的缓存项（键见 [`key`]）。
    pub sizes: BTreeMap<String, SizeEntry>,
    /// 是否还有未命中 / 已过期的项（页面据此标注「计算中」）。
    pub pending: bool,
    /// 需要后台补算的顶层目录名（相对该客户端根）。
    pub missing: Vec<String>,
}

impl DeferredSizes {
    /// 该根下受保护项的合计（未命中按 0 计）。
    pub fn total(&self) -> Usage {
        self.sizes
            .values()
            .fold(Usage::default(), |mut acc, entry| {
                acc.add(entry.usage());
                acc
            })
    }

    /// 按顶层目录名取体积（命不中返回 `None`，调用方区分「0 字节」与「还没算」）。
    pub fn get(&self, name: &str) -> Option<Usage> {
        self.sizes
            .iter()
            .find(|(key, _)| key.rsplit("::").next() == Some(name))
            .map(|(_, entry)| entry.usage())
    }
}

/// 缓存键：`<root_id>::<顶层目录名>`（`root_id` 只用 `cn` / `ai`，不写绝对路径，
/// 用户换家目录时缓存仍然可用）。
pub fn key(root_id: &str, name: &str) -> String {
    format!("{root_id}::{name}")
}

/// 当前 Unix 秒（0 表示取不到，永不命中 TTL）。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 缓存文件路径：`<工具数据根>/storage_size_cache.json`。
pub fn cache_path(store: &Path) -> PathBuf {
    store.join(CACHE_FILE)
}

/// 读缓存；文件缺失 / 非法 JSON / `variant` 不匹配都返回空缓存（= 全部待补算）。
pub fn load(store: &Path, variant: WbVariant) -> SizeCache {
    let Ok(text) = std::fs::read_to_string(cache_path(store)) else {
        return SizeCache::default();
    };
    let Ok(cache) = serde_json::from_str::<SizeCache>(&text) else {
        return SizeCache::default();
    };
    if cache.variant != variant.as_str() {
        return SizeCache::default();
    }
    cache
}

/// 缓存是否在 1 小时新鲜窗口内。
pub fn is_fresh(cache: &SizeCache, now: u64) -> bool {
    cache.at > 0 && now.saturating_sub(cache.at) < CACHE_TTL_SECS
}

/// 从缓存里解析某个根的受保护项：命中的给数字，未命中的进 `missing`。
pub fn resolve(cache: &SizeCache, fresh: bool, root_id: &str, names: &[String]) -> DeferredSizes {
    let mut out = DeferredSizes::default();
    for name in names {
        match cache.sizes.get(&key(root_id, name)) {
            Some(entry) if fresh => {
                out.sizes.insert(key(root_id, name), entry.clone());
            }
            _ => {
                out.pending = true;
                out.missing.push(name.clone());
            }
        }
    }
    out
}

/// 原子写缓存：先写 `<name>.tmp` 再 rename，避免半截 JSON 被下一次扫描当成 0 字节。
pub fn save(store: &Path, cache: &SizeCache) -> Result<(), String> {
    std::fs::create_dir_all(store).map_err(|error| format!("创建工具数据目录失败：{error}"))?;
    let path = cache_path(store);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(cache)
        .map_err(|error| format!("序列化体积缓存失败：{error}"))?;
    std::fs::write(&tmp, text).map_err(|error| format!("写入体积缓存失败：{error}"))?;
    std::fs::rename(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换体积缓存失败：{error}")
    })
}

/// 后台补算单飞标记：同一时刻只允许一个补算线程。
///
/// 用 `AtomicBool` 而不是锁：抢不到就放弃（下一次扫描还会补），不需要排队。
static FILLING: AtomicBool = AtomicBool::new(false);

/// 是否有补算在进行（诊断 / 单测用）。
pub fn is_filling() -> bool {
    FILLING.load(Ordering::SeqCst)
}

/// 起一个后台线程补算受保护项并写缓存；已在补算时直接返回。
///
/// 不阻塞请求线程：调用方（快照构建）只负责发起。线程内的错误一律静默（写缓存失败
/// 最多让下一次扫描继续 `pending`，不能弹 toast 打扰用户）。
pub fn fill_in_background(
    store: PathBuf,
    variant: WbVariant,
    root_id: String,
    root: PathBuf,
    names: Vec<String>,
) {
    if names.is_empty() {
        return;
    }
    if FILLING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name(FILL_THREAD_NAME.to_string())
        .spawn(move || {
            let mut cache = load(&store, variant);
            cache.variant = variant.as_str().to_string();
            for name in &names {
                let usage = usage_of(&root.join(name));
                cache
                    .sizes
                    .insert(key(&root_id, name), SizeEntry::from_usage(usage));
            }
            cache.at = now_secs();
            let _ = save(&store, &cache);
            FILLING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        // 起不了线程（资源耗尽）就放弃这次补算，标记必须还回 false。
        FILLING.store(false, Ordering::SeqCst);
    }
}

/// 该快照里待补算的项是否已经在缓存里齐了（用于「后台算完后重建一次」）。
///
/// `pending` 是 `(root_id, 顶层目录名)` 列表。
pub fn pending_resolved(store: &Path, variant: WbVariant, pending: &[(String, String)]) -> bool {
    if pending.is_empty() {
        return true;
    }
    let cache = load(store, variant);
    if !is_fresh(&cache, now_secs()) {
        return false;
    }
    pending
        .iter()
        .all(|(root_id, name)| cache.sizes.contains_key(&key(root_id, name)))
}

/// 让下一次扫描强制忽略缓存（「重新扫描」的受保护项刷新入口）。
///
/// 只把 `at` 归零：数字留着，页面不会突然掉到 0，但下一次扫描会重新补算。
pub fn invalidate(store: &Path, variant: WbVariant) -> Result<(), String> {
    let cache = load(store, variant);
    if cache.at == 0 {
        return Ok(());
    }
    save(
        store,
        &SizeCache {
            at: 0,
            variant: cache.variant,
            sizes: cache.sizes,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wb-storage-protected-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建夹具目录");
        dir
    }

    #[test]
    fn keeps_fast_directories_and_defers_the_rest() {
        assert!(client_keeps("logs"));
        assert!(client_keeps("projects"));
        assert!(client_keeps("incomplete-backup-2026"));
        assert!(!client_keeps("binaries"));
        assert!(!client_keeps("app"));
        assert!(!client_keeps("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"));
    }

    #[test]
    fn cache_roundtrips_and_expires_after_one_hour() {
        let store = temp_store("roundtrip");
        let mut cache = SizeCache {
            at: 1_000,
            variant: "cn".to_string(),
            sizes: BTreeMap::new(),
        };
        cache
            .sizes
            .insert(key("cn", "binaries"), SizeEntry { bytes: 7, files: 2 });
        save(&store, &cache).expect("写缓存");
        let loaded = load(&store, WbVariant::Cn);
        assert_eq!(loaded.at, 1_000);
        assert_eq!(loaded.sizes.len(), 1);
        assert!(is_fresh(&loaded, 1_000 + CACHE_TTL_SECS - 1));
        assert!(!is_fresh(&loaded, 1_000 + CACHE_TTL_SECS));
        // variant 不匹配 → 整体失效。
        assert!(load(&store, WbVariant::Ai).sizes.is_empty());
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn resolve_marks_missing_entries_pending() {
        let mut cache = SizeCache {
            at: now_secs(),
            variant: "cn".to_string(),
            sizes: BTreeMap::new(),
        };
        cache
            .sizes
            .insert(key("cn", "binaries"), SizeEntry { bytes: 5, files: 1 });
        let names = vec!["binaries".to_string(), "plugins".to_string()];

        let fresh = resolve(&cache, true, "cn", &names);
        assert!(fresh.pending, "plugins 未命中必须 pending");
        assert_eq!(fresh.missing, vec!["plugins".to_string()]);
        assert_eq!(fresh.total().bytes, 5);
        assert_eq!(fresh.get("binaries").map(|usage| usage.bytes), Some(5));

        let stale = resolve(&cache, false, "cn", &names);
        assert!(stale.pending);
        assert_eq!(stale.sizes.len(), 0, "过期缓存一个都不认");
    }

    #[test]
    fn invalidate_keeps_numbers_but_forces_refill() {
        let store = temp_store("invalidate");
        let mut cache = SizeCache {
            at: now_secs(),
            variant: "cn".to_string(),
            sizes: BTreeMap::new(),
        };
        cache
            .sizes
            .insert(key("cn", "plugins"), SizeEntry { bytes: 9, files: 1 });
        save(&store, &cache).expect("写缓存");
        invalidate(&store, WbVariant::Cn).expect("作废");
        let loaded = load(&store, WbVariant::Cn);
        assert_eq!(loaded.at, 0, "at 归零 = 过期");
        assert_eq!(loaded.sizes.len(), 1, "数字保留，下一帧不会掉到 0");
        let _ = std::fs::remove_dir_all(&store);
    }
}
