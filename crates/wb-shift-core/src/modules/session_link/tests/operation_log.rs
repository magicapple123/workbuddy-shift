// -----------------------------------------------------------------------
// 操作日志与锁
// -----------------------------------------------------------------------

#[test]
fn operations_round_trip_filter_by_variant_and_prune() {
    let dir = TempDir::new("ops");
    let paths = temp_paths(&dir);
    save_operation(
        &paths,
        &operation("op-cn", OpPhase::Prepared, "uid-b", "s1"),
    )
    .unwrap();
    let mut ai = operation("op-ai", OpPhase::Prepared, "uid-b", "s2");
    ai.variant = WbVariant::Ai;
    save_operation(&paths, &ai).unwrap();
    save_operation(
        &paths,
        &operation("op-done", OpPhase::Completed, "uid-b", "s3"),
    )
    .unwrap();

    let pending = pending_operations(&paths, WbVariant::Cn);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, "op-cn");
    assert_eq!(pending_operations(&paths, WbVariant::Ai).len(), 1);

    assert!(find_pending_operation(&pending, "uid-a", "sess-1", "uid-b").is_some());
    assert!(find_pending_operation(&pending, "uid-a", "sess-1", "uid-c").is_none());

    // 损坏的操作文件必须显式上报，不能当成没有。
    std::fs::write(paths.operations_dir().join("broken.json"), "not-json").unwrap();
    assert_eq!(scan_operations(&paths).problems.len(), 1);

    // 清理只删已完成的历史，保留未完成。
    for index in 0..5 {
        let mut done = operation(&format!("done-{index}"), OpPhase::Completed, "uid-b", "s9");
        done.updated_at = 100 + index;
        save_operation(&paths, &done).unwrap();
    }
    assert!(prune_operations(&paths, WbVariant::Cn, 2) >= 4);
    assert!(paths.operations_dir().join("op-cn.json").exists());
}

/// 仍被维护记录引用的操作日志不得被数量裁剪丢弃：补清理的唯一依据必须保留。
#[test]
fn prune_operations_keeps_logs_referenced_by_lifecycle_records() {
    let dir = TempDir::new("prune-lifecycle");
    let paths = temp_paths(&dir);
    let kept_id = uuid::Uuid::new_v4().to_string();
    let drop_id = uuid::Uuid::new_v4().to_string();
    save_operation(
        &paths,
        &operation(&kept_id, OpPhase::Completed, "uid-b", "s1"),
    )
    .unwrap();
    save_operation(
        &paths,
        &operation(&drop_id, OpPhase::Completed, "uid-b", "s2"),
    )
    .unwrap();
    session_backup::save_lifecycle(
        &paths,
        &session_backup::BackupLifecycle {
            version: session_backup::LIFECYCLE_VERSION,
            operation_id: kept_id.clone(),
            variant: WbVariant::Cn,
            kind: "copy".to_string(),
            state: session_backup::BackupState::CleanupPending,
            created_at: 1,
            updated_at: 1,
            last_error: Some("临时目录删除失败：权限不足".to_string()),
            session_id: None,
            title: None,
        },
    )
    .unwrap();

    // keep = 0：普通已完成日志被裁剪，带维护记录的那条必须留下。
    let removed = prune_operations(&paths, WbVariant::Cn, 0);
    assert_eq!(removed, 1);
    let ids: Vec<String> = scan_operations(&paths)
        .operations
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect();
    assert_eq!(ids, vec![kept_id]);
}

/// 维护记录损坏时状态不明：不得把「解析失败的记录」当成未引用而去裁剪日志。
#[test]
fn prune_operations_skips_when_lifecycle_scan_is_incomplete() {
    let dir = TempDir::new("prune-damaged");
    let paths = temp_paths(&dir);
    let drop_id = uuid::Uuid::new_v4().to_string();
    save_operation(
        &paths,
        &operation(&drop_id, OpPhase::Completed, "uid-b", "s1"),
    )
    .unwrap();
    let lifecycle_dir = session_backup::lifecycle_root(&paths, WbVariant::Cn);
    std::fs::create_dir_all(&lifecycle_dir).unwrap();
    std::fs::write(lifecycle_dir.join("broken.json"), b"{not json").unwrap();

    let removed = prune_operations(&paths, WbVariant::Cn, 0);
    assert_eq!(removed, 0, "扫描不完整时不得裁剪任何操作日志");
    assert!(paths
        .operations_dir()
        .join(format!("{drop_id}.json"))
        .exists());
}

