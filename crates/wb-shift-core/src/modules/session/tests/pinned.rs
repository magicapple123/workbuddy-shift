// 置顶状态跨账号同步（issue #205）：id 映射、顺序保留、幂等、失败不覆盖、组内传导。
//
// 全部用例在临时目录里造 `storage/user-<uid>-personal/global/conversations.json`，
// 备份根也注入临时目录（`*_with_backup_root` / `*_with`），不触碰真实 `~/.workbuddy-shift`。

/// 独立临时数据根（调用方负责清理）。
fn pinned_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("wb-pinned-{label}-{}", uuid::Uuid::new_v4().simple()))
}

/// 共用数据根的 `SessionPaths`（测试用：只区分数据根与存储根）。
fn paths_at(root: &Path) -> SessionPaths {
    SessionPaths {
        data_root: root.join("data"),
        store_root: root.join("store"),
        ..SessionPaths::for_variant(WbVariant::Cn)
    }
}

fn conversations_path(root: &Path, uid: &str) -> PathBuf {
    root.join("data")
        .join("storage")
        .join(format!("user-{uid}-personal"))
        .join("global")
        .join("conversations.json")
}

fn write_conversations(root: &Path, uid: &str, content: &str) {
    let file = conversations_path(root, uid);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, content).unwrap();
}

/// 备份目录里的文件（按文件名排序 = 写入顺序）。
fn backup_files(backup_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(backup_root) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
}

fn pinned_ids(root: &Path, uid: &str) -> Vec<String> {
    let text = std::fs::read_to_string(conversations_path(root, uid)).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    value
        .get("pinned")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn pinned_copy_maps_ids_and_keeps_group_key() {
    let root = pinned_root("map");
    write_conversations(
        &root,
        "u-src",
        r#"{"pinned":[{"id":"s1","groupKey":"D:\\Proj"},{"id":"s2","groupKey":"D:\\Proj"}]}"#,
    );
    write_conversations(&root, "u-dst", r#"{"pinned":[]}"#);
    let paths = paths_at(&root);

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &root.join("backup"),
    );

    assert_eq!(outcome.added, 1, "{outcome:?}");
    assert_eq!(outcome.skipped_reason, None);
    assert_eq!(pinned_ids(&root, "u-dst"), vec!["t1"]);
    // groupKey 等未知字段原样带过去。
    let text = std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["pinned"][0]["groupKey"], json!("D:\\Proj"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pinned_is_idempotent_and_keeps_target_own_pins_in_place() {
    let root = pinned_root("idem");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1","groupKey":"g"}]}"#);
    write_conversations(
        &root,
        "u-dst",
        r#"{"pinned":[{"id":"own","groupKey":"x"},{"id":"t1","groupKey":"g"}]}"#,
    );
    let paths = paths_at(&root);
    let backup_root = root.join("backup");

    // 目标已有 t1：不重复追加，也不打乱它自己的顺序。
    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &backup_root,
    );
    assert_eq!(outcome.added, 0, "{outcome:?}");
    assert_eq!(pinned_ids(&root, "u-dst"), vec!["own", "t1"]);

    // 换成新的目标 id 再跑一次：只补缺的，已有顺序不变。
    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t2".to_string())],
        &backup_root,
    );
    assert_eq!(outcome.added, 1, "{outcome:?}");
    assert_eq!(pinned_ids(&root, "u-dst"), vec!["own", "t1", "t2"]);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pinned_missing_target_file_is_noop() {
    let root = pinned_root("miss");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1"}]}"#);
    // 目标账号从没写过置顶：不凭空造一个结构未知的 conversations.json。
    let paths = paths_at(&root);

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &root.join("backup"),
    );

    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.skipped_reason, None);
    assert!(!conversations_path(&root, "u-dst").exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pinned_corrupt_target_is_reported_and_left_untouched() {
    let root = pinned_root("bad");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1"}]}"#);
    let original = "{not json";
    write_conversations(&root, "u-dst", original);
    let paths = paths_at(&root);

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &root.join("backup"),
    );

    assert_eq!(outcome.added, 0);
    assert!(
        outcome
            .skipped_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("目标账号置顶记录无法解析")),
        "{outcome:?}"
    );
    // 坏 JSON 一律不覆盖，原字节保持。
    assert_eq!(
        std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap(),
        original
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pinned_backs_up_target_before_overwrite() {
    let root = pinned_root("bak");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1","groupKey":"g"}]}"#);
    let original = r#"{"pinned":[],"keepMe":true}"#;
    write_conversations(&root, "u-dst", original);
    let paths = paths_at(&root);
    let backup_root = root.join("backup");

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &backup_root,
    );

    assert_eq!(outcome.added, 1, "{outcome:?}");
    // 备份是写前原字节，可直接人工回滚（文件名带毫秒 + 序号，见下一个用例）。
    let backups = backup_files(&backup_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
    // 未知顶层键在改写后仍在。
    let text = std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["keepMe"], json!(true));

    let _ = std::fs::remove_dir_all(root);
}

/// 同一 `backup_root`（秒级日期目录）里连续两次同步：两次都要留下备份，
/// 第一次的备份必须仍是**原始字节**（修复前用固定文件名，第二份会把第一份覆盖成中间态）。
#[test]
fn pinned_backups_do_not_overwrite_each_other() {
    let root = pinned_root("bak-twice");
    write_conversations(
        &root,
        "u-src",
        r#"{"pinned":[{"id":"s1"},{"id":"s2"}]}"#,
    );
    let original = r#"{"pinned":[]}"#;
    write_conversations(&root, "u-dst", original);
    let paths = paths_at(&root);
    let backup_root = root.join("backup");

    let first = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &backup_root,
    );
    assert_eq!(first.added, 1, "{first:?}");
    // 第一次同步之后的目标内容 = 第二次同步的「写前」状态。
    let intermediate = std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap();

    let second = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s2".to_string(), "t2".to_string())],
        &backup_root,
    );
    assert_eq!(second.added, 1, "{second:?}");

    let backups = backup_files(&backup_root);
    assert_eq!(
        backups.len(),
        2,
        "两次同步必须留下两份备份（同一秒内也不能覆盖）：{backups:?}"
    );
    let mut contents: Vec<String> = backups
        .iter()
        .map(|file| std::fs::read_to_string(file).unwrap())
        .collect();
    contents.sort();
    let mut expected = vec![original.to_string(), intermediate];
    expected.sort();
    assert_eq!(contents, expected, "第一份必须是原始字节：{backups:?}");
    // 文件名排序 = 写入顺序（毫秒 + 4 位补零序号）。
    assert!(
        backups
            .windows(2)
            .all(|pair| pair[0].file_name() < pair[1].file_name()),
        "{backups:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// 目标顶层不是对象 → 报结构异常原因，**不写文件**（修复前是 `added = 0` 且无原因）。
#[test]
fn pinned_non_object_target_is_reported_and_left_untouched() {
    let root = pinned_root("shape-top");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1"}]}"#);
    let original = r#"[{"id":"s1"}]"#;
    write_conversations(&root, "u-dst", original);
    let paths = paths_at(&root);
    let backup_root = root.join("backup");

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &backup_root,
    );

    assert_eq!(outcome.added, 0, "{outcome:?}");
    assert_eq!(
        outcome.skipped_reason.as_deref(),
        Some("目标账号置顶记录结构异常（顶层不是对象）"),
        "{outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap(),
        original,
        "结构异常一律不覆盖"
    );
    assert!(backup_files(&backup_root).is_empty(), "没写入就不该有备份");

    let _ = std::fs::remove_dir_all(root);
}

