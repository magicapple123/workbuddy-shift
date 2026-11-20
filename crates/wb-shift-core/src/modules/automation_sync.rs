//! 切号时的 WorkBuddy「自动化」定时任务：只读提示 + 可选同步（默认关闭）。
//!
//! 背景（上游 issue #150）：定时任务按 `owner_user_id` 绑账号，切号后原账号的任务不再
//! 执行且界面没有提示，容易漏签。本模块只做两件事：
//!
//! 1. **提示**（P0，只读）：数出当前账号有几个启用的任务，交给切号弹窗展示；
//! 2. **同步**（P1，默认关闭）：把简单任务复制给目标账号，并把源任务改成 `PAUSED`（不删）。
//!
//! 明确不做：不复制 `automation_runs` 历史、不复制 `automation_runtime_state`、不删除源
//! 任务、不修改目标账号已有任务、不伪造 `next_run_at`（复制时写 NULL，由客户端按 rrule 重算）。
//!
//! `next_run_at` 的受控实验结论（2026-11-20 实测本机 `~/.workbuddy/workbuddy.db`）：
//! 12 条未删除任务**全部** `next_run_at IS NULL`，其中 10 条是 `ACTIVE`，客户端照常把它们
//! 展示为启用并由本地调度器按 `rrule` 计算下次时间——因此复制写 NULL 是客户端自身状态，
//! 不是伪造值。本次会话无法等到定时触发，故「到点能执行」一项标 partial。

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::modules::session::{workbuddy_db_path, SessionPaths};
use crate::modules::variant::WbVariant;

/// 设置的落盘文件（与 `auto_checkin_config.json` 同级）。
const CONFIG_FILE: &str = "automation_sync_config.json";
/// 同步前备份的根目录（相对 store_root）；与 `backups/` 保留策略一致。
const BACKUP_DIR: &str = "backups/automation-sync";

/// 一条启用中的任务（提示用）。
pub struct ActiveTask {
    pub id: String,
    pub name: String,
}

