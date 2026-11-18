use super::paths::edge_sync_db_path;

// ---------------------------------------------------------------------------
// 基础路径与能力探测
// ---------------------------------------------------------------------------

/// A1 防回归：WorkBuddy 命名空间下三条关联路径必须与改造前**逐字相同**
/// （存量关联表 / 基线 / 锁都在这些名字上，改名即等于数据丢失）。
#[test]
fn workbuddy_link_paths_are_byte_identical_to_before() {
    let paths = SessionPaths::for_variant(WbVariant::Cn);
    assert!(paths.session_links_file().ends_with("session_links.json"));
    assert!(paths.session_links_dir().ends_with("session-links"));
    assert!(paths.baselines_dir().ends_with("session-links/baselines"));
    assert!(paths
        .preview_tokens_dir()
        .ends_with("session-links/previews"));
    assert!(paths.operations_dir().ends_with("session-links/operations"));
    assert!(paths
        .link_store_lock_file()
        .ends_with("locks/session-links.lock"));
    // 相对工具存储根逐段比对：写死分隔符会在 Windows 上反转断言方向。
    let root = std::env::temp_dir().join("workbuddy-shift-store");
    let at_root = SessionPaths {
        store_root: root.clone(),
        data_root: PathBuf::new(),
        auth_file: PathBuf::new(),
        link_namespace: LinkNamespace::WorkBuddy,
    };
    assert_eq!(
        at_root.session_links_file(),
        root.join("session_links.json")
    );
    assert_eq!(at_root.session_links_dir(), root.join("session-links"));
    assert_eq!(
        at_root.baselines_dir(),
        root.join("session-links").join("baselines")
    );
    assert_eq!(
        at_root.link_store_lock_file(),
        root.join("locks").join("session-links.lock")
    );
}

/// 命名空间隔离：VS Code 侧的关联表 / 目录 / 锁与 WorkBuddy 名字不同，
/// 且两者的存储根相同（同一 `~/.workbuddy-shift` 下并存而不互相污染）。
#[test]
fn vscode_link_paths_are_isolated_from_workbuddy() {
    let root = std::env::temp_dir().join("workbuddy-shift-store");
    let workbuddy = SessionPaths {
        store_root: root.clone(),
        data_root: PathBuf::new(),
        auth_file: PathBuf::new(),
        link_namespace: LinkNamespace::WorkBuddy,
    };
    let vscode = SessionPaths::for_vscode_ext_at(root.clone());
    assert_eq!(vscode.store_root, workbuddy.store_root);
    assert_eq!(
        vscode.session_links_file(),
        root.join("vscode_session_links.json")
    );
    assert_eq!(
        vscode.session_links_dir(),
        root.join("vscode-session-links")
    );
    assert_eq!(
        vscode.baselines_dir(),
        root.join("vscode-session-links").join("baselines")
    );
    assert_eq!(
        vscode.link_store_lock_file(),
        root.join("locks").join("vscode-session-links.lock")
    );
    assert_eq!(
        vscode.preview_tokens_dir(),
        root.join("vscode-session-links").join("previews")
    );
    assert_eq!(
        vscode.operations_dir(),
        root.join("vscode-session-links").join("operations")
    );
    assert_ne!(vscode.session_links_file(), workbuddy.session_links_file());
    assert_ne!(vscode.session_links_dir(), workbuddy.session_links_dir());
    assert_ne!(vscode.baselines_dir(), workbuddy.baselines_dir());
    assert_ne!(vscode.preview_tokens_dir(), workbuddy.preview_tokens_dir());
    assert_ne!(vscode.operations_dir(), workbuddy.operations_dir());
    assert_ne!(
        vscode.link_store_lock_file(),
        workbuddy.link_store_lock_file()
    );
    // 默认命名空间是 WorkBuddy：`SessionPaths::for_variant` 之外的历史构造点
    // 不会因为新增字段而漂移到 VS Code 名字上。
    assert_eq!(LinkNamespace::default(), LinkNamespace::WorkBuddy);
}

