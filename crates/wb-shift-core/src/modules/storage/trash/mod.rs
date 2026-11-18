//! 回收站：`trash/<yyyyMMdd-HHmmss>/manifest.json` + `files/<原相对路径>`。
//!
//! - 同卷用 `rename`；跨卷退化为「复制 → 校验大小 → 删除源」。
//! - 删 DB 行时把**整行快照（全部列 + 原值）**写进 manifest，恢复按原 id 重新插入；
//!   行已存在时拒绝，不覆盖。
//! - 恢复拒绝覆盖已存在目标；行恢复失败时文件仍可单独恢复，结果分项报告。

use super::roots::{ensure_within, StorageRoots, Usage};
use crate::modules::config::atomic_write;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod move_;
#[cfg(test)]
mod tests;

use move_::remove_source;
pub(crate) use move_::{copy_recursive, move_unit, new_entry_id};

pub const MANIFEST_NAME: &str = "manifest.json";
pub const TRASH_VERSION: i64 = 1;

/// 回收站里的一个文件 / 目录条目。
#[derive(Clone, Debug)]
pub struct TrashItem {
    pub root_id: String,
    pub original_path: String,
    pub stored_path: String,
    pub usage: Usage,
    pub mtime: i64,
    pub category: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub result: String,
}

/// 删除 DB 行前的整行快照。
#[derive(Clone, Debug)]
pub struct RowSnapshot {
    pub variant: String,
    pub table: String,
    /// 行所属数据库的绝对路径；恢复时按它定位，并复核仍在白名单根内。
    pub db_path: String,
    pub row: Map<String, Value>,
}

#[derive(Clone, Debug, Default)]
pub struct TrashEntry {
    pub id: String,
    pub created_at: i64,
    pub items: Vec<TrashItem>,
    pub rows: Vec<RowSnapshot>,
    pub db_backups: Vec<String>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn entry_dir(roots: &StorageRoots, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
        return Err("回收站条目 id 非法".to_string());
    }
    let dir = roots.trash_root().join(id);
    ensure_within(&roots.trash_root(), &dir)?;
    Ok(dir)
}

