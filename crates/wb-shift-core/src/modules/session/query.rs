//! 会话查询与数据库辅助：会话列表、正文定位、账号身份读取与 SQLite 打开 / 表列探测。
//!
//! 本模块只读客户端数据（`workbuddy.db` 与 `projects/` 正文），不做任何写入；
//! 写侧在 `copy` / `exec`。身份判定以 uid 为准（accountId 仅作展示）。

use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::modules::account;
use crate::modules::auth_file;
use crate::modules::variant::WbVariant;

use super::paths::SessionPaths;

/// 打开数据库并设置 busy_timeout（对照 Python `sqlite3.connect(timeout=5)`）。
pub(crate) fn open_db(path: &Path, read_only: bool) -> Option<Connection> {
    let conn = if read_only {
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?
    } else {
        Connection::open(path).ok()?
    };
    let _ = conn.busy_timeout(Duration::from_secs(5));
    Some(conn)
}

pub(crate) fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        == 1
}

pub(super) fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(iter) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = iter.flatten().collect();
    names.iter().any(|name| name == column)
}

/// 当前认证账号的 uid（该档位认证文件的 account.uid）。
pub fn current_user_uid(variant: WbVariant) -> Option<String> {
    current_user_uid_at(&variant.auth_file_path())
}

/// 从指定登录态文件读取 uid（单测注入临时文件用）。
pub fn current_user_uid_at(auth_file: &Path) -> Option<String> {
    let auth = auth_file::read_auth_file_at(auth_file)?;
    auth.get("account")
        .and_then(|a| a.get("uid"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 列出某账号未删除的会话（workbuddy.db sessions 表，db 为准）。
///
/// `title` 为 WorkBuddy 侧栏同款展示名；`isPlayground` 对应侧栏「任务」，
/// 其余按空间（`spaces.name`，缺失时前端回退 `cwd` 最后一段）归入「空间」。
pub fn list_sessions_for_user(variant: WbVariant, uid: &str) -> Value {
    list_sessions_for_user_at(&SessionPaths::for_variant(variant), uid)
}

/// 指定账号名下的会话列表（会话管理页的源账号视角）。
///
/// 与 `GET /api/sessions` 的返回同形（`sessions / current / variant`），但来源是
/// 显式账号而非登录态；账号缺 uid 时返回空列表 + `current: null`（与现有容错一致）。
pub fn list_sessions_for_account(account: &Value) -> Value {
    let variant = account::variant_of(account);
    let uid = account_uid(account);
    if uid.is_empty() {
        return json!({ "sessions": [], "current": Value::Null, "variant": variant.as_str() });
    }
    json!({
        "sessions": list_sessions_for_user(variant, &uid),
        "current": uid,
        "variant": variant.as_str(),
    })
}

pub(super) fn list_sessions_for_user_at(paths: &SessionPaths, uid: &str) -> Value {
    let db = paths.workbuddy_db();
    if !db.is_file() {
        return json!([]);
    }
    let Some(conn) = open_db(&db, true) else {
        return json!([]);
    };
    if !table_exists(&conn, "sessions") {
        return json!([]);
    }
    // 列组合按探测结果**拼装**而不是枚举：`custom_title` / `is_playground` 各自缺省为
    // 常量表达式，空间字段（`sessions.space_id` + `spaces` 表）再拼一段可选列 + JOIN。
    // 这样历史库的 4 种列组合行为与改造前逐字一致，而不会与空间可用性相乘成 8 条 SQL。
    let custom_expr = if column_exists(&conn, "sessions", "custom_title") {
        "s.custom_title"
    } else {
        "NULL"
    };
    let playground_expr = if column_exists(&conn, "sessions", "is_playground") {
        "s.is_playground"
    } else {
        "0"
    };
    let with_space = spaces_join_supported(&conn);
    let (space_columns, space_join) = if with_space {
        (
            ", s.space_id, sp.name",
            " LEFT JOIN spaces sp ON sp.id = s.space_id \
             AND sp.user_id = s.user_id AND sp.deleted_at = -1",
        )
    } else {
        (", NULL, NULL", "")
    };
    let sql = format!(
        "SELECT s.id, s.cwd, s.title, {custom_expr}, s.updated_at, {playground_expr}\
         {space_columns} FROM sessions s{space_join} \
         WHERE s.user_id = ?1 AND s.deleted_at IS NULL ORDER BY s.updated_at DESC"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return json!([]),
    };
    let rows = stmt.query_map([uid], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    });

    let mut sessions: Vec<Value> = Vec::new();
    // 一次扫出 `projects/` 下的正文 id，避免每行都重新遍历同一层目录
    // （189 个会话 × 数十个 workspace 的 stat 会占掉列表页相当一部分时间）。
    let present = project_session_ids(paths);
    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let (cid, cwd, title, custom_title, updated_at, is_playground, space_id, space_name) =
                r;
            let cid = cid.unwrap_or_default();
            let cwd = cwd.unwrap_or_default();
            if is_claw_workspace(&cwd) {
                continue;
            }
            let mut item = json!({
                "id": cid,
                "title": session_display_title(title, custom_title),
                "cwd": cwd,
                "updatedAt": updated_at.unwrap_or(0),
                "hasHistory": present.contains(&cid),
                "isPlayground": is_playground.unwrap_or(0) != 0,
            });
            // 只有真的 JOIN 过 `spaces` 才下发空间字段：老库（无 `spaces` 表 / 无
            // `space_id` 列）保持原返回形状，前端据此回退到按 cwd 分组。
            if with_space {
                let space_id = nonempty_text(space_id);
                let space_name = nonempty_text(space_name);
                if let Some(map) = item.as_object_mut() {
                    map.insert("spaceId".to_string(), json!(space_id));
                    // 空间行缺失 / 已删除（`deleted_at != -1`）时 JOIN 出 NULL，
                    // 由前端回退成 cwd 文件夹名。
                    map.insert("spaceName".to_string(), json!(space_name));
                }
            }
            sessions.push(item);
        }
    }
    json!(sessions)
}

