// 会话列表与 `spaces` 表关联（T1：按 `space_id` 归组、空间真名来自 `spaces.name`）。
//
// 由 `tests/mod.rs` 的 `include!` 平铺进 `modules::session::tests`，测试全名与同目录
// 其它片段一致（`modules::session::tests::*`）。所有用例都在临时目录里造库。

/// 给默认 `sessions` 表补 `space_id` 列并建 `spaces` 表（WorkBuddy 5.x 形状）。
fn create_spaces_table(env: &Env) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "ALTER TABLE sessions ADD COLUMN space_id TEXT;
         CREATE TABLE spaces (
             id TEXT PRIMARY KEY,
             user_id TEXT NOT NULL,
             name TEXT,
             deleted_at INTEGER
         );",
    )
    .unwrap();
}

/// 真实库里「未删除」写作 `deleted_at = -1`（不是 NULL），已删除写时间戳。
fn add_space(env: &Env, id: &str, uid: &str, name: &str, deleted_at: i64) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO spaces (id, user_id, name, deleted_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, uid, name, deleted_at],
    )
    .unwrap();
}

/// 直接插一行会话：cwd / space_id / updated_at 都要精确控制（`Env::add_session` 写死）。
fn add_session_with_space(
    env: &Env,
    id: &str,
    uid: &str,
    cwd: &str,
    space_id: Option<&str>,
    updated_at: i64,
) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, \
         deleted_at, is_playground, space_id) VALUES (?1, ?2, ?3, NULL, ?4, 1000, ?5, NULL, 0, ?6)",
        rusqlite::params![id, uid, format!("标题 {id}"), cwd, updated_at, space_id],
    )
    .unwrap();
}

fn item<'a>(sessions: &'a Value, id: &str) -> &'a Value {
    sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == id)
        .unwrap_or_else(|| panic!("会话 {id} 不在列表里"))
}

/// 同一空间的会话 cwd 不同也返回同一个 `spaceId` / `spaceName`。
///
/// 这就是「日期空间」的根因现场：真实库里 4 条会话同一个 `space_id`，cwd 有两种
/// （仓库目录与 `WorkBuddy_data/spaces/<时间戳>`）。前端只按 `spaceId` 分组即可合成
/// 一个空间，不再出现名为时间戳的空间。
#[test]
fn list_sessions_returns_shared_space_for_same_space_across_cwds() {
    let env = Env::new("list-space-merge");
    env.create_db();
    create_spaces_table(&env);
    add_space(&env, "space-a", "uid-a", "workbuddy-shift", -1);
    let date_dir = "D:/WorkBuddy_data/spaces/2026_11_16_13-46-13";
    add_session_with_space(
        &env,
        "s1",
        "uid-a",
        "D:/repos/workbuddy-shift",
        Some("space-a"),
        4000,
    );
    add_session_with_space(
        &env,
        "s2",
        "uid-a",
        "D:/repos/workbuddy-shift",
        Some("space-a"),
        3000,
    );
    add_session_with_space(&env, "s3", "uid-a", date_dir, Some("space-a"), 2000);
    add_session_with_space(&env, "s4", "uid-a", date_dir, Some("space-a"), 1000);

    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions.as_array().unwrap().len(), 4);
    for id in ["s1", "s2", "s3", "s4"] {
        assert_eq!(item(&sessions, id)["spaceId"], "space-a");
        assert_eq!(item(&sessions, id)["spaceName"], "workbuddy-shift");
    }
    // cwd 确实不同：分组只能靠 spaceId，不能靠目录名。
    assert_ne!(item(&sessions, "s1")["cwd"], item(&sessions, "s3")["cwd"]);
    // 既有排序行为不变（updated_at DESC）。
    let ids: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s1", "s2", "s3", "s4"]);
}

/// 老库（没有 `spaces` 表）不返回空间字段，列表本身照旧可用。
#[test]
fn list_sessions_without_spaces_table_keeps_legacy_shape() {
    let env = ready_env("list-space-legacy");

    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], "sess-1");
    assert_eq!(sessions[0]["hasHistory"], true);
    // 老库不返回空间字段：前端据此回退到按 cwd 分组（与改造前一致）。
    assert!(sessions[0].get("spaceId").is_none());
    assert!(sessions[0].get("spaceName").is_none());
}

