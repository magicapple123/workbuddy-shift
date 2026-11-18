//! 复制写库：目标账号空间（spaces 表）映射、创建与回滚。
use super::*;
/// 一条 space 行里「会话复制」需要的信息。
///
/// 目标账号按 `name` 找自己的同名 space；新 space 行要跟客户端自建的行同构，
/// 因此还要沿用源行的 `device_id`（NOT NULL）与 `source_kind`（`seed` 等来源标记）。
pub(in crate::modules::session) struct SpaceMeta {
    /// 项目名（如 `ResumeForge`）：同名匹配的唯一依据。
    pub(in crate::modules::session) name: String,
    /// 设备标识；源行取不到时由调用方回落到目标账号任一 space。
    pub(in crate::modules::session) device_id: Option<String>,
    /// 空间来源标记。
    pub(in crate::modules::session) source_kind: Option<String>,
    /// 项目文件夹（实测真实行有值，如 `D:\ResumeForge`；`primary_folder_path` 才是 NULL）。
    /// 新建 space 时沿用源行同列，**不**拿会话的 `cwd` 硬猜——`cwd` 可能是项目子目录。
    pub(in crate::modules::session) create_folder_path: Option<String>,
}

/// 读取一行 space（列名 + 值，按表自身列序）。
///
/// 不写死列名：老版本客户端可能没有 `device_id` / `source_kind` 这些列，
/// 缺列时按缺失处理，而不是整行读不出来（与 sessions 的「列取交集」策略同构）。
pub(in crate::modules::session) fn read_space_row(
    conn: &Connection,
    space_id: &str,
) -> Option<Vec<(String, rusqlite::types::Value)>> {
    if !table_exists(conn, "spaces") {
        return None;
    }
    let mut stmt = conn.prepare("SELECT * FROM spaces WHERE id = ?1").ok()?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query([space_id]).ok()?;
    let row = match rows.next() {
        Ok(Some(row)) => row,
        _ => return None,
    };
    let mut values: Vec<(String, rusqlite::types::Value)> = Vec::with_capacity(cols.len());
    for (index, col) in cols.into_iter().enumerate() {
        let value = row
            .get::<_, rusqlite::types::Value>(index)
            .unwrap_or(rusqlite::types::Value::Null);
        values.push((col, value));
    }
    Some(values)
}

/// 取行里的文本列；NULL 与空白一律视为缺失（`space_id` 的判空也走这里）。
pub(in crate::modules::session) fn row_text(
    row: &[(String, rusqlite::types::Value)],
    column: &str,
) -> Option<String> {
    let (_, value) = row.iter().find(|(name, _)| name == column)?;
    match value {
        rusqlite::types::Value::Text(text) => nonempty_text(Some(text.clone())),
        _ => None,
    }
}

/// 从 space 行提取匹配所需的元信息；没有 `name` 的行不可用于同名匹配。
pub(in crate::modules::session) fn space_meta_from_row(
    row: &[(String, rusqlite::types::Value)],
) -> Option<SpaceMeta> {
    Some(SpaceMeta {
        name: row_text(row, "name")?,
        device_id: row_text(row, "device_id"),
        source_kind: row_text(row, "source_kind"),
        create_folder_path: row_text(row, "create_folder_path"),
    })
}

/// 读源 space 的元信息：先查目标库（同库跨账号是常态），跨档时再查源档库。
///
/// 跨档复制（国内版 ↔ 国际版）时源 space 与目标账号不在同一个库里，只查目标库
/// 会拿不到名字；此时按 id 回源档只读连接查一次。两处都查不到返回 None，
/// 由调用方降级（写 NULL），绝不把源账号的 id 当悬空外键带过去。
pub(in crate::modules::session) fn read_source_space_meta(
    conn: &Connection,
    source_paths: &SessionPaths,
    same_db: bool,
    space_id: &str,
) -> Option<SpaceMeta> {
    if let Some(row) = read_space_row(conn, space_id) {
        if let Some(meta) = space_meta_from_row(&row) {
            return Some(meta);
        }
    }
    if same_db {
        return None;
    }
    let src_db = source_paths.workbuddy_db();
    if !src_db.is_file() {
        return None;
    }
    // 源档只读：跨档复制不触碰源档的 WAL 恢复。
    let src_conn = open_db(&src_db, true)?;
    space_meta_from_row(&read_space_row(&src_conn, space_id)?)
}