/// `spaces` 表与 `sessions.space_id` 是否可用于关联（WorkBuddy 5.x 的新形状）。
///
/// 四个必需列都要探测：`spaces` 表在更早的库里不存在或列不全，缺任何一列都无法
/// 安全 JOIN（缺 `deleted_at` 会把已删空间当成有效空间），此时整体回退到「不返回
/// 空间字段」，前端按 cwd 分组——与改造前完全一致。
fn spaces_join_supported(conn: &Connection) -> bool {
    table_exists(conn, "spaces")
        && column_exists(conn, "sessions", "space_id")
        && ["id", "user_id", "name", "deleted_at"]
            .iter()
            .all(|column| column_exists(conn, "spaces", column))
}

/// `projects/` 下所有会话正文的 id 集合。
///
/// 覆盖 [`find_project_jsonl`] 的两层布局（`projects/{cid}.jsonl` 与
/// `projects/{workspace}/{cid}.jsonl`），调用方只需判存在性时用它替代逐条调用。
fn project_session_ids(paths: &SessionPaths) -> HashSet<String> {
    let mut ids = HashSet::new();
    let projects = paths.projects_dir();
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return ids;
    };
    let mut collect = |path: &Path| {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            return;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            ids.insert(stem.to_string());
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for file in files.flatten() {
                collect(&file.path());
            }
        } else {
            collect(&path);
        }
    }
    ids
}

/// 参与成员会话行的存活索引：按「数据库文件 + 账号」缓存，每个组合只查一次库。
///
/// 为什么批量：预览原来对每个组成员各查一次 `sessions`（本机 162 组 → 数百次开库）。
/// 这里把「该账号在该库里**仍然存活**的会话 id」一次取成集合，判定退化为 O(1) 查找。
/// 缓存键是**数据库文件路径**而不是数据根：`SessionPaths::for_vscode_ext()` 的
/// `data_root` 是空 `PathBuf`，按数据根作键会把所有插件命名空间串成同一格；同时
/// 跨档预览（源读源档、目标读目标档）与单测注入路径都按真实库文件区分才不会串档。
#[derive(Debug, Default)]
pub(crate) struct LiveSessionRows {
    /// 数据库文件 → 存活会话索引；`None` = 该库没有判定依据（库/表/列不可用）。
    by_db: HashMap<PathBuf, Option<HashMap<String, HashSet<String>>>>,
}

impl LiveSessionRows {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 该账号在该档位数据根下仍然存活的会话 id 集合。
    ///
    /// 存活口径与 `session_row_owner` 逐条判定一致：行存在、`deleted_at IS NULL`（缺列时
    /// 不额外过滤）、`user_id` 仍是该账号。
    ///
    /// `None` = **索引不可用**（库打不开、没有 `sessions` 表、缺 `user_id` 列或查询失败），
    /// 调用方必须据此**不参与过滤**（fail-open）。不能用空集代替：那会让历史/异常库在预览里
    /// 静默清空所有组，而 `storeStatus` 仍是 `ready`，用户得不到任何解释；逐条口径
    /// `session_row_owner` 在同一场景也返回 `None`（列出但标 unknown），两条路径必须同结论。
    pub(crate) fn live_ids(&mut self, paths: &SessionPaths, uid: &str) -> Option<&HashSet<String>> {
        let db = paths.workbuddy_db();
        let cached = self
            .by_db
            .entry(db)
            .or_insert_with(|| read_live_session_ids(paths));
        let index = cached.as_mut()?;
        Some(index.entry(uid.to_string()).or_default())
    }
}

