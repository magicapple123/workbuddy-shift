//! 会话资产包：一行会话对应的正文、附属目录与四类索引/快照。
//!
//! 删除单元是**单个子项**（正文 / 附属目录 / 索引 / 快照 / 变更），不是整个目录；
//! 「删除会话」额外删 DB 行与关联成员，二者语义在 [`super::clean`] 里分开处理。
//!
//! 分类口径来自 AUDIT §3 T3：不得按文件名判孤儿——无行文件里大量是现有会话的
//! `subagents/` 与 `tool-results/`，所以孤儿 = **任何档位库都无行且不被关联组引用**。

use super::roots::{usage_of, Usage};
use crate::modules::variant::WbVariant;
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 资产包子项（删除粒度）。
#[derive(Clone, Debug)]
pub struct Entry {
    pub kind: &'static str,
    pub label: String,
    pub path: PathBuf,
    pub usage: Usage,
    pub exists: bool,
}

/// 一条会话的资产包与状态。
#[derive(Clone, Debug)]
pub struct Bundle {
    pub id: String,
    pub uid: String,
    pub title: String,
    pub cwd: String,
    pub updated_at: i64,
    /// `live` / `softDeleted` / `none`（无行）。
    pub row_status: &'static str,
    /// `present` / `partial` / `bodyMissing` / `orphan`。
    pub presence: &'static str,
    pub redundant: bool,
    pub group_id: Option<String>,
    pub live_copies: usize,
    pub entries: Vec<Entry>,
    /// DB 行全列快照（删行前写进回收站 manifest；无行时为 None）。
    pub row: Option<Map<String, Value>>,
    pub variant: String,
}

impl Bundle {
    pub fn total(&self) -> Usage {
        self.entries
            .iter()
            .fold(Usage::default(), |mut acc, entry| {
                acc.add(entry.usage);
                acc
            })
    }

    pub fn has_body(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind == "body" && entry.exists)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "uid": self.uid,
            "title": self.title,
            "cwd": self.cwd,
            "updatedAt": self.updated_at,
            "variant": self.variant,
            "rowStatus": self.row_status,
            "presence": self.presence,
            "redundant": self.redundant,
            "groupId": self.group_id,
            "liveCopies": self.live_copies,
            "bytes": self.total().bytes,
            "files": self.total().files,
            "entries": self.entries.iter().map(|entry| json!({
                "kind": entry.kind,
                "label": entry.label,
                "path": entry.path.to_string_lossy(),
                "bytes": entry.usage.bytes,
                "files": entry.usage.files,
                "exists": entry.exists,
            })).collect::<Vec<_>>(),
        })
    }
}

/// `projects/` 的一次扫描结果：正文文件与同名附属目录。
#[derive(Default)]
struct ProjectIndex {
    bodies: HashMap<String, PathBuf>,
    sidecars: HashMap<String, PathBuf>,
}

fn project_index(data_root: &Path) -> ProjectIndex {
    let mut index = ProjectIndex::default();
    let projects = data_root.join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return index;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if super::roots::is_link(&meta) {
            continue;
        }
        if meta.is_dir() {
            let Ok(children) = std::fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                let child_path = child.path();
                let Ok(child_meta) = std::fs::symlink_metadata(&child_path) else {
                    continue;
                };
                if super::roots::is_link(&child_meta) {
                    continue;
                }
                let name = child.file_name().to_string_lossy().to_string();
                if child_meta.is_file() {
                    if let Some(stem) = name.strip_suffix(".jsonl") {
                        index.bodies.insert(stem.to_string(), child_path);
                    }
                } else if child_meta.is_dir() {
                    index.sidecars.insert(name, child_path);
                }
            }
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        if let Some(stem) = name.as_deref().and_then(|n| n.strip_suffix(".jsonl")) {
            index.bodies.insert(stem.to_string(), path);
        }
    }
    index
}

fn asset_ids(root: &Path, sub: &str, extension: Option<&str>) -> Vec<String> {
    let mut ids = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join(sub)) else {
        return ids;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(ext) = extension {
            if let Some(stem) = name.strip_suffix(ext) {
                ids.push(stem.to_string());
            }
        } else {
            ids.push(name);
        }
    }
    ids
}