/// 命名空间隔离：CodeBuddy IDE 侧的关联表 / 目录 / 锁与 WorkBuddy、VS Code 都不同，
/// 三个目标在同一 `~/.workbuddy-shift` 下并存而不互相污染。
#[test]
fn codebuddy_ide_link_paths_are_isolated_from_other_namespaces() {
    let root = std::env::temp_dir().join("workbuddy-shift-store");
    let workbuddy = SessionPaths {
        store_root: root.clone(),
        data_root: PathBuf::new(),
        auth_file: PathBuf::new(),
        link_namespace: LinkNamespace::WorkBuddy,
    };
    let vscode = SessionPaths::for_vscode_ext_at(root.clone());
    let ide = SessionPaths::for_codebuddy_ide_at(root.clone());
    assert_eq!(ide.store_root, workbuddy.store_root);
    assert_eq!(
        ide.session_links_file(),
        root.join("codebuddy_ide_session_links.json")
    );
    assert_eq!(
        ide.session_links_dir(),
        root.join("codebuddy-ide-session-links")
    );
    assert_eq!(
        ide.baselines_dir(),
        root.join("codebuddy-ide-session-links").join("baselines")
    );
    assert_eq!(
        ide.link_store_lock_file(),
        root.join("locks").join("codebuddy-ide-session-links.lock")
    );
    assert_eq!(
        ide.preview_tokens_dir(),
        root.join("codebuddy-ide-session-links").join("previews")
    );
    assert_eq!(
        ide.operations_dir(),
        root.join("codebuddy-ide-session-links").join("operations")
    );
    for other in [&workbuddy, &vscode] {
        assert_ne!(ide.session_links_file(), other.session_links_file());
        assert_ne!(ide.session_links_dir(), other.session_links_dir());
        assert_ne!(ide.baselines_dir(), other.baselines_dir());
        assert_ne!(ide.preview_tokens_dir(), other.preview_tokens_dir());
        assert_ne!(ide.operations_dir(), other.operations_dir());
        assert_ne!(ide.link_store_lock_file(), other.link_store_lock_file());
    }
}

#[test]
fn db_paths_follow_variant_data_root() {
    let cn = SessionPaths::for_variant(WbVariant::Cn);
    // Path::ends_with 按路径分量比较，Windows 上 `\` 与 `/` 等价；
    // 不要用 to_string_lossy().ends_with()——那会把分隔符写进断言。
    assert!(cn.workbuddy_db().ends_with(".workbuddy/workbuddy.db"));
    // 映射库文件名交给解析器：真实数据根可能是任意版本（WorkBuddy 5.6 已迁移到
    // v4 且 v2/v3 残留并存），这里只断言落在国内版数据根下的 edge-sync-mapping-*.db，
    // 具体发现规则由 edge_sync_db_picks_largest_discovered_version 用临时目录覆盖。
    let cn_edge = cn.edge_sync_db(WbVariant::Cn);
    assert_eq!(cn_edge.parent(), Some(cn.data_root.as_path()));
    assert!(cn_edge.to_string_lossy().contains("edge-sync-mapping-"));

    let ai = SessionPaths::for_variant(WbVariant::Ai);
    assert_eq!(ai.workbuddy_db().parent(), Some(ai.data_root.as_path()));
    assert_ne!(cn.workbuddy_db(), ai.workbuddy_db());
    // 国际版数据根与国内版不同构，默认文件名为 v4，且同样走动态发现。
    assert!(ai
        .edge_sync_db(WbVariant::Ai)
        .to_string_lossy()
        .ends_with("edge-sync-mapping-v4.db"));
    assert_ne!(
        cn.edge_sync_db(WbVariant::Cn),
        ai.edge_sync_db(WbVariant::Ai)
    );
    // 锁与关联存储都挂在工具存储根下，顺序固定为「档位锁 → 存储锁」。
    assert!(cn
        .variant_ops_lock_file(WbVariant::Cn)
        .ends_with("locks/session-ops-cn.lock"));
    assert_ne!(
        cn.variant_ops_lock_file(WbVariant::Cn),
        cn.variant_ops_lock_file(WbVariant::Ai)
    );
    assert!(cn
        .link_store_lock_file()
        .ends_with("locks/session-links.lock"));
}

/// 在临时数据根里放好给定文件，返回国内版（或指定档位）解析出的映射库文件名。
fn edge_sync_pick(variant: WbVariant, files: &[&str]) -> String {
    let root = temp_root("edge_sync_pick");
    std::fs::create_dir_all(&root).unwrap();
    for name in files {
        std::fs::write(root.join(name), b"stub").unwrap();
    }
    let paths = SessionPaths {
        data_root: root.clone(),
        ..SessionPaths::for_variant(variant)
    };
    let picked = paths
        .edge_sync_db(variant)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::remove_dir_all(&root).unwrap();
    picked
}