/// 锁失败文案前缀常量与 [`LockError::message`] 的输出一致：宿主据此判断锁失败，
/// 不再嗅探整句错误文案。
#[test]
fn lock_failure_prefixes_match_error_messages() {
    let busy = LockError::Busy.message(VARIANT_OPS_LOCK_NAME);
    assert!(busy.starts_with(LOCK_BUSY_MESSAGE_PREFIX), "{busy}");
    let unavailable =
        LockError::Unavailable("磁盘异常".to_string()).message(VARIANT_OPS_LOCK_NAME);
    assert!(
        unavailable.starts_with(LOCK_UNAVAILABLE_MESSAGE_PREFIX),
        "{unavailable}"
    );
    // 其它锁对象的文案不得被误判成档位锁失败。
    assert!(!LockError::Busy
        .message("关联存储")
        .starts_with(LOCK_BUSY_MESSAGE_PREFIX));
}

#[test]
fn second_lock_handle_is_busy_until_first_releases() {
    let dir = TempDir::new("lock-fd");
    let path = dir.path().join("a.lock");
    let held = try_lock_file(&path).unwrap();
    match try_lock_file(&path) {
        Err(LockError::Busy) => {}
        Err(LockError::Unavailable(reason)) => panic!("期望 Busy，实际 Unavailable: {reason}"),
        Ok(_) => panic!("第二个句柄不应拿到同一把锁"),
    }
    drop(held);
    assert!(try_lock_file(&path).is_ok());
}

/// 子进程助手：设置 WB_SHIFT_TEST_HOLD_LOCK_MS 时持锁并写就绪文件。
#[test]
fn lock_holder_child_process() {
    let Ok(hold_ms) = std::env::var("WB_SHIFT_TEST_HOLD_LOCK_MS") else {
        return;
    };
    let lock_path = std::env::var("WB_SHIFT_TEST_LOCK_PATH").expect("lock path");
    let ready_path = std::env::var("WB_SHIFT_TEST_READY_PATH").expect("ready path");
    let _lock = try_lock_file(Path::new(&lock_path)).expect("child must acquire lock");
    std::fs::write(&ready_path, b"ready").unwrap();
    let hold_ms: u64 = hold_ms.parse().unwrap_or(2000);
    std::thread::sleep(Duration::from_millis(hold_ms));
}

/// 跨进程互斥：另一个进程持锁时本进程只能得到 Busy。
#[test]
fn cross_process_lock_is_busy() {
    let dir = TempDir::new("lock-proc");
    let lock_path = dir.path().join("cross.lock");
    let ready_path = dir.path().join("ready");
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Ok(mut child) = std::process::Command::new(exe)
        .args([
            "--exact",
            "modules::session_link::tests::lock_holder_child_process",
            "--nocapture",
        ])
        .env("WB_SHIFT_TEST_HOLD_LOCK_MS", "2000")
        .env("WB_SHIFT_TEST_LOCK_PATH", &lock_path)
        .env("WB_SHIFT_TEST_READY_PATH", &ready_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };

    let mut waited_ms = 0u64;
    while !ready_path.exists() && waited_ms < 5000 {
        std::thread::sleep(Duration::from_millis(20));
        waited_ms += 20;
    }
    if !ready_path.exists() {
        // 子进程没起来（受限环境）：不误报，跳过。
        let _ = child.kill();
        let _ = child.wait();
        return;
    }

    match try_lock_file(&lock_path) {
        Err(LockError::Busy) => {}
        Err(LockError::Unavailable(reason)) => panic!("期望 Busy，实际 Unavailable: {reason}"),
        Ok(_) => panic!("跨进程必须互斥：另一个进程持锁时不应拿到锁"),
    }
    let _ = child.kill();
    let _ = child.wait();
}
