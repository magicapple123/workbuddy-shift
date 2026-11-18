//! 存储管理：扫描、按会话 / 分类清理、回收站恢复、跳转定位与外部删除重校验。
//!
//! 边界（AUDIT §3 T3）：
//! - 白名单根只有 `~/.workbuddy-shift`、`~/.workbuddy`（子目录）、`~/.workbuddy-ai`；
//! - 破坏性动作一律先进回收站（`trash/<ts>/`），彻底删除是独立按钮；
//! - 会话类操作要求客户端退出；删 DB 行前整库备份 + 整行快照；
//! - `blobs` 不整目录删；`binaries` / `plugins` / `app` / `storage` 不可删；
//! - 不引入文件监听：外部删除由页面 focus / visibility 触发
//!   [`revalidate::revalidate`]（3 s 防抖，只校验当前一页）。

mod catalog;
mod clean;
mod revalidate;
mod reveal;
mod roots;
mod sessions;
mod trash;

pub use clean::{clean, client_running, CleanContext};
pub use revalidate::revalidate;
pub use reveal::reveal;
pub use roots::{usage_of, StorageRoots, Usage};
pub use sessions::scan_sessions;
pub use trash::{list_entries, purge, restore_entry};

#[cfg(test)]
mod tests;

use crate::modules::variant::WbVariant;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 扫描结果缓存：10 万文件的元数据遍历在慢盘上要数秒，60 s 内重复打开页面不再重扫。
///
/// 用 `recover` 而不是 `guard`：这是可重建缓存，中毒时下一次请求会整体覆盖。
static SCAN_CACHE: Mutex<Option<(String, Instant, Value)>> = Mutex::new(None);

/// 缓存有效期（秒）。
pub const SCAN_CACHE_TTL_SECS: u64 = 60;

/// 清理 / 恢复 / 彻底删除后作废扫描缓存：页面紧接着的「重新扫描」必须拿到新数字，
/// 不能等 60 秒自然过期。
pub fn invalidate_scan_cache() {
    let mut cache = crate::modules::locks::recover(&SCAN_CACHE);
    *cache = None;
}

/// `GET /api/storage/scan`：总览 + 分类明细 + 会话 / 回收站计数。
pub fn scan(roots: &StorageRoots, variant: WbVariant) -> Value {
    let key = format!(
        "{}|{}|{}|{}",
        roots.store.to_string_lossy(),
        roots.client_cn.to_string_lossy(),
        roots.client_ai.to_string_lossy(),
        variant.as_str()
    );
    {
        let cache = crate::modules::locks::recover(&SCAN_CACHE);
        if let Some((cached_key, at, value)) = cache.as_ref() {
            if cached_key == &key && at.elapsed() < Duration::from_secs(SCAN_CACHE_TTL_SECS) {
                let mut value = value.clone();
                if let Some(object) = value.as_object_mut() {
                    object.insert("cached".to_string(), json!(true));
                }
                return value;
            }
        }
    }

    let value = build_scan(roots, variant);
    let mut cache = crate::modules::locks::recover(&SCAN_CACHE);
    *cache = Some((key, Instant::now(), value.clone()));
    value
}

fn build_scan(roots: &StorageRoots, variant: WbVariant) -> Value {
    let catalog = catalog::catalog(roots);
    let bundles = sessions::scan_sessions(&roots.store, &variant.data_root(), variant);
    let (trash_usage, trash_entries) = trash::usage(roots);

    let mut counts = json!({
        "live": 0, "softDeleted": 0, "orphan": 0, "redundant": 0,
        "present": 0, "partial": 0, "bodyMissing": 0,
    });
    let mut session_usage = Usage::default();
    for bundle in &bundles {
        let usage = bundle.total();
        session_usage.add(usage);
        match bundle.row_status {
            "live" => bump(&mut counts, "live"),
            "softDeleted" => bump(&mut counts, "softDeleted"),
            _ => bump(&mut counts, "orphan"),
        }
        if bundle.redundant {
            bump(&mut counts, "redundant");
        }
        match bundle.presence {
            "present" => bump(&mut counts, "present"),
            "partial" => bump(&mut counts, "partial"),
            _ => bump(&mut counts, "bodyMissing"),
        }
    }

    let totals = Usage {
        bytes: catalog.store_total.bytes + catalog.client_total.bytes,
        files: catalog.store_total.files + catalog.client_total.files,
    };

    json!({
        "generatedAt": catalog::now_ms(),
        "variant": variant.as_str(),
        "cached": false,
        "roots": [
            { "id": "store", "label": "工具数据", "path": roots.store.to_string_lossy(), "exists": roots.store.is_dir(),
              "bytes": catalog.store_total.bytes, "files": catalog.store_total.files },
            { "id": "cn", "label": "WorkBuddy（国内版）", "path": roots.client_cn.to_string_lossy(), "exists": roots.client_cn.is_dir(),
              "bytes": client_usage(&catalog, "cn").bytes, "files": client_usage(&catalog, "cn").files },
            { "id": "ai", "label": "WorkBuddy（国际版）", "path": roots.client_ai.to_string_lossy(), "exists": roots.client_ai.is_dir(),
              "bytes": client_usage(&catalog, "ai").bytes, "files": client_usage(&catalog, "ai").files },
        ],
        "totals": { "bytes": totals.bytes, "files": totals.files },
        "categories": catalog.categories.iter().map(|category| json!({
            "id": category.id,
            "group": category.group,
            "label": category.label,
            "path": category.path.to_string_lossy(),
            "bytes": category.usage.bytes,
            "files": category.usage.files,
            "units": category.units.len(),
            "deletable": category.deletable,
            "note": category.note,
            "protected": category.protected,
        })).collect::<Vec<_>>(),
        "sessions": {
            "counts": counts,
            "bytes": session_usage.bytes,
            "files": session_usage.files,
        },
        "trash": {
            "entries": trash_entries,
            "bytes": trash_usage.bytes,
            "files": trash_usage.files,
        },
        "clientRunning": client_running(variant),
        "warnings": [
            "回收站不释放空间：只有「彻底删除」才会真正释放。",
            "删除会话后云端同步可能重新拉回，页面会给出提示。",
        ],
    })
}

fn client_usage(catalog: &catalog::Catalog, id: &str) -> Usage {
    catalog
        .client_roots
        .iter()
        .find(|(root_id, _)| root_id == id)
        .map(|(_, usage)| *usage)
        .unwrap_or_default()
}

fn bump(counts: &mut Value, key: &str) {
    if let Some(object) = counts.as_object_mut() {
        let current = object.get(key).and_then(Value::as_i64).unwrap_or(0);
        object.insert(key.to_string(), json!(current + 1));
    }
}

/// `POST /api/storage/sessions`：全部会话资产包（含逐子项）。
pub fn sessions_payload(
    roots: &StorageRoots,
    variant: WbVariant,
    data_root: &std::path::Path,
) -> Value {
    let bundles = sessions::scan_sessions(&roots.store, data_root, variant);
    json!({
        "variant": variant.as_str(),
        "clientRunning": client_running(variant),
        "sessions": bundles.iter().map(|bundle| bundle.to_json()).collect::<Vec<_>>(),
    })
}

/// `GET /api/storage/trash`：回收站条目。
pub fn trash_payload(roots: &StorageRoots) -> Value {
    let (entries, problems) = trash::list_entries(roots);
    let (usage, _) = trash::usage(roots);
    json!({
        "entries": entries,
        "problems": problems,
        "bytes": usage.bytes,
        "files": usage.files,
    })
}
