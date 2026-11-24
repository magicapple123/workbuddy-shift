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
    /// 源 space 绑定过的目录（`space_folders.path` 全部 + 两个路径列，去重前的原始值）。
    ///
    /// 只用于**匹配**目标账号的空间，不搬到目标账号（目标机器可能没有这些目录）。
    pub(in crate::modules::session) folder_paths: Vec<String>,
    /// 源空间的置顶标记（`spaces.pinned`）。老库没有该列、或该行为 NULL 时为 None。
    ///
    /// 新建 space 时透传：源项目已置顶却不在目标账号里置顶，等于悄悄丢掉用户的整理
    /// 结果。`None` 不能原样写库——该列 NOT NULL，调用方回落到 0。
    pub(in crate::modules::session) pinned: Option<i64>,
}

/// 路径比较键：分隔符统一为 `/`、转小写、去尾分隔符。
///
/// **不做 `canonicalize`**：目标机器上目录可能不存在，canonicalize 会失败并把比较结果
/// 翻转（见 `config/paths.rs::sandbox_path_key`，那个只适合本机沙箱判定）。
pub(in crate::modules::session) fn space_path_key(path: &str) -> String {
    let unified = path.replace('\\', "/");
    unified.trim_end_matches('/').to_lowercase()
}

/// 目标账号空间查找结果：`ambiguous` 表示同一 cwd 命中多个空间（不猜，走新建）。
#[derive(Debug, Clone, Default)]
pub(in crate::modules::session) struct SpaceLookup {
    pub(in crate::modules::session) id: Option<String>,
    pub(in crate::modules::session) ambiguous: bool,
}

/// 目标账号有多个空间绑定同一目录时写进复制报告的诊断。
pub(in crate::modules::session) const SPACE_AMBIGUOUS_NOTE: &str =
    "目标账号有多个空间绑定该目录，已按目录新建分组";

/// 源会话**有**项目归属却没能落到目标账号任何分组时写进复制报告的诊断。
///
/// 两种成因共用一条文案（对用户是同一件事：副本没进项目）：
/// - 源 space 在两边都查不到（跨档库丢失 / 老库没有 `spaces` 表）；
/// - 目标账号取不到可用的 `device_id`，新建分组失败（`insert_target_space` 返回 None）。
///
/// 不报警就等于「会话在列表里，用户按项目怎么都找不到，也没有任何解释」。
/// 源会话本身没有项目（`space_id` 为 NULL）是正常情况，**不**发这条诊断。
pub(in crate::modules::session) const SPACE_UNASSIGNED_NOTE: &str =
    "目标账号里未能定位或新建项目分组，副本未归入任何项目";

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