/// 只读打开库（`busy_timeout` 与 `session::query` 同口径；本模块不复用它的私有 helper，
/// 避免与 T1 对 `session/query.rs` 的改动相互影响）。
fn open_db(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    let conn =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    Some(conn)
}

/// 侧栏同款展示名：优先 `custom_title`，否则 `title`，都为空时给占位。
fn display_title(title: Option<String>, custom_title: Option<String>) -> String {
    let pick = |value: Option<String>| {
        value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    pick(custom_title)
        .or_else(|| pick(title))
        .unwrap_or_else(|| "(无标题)".to_string())
}

/// 读 `sessions` 表的全部行（`SELECT *`，列名动态）。
fn read_rows(conn: &Connection) -> Vec<Map<String, Value>> {
    let Ok(mut stmt) = conn.prepare("SELECT * FROM sessions") else {
        return Vec::new();
    };
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let mut map = Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = row.get::<_, SqlValue>(index).unwrap_or(SqlValue::Null);
            map.insert(name.clone(), sql_to_json(value));
        }
        out.push(map);
    }
    out
}

fn sql_to_json(value: SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(number) => json!(number),
        SqlValue::Real(number) => json!(number),
        SqlValue::Text(text) => Value::String(text),
        SqlValue::Blob(bytes) => Value::String(format!("<blob {} bytes>", bytes.len())),
    }
}

