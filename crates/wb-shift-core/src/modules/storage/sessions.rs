//! 会话资产包：一行会话对应的正文、附属目录与四类索引/快照。
//!
//! 删除单元是**单个子项**（正文 / 附属目录 / 索引 / 快照 / 变更），不是整个目录；
//! 「删除会话」额外删 DB 行与关联成员，二者语义在 [`super::clean`] 里分开处理。
//!
//! 分类口径来自 AUDIT §3 T3：不得按文件名判孤儿——无行文件里大量是现有会话的
//! `subagents/` 与 `tool-results/`，所以孤儿 = **任何档位库都无行且不被关联组引用**。

use super::redundancy::{apply_live_copy_counts, group_index_from_text};
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
    /// 冗余副本：**同一组内同一账号**还有别的**存活行**（跨账号的合法副本不算）。
    ///
    /// 口径见 [`super::redundancy`]：一个组里 A / B 各一份是正常状态，不是冗余；
    /// 同账号里另一份已被软删也不算（那份删不了，也不占「可清理条数」）。
    pub redundant: bool,
    pub group_id: Option<String>,
    /// **存活行口径**：本组内本账号还活着的会话行条数（`0` = 不在关联组里或本账号在本组
    /// 没有存活行）。由 [`super::redundancy::apply_live_copy_counts`] 统一写入，与
    /// `clean` 的 `redundantAll` 取舍一致。
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