/// 取行里的整数列；NULL 与非整数一律视为缺失（`pinned` 只可能是 0/1）。
///
/// 「缺列」与「NULL」都返回 None：行由 `SELECT *` 读出，老库没有 `pinned` 列时行里
/// 根本没有这个键——与 `column_exists` 判定等价，而且跨档复制读的是**源库**的行，
/// 不能拿目标库的 schema 去判断源行有没有这一列。
fn row_int(row: &[(String, rusqlite::types::Value)], column: &str) -> Option<i64> {
    let (_, value) = row.iter().find(|(name, _)| name == column)?;
    match value {
        rusqlite::types::Value::Integer(number) => Some(*number),
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
        folder_paths: Vec::new(),
        pinned: row_int(row, "pinned"),
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
        if let Some(mut meta) = space_meta_from_row(&row) {
            meta.folder_paths = space_folder_paths(conn, space_id, &meta);
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
    let mut meta = space_meta_from_row(&read_space_row(&src_conn, space_id)?)?;
    meta.folder_paths = space_folder_paths(&src_conn, space_id, &meta);
    Some(meta)
}

/// 源 space 绑定过的全部目录：`space_folders.path` + 两个路径列（去重）。
fn space_folder_paths(conn: &Connection, space_id: &str, meta: &SpaceMeta) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    if let Some(folder) = &meta.create_folder_path {
        paths.push(folder.clone());
    }
    if table_exists(conn, "space_folders") && column_exists(conn, "space_folders", "path") {
        if let Ok(mut stmt) =
            conn.prepare("SELECT path FROM space_folders WHERE space_id = ?1 ORDER BY sort_order")
        {
            if let Ok(rows) = stmt.query_map([space_id], |row| row.get::<_, Option<String>>(0)) {
                for path in rows.flatten().flatten() {
                    paths.push(path);
                }
            }
        }
    }
    if table_exists(conn, "spaces") && column_exists(conn, "spaces", "primary_folder_path") {
        if let Ok(Some(path)) = conn.query_row(
            "SELECT primary_folder_path FROM spaces WHERE id = ?1",
            [space_id],
            |row| row.get::<_, Option<String>>(0),
        ) {
            paths.push(path);
        }
    }
    paths.retain(|path| !path.trim().is_empty());
    paths.dedup();
    paths
}

/// 目标账号的空间匹配：**路径优先 → 同名兜底 → 新建**（R5-20）。
///
/// 路径匹配用 [`space_path_key`] 归一后比较（不分大小写/分隔符），比较对象是
/// `space_folders.path` 与 `spaces.primary_folder_path` / `create_folder_path`。
/// 同一目录命中多个空间时视为 `ambiguous`：**不猜**（与客户端 `resolveUniqueSpaceByCwd`
/// 同口径），退回同名兜底，仍未命中则新建并让调用方写一条诊断。
///
/// 名字兜底保留改造前的两级：先「同名 + 同文件夹」精确命中，再退回只按名字——
/// 目标行的路径列可能为空或换了位置，此时复用明显优于再造一个重复项目。
pub(in crate::modules::session) fn find_target_space_id(
    conn: &Connection,
    target_uid: &str,
    meta: &SpaceMeta,
    cwd: &str,
) -> SpaceLookup {
    if !table_exists(conn, "spaces") {
        return SpaceLookup::default();
    }
    let mut keys: std::collections::BTreeSet<String> = meta
        .folder_paths
        .iter()
        .map(|path| space_path_key(path))
        .filter(|key| !key.is_empty())
        .collect();
    let cwd_key = space_path_key(cwd);
    if !cwd_key.is_empty() {
        keys.insert(cwd_key);
    }

    let mut hits: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // 1) space_folders.path（真实客户端的「工作目录」来源）
    if !keys.is_empty()
        && table_exists(conn, "space_folders")
        && column_exists(conn, "space_folders", "path")
    {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT sf.space_id, sf.path FROM space_folders sf JOIN spaces s ON s.id = sf.space_id WHERE s.user_id = ?1 AND COALESCE(s.deleted_at, -1) = -1",
        ) {
            if let Ok(rows) = stmt.query_map([target_uid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            }) {
                for (space_id, path) in rows.flatten() {
                    if let Some(path) = path {
                        if keys.contains(&space_path_key(&path)) {
                            hits.insert(space_id);
                        }
                    }
                }
            }
        }
    }
    // 2) spaces 自身的路径列（`primary_folder_path` 实测全 NULL，`create_folder_path` 才有值）
    if hits.is_empty() && !keys.is_empty() {
        let mut selected = String::from("id");
        if column_exists(conn, "spaces", "primary_folder_path") {
            selected.push_str(", primary_folder_path");
        }
        if column_exists(conn, "spaces", "create_folder_path") {
            selected.push_str(", create_folder_path");
        }
        let sql = format!(
            "SELECT {selected} FROM spaces WHERE user_id = ?1 AND COALESCE(deleted_at, -1) = -1"
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            let has_primary = column_exists(conn, "spaces", "primary_folder_path");
            if let Ok(rows) = stmt.query_map([target_uid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    if has_primary {
                        row.get::<_, Option<String>>(1)?
                    } else {
                        None
                    },
                    row.get::<_, Option<String>>(if has_primary { 2 } else { 1 })
                        .unwrap_or(None),
                ))
            }) {
                for (space_id, primary, folder) in rows.flatten() {
                    let hit = [primary, folder]
                        .into_iter()
                        .flatten()
                        .any(|path| keys.contains(&space_path_key(&path)));
                    if hit {
                        hits.insert(space_id);
                    }
                }
            }
        }
    }
    let ambiguous = hits.len() > 1;
    if hits.len() == 1 {
        return SpaceLookup {
            id: hits.into_iter().next(),
            ambiguous: false,
        };
    }

    // 3) 同名兜底（改造前逻辑）。ambiguous 时若兜底命中的正是「争议空间」之一，
    // 就**不复用**——那等于在两个候选里偷偷选一个，违反「不猜」的约定。
    let mut by_name: Option<String> = None;
    if let Some(folder) = meta.create_folder_path.as_deref() {
        if column_exists(conn, "spaces", "create_folder_path") {
            by_name = conn
                .query_row(
                    "SELECT id FROM spaces WHERE user_id = ?1 AND name = ?2 AND deleted_at = -1 AND create_folder_path = ?3 ORDER BY updated_at DESC LIMIT 1",
                    rusqlite::params![target_uid, meta.name, folder],
                    |row| row.get::<_, String>(0),
                )
                .ok();
        }
    }
    if by_name.is_none() {
        by_name = conn
            .query_row(
                "SELECT id FROM spaces WHERE user_id = ?1 AND name = ?2 AND deleted_at = -1 ORDER BY updated_at DESC LIMIT 1",
                rusqlite::params![target_uid, meta.name],
                |row| row.get::<_, String>(0),
            )
            .ok();
    }
    if ambiguous {
        if let Some(id) = &by_name {
            if hits.contains(id) {
                return SpaceLookup {
                    id: None,
                    ambiguous: true,
                };
            }
        }
    }
    SpaceLookup {
        id: by_name,
        ambiguous,
    }
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
    target_paths: &SessionPaths,
    target_uid: &str,
    meta: &SpaceMeta,
    cwd: &str,
) -> Result<Option<String>, String> {
    if !table_exists(conn, "spaces") {
        return Ok(None);
    }
    // 三级兜底：源 space → 目标账号任一 space → 设备级 `~/.workbuddy/device-id`。
    // 设备标识是 NOT NULL 且跨账号共享（实测本机 32 条 space 的 device_id 全等），
    // 目标账号一个 space 都没有时靠它建组，不再把 `space_id` 写成 NULL。
    let Some(device_id) = meta
        .device_id
        .clone()
        .or_else(|| fallback_device_id(conn, target_uid))
        .or_else(|| device_id_from_store_root(target_paths))
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
        // 置顶状态透传源 space：源项目已置顶时目标也必须置顶。列 NOT NULL，
        // 源行没有可用值（缺列 / NULL）时写 0，不能写 NULL。
        (
            "pinned",
            rusqlite::types::Value::Integer(meta.pinned.unwrap_or(0)),
        ),
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
    // 客户端建空间必然同时写 `space_folders`（`createMigratedSpace` 的 folders 参数），
    // 少了它目标账号里这个分组「没有工作目录」。只绑会话 cwd；源空间的其它目录不搬。
    bind_space_folders(conn, &new_id, cwd, meta.create_folder_path.as_deref())?;
    Ok(Some(new_id))
}