/// 写 manifest（原子写；失败即视为本次操作不可恢复，调用方必须报错）。
pub fn write_manifest(roots: &StorageRoots, entry: &TrashEntry) -> Result<PathBuf, String> {
    let dir = entry_dir(roots, &entry.id)?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建回收站条目失败：{error}"))?;
    let manifest = dir.join(MANIFEST_NAME);
    let payload = json!({
        "version": TRASH_VERSION,
        "id": entry.id,
        "createdAt": entry.created_at,
        "items": entry.items.iter().map(|item| json!({
            "rootId": item.root_id,
            "originalPath": item.original_path,
            "storedPath": item.stored_path,
            "bytes": item.usage.bytes,
            "files": item.usage.files,
            "mtime": item.mtime,
            "category": item.category,
            "sessionId": item.session_id,
            "kind": item.kind,
            "result": item.result,
        })).collect::<Vec<_>>(),
        "rows": entry.rows.iter().map(|snapshot| json!({
            "variant": snapshot.variant,
            "table": snapshot.table,
            "dbPath": snapshot.db_path,
            "row": snapshot.row,
        })).collect::<Vec<_>>(),
        "dbBackups": entry.db_backups,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?;
    atomic_write(&manifest, &text).map_err(|error| format!("写入回收站清单失败：{error}"))?;
    Ok(manifest)
}

/// 列出回收站条目（按 id 倒序；损坏的 manifest 跳过并记入 `problems`）。
pub fn list_entries(roots: &StorageRoots) -> (Vec<Value>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let dir = roots.trash_root();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (out, problems);
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        match read_manifest(roots, &id) {
            Ok(value) => out.push(value),
            Err(error) => problems.push(format!("{id}: {error}")),
        }
    }
    out.sort_by(|a, b| {
        b.get("id")
            .and_then(Value::as_str)
            .cmp(&a.get("id").and_then(Value::as_str))
    });
    (out, problems)
}

fn read_manifest(roots: &StorageRoots, id: &str) -> Result<Value, String> {
    let path = entry_dir(roots, id)?.join(MANIFEST_NAME);
    let text = std::fs::read_to_string(&path).map_err(|error| format!("读取清单失败：{error}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("清单不是合法 JSON：{error}"))?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let bytes = item.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                    let files = item.get("files").and_then(Value::as_u64).unwrap_or(0);
                    let stored = item
                        .get("storedPath")
                        .and_then(Value::as_str)
                        .and_then(|stored| {
                            item.get("originalPath")
                                .and_then(Value::as_str)
                                .map(|original| {
                                    restore_state(Path::new(&stored), Path::new(original))
                                })
                        })
                        .unwrap_or("missing");
                    json!({
                        "rootId": item.get("rootId").cloned().unwrap_or(Value::Null),
                        "originalPath": item.get("originalPath").cloned().unwrap_or(Value::Null),
                        "bytes": bytes,
                        "files": files,
                        "mtime": item.get("mtime").cloned().unwrap_or(Value::Null),
                        "category": item.get("category").cloned().unwrap_or(Value::Null),
                        "sessionId": item.get("sessionId").cloned().unwrap_or(Value::Null),
                        "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                        "result": item.get("result").cloned().unwrap_or(Value::Null),
                        "state": stored,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let totals = items.iter().fold((0_u64, 0_u64), |(bytes, files), item| {
        (
            bytes + item.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            files + item.get("files").and_then(Value::as_u64).unwrap_or(0),
        )
    });
    let row_count = value
        .get("rows")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Ok(json!({
        "id": id,
        "createdAt": value.get("createdAt").cloned().unwrap_or(Value::Null),
        "bytes": totals.0,
        "files": totals.1,
        "rows": row_count,
        "dbBackups": value.get("dbBackups").cloned().unwrap_or(json!([])),
        "items": items,
        "warnings": value.get("warnings").cloned().unwrap_or(json!([])),
    }))
}

fn restore_state(stored: &Path, original: &Path) -> &'static str {
    if !stored.exists() {
        "missing"
    } else if original.exists() {
        "conflict"
    } else {
        "restorable"
    }
}

/// 恢复一个回收站条目。返回分项结果，不因单项失败中断其它项。
pub fn restore_entry(roots: &StorageRoots, id: &str) -> Result<Value, String> {
    let dir = entry_dir(roots, id)?;
    let text = std::fs::read_to_string(dir.join(MANIFEST_NAME))
        .map_err(|error| format!("读取清单失败：{error}"))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|error| format!("清单不是合法 JSON：{error}"))?;

    let mut restored_files = 0_usize;
    let mut restored_rows = 0_usize;
    let mut failures: Vec<Value> = Vec::new();

    for item in manifest
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(stored) = item.get("storedPath").and_then(Value::as_str) else {
            continue;
        };
        let Some(original) = item.get("originalPath").and_then(Value::as_str) else {
            continue;
        };
        let stored = PathBuf::from(stored);
        let original = PathBuf::from(original);
        let root = roots.by_id(
            item.get("rootId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let Some(root) = root else {
            failures.push(json!({"path": original, "error": "未知的白名单根"}));
            continue;
        };
        if ensure_within(&root, &original).is_err() {
            failures.push(json!({"path": original, "error": "原路径已不在白名单根内"}));
            continue;
        }
        if original.exists() {
            failures.push(json!({"path": original, "error": "目标已存在，拒绝覆盖"}));
            continue;
        }
        if !stored.exists() {
            failures.push(json!({"path": original, "error": "回收站内副本缺失"}));
            continue;
        }
        let result = (|| -> Result<(), String> {
            if let Some(parent) = original.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建父目录失败：{error}"))?;
            }
            if std::fs::rename(&stored, &original).is_ok() {
                return Ok(());
            }
            copy_recursive(&stored, &original)?;
            remove_source(&stored)
        })();
        match result {
            Ok(()) => restored_files += 1,
            Err(error) => failures.push(json!({"path": original, "error": error})),
        }
    }

    for snapshot in manifest
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match restore_row(roots, snapshot) {
            Ok(()) => restored_rows += 1,
            Err(error) => failures.push(
                json!({"row": snapshot.get("row").and_then(|r| r.get("id")), "error": error}),
            ),
        }
    }

    // 全部恢复成功才删除条目；否则保留剩余材料并回报。
    if failures.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    super::invalidate_scan_cache();
    Ok(json!({
        "id": id,
        "restoredFiles": restored_files,
        "restoredRows": restored_rows,
        "failures": failures,
        "purged": failures.is_empty(),
    }))
}

fn restore_row(roots: &StorageRoots, snapshot: &Value) -> Result<(), String> {
    let db = match snapshot.get("dbPath").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => PathBuf::from(text),
        _ => {
            let variant_text = snapshot
                .get("variant")
                .and_then(Value::as_str)
                .unwrap_or("cn");
            crate::modules::variant::WbVariant::parse(Some(variant_text))
                .data_root()
                .join("workbuddy.db")
        }
    };
    if !db.is_file() {
        return Err("目标数据库不存在".to_string());
    }
    let root = roots
        .root_containing(&db)
        .ok_or_else(|| "数据库不在白名单根内，拒绝恢复".to_string())?;
    ensure_within(&root, &db)?;
    let row = snapshot
        .get("row")
        .and_then(Value::as_object)
        .ok_or("行快照缺少字段")?;
    let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
    if id.is_empty() {
        return Err("行快照缺少 id".to_string());
    }
    let conn = Connection::open(&db).map_err(|error| format!("打开数据库失败：{error}"))?;
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .map_err(|error| format!("查询现有行失败：{error}"))?;
    if existing > 0 {
        return Err("行已存在，拒绝覆盖".to_string());
    }
    let columns: Vec<String> = row.keys().cloned().collect();
    let placeholders: Vec<String> = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect();
    let sql = format!(
        "INSERT INTO sessions ({}) VALUES ({})",
        columns
            .iter()
            .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders.join(", ")
    );
    let params: Vec<rusqlite::types::Value> = columns
        .iter()
        .map(|column| json_to_sql(row.get(column).unwrap_or(&Value::Null)))
        .collect();
    conn.execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|error| format!("插入行失败：{error}"))?;
    Ok(())
}

