// 工作记忆（`MEMORY.md`）跨账号同步（issue #209）：只追加不覆盖、按行去重、
// 源缺失 / 目标账号目录缺失 no-op、写前备份原字节、未勾选时完全不执行。
//
// 全部用例在临时目录里造 `<数据根>/user-<uid>-personal/MEMORY.md`，
// 备份根也注入临时目录（`*_with_backup_root`），不触碰真实 `~/.workbuddy-shift`
// 与真实客户端数据。

/// 独立临时数据根（调用方负责清理）。
fn memory_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "wb-memory-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

/// 共用数据根的 `SessionPaths`（测试用：只区分数据根与存储根）。
fn memory_paths(root: &Path) -> SessionPaths {
    SessionPaths {
        data_root: root.join("data"),
        store_root: root.join("store"),
        ..SessionPaths::for_variant(WbVariant::Cn)
    }
}

/// 工作记忆文件位置：`<数据根>/user-<uid>-personal/MEMORY.md`（不在 `storage/` 下）。
fn memory_file_at(root: &Path, uid: &str) -> PathBuf {
    root.join("data")
        .join(format!("user-{uid}-personal"))
        .join("MEMORY.md")
}

fn write_memory(root: &Path, uid: &str, content: &str) {
    let file = memory_file_at(root, uid);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, content).unwrap();
}

fn read_memory(root: &Path, uid: &str) -> String {
    std::fs::read_to_string(memory_file_at(root, uid)).unwrap()
}