fn text_of(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn int_of(row: &Map<String, Value>, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// 关联组索引：sessionId → (groupId, 组内该档位成员数)。
fn group_index(store_root: &Path, variant: WbVariant) -> HashMap<String, (String, usize)> {
    let file = store_root.join("session_links.json");
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(&file) else {
        return out;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    let Some(groups) = json.get("groups").and_then(Value::as_array) else {
        return out;
    };
    for group in groups {
        let group_id = group
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(members) = group.get("members").and_then(Value::as_array) else {
            continue;
        };
        let same_variant: Vec<&Value> = members
            .iter()
            .filter(|member| {
                member.get("variant").and_then(Value::as_str) == Some(variant.as_str())
            })
            .filter(|member| member.get("state").and_then(Value::as_str) != Some("removed"))
            .collect();
        let count = same_variant.len();
        for member in same_variant {
            if let Some(session_id) = member.get("sessionId").and_then(Value::as_str) {
                out.insert(session_id.to_string(), (group_id.clone(), count));
            }
        }
    }
    out
}

/// 读一个档位的全部会话资产包（含无行的孤儿）。
///
/// `store_root` 只用于读关联主表（判断冗余副本）；`data_root` 是客户端数据根。
pub fn scan_sessions(store_root: &Path, data_root: &Path, variant: WbVariant) -> Vec<Bundle> {
    let db = data_root.join("workbuddy.db");
    let rows = match open_db(&db) {
        Some(conn) => read_rows(&conn),
        None => Vec::new(),
    };
    scan_sessions_at(variant, data_root, store_root, rows)
}

/// 可测实现：显式给数据根、关联主表根与行集合（夹具不碰真实客户端数据）。
pub fn scan_sessions_at(
    variant: WbVariant,
    data_root: &Path,
    store_root: &Path,
    rows: Vec<Map<String, Value>>,
) -> Vec<Bundle> {
    let projects = project_index(data_root);
    let artifact_ids = asset_ids(data_root, "artifact-index", Some(".json"));
    let history_ids = asset_ids(data_root, "file-history", None);
    let changes_index_ids = asset_ids(data_root, "changes-index", Some(".json"));
    let changes_detail_ids = asset_ids(data_root, "changes-detail", None);
    let groups = group_index(store_root, variant);

    let mut row_by_id: HashMap<String, Map<String, Value>> = HashMap::new();
    for row in &rows {
        let id = text_of(row, "id");
        if !id.is_empty() {
            row_by_id.insert(id, row.clone());
        }
    }

    let mut ids: HashSet<String> = row_by_id.keys().cloned().collect();
    ids.extend(projects.bodies.keys().cloned());
    ids.extend(projects.sidecars.keys().cloned());
    ids.extend(artifact_ids);
    ids.extend(history_ids);
    ids.extend(changes_index_ids);
    ids.extend(changes_detail_ids);
    ids.extend(groups.keys().cloned());

    // 只有「本机有行」或「本机有文件」的 id 才构成资产包：关联组成员可能指向其它账号
    // 本机的另一份副本，本机没有任何文件时它不占空间，也不该被算成「正文缺失」。
    let mut bundles: Vec<Bundle> = ids
        .into_iter()
        .filter_map(|id| {
            let row = row_by_id.get(&id);
            let (group_id, copies) = match groups.get(&id) {
                Some((group, count)) => (Some(group.clone()), *count),
                None => (None, 0),
            };
            let bundle = build_bundle(variant, data_root, &projects, &id, row, group_id, copies);
            let present = bundle.entries.iter().any(|entry| entry.exists);
            (present || bundle.row.is_some()).then_some(bundle)
        })
        .collect();

    bundles.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    bundles
}

fn build_bundle(
    variant: WbVariant,
    data_root: &Path,
    projects: &ProjectIndex,
    id: &str,
    row: Option<&Map<String, Value>>,
    group_id: Option<String>,
    live_copies: usize,
) -> Bundle {
    let deleted_at = row
        .map(|row| int_of(row, "deleted_at"))
        .filter(|value| *value != 0);
    let row_status = match (row.is_some(), deleted_at) {
        (false, _) => "none",
        (true, Some(value)) if value > 0 => "softDeleted",
        _ => "live",
    };

    let entries = vec![
        entry("body", "会话正文", projects.bodies.get(id).cloned()),
        entry("sidecar", "附属目录", projects.sidecars.get(id).cloned()),
        entry(
            "artifact-index",
            "产物索引",
            Some(data_root.join("artifact-index").join(format!("{id}.json"))),
        ),
        entry(
            "file-history",
            "文件历史",
            Some(data_root.join("file-history").join(id)),
        ),
        entry(
            "changes-index",
            "变更索引",
            Some(data_root.join("changes-index").join(format!("{id}.json"))),
        ),
        entry(
            "changes-detail",
            "变更明细",
            Some(data_root.join("changes-detail").join(id)),
        ),
    ];

    let present = entries.iter().filter(|entry| entry.exists).count();
    let has_body = entries
        .iter()
        .any(|entry| entry.kind == "body" && entry.exists);
    let presence = if row.is_none() {
        // 无行：本机只剩文件（含只有附属目录 / 索引的情况）。
        "orphan"
    } else if !has_body {
        "bodyMissing"
    } else if present == entries.len() {
        "present"
    } else {
        "partial"
    };

    let title = row
        .map(|row| {
            display_title(
                row.get("title").and_then(Value::as_str).map(str::to_string),
                row.get("custom_title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .unwrap_or_else(|| "(仅残留文件)".to_string());

    Bundle {
        id: id.to_string(),
        uid: row.map(|row| text_of(row, "user_id")).unwrap_or_default(),
        title,
        cwd: row.map(|row| text_of(row, "cwd")).unwrap_or_default(),
        updated_at: row.map(|row| int_of(row, "updated_at")).unwrap_or(0),
        row_status,
        presence,
        redundant: live_copies > 1,
        group_id,
        live_copies,
        entries,
        row: row.cloned(),
        variant: variant.as_str().to_string(),
    }
}

fn entry(kind: &'static str, label: &str, path: Option<PathBuf>) -> Entry {
    let path = path.unwrap_or_default();
    let meta = std::fs::symlink_metadata(&path).ok();
    // 符号链接不算存在：删除时会拒绝，展示上也不能计入体积。
    let exists = match &meta {
        Some(meta) => !super::roots::is_link(meta),
        None => false,
    };
    let usage = if exists {
        usage_of(&path)
    } else {
        Usage::default()
    };
    Entry {
        kind,
        label: label.to_string(),
        path,
        usage,
        exists,
    }
}