fn json_to_sql(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(flag) => rusqlite::types::Value::Integer(i64::from(*flag)),
        Value::Number(number) => number
            .as_i64()
            .map(rusqlite::types::Value::Integer)
            .or_else(|| number.as_f64().map(rusqlite::types::Value::Real))
            .unwrap_or(rusqlite::types::Value::Null),
        Value::String(text) => rusqlite::types::Value::Text(text.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

/// 彻底删除：`id` 为空表示清空回收站。
pub fn purge(roots: &StorageRoots, id: Option<&str>) -> Result<Value, String> {
    let target = match id {
        Some(id) => entry_dir(roots, id)?,
        None => roots.trash_root(),
    };
    if !target.exists() {
        return Ok(json!({"deleted": 0, "bytes": 0}));
    }
    let usage = super::roots::usage_of(&target);
    if let Some(id) = id {
        std::fs::remove_dir_all(entry_dir(roots, id)?)
            .map_err(|error| format!("彻底删除失败：{error}"))?;
    } else {
        for entry in std::fs::read_dir(&target)
            .map_err(|error| format!("读回收站失败：{error}"))?
            .flatten()
        {
            if entry.path().is_dir() {
                std::fs::remove_dir_all(entry.path())
                    .map_err(|error| format!("彻底删除失败：{error}"))?;
            } else {
                std::fs::remove_file(entry.path())
                    .map_err(|error| format!("彻底删除失败：{error}"))?;
            }
        }
    }
    super::invalidate_scan_cache();
    Ok(json!({"deleted": 1, "bytes": usage.bytes, "files": usage.files}))
}

/// 回收站总量（扫描总览用）。
pub fn usage(roots: &StorageRoots) -> (Usage, usize) {
    let dir = roots.trash_root();
    let usage = super::roots::usage_of(&dir);
    let entries = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    (usage, entries)
}
