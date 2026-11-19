//! 外部删除后的重校验：只接受 `session:<id>` / `category:<id>` / `trash:<id>`。
//!
//! 服务端解析路径，只查 exists / size / mtime，**不接受任意路径**——前端拿不到「探测文件系统」
//! 的能力，扫描结果里的路径也不会被回传成查询参数。页面 focus / visibility 变化时校验当前
//! 一页（≤100 项），由前端做 3 秒防抖。

use super::catalog;
use super::roots::{usage_of, StorageRoots};
use super::sessions::scan_sessions;
use super::trash;
use crate::modules::variant::WbVariant;
use serde_json::{json, Value};

pub const MAX_TARGETS: usize = 100;

pub fn revalidate(
    roots: &StorageRoots,
    variant: WbVariant,
    data_root: &std::path::Path,
    targets: &Value,
) -> Result<Value, String> {
    let list = targets
        .as_array()
        .ok_or_else(|| "targets 必须是数组".to_string())?;
    if list.len() > MAX_TARGETS {
        return Err(format!("一次最多校验 {MAX_TARGETS} 项"));
    }

    let mut categories: Option<Vec<catalog::Category>> = None;
    let mut bundles: Option<Vec<super::sessions::Bundle>> = None;
    let mut out = Vec::new();

    for target in list {
        let text = target
            .as_str()
            .ok_or_else(|| "targets 里的每一项都必须是字符串".to_string())?;
        let (kind, id) = text
            .split_once(':')
            .ok_or_else(|| format!("无法识别的目标：{text}"))?;
        match kind {
            "session" => {
                let all =
                    bundles.get_or_insert_with(|| scan_sessions(&roots.store, data_root, variant));
                let Some(bundle) = all.iter().find(|bundle| bundle.id == id) else {
                    out.push(json!({
                        "target": text,
                        "exists": false,
                        "presence": "missing",
                        "bytes": 0,
                        "files": 0,
                    }));
                    continue;
                };
                let usage = bundle.total();
                out.push(json!({
                    "target": text,
                    "exists": bundle.entries.iter().any(|entry| entry.exists),
                    "presence": bundle.presence,
                    "rowStatus": bundle.row_status,
                    "bytes": usage.bytes,
                    "files": usage.files,
                    "entries": bundle.entries.iter().map(|entry| json!({
                        "kind": entry.kind,
                        "exists": entry.exists,
                        "bytes": entry.usage.bytes,
                    })).collect::<Vec<_>>(),
                }));
            }
            "category" => {
                let all = categories.get_or_insert_with(|| {
                    let links = super::sessions::read_session_links(&roots.store);
                    catalog::catalog(roots, variant, &links).categories
                });
                let Some(category) = catalog::find(all, id) else {
                    out.push(json!({ "target": text, "exists": false, "bytes": 0, "files": 0 }));
                    continue;
                };
                let exists = category.path.exists();
                let usage = if exists {
                    usage_of(&category.path)
                } else {
                    super::roots::Usage::default()
                };
                out.push(json!({
                    "target": text,
                    "exists": exists,
                    "bytes": usage.bytes,
                    "files": usage.files,
                    "deletable": category.deletable,
                }));
            }
            "trash" => {
                let dir = trash::entry_dir(roots, id);
                let exists = dir.as_ref().map(|dir| dir.is_dir()).unwrap_or(false);
                let usage = dir
                    .as_ref()
                    .ok()
                    .map(|dir| usage_of(dir))
                    .unwrap_or_default();
                out.push(json!({
                    "target": text,
                    "exists": exists,
                    "bytes": usage.bytes,
                    "files": usage.files,
                }));
            }
            other => return Err(format!("不支持的目标类型：{other}")),
        }
    }

    Ok(json!({ "items": out }))
}