#[test]
fn edge_sync_db_picks_largest_discovered_version() {
    // 回归：WorkBuddy 客户端自行演进映射库文件名（本机实测 v2 迁移残留、v3、v4
    // 并存，v3 从未出现在本工具代码里）。路径必须动态发现最大版本号，写死任何
    // 名字都会再次失效——写死 v2 会把登记写进迁移残留库，云端归属随之丢失。
    // 六种组合：空 / 仅无后缀 / 仅 v2 / v2+v4 / v2+v3+v4 / 仅 v4。
    let cn = WbVariant::Cn;
    assert_eq!(edge_sync_pick(cn, &[]), "edge-sync-mapping-v2.db");
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping.db"]),
        "edge-sync-mapping.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v0.db"]),
        "edge-sync-mapping-v0.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v2.db"]),
        "edge-sync-mapping-v2.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v2.db", "edge-sync-mapping-v4.db"]),
        "edge-sync-mapping-v4.db"
    );
    assert_eq!(
        edge_sync_pick(
            cn,
            &[
                "edge-sync-mapping-v2.db",
                "edge-sync-mapping-v3.db",
                "edge-sync-mapping-v4.db",
            ]
        ),
        "edge-sync-mapping-v4.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v4.db"]),
        "edge-sync-mapping-v4.db"
    );

    // 伴生文件（-shm / -wal）不是候选；更高版本出现时自动适配。
    assert_eq!(
        edge_sync_pick(
            cn,
            &[
                "edge-sync-mapping-v4.db",
                "edge-sync-mapping-v4.db-shm",
                "edge-sync-mapping-v4.db-wal",
            ]
        ),
        "edge-sync-mapping-v4.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v4.db", "edge-sync-mapping-v5.db"]),
        "edge-sync-mapping-v5.db"
    );

    // 版本号只接受十进制数字：前导零仍按数值比较；非数字、溢出、大小写差异和
    // 额外前后缀都静默忽略，避免把相似但非 WorkBuddy 文件误当成映射库。
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v01.db"]),
        "edge-sync-mapping-v01.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v01.db", "edge-sync-mapping-v1.db"]),
        "edge-sync-mapping-v1.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v+9.db"]),
        "edge-sync-mapping-v2.db"
    );
    assert_eq!(
        edge_sync_pick(cn, &["edge-sync-mapping-v18446744073709551616.db"]),
        "edge-sync-mapping-v2.db"
    );
    assert_eq!(
        edge_sync_pick(
            cn,
            &[
                "edge-sync-mapping-vx.db",
                "Edge-sync-mapping-v9.db",
                "prefix-edge-sync-mapping-v9.db",
                "edge-sync-mapping-v9.db.bak",
                "edge-sync-mapping-v9-extra.db",
            ]
        ),
        "edge-sync-mapping-v2.db"
    );

    // 国际版走同一套发现逻辑（不再写死 v4）。
    assert_eq!(
        edge_sync_pick(WbVariant::Ai, &[]),
        "edge-sync-mapping-v4.db"
    );
    assert_eq!(
        edge_sync_pick(
            WbVariant::Ai,
            &["edge-sync-mapping-v4.db", "edge-sync-mapping-v6.db"]
        ),
        "edge-sync-mapping-v6.db"
    );

    // 目录不存在：安全回落到默认文件名，不 panic（调用方据此报「云端映射库不存在」）。
    let missing = temp_root("edge_sync_missing");
    assert_eq!(
        edge_sync_db_path(&missing, cn),
        missing.join("edge-sync-mapping-v2.db")
    );
    assert_eq!(
        edge_sync_db_path(&missing, WbVariant::Ai),
        missing.join("edge-sync-mapping-v4.db")
    );
}

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "wb_shift_session_root_{}_{name}",
        uuid::Uuid::new_v4().simple()
    ))
}

fn create_sessions_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT, title TEXT);")
        .unwrap();
}