/// 读取设置；文件缺失/损坏一律按「关闭」处理（默认关闭是决策要求）。
pub fn is_enabled(paths: &SessionPaths) -> bool {
    let path = paths.store_root.join(CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| value.get("enabled").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// 写设置。只改 `enabled` 一个键，其它键（未来扩展）原样保留。
pub fn set_enabled(paths: &SessionPaths, enabled: bool) -> Result<(), String> {
    std::fs::create_dir_all(&paths.store_root)
        .map_err(|error| format!("设置目录创建失败：{error}"))?;
    let path = paths.store_root.join(CONFIG_FILE);
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    if !value.is_object() {
        value = json!({});
    }
    value["enabled"] = json!(enabled);
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    std::fs::write(&path, format!("{text}\n")).map_err(|error| format!("设置保存失败：{error}"))
}

fn open_db(db: &Path) -> Option<Connection> {
    let conn = Connection::open(db).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    Some(conn)
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        == 1
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(mut rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    rows.any(|name| name.is_ok_and(|name| name == column))
}

/// 当前账号启用中的任务（`deleted_at IS NULL AND status='ACTIVE'`），按创建时间排序。
fn active_tasks_at(conn: &Connection, uid: &str) -> Vec<ActiveTask> {
    if !table_exists(conn, "automations") || !column_exists(conn, "automations", "owner_user_id") {
        return Vec::new();
    }
    let deleted = if column_exists(conn, "automations", "deleted_at") {
        "AND deleted_at IS NULL"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id, COALESCE(name, '未命名任务') FROM automations \
         WHERE owner_user_id = ?1 AND status = 'ACTIVE' {deleted} ORDER BY created_at"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([uid], |row| {
        Ok(ActiveTask {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// 切号弹窗用：当前账号的启用任务摘要（只读，不写库）。
///
/// 返回 `{ count, names: [最多 5 条], enabled }`；`count = 0` 时前端不渲染这一块。
pub fn summary(uid: &str, variant: WbVariant) -> Value {
    let paths = SessionPaths::for_variant(variant);
    let tasks = open_db(&paths.workbuddy_db())
        .map(|conn| active_tasks_at(&conn, uid))
        .unwrap_or_default();
    let names: Vec<String> = tasks.iter().take(5).map(|task| task.name.clone()).collect();
    json!({
        "count": tasks.len(),
        "names": names,
        "enabled": is_enabled(&paths),
    })
}

/// 切号弹窗用（自动判定档位）：当前登录账号在两个档位里各自的启用任务数，
/// 取有任务的那个档位；都没有则返回 0（前端据此不渲染提示块）。
///
/// 为什么要自动判档：前端手上的 `variant` 是**目标账号**的档位，而任务属于**源账号**
/// （当前登录），两者可能不同档；让 core 按 uid 去两个库各查一次最省事也最准。
pub fn summary_any(uid: &str) -> Value {
    if uid.trim().is_empty() {
        return json!({ "count": 0, "names": [], "enabled": false });
    }
    let mut fallback: Option<Value> = None;
    for variant in [WbVariant::Cn, WbVariant::Ai] {
        let value = summary(uid, variant);
        if value["count"].as_u64().unwrap_or(0) > 0 {
            return value;
        }
        if fallback.is_none() {
            fallback = Some(value);
        }
    }
    fallback.unwrap_or_else(|| json!({ "count": 0, "names": [], "enabled": false }))
}

/// 同步结果（`SwitchResult.automationSync`）。
fn report(
    ok: bool,
    backup: Option<String>,
    copied: Vec<Value>,
    paused: Vec<Value>,
    skipped: Vec<Value>,
    error: Option<String>,
) -> Value {
    let mut value = json!({
        "ok": ok,
        "copied": copied,
        "paused": paused,
        "skipped": skipped,
    });
    if let Some(backup) = backup {
        value["backup"] = json!(backup);
    }
    if let Some(error) = error {
        value["error"] = json!(error);
        value["ok"] = json!(false);
    }
    value
}

fn text_column(row: &[(String, rusqlite::types::Value)], column: &str) -> Option<String> {
    let (_, value) = row.iter().find(|(name, _)| name == column)?;
    match value {
        rusqlite::types::Value::Text(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

fn int_column(row: &[(String, rusqlite::types::Value)], column: &str) -> Option<i64> {
    let (_, value) = row.iter().find(|(name, _)| name == column)?;
    match value {
        rusqlite::types::Value::Integer(number) => Some(*number),
        _ => None,
    }
}

fn read_row(conn: &Connection, id: &str) -> Option<Vec<(String, rusqlite::types::Value)>> {
    let mut stmt = conn
        .prepare("SELECT * FROM automations WHERE id = ?1")
        .ok()?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query([id]).ok()?;
    let row = rows.next().ok()??;
    let mut values = Vec::with_capacity(cols.len());
    for (index, column) in cols.into_iter().enumerate() {
        let value = row
            .get::<_, rusqlite::types::Value>(index)
            .unwrap_or(rusqlite::types::Value::Null);
        values.push((column, value));
    }
    Some(values)
}

/// 参考列名（v1 复制范围）：这些列在目标表存在时原样搬运。
const COPY_COLUMNS: &[&str] = &[
    "name",
    "prompt",
    "cwds",
    "workspace_scope",
    "schedule_type",
    "rrule",
    "scheduled_at",
    "valid_from",
    "valid_until",
    "expert_id",
    "model_id",
    "model_is_thinking",
    "context_window",
    "reasoning_effort",
    "permission_mode",
    "push_enabled",
    "push_channel",
    "push_target",
];

/// 复制时**不搬**的列：身份、归属、调度状态与时间戳一律由本模块重写。
const RESET_COLUMNS: &[&str] = &[
    "id",
    "owner_user_id",
    "owner_status",
    "owner_source",
    "status",
    "next_run_at",
    "last_run_at",
    "created_at",
    "updated_at",
    "deleted_at",
];

/// 一条任务是否属于「v1 可复制」的简单任务。
///
/// 允许 `expert_id`（实测 11 个账号的 `marketExpertId` 全一致、与账号无关，本机 9/12 属此类，
/// 决策要求照搬）；`skills_json` / `connector_ids_json` / `space_id` 非空则**只提示不复制**
/// ——这些引用在目标账号下大概率不存在，复制过去只会让任务静默失败。
fn simple_task_reason(row: &[(String, rusqlite::types::Value)]) -> Option<String> {
    let non_empty = |column: &str| -> bool {
        match text_column(row, column) {
            Some(text) => {
                let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                !matches!(compact.as_str(), "" | "[]" | "null" | "{}")
            }
            None => false,
        }
    };
    if non_empty("skills_json") {
        return Some("任务引用了技能，v1 不复制".to_string());
    }
    if non_empty("connector_ids_json") {
        return Some("任务引用了连接器，v1 不复制".to_string());
    }
    if text_column(row, "space_id").is_some() {
        return Some("任务绑定了项目空间，v1 不复制".to_string());
    }
    None
}

/// 把 `automations` 一致备份到 `backups/automation-sync/<时间戳>/workbuddy.db`。
///
/// 用 `VACUUM INTO` 而不是文件拷贝：活动库带 `-wal`，直接拷主文件会拿到不一致快照。
fn backup_db(conn: &Connection, store_root: &Path) -> Result<PathBuf, String> {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f").to_string();
    let dir = store_root.join(BACKUP_DIR).join(&stamp);
    std::fs::create_dir_all(&dir).map_err(|error| format!("备份目录创建失败：{error}"))?;
    let target = dir.join("workbuddy.db");
    let path = target.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?1", [path])
        .map_err(|error| format!("自动化任务备份失败：{error}"))?;
    Ok(target)
}

/// 同步入口：把 `source_uid` 的启用任务复制给 `target_uid`，并把源任务改成 `PAUSED`。
///
/// 调用方只在 `restart = true`（客户端已关闭）且设置开启时调用。返回报告，**不抛错**
/// 也不阻断认证切换：失败时 `ok=false` + `error`，与会话复制「失败进报告」的口径一致。
pub fn sync(source_uid: &str, target_uid: &str, variant: WbVariant) -> Value {
    let paths = SessionPaths::for_variant(variant);
    sync_at(
        &paths.store_root,
        &paths.workbuddy_db(),
        source_uid,
        target_uid,
    )
}

/// [`sync`] 的可注入实现（单测用临时库，不触碰真实 `~/.workbuddy`）。
pub fn sync_at(store_root: &Path, db: &Path, source_uid: &str, target_uid: &str) -> Value {
    if source_uid.trim().is_empty() || target_uid.trim().is_empty() {
        return report(
            false,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some("缺少账号 uid，未同步自动化任务".to_string()),
        );
    }
    if source_uid == target_uid {
        return report(false, None, Vec::new(), Vec::new(), Vec::new(), None);
    }
    let Some(mut conn) = open_db(db) else {
        return report(
            false,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some("自动化任务数据无法打开".to_string()),
        );
    };
    if !table_exists(&conn, "automations") {
        return report(
            false,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some("当前 WorkBuddy 版本不支持自动化同步（缺少 automations 表）".to_string()),
        );
    }
    let tasks = active_tasks_at(&conn, source_uid);
    if tasks.is_empty() {
        return report(true, None, Vec::new(), Vec::new(), Vec::new(), None);
    }
    // 备份先于任何写入：备份不可信就零写入。
    let backup = match backup_db(&conn, store_root) {
        Ok(path) => path,
        Err(error) => return report(false, None, Vec::new(), Vec::new(), Vec::new(), Some(error)),
    };
    let backup_text = backup.to_string_lossy().to_string();

    let mut copied: Vec<Value> = Vec::new();
    let mut paused: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let targets = column_names(&conn, "automations");
    let now = now_ms();

    let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(error) => {
            return report(
                false,
                Some(backup_text),
                copied,
                paused,
                skipped,
                Some(format!("自动化任务同步失败：{error}")),
            )
        }
    };
    for task in &tasks {
        let Some(row) = read_row(&tx, &task.id) else {
            skipped.push(json!({"name": task.name, "reason": "任务已不存在"}));
            continue;
        };
        if let Some(reason) = simple_task_reason(&row) {
            skipped.push(json!({"name": task.name, "reason": reason}));
            continue;
        }
        // once 且时间已过 / valid_until 已过：复制过去也不会执行。
        if let Some(due) = int_column(&row, "scheduled_at") {
            if text_column(&row, "schedule_type").as_deref() == Some("once") && due < now {
                skipped.push(json!({"name": task.name, "reason": "一次性任务已过执行时间"}));
                continue;
            }
        }
        if let Some(until) = int_column(&row, "valid_until") {
            if until < now {
                skipped.push(json!({"name": task.name, "reason": "任务已过有效期"}));
                continue;
            }
        }
        // 目标账号同名任务（未删除）不重复创建。
        let duplicate: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM automations WHERE owner_user_id = ?1 AND name = ?2 AND deleted_at IS NULL)",
                rusqlite::params![target_uid, task.name],
                |row| row.get::<_, i64>(0),
            )
            .map(|flag| flag == 1)
            .unwrap_or(false);
        if duplicate {
            skipped.push(json!({"name": task.name, "reason": "目标账号已有同名任务"}));
            continue;
        }
        match insert_copy(&tx, &row, &targets, target_uid, now) {
            Ok(new_id) => copied.push(json!({"id": new_id, "name": task.name})),
            Err(error) => {
                // 单条失败整批回滚：宁可一条都不复制，也不要半个批次。
                drop(tx);
                return report(
                    false,
                    Some(backup_text),
                    Vec::new(),
                    Vec::new(),
                    skipped,
                    Some(error),
                );
            }
        }
        if tx
            .execute(
                "UPDATE automations SET status = 'PAUSED', updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now, task.id],
            )
            .is_ok()
        {
            paused.push(json!({"id": task.id, "name": task.name}));
        }
    }
    if let Err(error) = tx.commit() {
        return report(
            false,
            Some(backup_text),
            Vec::new(),
            Vec::new(),
            skipped,
            Some(format!("自动化任务同步失败：{error}")),
        );
    }
    report(true, Some(backup_text), copied, paused, skipped, None)
}

fn column_names(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(stmt) = conn.prepare(&format!("SELECT * FROM {table} LIMIT 0")) else {
        return Vec::new();
    };
    stmt.column_names()
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// 按目标表列序写一条副本：只搬 [`COPY_COLUMNS`]，归属/调度/时间戳由本模块重写。
fn insert_copy(
    tx: &rusqlite::Transaction<'_>,
    source_row: &[(String, rusqlite::types::Value)],
    target_columns: &[String],
    target_uid: &str,
    now: i64,
) -> Result<String, String> {
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<rusqlite::types::Value> = Vec::new();
    for column in target_columns {
        if RESET_COLUMNS.contains(&column.as_str()) {
            continue;
        }
        if !COPY_COLUMNS.contains(&column.as_str()) {
            // 未登记的列不搬：避免把未来新增的账号相关列悄悄带过去。
            continue;
        }
        let Some((_, value)) = source_row.iter().find(|(name, _)| name == column) else {
            continue;
        };
        cols.push(column.clone());
        vals.push(value.clone());
    }
    // 归属与调度字段显式重写。
    let mut push = |column: &str, value: rusqlite::types::Value| {
        if target_columns.iter().any(|name| name == column) {
            cols.push(column.to_string());
            vals.push(value);
        }
    };
    push("id", rusqlite::types::Value::Text(new_id.clone()));
    push(
        "owner_user_id",
        rusqlite::types::Value::Text(target_uid.to_string()),
    );
    push(
        "owner_status",
        rusqlite::types::Value::Text("confirmed".to_string()),
    );
    push(
        "owner_source",
        rusqlite::types::Value::Text("created".to_string()),
    );
    push("status", rusqlite::types::Value::Text("ACTIVE".to_string()));
    // next_run_at 写 NULL：客户端按 rrule 自行重算（受控实验见模块文档）。
    push("next_run_at", rusqlite::types::Value::Null);
    push("last_run_at", rusqlite::types::Value::Null);
    push("deleted_at", rusqlite::types::Value::Null);
    push("created_at", rusqlite::types::Value::Integer(now));
    push("updated_at", rusqlite::types::Value::Integer(now));
    let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "INSERT INTO automations ({}) VALUES ({placeholders})",
        cols.join(", ")
    );
    let params: Vec<&rusqlite::types::Value> = vals.iter().collect();
    tx.execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|error| format!("自动化任务复制失败：{error}"))?;
    Ok(new_id)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 测试/诊断用：直接按库文件统计（不碰真实路径）。
pub fn active_count_at(db: &Path, uid: &str) -> usize {
    open_db(db)
        .map(|conn| active_tasks_at(&conn, uid).len())
        .unwrap_or(0)
}

/// 测试用：库文件路径（保持与生产同一套 `workbuddy_db_path` 口径）。
pub fn db_path(variant: WbVariant) -> PathBuf {
    workbuddy_db_path(variant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Env {
        root: PathBuf,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "wb_shift_automation_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let env = Env { root };
            env.schema();
            env
        }

        fn db(&self) -> PathBuf {
            self.root.join("workbuddy.db")
        }

        fn schema(&self) {
            let conn = Connection::open(self.db()).unwrap();
            conn.execute_batch(
                "CREATE TABLE automations (
                    id TEXT PRIMARY KEY,
                    owner_user_id TEXT NOT NULL,
                    name TEXT,
                    prompt TEXT,
                    cwds TEXT,
                    workspace_scope TEXT,
                    schedule_type TEXT,
                    rrule TEXT,
                    scheduled_at INTEGER,
                    valid_from INTEGER,
                    valid_until INTEGER,
                    next_run_at INTEGER,
                    last_run_at INTEGER,
                    status TEXT,
                    owner_status TEXT,
                    owner_source TEXT,
                    expert_id TEXT,
                    skills_json TEXT,
                    connector_ids_json TEXT,
                    space_id TEXT,
                    model_id TEXT,
                    created_at INTEGER,
                    updated_at INTEGER,
                    deleted_at INTEGER
                );",
            )
            .unwrap();
        }

        fn add(&self, id: &str, uid: &str, name: &str, status: &str) {
            let conn = Connection::open(self.db()).unwrap();
            conn.execute(
                "INSERT INTO automations (id, owner_user_id, name, schedule_type, rrule, status, \
                 owner_status, created_at, updated_at, skills_json, connector_ids_json) \
                 VALUES (?1, ?2, ?3, 'recurring', 'FREQ=DAILY;BYHOUR=9;BYMINUTE=0', ?4, \
                 'confirmed', 1, 1, '[]', '[]')",
                rusqlite::params![id, uid, name, status],
            )
            .unwrap();
        }

        fn set_space(&self, id: &str, space: &str) {
            let conn = Connection::open(self.db()).unwrap();
            conn.execute(
                "UPDATE automations SET space_id = ?1 WHERE id = ?2",
                rusqlite::params![space, id],
            )
            .unwrap();
        }

        fn status_of(&self, id: &str) -> String {
            let conn = Connection::open(self.db()).unwrap();
            conn.query_row(
                "SELECT status FROM automations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn rows_for(&self, uid: &str) -> Vec<String> {
            let conn = Connection::open(self.db()).unwrap();
            let mut stmt = conn
                .prepare("SELECT name FROM automations WHERE owner_user_id = ?1 ORDER BY name")
                .unwrap();
            stmt.query_map([uid], |row| row.get(0))
                .unwrap()
                .flatten()
                .collect()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 只数「未删除 + ACTIVE」的任务。
    #[test]
    fn counts_only_active_and_alive() {
        let env = Env::new("count");
        env.add("a1", "uid-a", "每日新闻", "ACTIVE");
        env.add("a2", "uid-a", "已暂停", "PAUSED");
        env.add("a3", "uid-b", "别人的", "ACTIVE");
        assert_eq!(active_count_at(&env.db(), "uid-a"), 1);
        assert_eq!(active_count_at(&env.db(), "uid-b"), 1);
    }

    /// 复制简单任务 → 目标账号出现同名 ACTIVE、源任务变 PAUSED、next_run_at 为 NULL。
    #[test]
    fn copies_simple_tasks_and_pauses_source() {
        let env = Env::new("copy");
        env.add("a1", "uid-a", "每日新闻", "ACTIVE");
        env.add("a2", "uid-a", "周报", "ACTIVE");

        let report = sync_at(&env.root, &env.db(), "uid-a", "uid-b");
        assert_eq!(report["ok"], true, "{report}");
        assert_eq!(report["copied"].as_array().unwrap().len(), 2);
        assert_eq!(report["paused"].as_array().unwrap().len(), 2);
        assert_eq!(env.rows_for("uid-b"), vec!["周报", "每日新闻"]);
        assert_eq!(env.status_of("a1"), "PAUSED");
        assert_eq!(env.status_of("a2"), "PAUSED");

        let conn = Connection::open(env.db()).unwrap();
        let null_next: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automations WHERE owner_user_id = 'uid-b' AND next_run_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(null_next, 2, "复制时 next_run_at 必须写 NULL");
        let backup = report["backup"].as_str().unwrap();
        assert!(Path::new(backup).is_file(), "备份不存在：{backup}");
    }

    /// 目标已有同名 → 跳过；带空间引用 → 跳过并写原因。
    #[test]
    fn skips_duplicates_and_unsupported_references() {
        let env = Env::new("skip");
        env.add("a1", "uid-a", "每日新闻", "ACTIVE");
        env.add("a2", "uid-b", "每日新闻", "ACTIVE");
        env.add("a3", "uid-a", "带空间", "ACTIVE");
        env.set_space("a3", "sp-1");

        let report = sync_at(&env.root, &env.db(), "uid-a", "uid-b");
        let skipped = report["skipped"].as_array().unwrap();
        assert_eq!(skipped.len(), 2, "{report}");
        let reasons: Vec<&str> = skipped
            .iter()
            .map(|item| item["reason"].as_str().unwrap())
            .collect();
        assert!(reasons.iter().any(|r| r.contains("同名")));
        assert!(reasons.iter().any(|r| r.contains("项目空间")));
        assert_eq!(env.status_of("a3"), "ACTIVE", "跳过项不得暂停源任务");
    }

    /// 缺 automations 表 → 明确报「不支持」，不写库、不抛错。
    #[test]
    fn missing_table_is_reported_not_panicked() {
        let root = std::env::temp_dir().join(format!(
            "wb_shift_automation_missing_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("workbuddy.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(x)")
            .unwrap();

        let report = sync_at(&root, &db, "uid-a", "uid-b");
        assert_eq!(report["ok"], false);
        assert!(report["error"].as_str().unwrap().contains("不支持"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 插入失败 → 整批回滚：目标账号零副本、源任务仍 ACTIVE。
    #[test]
    fn insert_failure_rolls_back_and_keeps_source_active() {
        let env = Env::new("rollback");
        env.add("a1", "uid-a", "每日新闻", "ACTIVE");
        let conn = Connection::open(env.db()).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON automations \
             BEGIN SELECT RAISE(ABORT, 'no space'); END;",
        )
        .unwrap();
        drop(conn);

        let report = sync_at(&env.root, &env.db(), "uid-a", "uid-b");
        assert_eq!(report["ok"], false, "{report}");
        assert!(env.rows_for("uid-b").is_empty(), "不得留下半批副本");
        assert_eq!(env.status_of("a1"), "ACTIVE", "回滚后源任务必须还是 ACTIVE");
    }

    /// 设置默认关闭，写读往返一致。
    #[test]
    fn config_defaults_off_and_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "wb_shift_automation_cfg_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = SessionPaths {
            store_root: root.clone(),
            ..SessionPaths::for_variant(WbVariant::Cn)
        };
        assert!(!is_enabled(&paths), "默认必须关闭");
        set_enabled(&paths, true).unwrap();
        assert!(is_enabled(&paths));
        set_enabled(&paths, false).unwrap();
        assert!(!is_enabled(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }
}