#[test]
fn memory_appends_only_missing_lines_and_keeps_target_intact() {
    let root = memory_root("append");
    write_memory(
        &root,
        "u-src",
        "# 长期偏好\n\n- 用 pnpm\n- 先读 AGENTS.md\n",
    );
    write_memory(&root, "u-dst", "# 长期偏好\n\n- 目标自己的偏好\n");
    let paths = memory_paths(&root);

    let outcome =
        sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &root.join("backup"));

    assert_eq!(
        outcome,
        MemoryOutcome {
            appended_lines: 2,
            skipped_reason: None,
        }
    );
    // 目标原有内容逐字保留在开头，新行按源顺序追加在末尾。
    assert_eq!(
        read_memory(&root, "u-dst"),
        "# 长期偏好\n\n- 目标自己的偏好\n- 用 pnpm\n- 先读 AGENTS.md\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_dedupes_target_and_source_duplicate_lines() {
    let root = memory_root("dedupe");
    write_memory(&root, "u-src", "- a\n- b\n- a\n\n- c\n");
    write_memory(&root, "u-dst", "- b\n- mine\n");
    let paths = memory_paths(&root);
    let backup_root = root.join("backup");

    let outcome = sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &backup_root);
    // `- b` 目标已有；源内部重复的 `- a` 只搬一次；空行不搬。
    assert_eq!(outcome.appended_lines, 2, "{outcome:?}");
    assert_eq!(read_memory(&root, "u-dst"), "- b\n- mine\n- a\n- c\n");

    // 幂等：同一份内容再跑一次不再追加，也不备份（没有写入）。
    let again = sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &backup_root);
    assert_eq!(again, MemoryOutcome::default());
    assert_eq!(read_memory(&root, "u-dst"), "- b\n- mine\n- a\n- c\n");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_missing_source_is_noop() {
    let root = memory_root("no-source");
    // 源账号从没写过工作记忆：不报错、不创建目标文件。
    std::fs::create_dir_all(memory_file_at(&root, "u-dst").parent().unwrap()).unwrap();
    let paths = memory_paths(&root);

    let outcome =
        sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &root.join("backup"));

    assert_eq!(outcome, MemoryOutcome::default());
    assert!(!memory_file_at(&root, "u-dst").exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_missing_target_account_dir_is_noop() {
    let root = memory_root("no-dir");
    write_memory(&root, "u-src", "- 用 pnpm\n");
    let paths = memory_paths(&root);

    // 目标账号目录不存在 = 客户端从没为该账号落过数据：不凭空造目录。
    let outcome =
        sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &root.join("backup"));

    assert_eq!(outcome, MemoryOutcome::default());
    assert!(!memory_file_at(&root, "u-dst").exists());
    assert!(!memory_file_at(&root, "u-dst").parent().unwrap().exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_creates_target_file_when_account_dir_exists() {
    let root = memory_root("create");
    write_memory(&root, "u-src", "- 用 pnpm\n- 先读 AGENTS.md\n");
    // 账号目录在（该账号用过客户端），但没有 MEMORY.md：可以直接创建一份追加内容。
    std::fs::create_dir_all(memory_file_at(&root, "u-dst").parent().unwrap()).unwrap();
    let paths = memory_paths(&root);

    let outcome =
        sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &root.join("backup"));

    assert_eq!(outcome.appended_lines, 2, "{outcome:?}");
    assert_eq!(read_memory(&root, "u-dst"), "- 用 pnpm\n- 先读 AGENTS.md\n");
    // 目标原本不存在：没有可回滚的原字节，不做空备份。
    assert!(!root.join("backup").join("u-dst-MEMORY.md").exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_backup_is_original_bytes() {
    let root = memory_root("backup");
    write_memory(&root, "u-src", "- a\n- b\n");
    // 目标是 CRLF 文件：备份必须逐字节相同，追加也沿用 CRLF（不掺 LF）。
    let original = "# 我自己的偏好\r\n- a\r\n";
    write_memory(&root, "u-dst", original);
    let paths = memory_paths(&root);
    let backup_root = root.join("backup");

    let outcome = sync_memory_with_backup_root(&paths, "u-src", &paths, "u-dst", &backup_root);

    assert_eq!(outcome.appended_lines, 1, "{outcome:?}");
    assert_eq!(
        std::fs::read(backup_root.join("u-dst-MEMORY.md")).unwrap(),
        original.as_bytes()
    );
    assert_eq!(
        read_memory(&root, "u-dst"),
        "# 我自己的偏好\r\n- a\r\n- b\r\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_not_checked_never_reads_source() {
    let unchecked = ready_env("memory_off");
    // 源「MEMORY.md」做成目录：真被读过就会得到 `skippedReason`，这是可观测的差异。
    std::fs::create_dir_all(
        unchecked
            .paths
            .data_root
            .join("user-uid-a-personal")
            .join("MEMORY.md"),
    )
    .unwrap();
    let ids: Vec<String> = vec!["sess-1".to_string()];

    // 未勾选：报告里完全没有 `memory` 字段（连源文件都没读）。
    let off = copy_sessions_for_switch_at_with_options(
        &unchecked.paths(),
        WbVariant::Cn,
        &unchecked.target("uid-b"),
        &ids,
        CopyOptions::new(false, false),
        |_| false,
    )
    .unwrap();
    assert_eq!(off["copied"].as_array().map(Vec::len), Some(1), "{off}");
    assert!(off.get("memory").is_none(), "{off}");

    // 同一夹具勾选后立刻出现 `memory`：证明上面的缺失来自「未执行」，而不是夹具不可观测。
    let checked = ready_env("memory_on");
    std::fs::create_dir_all(
        checked
            .paths
            .data_root
            .join("user-uid-a-personal")
            .join("MEMORY.md"),
    )
    .unwrap();
    let on = copy_sessions_for_switch_at_with_options(
        &checked.paths(),
        WbVariant::Cn,
        &checked.target("uid-b"),
        &ids,
        CopyOptions::new(false, true),
        |_| false,
    )
    .unwrap();
    assert!(
        on["memory"]["skippedReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("不是普通文件")),
        "{on}"
    );
    // 失败只落报告字段：正文复制结果不受影响。
    assert_eq!(on["copied"].as_array().map(Vec::len), Some(1), "{on}");
}
