//! 存储管理：扫描、按会话 / 分类清理、回收站恢复、跳转定位与外部删除重校验。
//!
//! 边界（AUDIT §3 T3）：
//! - 白名单根只有 `~/.workbuddy-shift`、`~/.workbuddy`（子目录）、`~/.workbuddy-ai`；
//! - 破坏性动作一律先进回收站（`trash/<ts>/`），彻底删除是独立按钮；
//! - 会话类操作要求客户端退出；删 DB 行前整库备份 + 整行快照；
//! - `blobs` 不整目录删；`binaries` / `plugins` / `app` / `storage` 不可删；
//! - 不引入文件监听：外部删除由页面 focus / visibility 触发
//!   [`revalidate::revalidate`]（3 s 防抖，只校验当前一页）。
//!
//! 加载方式（第三轮第 6 条）：总览 / 会话资产 / 回收站共享一份 60 秒
//! [`snapshot::Snapshot`]，见 `snapshot` 模块的文档。

mod catalog;
mod catalog_client;
mod clean;
mod protected;
mod redundancy;
mod revalidate;
mod reveal;
mod roots;
mod sessions;
mod snapshot;
mod trash;

pub use clean::{clean, client_running, CleanContext};
pub use protected::CACHE_TTL_SECS as PROTECTED_CACHE_TTL_SECS;
pub use revalidate::revalidate;
pub use reveal::{
    resolve_target as resolve_reveal_target, reveal, RevealTarget, SCOPE_CWD, SCOPE_STORAGE,
};
pub use roots::{usage_of, StorageRoots, Usage};
pub use sessions::scan_sessions;
pub use snapshot::SNAPSHOT_TTL_SECS;
pub use trash::{list_entries, purge, restore_entry};

#[cfg(test)]
mod tests;

use crate::modules::variant::WbVariant;
use serde_json::{json, Value};
use std::path::Path;

/// 清理 / 恢复 / 彻底删除后作废整份快照：页面紧接着的「重新扫描」必须拿到新数字，
/// 不能等 60 秒自然过期。旧名保留，`clean` / `trash` 的调用点不需要跟着改。
pub fn invalidate_scan_cache() {
    snapshot::invalidate();
}

/// `GET /api/storage/scan`：总览 + 分类明细 + 会话 / 回收站计数。
pub fn scan(roots: &StorageRoots, variant: WbVariant) -> Value {
    scan_with(roots, variant, false)
}

/// 同上；`refresh = true` 是「重新扫描」：作废服务端快照与受保护项缓存后重算。
///
/// 60 秒缓存的意义是「切页签不重扫」，不能变成「按钮点不动」。
pub fn scan_with(roots: &StorageRoots, variant: WbVariant, refresh: bool) -> Value {
    scan_at(roots, variant, &variant.data_root(), refresh)
}

/// 显式给数据根（单测 / 沙箱入口）：与 [`sessions_payload`] 同款参数。
///
/// 快照键包含数据根，因此「总览」与「会话资产」必须用同一个 `data_root` 才会共享
/// 同一份快照；生产路径两者都是 `variant.data_root()`。
pub fn scan_at(roots: &StorageRoots, variant: WbVariant, data_root: &Path, refresh: bool) -> Value {
    if refresh {
        snapshot::invalidate();
        // 受保护项也要重算：只把 `at` 归零，数字留着（页面不会突然掉到 0）。
        if let Err(error) = protected::invalidate(&roots.store, variant) {
            eprintln!("[storage] 作废受保护项缓存失败：{error}");
        }
    }
    let (snapshot, reused) = snapshot::ensure(roots, variant, data_root);
    let mut value = scan_value(roots, &snapshot);
    if let Some(object) = value.as_object_mut() {
        object.insert("cached".to_string(), json!(reused));
    }
    value
}

fn scan_value(roots: &StorageRoots, snapshot: &snapshot::Snapshot) -> Value {
    let catalog = &snapshot.catalog;
    let session_usage = snapshot.session_usage();

    let mut counts = json!({
        "live": 0, "softDeleted": 0, "orphan": 0, "redundant": 0,
        "present": 0, "partial": 0, "bodyMissing": 0,
    });
    for bundle in &snapshot.bundles {
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
        "generatedAt": snapshot.generated_at,
        "variant": snapshot.variant.as_str(),
        "cached": false,
        "roots": [
            { "id": "store", "label": "工具数据", "path": roots.store.to_string_lossy(),
              "exists": roots.store.is_dir(),
              "bytes": catalog.store_total.bytes, "files": catalog.store_total.files },
            { "id": "cn", "label": "WorkBuddy（国内版）", "path": roots.client_cn.to_string_lossy(),
              "exists": roots.client_cn.is_dir(),
              "bytes": client_usage(catalog, "cn").bytes, "files": client_usage(catalog, "cn").files },
            { "id": "ai", "label": "WorkBuddy（国际版）", "path": roots.client_ai.to_string_lossy(),
              "exists": roots.client_ai.is_dir(),
              "bytes": client_usage(catalog, "ai").bytes, "files": client_usage(catalog, "ai").files },
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
            "entries": snapshot.trash_entries,
            "bytes": snapshot.trash_usage.bytes,
            "files": snapshot.trash_usage.files,
        },
        "clientRunning": snapshot.client_running,
        "protectedPending": catalog.protected_pending,
        "protectedFilling": protected::is_filling(),
        "timings": snapshot.timings.to_json(),
        "warnings": warnings(catalog.protected_pending),
    })
}

fn warnings(pending: bool) -> Vec<String> {
    let mut out = vec![
        "回收站不释放空间：只有「彻底删除」才会真正释放。".to_string(),
        "删除会话后云端同步可能重新拉回，页面会给出提示。".to_string(),
        "四张总览卡不可相加：会话资产与回收站已包含在对应根内。".to_string(),
    ];
    if pending {
        out.push(
            "受保护大目录（运行时 / 插件 / 图片附件 / 账号目录等）正在后台计量，总量暂为部分和。"
                .to_string(),
        );
    }
    out
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
pub fn sessions_payload(roots: &StorageRoots, variant: WbVariant, data_root: &Path) -> Value {
    let (snapshot, reused) = snapshot::ensure(roots, variant, data_root);
    json!({
        "variant": variant.as_str(),
        "cached": reused,
        "clientRunning": snapshot.client_running,
        // 「一键清理冗余副本」实际会删掉的条数：与 `clean` 的 `redundantAll` 同源同口径。
        // 前端按钮上的数字取这里，避免自己算出一个「点了会报错」的数字。
        "redundantCleanable": redundancy::cleanable_redundant_total(&snapshot.bundles),
        "sessions": snapshot.bundles.iter().map(|bundle| bundle.to_json()).collect::<Vec<_>>(),
    })
}

/// `GET /api/storage/trash`：回收站条目。
///
/// 条目清单每次都从清单文件读（恢复后必须立刻消失），体积来自同一份快照，
/// 因此同一 TTL 内回收站目录只走盘一次。
pub fn trash_payload(roots: &StorageRoots, variant: WbVariant, data_root: &Path) -> Value {
    let (snapshot, reused) = snapshot::ensure(roots, variant, data_root);
    let (entries, problems) = trash::list_entries(roots);
    json!({
        "cached": reused,
        "entries": entries,
        "problems": problems,
        "bytes": snapshot.trash_usage.bytes,
        "files": snapshot.trash_usage.files,
        "timings": snapshot.timings.to_json(),
    })
}