/// `pinned` 存在但不是数组（`{"pinned":null}`）→ 报结构异常原因，**不写文件**。
#[test]
fn pinned_non_array_pinned_is_reported_and_left_untouched() {
    let root = pinned_root("shape-pinned");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1"}]}"#);
    let original = r#"{"pinned":null,"keepMe":1}"#;
    write_conversations(&root, "u-dst", original);
    let paths = paths_at(&root);
    let backup_root = root.join("backup");

    let outcome = sync_pinned_with_backup_root(
        &paths,
        "u-src",
        &paths,
        "u-dst",
        &[("s1".to_string(), "t1".to_string())],
        &backup_root,
    );

    assert_eq!(outcome.added, 0, "{outcome:?}");
    assert_eq!(
        outcome.skipped_reason.as_deref(),
        Some("目标账号置顶记录结构异常（pinned 不是数组）"),
        "{outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(conversations_path(&root, "u-dst")).unwrap(),
        original
    );
    assert!(backup_files(&backup_root).is_empty(), "没写入就不该有备份");

    let _ = std::fs::remove_dir_all(root);
}

fn pinned_member(
    member_id: &str,
    uid: &str,
    session_id: &str,
    state: MemberState,
) -> LinkMember {
    LinkMember {
        member_id: member_id.to_string(),
        account_id: None,
        uid: uid.to_string(),
        session_id: session_id.to_string(),
        variant: Some(WbVariant::Cn),
        state,
        linked_at: 0,
        last_synced_at: None,
    }
}

#[test]
fn pinned_group_sync_propagates_to_active_members_only() {
    let root = pinned_root("grp");
    write_conversations(&root, "u-src", r#"{"pinned":[{"id":"s1","groupKey":"g"}]}"#);
    write_conversations(&root, "u-dst", r#"{"pinned":[]}"#);
    write_conversations(&root, "u-stale", r#"{"pinned":[]}"#);

    let mut store = LinkStore::empty();
    store.groups.push(LinkGroup {
        id: "g1".to_string(),
        variant: WbVariant::Cn,
        created_at: 0,
        members: vec![
            pinned_member("m1", "u-src", "s1", MemberState::Active),
            pinned_member("m2", "u-dst", "t1", MemberState::Active),
            pinned_member("m3", "u-stale", "x1", MemberState::Stale),
        ],
        pair_bases: Vec::new(),
    });

    let root_for_closure = root.clone();
    let outcome = sync_pinned_for_groups_with(
        &store,
        "u-src",
        &["g1".to_string()],
        &root.join("backup"),
        move |_variant| paths_at(&root_for_closure),
    );

    assert_eq!(outcome.added, 1, "{outcome:?}");
    assert_eq!(pinned_ids(&root, "u-dst"), vec!["t1"]);
    // stale 成员不参与：给废弃副本置顶只会在账号列表留一条指向已失效会话的条目。
    assert_eq!(pinned_ids(&root, "u-stale"), Vec::<String>::new());

    let _ = std::fs::remove_dir_all(root);
}