fn asset_ids(root: &Path, sub: &str, extension: Option<&str>) -> HashSet<String> {
    let mut ids = HashSet::new();
    let Ok(entries) = std::fs::read_dir(root.join(sub)) else {
        return ids;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // 链接不算存在：删除会拒绝，展示上也不能计入体积（与 `Entry::exists` 同口径）。
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if super::roots::is_link(&meta) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        match extension {
            Some(ext) => {
                if let Some(stem) = name.strip_suffix(ext) {
                    ids.insert(stem.to_string());
                }
            }
            None => {
                ids.insert(name);
            }
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

/// 读关联主表原文（`session_links.json`）。
///
/// 同一份快照里 `catalog`（无引用基线）与 `sessions`（关联组）都要用它，因此由快照
/// 统一读一次再分发，避免同一文件解析两遍（第三轮第 6 条 6.5）。
pub fn read_session_links(store_root: &Path) -> String {
    std::fs::read_to_string(store_root.join("session_links.json")).unwrap_or_default()
}

/// 读一个档位的全部会话行（快照构建与 `scan_sessions` 共用；库缺失/被占用时返回空）。
pub fn read_rows_at(data_root: &Path) -> Vec<Map<String, Value>> {
    match open_db(&data_root.join("workbuddy.db")) {
        Some(conn) => read_rows(&conn),
        None => Vec::new(),
    }
}

/// 读一个档位的全部会话资产包（含无行的孤儿）。
///
/// `store_root` 只用于读关联主表（判断冗余副本）；`data_root` 是客户端数据根。
pub fn scan_sessions(store_root: &Path, data_root: &Path, variant: WbVariant) -> Vec<Bundle> {
    let rows = read_rows_at(data_root);
    scan_sessions_with_links(
        variant,
        data_root,
        store_root,
        rows,
        &read_session_links(store_root),
    )
}

/// 快照入口：关联主表原文由调用方读一次后传入（不再二次读盘）。
pub fn scan_sessions_with_links(
    variant: WbVariant,
    data_root: &Path,
    _store_root: &Path,
    rows: Vec<Map<String, Value>>,
    links: &str,
) -> Vec<Bundle> {
    let projects = project_index(data_root);
    let artifact_ids = asset_ids(data_root, "artifact-index", Some(".json"));
    let history_ids = asset_ids(data_root, "file-history", None);
    let changes_index_ids = asset_ids(data_root, "changes-index", Some(".json"));
    let changes_detail_ids = asset_ids(data_root, "changes-detail", None);
    let groups = group_index_from_text(links, variant);

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
    ids.extend(artifact_ids.iter().cloned());
    ids.extend(history_ids.iter().cloned());
    ids.extend(changes_index_ids.iter().cloned());
    ids.extend(changes_detail_ids.iter().cloned());
    ids.extend(groups.keys().cloned());

    // 只有「本机有行」或「本机有文件」的 id 才构成资产包：关联组成员可能指向其它账号
    // 本机的另一份副本，本机没有任何文件时它不占空间，也不该被算成「正文缺失」。
    let mut bundles: Vec<Bundle> = ids
        .into_iter()
        .filter_map(|id| {
            let row = row_by_id.get(&id);
            // 这里填的是**关联表口径**（成员记录数）的初值；构造完成后统一由
            // `apply_live_copy_counts` 用**存活行口径**重算 `live_copies` / `redundant`。
            // 两者会不一致：成员可能已被软删或正文/行已被客户端清掉，最终以存活行为准。
            let (group_id, copies) = match groups.get(&id) {
                Some(found) => (Some(found.group_id.clone()), found.same_uid_copies),
                None => (None, 0),
            };
            let bundle = build_bundle(
                variant,
                data_root,
                &projects,
                &id,
                row,
                group_id,
                copies,
                &artifact_ids,
                &history_ids,
                &changes_index_ids,
                &changes_detail_ids,
            );
            let present = bundle.entries.iter().any(|entry| entry.exists);
            (present || bundle.row.is_some()).then_some(bundle)
        })
        .collect();

    // 统一按存活行口径重算副本数与冗余标记：列表标记、冗余筛选与一键清理必须同一个口径，
    // 否则会出现「按钮说有 1 条可清理、点下去报没有冗余副本」（见 `super::redundancy`）。
    apply_live_copy_counts(&mut bundles);

    bundles.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    bundles
}

/// 一个会话资产包的 6 个子项（删除粒度）。
///
/// 索引类子项的存在性来自 `read_dir` 得到的 id 集合：不再为每个会话的 6 个 entry
/// 各做一次 `symlink_metadata`（不存在也 stat），只有确实存在的子项才 `usage_of`
/// 一次（第三轮第 6 条 6.5「entry 去掉双 stat」）。
#[allow(clippy::too_many_arguments)]
fn build_bundle(
    variant: WbVariant,
    data_root: &Path,
    projects: &ProjectIndex,
    id: &str,
    row: Option<&Map<String, Value>>,
    group_id: Option<String>,
    live_copies: usize,
    artifact_ids: &HashSet<String>,
    history_ids: &HashSet<String>,
    changes_index_ids: &HashSet<String>,
    changes_detail_ids: &HashSet<String>,
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
        entry(
            "body",
            "会话正文",
            projects.bodies.get(id).cloned(),
            projects.bodies.contains_key(id),
        ),
        entry(
            "sidecar",
            "附属目录",
            projects.sidecars.get(id).cloned(),
            projects.sidecars.contains_key(id),
        ),
        entry(
            "artifact-index",
            "产物索引",
            Some(data_root.join("artifact-index").join(format!("{id}.json"))),
            artifact_ids.contains(id),
        ),
        entry(
            "file-history",
            "文件历史",
            Some(data_root.join("file-history").join(id)),
            history_ids.contains(id),
        ),
        entry(
            "changes-index",
            "变更索引",
            Some(data_root.join("changes-index").join(format!("{id}.json"))),
            changes_index_ids.contains(id),
        ),
        entry(
            "changes-detail",
            "变更明细",
            Some(data_root.join("changes-detail").join(id)),
            changes_detail_ids.contains(id),
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
        // 初值：同账号同组多份才算冗余（跨账号各一份是正常状态，见 `super::redundancy`）。
        // 扫描结束后 `apply_live_copy_counts` 会按「存活行口径」覆盖这两个字段。
        redundant: live_copies > 1,
        group_id,
        live_copies,
        entries,
        row: row.cloned(),
        variant: variant.as_str().to_string(),
    }
}

/// 构造一个资产包子项。
///
/// `present` 由调用方给出（正文 / 附属目录来自一次 `read_dir` 的索引，四类索引来自
/// `read_dir` 得到的 id 集合）；只有 `present` 时才 `usage_of` 走盘，避免「不存在也 stat」。
fn entry(kind: &'static str, label: &str, path: Option<PathBuf>, present: bool) -> Entry {
    let path = path.unwrap_or_default();
    let usage = if present {
        usage_of(&path)
    } else {
        Usage::default()
    };
    Entry {
        kind,
        label: label.to_string(),
        path,
        usage,
        exists: present,
    }
}