/// 目标账号下找同名 space：先按「同名 + 同文件夹」精确匹配，再退回只按名字匹配。
///
/// 只按名字匹配在「同一账号有两个同名项目」时会挂错，故先试精确的；精确未命中仍按
/// 名字复用——目标行的路径列可能为空或换了位置，此时复用明显优于再造一个重复项目。
pub(in crate::modules::session) fn find_target_space_id(
    conn: &Connection,
    target_uid: &str,
    meta: &SpaceMeta,
) -> Option<String> {
    if !table_exists(conn, "spaces") {
        return None;
    }
    if let Some(folder) = meta.create_folder_path.as_deref() {
        if column_exists(conn, "spaces", "create_folder_path") {
            let exact = conn
                .query_row(
                    "SELECT id FROM spaces WHERE user_id = ?1 AND name = ?2 AND deleted_at = -1 \
                     AND create_folder_path = ?3 ORDER BY updated_at DESC LIMIT 1",
                    rusqlite::params![target_uid, meta.name, folder],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            if exact.is_some() {
                return exact;
            }
        }
    }
    conn.query_row(
        "SELECT id FROM spaces WHERE user_id = ?1 AND name = ?2 AND deleted_at = -1 \
         ORDER BY updated_at DESC LIMIT 1",
        rusqlite::params![target_uid, meta.name],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// 目标账号已有 space 的 `device_id`（源 space 也没带设备标识时的唯一来源）。
pub(in crate::modules::session) fn fallback_device_id(
    conn: &Connection,
    target_uid: &str,
) -> Option<String> {
    if !table_exists(conn, "spaces") || !column_exists(conn, "spaces", "device_id") {
        return None;
    }
    conn.query_row(
        "SELECT device_id FROM spaces WHERE user_id = ?1 ORDER BY updated_at DESC LIMIT 1",
        [target_uid],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

/// 为**目标账号**新建一条与源 space 同构的记录，返回新 id。
///
/// 列按目标库实际存在性写入：老库缺列时不写，由库默认值补齐。`device_id` 是
/// NOT NULL 且无法凭空编造——源 space 与目标账号都取不到时**不建**（返回 None），
/// 让调用方写 NULL：宁可会话落回「最近」，也不造一个设备标识可疑的空项目。
pub(in crate::modules::session) fn insert_target_space(
    conn: &Connection,
    target_uid: &str,
    meta: &SpaceMeta,
) -> Result<Option<String>, String> {
    if !table_exists(conn, "spaces") {
        return Ok(None);
    }
    let Some(device_id) = meta
        .device_id
        .clone()
        .or_else(|| fallback_device_id(conn, target_uid))
    else {
        return Ok(None);
    };
    let new_id = uuid::Uuid::new_v4().to_string();
    let now = now_ms();
    // 只写「有把握」的列：`create_folder_path` 沿用源行（客户端靠它展示/打开项目）；
    // 其余可空列（`primary_folder_path`、sort_order、conversation_origin）一律留 NULL，
    // 实测真实行这些本来就是 NULL，写进来反而会引入源账号的残留值。
    let candidates: Vec<(&str, rusqlite::types::Value)> = vec![
        ("id", rusqlite::types::Value::Text(new_id.clone())),
        (
            "user_id",
            rusqlite::types::Value::Text(target_uid.to_string()),
        ),
        ("name", rusqlite::types::Value::Text(meta.name.clone())),
        ("device_id", rusqlite::types::Value::Text(device_id)),
        (
            "source_kind",
            match &meta.source_kind {
                Some(kind) => rusqlite::types::Value::Text(kind.clone()),
                None => rusqlite::types::Value::Null,
            },
        ),
        (
            "create_folder_path",
            match &meta.create_folder_path {
                Some(folder) => rusqlite::types::Value::Text(folder.clone()),
                None => rusqlite::types::Value::Null,
            },
        ),
        ("status", rusqlite::types::Value::Text("active".to_string())),
        (
            "transport",
            rusqlite::types::Value::Text("local".to_string()),
        ),
        ("pinned", rusqlite::types::Value::Integer(0)),
        ("deleted_at", rusqlite::types::Value::Integer(-1)),
        ("created_at", rusqlite::types::Value::Integer(now)),
        ("updated_at", rusqlite::types::Value::Integer(now)),
    ];
    let mut cols: Vec<String> = Vec::with_capacity(candidates.len());
    let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(candidates.len());
    for (column, value) in candidates {
        if !column_exists(conn, "spaces", column) {
            continue;
        }
        cols.push(column.to_string());
        vals.push(value);
    }
    let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "INSERT INTO spaces ({}) VALUES ({placeholders})",
        cols.join(", ")
    );
    let params: Vec<&rusqlite::types::Value> = vals.iter().collect();
    conn.execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|error| format!("项目空间记录保存失败：{error}"))?;
    Ok(Some(new_id))
}

/// 把源会话的 `space_id` 映射成**目标账号自己的** space id（本缺陷的修复核心）。
///
/// WorkBuddy 的项目归属靠 `sessions.space_id` 指向 `spaces`：每个账号都有各自独立的
/// ResumeForge space（id 不同、name 相同）。跨账号复制时把源 id 原样搬过去，目标行就
/// 指向一个不存在的 space，客户端于是把会话归到「最近」而不是项目里。
///
/// 返回语义：
/// - 源 `space_id` 为空 → `Ok(None)`：目标写 NULL，**不**新建 space；
/// - 目标账号有同名 space → 用它的 id；
/// - 没有同名 space → 为目标账号插一条同构 space 行，用新 id；
/// - 源 space 查不到（跨档库丢失 / 老库没有 spaces 表）→ `Ok(None)`，如实写 NULL。
pub(in crate::modules::session) fn map_target_space_id(
    conn: &Connection,
    source_paths: &SessionPaths,
    same_db: bool,
    source_row: &[(String, rusqlite::types::Value)],
    target_uid: &str,
) -> Result<Option<String>, String> {
    let Some(space_id) = row_text(source_row, "space_id") else {
        return Ok(None);
    };
    let Some(meta) = read_source_space_meta(conn, source_paths, same_db, &space_id) else {
        return Ok(None);
    };
    if let Some(existing) = find_target_space_id(conn, target_uid, &meta) {
        return Ok(Some(existing));
    }
    insert_target_space(conn, target_uid, &meta)
}