/// `spaces` 表在但 `sessions` 缺 `space_id` 列：同样整体回退，不做半套 JOIN。
#[test]
fn list_sessions_without_space_id_column_keeps_legacy_shape() {
    let env = ready_env("list-space-no-column");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE spaces (
             id TEXT PRIMARY KEY,
             user_id TEXT NOT NULL,
             name TEXT,
             deleted_at INTEGER
         );",
    )
    .unwrap();
    drop(conn);

    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert!(sessions[0].get("spaceId").is_none());
    assert!(sessions[0].get("spaceName").is_none());
}

/// 空间行缺失 / 已删除 / 属于别的账号时 `spaceName` 为 null（`spaceId` 仍在）。
///
/// 前端拿到 null 名字会回退成 cwd 文件夹名，不会出现「无名的空空间」。
#[test]
fn list_sessions_falls_back_when_space_row_missing_or_deleted() {
    let env = Env::new("list-space-fallback");
    env.create_db();
    create_spaces_table(&env);
    add_space(&env, "space-live", "uid-a", "ResumeForge", -1);
    add_space(&env, "space-deleted", "uid-a", "已删空间", 1700000000000);
    add_space(&env, "space-other", "uid-b", "别人的空间", -1);
    add_session_with_space(&env, "live", "uid-a", "/ws/a", Some("space-live"), 5000);
    add_session_with_space(&env, "gone", "uid-a", "/ws/b", Some("space-missing"), 4000);
    add_session_with_space(
        &env,
        "deleted",
        "uid-a",
        "/ws/c",
        Some("space-deleted"),
        3000,
    );
    add_session_with_space(
        &env,
        "other-user-space",
        "uid-a",
        "/ws/d",
        Some("space-other"),
        2000,
    );
    add_session_with_space(&env, "no-space", "uid-a", "/ws/e", None, 1000);

    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(item(&sessions, "live")["spaceName"], "ResumeForge");
    // 空间行不存在：spaceId 留着（前端可据此分组），名字缺失由前端回退。
    assert_eq!(item(&sessions, "gone")["spaceId"], "space-missing");
    assert!(item(&sessions, "gone")["spaceName"].is_null());
    // 软删除的空间（deleted_at != -1，真实库用时间戳）不算有效空间。
    assert_eq!(item(&sessions, "deleted")["spaceId"], "space-deleted");
    assert!(item(&sessions, "deleted")["spaceName"].is_null());
    // JOIN 带 user_id：别的账号的空间名不得泄漏给当前账号。
    assert!(item(&sessions, "other-user-space")["spaceName"].is_null());
    // 没有 space_id 的会话 spaceId 为 null（不是空串）。
    assert!(item(&sessions, "no-space")["spaceId"].is_null());
}

/// 加了 JOIN 之后，`user_id` / `deleted_at` / claw 工作区过滤与改造前完全一致。
#[test]
fn list_sessions_keeps_existing_filters_with_spaces_join() {
    let env = Env::new("list-space-filters");
    env.create_db();
    create_spaces_table(&env);
    add_space(&env, "space-a", "uid-a", "空间 A", -1);
    add_session_with_space(&env, "mine", "uid-a", "/ws/a", Some("space-a"), 3000);
    add_session_with_space(
        &env,
        "other-account",
        "uid-b",
        "/ws/a",
        Some("space-a"),
        2000,
    );
    add_session_with_space(
        &env,
        "soft-deleted",
        "uid-a",
        "/ws/a",
        Some("space-a"),
        1500,
    );
    add_session_with_space(
        &env,
        "claw",
        "uid-a",
        "/Users/me/WorkBuddy/Claw",
        Some("space-a"),
        1000,
    );
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET deleted_at = 1700000000000 WHERE id = 'soft-deleted'",
        [],
    )
    .unwrap();
    drop(conn);

    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    let ids: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["mine"], "user_id / deleted_at / claw 过滤保持不变");
}