/// 设备级 device-id：`~/.workbuddy/device-id`（国内版）→ `~/.workbuddy-ai/device-id`。
///
/// 生产里 `store_root` 是 `~/.workbuddy-shift`，其父目录就是用户主目录；沙箱/单测的
/// 临时目录下没有这个文件，因此不会把真实机器标识带进测试。
fn device_id_from_store_root(paths: &SessionPaths) -> Option<String> {
    let home = paths.store_root.parent()?;
    for dir in [".workbuddy", ".workbuddy-ai"] {
        let Ok(text) = std::fs::read_to_string(home.join(dir).join("device-id")) else {
            continue;
        };
        let value = text.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// 给新空间补 `space_folders` 行：会话 cwd 记 `sort_order = 0`，`create_folder_path`
/// 与 cwd 不同时再补一条 `sort_order = 1`。目标库没有该表（旧客户端）时整步跳过。
fn bind_space_folders(
    conn: &Connection,
    space_id: &str,
    cwd: &str,
    create_folder_path: Option<&str>,
) -> Result<(), String> {
    if !table_exists(conn, "space_folders")
        || !column_exists(conn, "space_folders", "space_id")
        || !column_exists(conn, "space_folders", "path")
    {
        return Ok(());
    }
    let has_sort = column_exists(conn, "space_folders", "sort_order");
    let mut folders: Vec<(String, i64)> = Vec::new();
    let cwd = cwd.trim();
    if !cwd.is_empty() {
        folders.push((cwd.to_string(), 0));
    }
    if let Some(folder) = create_folder_path {
        let folder = folder.trim();
        if !folder.is_empty() && space_path_key(folder) != space_path_key(cwd) {
            folders.push((folder.to_string(), 1));
        }
    }
    for (path, sort_order) in folders {
        let result = if has_sort {
            conn.execute(
                "INSERT OR IGNORE INTO space_folders (space_id, path, sort_order) VALUES (?1, ?2, ?3)",
                rusqlite::params![space_id, path, sort_order],
            )
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO space_folders (space_id, path) VALUES (?1, ?2)",
                rusqlite::params![space_id, path],
            )
        };
        result.map_err(|error| format!("项目空间目录保存失败：{error}"))?;
    }
    Ok(())
}

/// 把源会话的 `space_id` 映射成**目标账号自己的** space id（本缺陷的修复核心）。
///
/// WorkBuddy 的项目归属靠 `sessions.space_id` 指向 `spaces`：每个账号都有各自独立的
/// ResumeForge space（id 不同、name 相同）。跨账号复制时把源 id 原样搬过去，目标行就
/// 指向一个不存在的 space，客户端于是把会话归到「最近」而不是项目里。
///
/// 返回语义（R5-20 起按**路径优先**匹配）：
/// - 源 `space_id` 为空 → `Ok((None, None))`：目标写 NULL，**不**新建 space，也不发诊断
///   （源会话本来就没有项目，属于正常情况）；
/// - 目标账号有空间绑定同一目录（唯一命中）→ 用它的 id；
/// - 同一目录命中多个空间 → `ambiguous`，退回同名兜底，仍未命中则新建并带诊断；
/// - 都没有 → 为目标账号插一条同构 space 行（含 `space_folders`），用新 id；
/// - 源 space 查不到（跨档库丢失 / 老库没有 spaces 表）或新建失败 → `Ok((None,
///   Some(SPACE_UNASSIGNED_NOTE)))`：如实写 NULL，但把原因带回复制报告——用户看到
///   「未归入任何项目」才知道为什么在「项目」里找不到副本。
pub(in crate::modules::session) fn map_target_space_id(
    conn: &Connection,
    source_paths: &SessionPaths,
    target_paths: &SessionPaths,
    same_db: bool,
    source_row: &[(String, rusqlite::types::Value)],
    target_uid: &str,
    cwd: &str,
) -> Result<(Option<String>, Option<&'static str>), String> {
    let Some(space_id) = row_text(source_row, "space_id") else {
        return Ok((None, None));
    };
    let Some(meta) = read_source_space_meta(conn, source_paths, same_db, &space_id) else {
        return Ok((None, Some(SPACE_UNASSIGNED_NOTE)));
    };
    let lookup = find_target_space_id(conn, target_uid, &meta, cwd);
    if let Some(existing) = lookup.id {
        return Ok((Some(existing), None));
    }
    // 新建失败（三级 device_id 兜底全部取不到 / 目标库没有 spaces 表）时同样要有诊断：
    // 这是「用户在项目里看不到副本」的另一半成因，静默 NULL 等于让问题再次隐身。
    let Some(created) = insert_target_space(conn, target_paths, target_uid, &meta, cwd)? else {
        return Ok((None, Some(SPACE_UNASSIGNED_NOTE)));
    };
    Ok((
        Some(created),
        lookup.ambiguous.then_some(SPACE_AMBIGUOUS_NOTE),
    ))
}