/// 读取一个库下所有未删除会话，按 `user_id` 归集 id（一次查询）。
///
/// 返回 `None` 表示**没有判定依据**（库/表/列不可用），与「查到了但一条都没有」（`Some(空表)`）
/// 严格区分，见 [`LiveSessionRows::live_ids`] 的 fail-open 约定。
/// 缺 `deleted_at` 列的历史库没有软删除标记，按「全部存活」读（不额外过滤），
/// 只有 `user_id` 缺失才无法归属任何账号（视为无判定依据）。
fn read_live_session_ids(paths: &SessionPaths) -> Option<HashMap<String, HashSet<String>>> {
    // 插件命名空间没有档位数据根（`data_root` 为空）：会话不在 `workbuddy.db` 里，
    // 没有判定依据；这里必须早退，否则会退化成读「当前工作目录下的 workbuddy.db」。
    if paths.data_root.as_os_str().is_empty() {
        return None;
    }
    let db = paths.workbuddy_db();
    let conn = open_db(&db, true)?;
    if !table_exists(&conn, "sessions") || !column_exists(&conn, "sessions", "user_id") {
        return None;
    }
    let sql = if column_exists(&conn, "sessions", "deleted_at") {
        "SELECT id, user_id FROM sessions WHERE deleted_at IS NULL"
    } else {
        "SELECT id, user_id FROM sessions"
    };
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .ok()?;
    let mut index: HashMap<String, HashSet<String>> = HashMap::new();
    for (id, uid) in rows.flatten() {
        if let (Some(id), Some(uid)) = (id, uid) {
            index.entry(uid).or_default().insert(id);
        }
    }
    Some(index)
}

/// 在 `{档位数据根}/projects/{workspace}/{cid}.jsonl` 定位会话正文。
pub(crate) fn find_project_jsonl(paths: &SessionPaths, cid: &str) -> Option<PathBuf> {
    let projects = paths.projects_dir();
    if !projects.is_dir() {
        return None;
    }
    let direct = projects.join(format!("{cid}.jsonl"));
    if direct.is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(&projects).ok()?.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let p = entry.path().join(format!("{cid}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 当前账号库中按 uid 找账号 id（成员 accountId 仅作展示，身份判定仍以 uid 为准）。
pub(super) fn account_id_for_uid(paths: &SessionPaths, uid: &str) -> Option<String> {
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    accounts
        .iter()
        .find(|account| account.get("uid").and_then(Value::as_str) == Some(uid))
        .and_then(|account| account.get("id").and_then(Value::as_str))
        .map(String::from)
}

pub(super) fn nonempty_text(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 账号对象里的 uid；前后空白视为缺失。
pub(super) fn account_uid(account: &Value) -> String {
    account
        .get("uid")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// WorkBuddy 侧栏展示名：优先 custom_title（用户改名 / 定时任务名），否则 title。
pub(super) fn session_display_title(title: Option<String>, custom_title: Option<String>) -> String {
    nonempty_text(custom_title)
        .or_else(|| nonempty_text(title))
        .unwrap_or_else(|| "(无标题)".to_string())
}

/// Claw 是账号绑定的 IM 渠道工作区，复制会话行不够，目标账号也用不了。
pub(super) fn is_claw_workspace(cwd: &str) -> bool {
    cwd.trim()
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("claw"))
}

/// 会话复制能力探测：数据根同时具备 `projects/` 目录与 `workbuddy.db` 的
/// `sessions` 表才算可用（design D6）。
///
/// 为什么必须探测而不是按档位写死：国际版数据根与国内版**不同构**——本机实测
/// 国际版数据根下没有 `projects/`、edge-sync 为 v4。若直接套用国内版假设，
/// 会写出「有 db 记录但没有正文」的半成品会话。
///
/// 纯函数，接受根路径参数以便用临时目录做单元测试。
pub fn session_copy_supported_at(root: &Path) -> bool {
    if !root.join("projects").is_dir() {
        return false;
    }
    let db = root.join("workbuddy.db");
    if !db.is_file() {
        return false;
    }
    let Some(conn) = open_db(&db, true) else {
        return false;
    };
    table_exists(&conn, "sessions")
}