/// 能力探测：`projects/` 目录 + `workbuddy.db` 的 `sessions` 表同时存在才可用。
#[test]
fn session_copy_capability_probe() {
    let bare = temp_root("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(!session_copy_supported_at(&bare));

    let only_projects = temp_root("only-projects");
    std::fs::create_dir_all(only_projects.join("projects")).unwrap();
    assert!(!session_copy_supported_at(&only_projects));

    let empty_db = temp_root("empty-db");
    std::fs::create_dir_all(&empty_db).unwrap();
    let conn = Connection::open(empty_db.join("workbuddy.db")).unwrap();
    conn.execute_batch("CREATE TABLE other (x INTEGER);")
        .unwrap();
    drop(conn);
    assert!(!session_copy_supported_at(&empty_db));

    let db_only = temp_root("db-only");
    std::fs::create_dir_all(&db_only).unwrap();
    create_sessions_db(&db_only.join("workbuddy.db"));
    assert!(!session_copy_supported_at(&db_only));

    let ready = temp_root("ready");
    std::fs::create_dir_all(ready.join("projects")).unwrap();
    create_sessions_db(&ready.join("workbuddy.db"));
    assert!(session_copy_supported_at(&ready));

    for dir in [bare, only_projects, empty_db, db_only, ready] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 能力不满足时返回明确错误，且不写任何文件。
#[test]
fn copy_sessions_for_switch_rejects_unsupported_root() {
    let env = Env::new("unsupported");
    let bare = Env {
        root: env.root.clone(),
        paths: SessionPaths {
            store_root: env.root.join("bare-store"),
            data_root: env.root.join("bare-data"),
            auth_file: env.root.join("auth.info"),
            link_namespace: LinkNamespace::WorkBuddy,
        },
    };
    std::fs::create_dir_all(bare.paths.data_root.clone()).unwrap();
    let err = copy_sessions_for_switch_at(
        &bare.paths(),
        WbVariant::Ai,
        &json!({"id": "ai-1", "uid": "u-ai", "variant": "ai"}),
        &["cid-1".to_string()],
        |_| false,
    )
    .expect_err("不支持的档位必须返回错误");
    assert!(err.contains(SESSION_COPY_UNSUPPORTED), "{err}");
    assert_eq!(std::fs::read_dir(&bare.paths.data_root).unwrap().count(), 0);
}

/// 能力探测只对国际版生效：国内版在探测不通过的数据根上仍走改造前的路径。
#[test]
fn session_copy_probe_only_gates_ai() {
    let env = Env::new("cn-no-probe");
    let bare = SessionPaths {
        store_root: env.root.join("bare-store"),
        data_root: env.root.join("bare-data"),
        auth_file: env.root.join("auth.info"),
        link_namespace: LinkNamespace::WorkBuddy,
    };
    std::fs::create_dir_all(&bare.data_root).unwrap();

    let cn_err = copy_sessions_for_switch_at(
        &bare,
        WbVariant::Cn,
        &json!({"id": "cn-1", "variant": "cn", "uid": "   "}),
        &["cid-1".to_string()],
        |_| false,
    )
    .expect_err("缺 uid 仍必须返回错误");
    assert_eq!(cn_err, "目标账号缺少 uid，无法复制会话");
    assert!(
        !cn_err.contains(SESSION_COPY_UNSUPPORTED),
        "国内版不得被能力探测拦截: {cn_err}"
    );

    let ai_err = copy_sessions_for_switch_at(
        &bare,
        WbVariant::Ai,
        &json!({"id": "ai-1", "uid": "u-ai", "variant": "ai"}),
        &["cid-1".to_string()],
        |_| false,
    )
    .expect_err("国际版不满足能力探测必须返回错误");
    assert!(ai_err.contains(SESSION_COPY_UNSUPPORTED), "{ai_err}");
}

/// 能力可用时继续走 uid 校验（证明探测不会误短路）。
#[test]
fn copy_sessions_for_switch_requires_target_uid() {
    let env = ready_env("requires-uid");
    let err = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &json!({"id": "a-1", "variant": "cn", "uid": "   "}),
        &["sess-1".to_string()],
        |_| false,
    )
    .expect_err("缺 uid 必须返回错误");
    assert_eq!(err, "目标账号缺少 uid，无法复制会话");
}

/// 未登录 / 目标即当前账号：拒绝且不写任何东西。
#[test]
fn copy_rejects_missing_login_and_same_account() {
    let env = ready_env("login-checks");
    std::fs::remove_file(&env.paths.auth_file).unwrap();
    let err = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        |_| false,
    )
    .expect_err("缺登录态必须报错");
    assert!(err.contains("未读取到本机登录态"), "{err}");

    env.set_login("uid-b");
    let err = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        |_| false,
    )
    .expect_err("同一账号必须报错");
    assert!(err.contains("当前账号与目标账号相同"), "{err}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

/// 插件命名空间没有档位数据根（`data_root` 为空）：存活判定必须是「无依据」。
///
/// `SessionPaths::workbuddy_db()` 在空 `data_root` 下会退化成相对路径 `workbuddy.db`
/// （即「当前工作目录下的库」）。若不加这条早退，插件组的存活判定会去读一个与本进程
/// 启动目录相关的文件：既不可信，也会让结果随启动位置漂移。这里直接锁死该分支。
#[test]
fn live_session_rows_has_no_index_without_a_data_root() {
    let store_root = temp_root("live_rows_no_data_root");
    std::fs::create_dir_all(&store_root).unwrap();
    let paths = SessionPaths::for_vscode_ext_at(store_root.clone());
    assert!(paths.data_root.as_os_str().is_empty());
    assert_eq!(paths.workbuddy_db(), PathBuf::from("workbuddy.db"));

    let mut live = LiveSessionRows::new();
    assert!(
        live.live_ids(&paths, "uid-a").is_none(),
        "空 data_root 必须视为「无判定依据」（None），不能返回空集"
    );
    // 缓存里存的也是「无依据」：再查一次不会变成空集（空集会让整片列表被过滤掉）。
    assert!(live.live_ids(&paths, "uid-a").is_none());

    let _ = std::fs::remove_dir_all(&store_root);
}
