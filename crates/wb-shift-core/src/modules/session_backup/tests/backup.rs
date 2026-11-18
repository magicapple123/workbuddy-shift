    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wb_shift_backup_test_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn paths(&self) -> SessionPaths {
            SessionPaths {
                store_root: self.0.join("store"),
                data_root: self.0.join("data"),
                link_namespace: crate::modules::session::LinkNamespace::WorkBuddy,
                auth_file: self.0.join("auth.info"),
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // 失败注入用例可能把目录改成只读，先恢复权限再清理。
            #[cfg(unix)]
            restore_permissions(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn restore_permissions(root: &Path) {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = std::fs::symlink_metadata(&path) {
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        let mut permissions = meta.permissions();
                        permissions.set_mode(0o755);
                        let _ = std::fs::set_permissions(&path, permissions);
                        restore_permissions(&path);
                    }
                }
            }
        }
    }

    fn operation(id: &str, phase: OpPhase, lifecycle: Option<u32>) -> Operation {
        Operation {
            version: OPERATION_VERSION,
            operation_id: id.to_string(),
            kind: "copy".to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id: "g-1".to_string(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-a".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "sess-b".to_string(),
            },
            expected_content_digest: "digest".to_string(),
            expected_record_count: 1,
            phase,
            backup: None,
            lifecycle_version: lifecycle,
            cleanup_state: None,
            last_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn durable_write_persists_and_replaces_atomically() {
        let dir = TempDir::new("durable");
        let target = dir.0.join("nested").join("file.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        durable_write_str(&target, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "first");
        durable_write_str(&target, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        // 没有残留的临时文件（rename 语义）。
        let leftovers = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn begin_operation_rejects_duplicate_directory() {
        let dir = TempDir::new("duplicate");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let created = transaction_dir(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        assert!(created.is_dir());
        // 复用同一目录必须失败：不允许两个操作共用目录。
        assert!(std::fs::create_dir(&created).is_err());
    }

    #[test]
    fn allocating_without_operation_log_is_reclaimed() {
        let dir = TempDir::new("allocating");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        std::fs::write(
            transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .join("workbuddy.db"),
            b"partial",
        )
        .unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(
            !transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .exists()
        );
        assert!(
            scan_lifecycle(&paths).records.is_empty(),
            "回收后维护记录必须删除"
        );
    }

    #[test]
    fn protected_without_operation_log_is_preserved_and_reported() {
        let dir = TempDir::new("protected");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        mark_protected(&paths, &mut record).unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].state, "needsRecovery");
        assert!(
            transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .is_dir(),
            "缺日志不得作为删除依据"
        );
    }

    /// 回归（issue #76）：备份目录里有真实文件时，「转 protected」必须成功。
    ///
    /// Windows 的 `FlushFileBuffers` 要求句柄具备写权限，而 `File::open` 只申请
    /// `GENERIC_READ`：只读句柄上 `sync_all` 必定返回 `ERROR_ACCESS_DENIED(5)`，导致
    /// Windows 上复制会话 100% 报「备份目录持久化失败：拒绝访问。(os error 5)」。
    /// 目录为空时不会触发（空目录不进入文件分支），所以必须显式写入真实文件。
    #[test]
    fn sync_tree_flushes_files_before_protected() {
        let dir = TempDir::new("flush");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let target = transaction_dir(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::write(target.join("workbuddy.db"), b"snapshot").unwrap();
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(target.join("nested").join("workbuddy.db-wal"), b"wal").unwrap();

        sync_tree(&target).expect("备份目录必须能持久化其中的文件");
        mark_protected(&paths, &mut record).expect("备份目录持久化必须成功");
        assert_eq!(record.state, BackupState::Protected);
    }

    #[test]
    fn protected_with_completed_log_is_swept_but_unfinished_is_not() {
        let dir = TempDir::new("sweep");
        let paths = dir.paths();

        let mut done = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        std::fs::write(
            transaction_dir(&paths, WbVariant::Cn, &done.operation_id)
                .unwrap()
                .join("workbuddy.db"),
            b"snapshot",
        )
        .unwrap();
        mark_protected(&paths, &mut done).unwrap();
        let mut log = operation(&done.operation_id, OpPhase::Completed, Some(1));
        log.backup = Some("backups/session-transactions/cn/x".to_string());
        session_link::save_operation(&paths, &log).unwrap();

        let mut stuck = begin_operation(&paths, WbVariant::Cn, "sync", None, None).unwrap();
        mark_protected(&paths, &mut stuck).unwrap();
        session_link::save_operation(
            &paths,
            &operation(&stuck.operation_id, OpPhase::BodyWritten, Some(1)),
        )
        .unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].operation_id, stuck.operation_id);
        assert!(
            !transaction_dir(&paths, WbVariant::Cn, &done.operation_id)
                .unwrap()
                .exists(),
            "完成的业务必须被补清理"
        );
        assert!(transaction_dir(&paths, WbVariant::Cn, &stuck.operation_id)
            .unwrap()
            .is_dir());
        // 已清理的日志：backup=null 且 cleanupState=cleaned。
        let stored = session_link::scan_operations(&paths)
            .operations
            .into_iter()
            .find(|operation| operation.operation_id == done.operation_id)
            .unwrap();
        assert!(stored.backup.is_none());
        assert_eq!(stored.cleanup_state.as_deref(), Some(CLEANUP_STATE_CLEANED));
    }

    #[test]
    fn cleared_log_without_lifecycle_marker_is_preserved() {
        let dir = TempDir::new("legacy-mark");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        mark_protected(&paths, &mut record).unwrap();
        // 旧格式日志（无生命周期标记）不能被当作删除授权。
        session_link::save_operation(
            &paths,
            &operation(&record.operation_id, OpPhase::Completed, None),
        )
        .unwrap();
        let issues = maintain(&paths, WbVariant::Cn);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].reason.contains("生命周期标记"), "{issues:?}");
        assert!(transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
            .unwrap()
            .is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_transaction_dir_is_preserved() {
        let dir = TempDir::new("symlink");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let target = transaction_dir(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::remove_dir(&target).unwrap();
        let outside = dir.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();

        let outcome = cleanup_operation(&paths, WbVariant::Cn, &record);
        match outcome {
            CleanupOutcome::Protected { reason } => assert!(reason.contains("符号链接")),
            other => panic!("必须拒绝符号链接：{other:?}"),
        }
        assert!(outside.join("keep.txt").exists(), "链接目标不得被删除");
    }

    #[cfg(unix)]
    #[test]
    fn permission_failure_keeps_record_for_retry() {
        let dir = TempDir::new("perm");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let root = transactions_root(&paths, WbVariant::Cn);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();

        let outcome = cleanup_operation(&paths, WbVariant::Cn, &record);
        match outcome {
            CleanupOutcome::Pending { reason } => assert!(reason.contains("删除失败"), "{reason}"),
            other => panic!("只读目录必须报告待重试：{other:?}"),
        }
        assert!(referenced_operation_ids(&paths)
            .expect("维护记录扫描必须完整")
            .contains(&record.operation_id));

        // 解除故障后：幂等补清理成功，不再有上报项。
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(scan_lifecycle(&paths).records.is_empty());
    }

    #[test]
    fn partial_delete_is_idempotent() {
        let dir = TempDir::new("partial");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let target = transaction_dir(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::create_dir_all(target.join("bodies")).unwrap();
        std::fs::write(target.join("bodies").join("original.jsonl"), b"x").unwrap();
        // 模拟「删除到一半」：手工清掉目录内容，等价于上次中断。
        std::fs::remove_dir_all(&target).unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(scan_lifecycle(&paths).records.is_empty());
    }

    #[test]
    fn damaged_lifecycle_record_blocks_deletion() {
        let dir = TempDir::new("damaged");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let file = lifecycle_file(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::write(&file, b"{not json").unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.iter().any(|issue| issue.reason.contains("解析失败")));
        assert!(
            transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .is_dir(),
            "受损记录存在时不得删除任何目录"
        );
    }
