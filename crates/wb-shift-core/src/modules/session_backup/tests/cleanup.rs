    #[test]
    fn cleanup_refuses_invalid_operation_id() {
        let dir = TempDir::new("bad-id");
        let paths = dir.paths();
        let record = BackupLifecycle {
            version: LIFECYCLE_VERSION,
            operation_id: "../escape".to_string(),
            variant: WbVariant::Cn,
            kind: "copy".to_string(),
            state: BackupState::CleanupPending,
            created_at: 1,
            updated_at: 1,
            last_error: None,
            session_id: None,
            title: None,
        };
        match cleanup_operation(&paths, WbVariant::Cn, &record) {
            CleanupOutcome::Protected { reason } => assert!(reason.contains("UUID")),
            other => panic!("非法标识必须拒绝：{other:?}"),
        }
    }

    #[test]
    fn cn_and_ai_records_are_isolated() {
        let dir = TempDir::new("variants");
        let paths = dir.paths();
        let cn = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let ai = begin_operation(&paths, WbVariant::Ai, "copy", None, None).unwrap();
        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(!transaction_dir(&paths, WbVariant::Cn, &cn.operation_id)
            .unwrap()
            .exists());
        assert!(
            transaction_dir(&paths, WbVariant::Ai, &ai.operation_id)
                .unwrap()
                .is_dir(),
            "另一档位的准备残留不得被本档位维护删除"
        );
    }

    #[test]
    fn ensure_full_synchronous_upgrades_below_full() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA synchronous=NORMAL").unwrap();
        let before: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert!(before < 2, "前置条件：NORMAL 必须低于 FULL");
        ensure_full_synchronous(&conn).unwrap();
        let after: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert!(after >= 2, "必须提升到 FULL 或更高：{after}");
    }

    #[test]
    fn mismatched_filename_blocks_deletion() {
        let dir = TempDir::new("name-mismatch");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let original = lifecycle_file(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        let spoofed_id = uuid::Uuid::new_v4().to_string();
        let spoofed = lifecycle_root(&paths, WbVariant::Cn).join(format!("{spoofed_id}.json"));
        std::fs::rename(&original, &spoofed).unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(
            issues
                .iter()
                .any(|issue| issue.reason.contains("文件名与操作标识不一致")),
            "{issues:?}"
        );
        assert!(
            transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .is_dir(),
            "归属不一致时不得删除目录"
        );
    }

    #[test]
    fn mismatched_variant_blocks_deletion() {
        let dir = TempDir::new("variant-mismatch");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        record.variant = WbVariant::Ai;
        let file = lifecycle_file(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::write(&file, serde_json::to_string(&record).unwrap()).unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(
            issues
                .iter()
                .any(|issue| issue.reason.contains("档位与目录不一致")),
            "{issues:?}"
        );
        assert!(transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
            .unwrap()
            .is_dir());
    }

    #[test]
    fn unknown_kind_blocks_deletion() {
        let dir = TempDir::new("kind");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        record.kind = "evil".to_string();
        save_lifecycle(&paths, &record).unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(
            issues
                .iter()
                .any(|issue| issue.reason.contains("kind 不受支持")),
            "{issues:?}"
        );
        assert!(transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
            .unwrap()
            .is_dir());
    }

    #[test]
    fn abandoned_without_safe_terminated_marker_is_preserved() {
        let dir = TempDir::new("abandoned-protect");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        mark_protected(&paths, &mut record).unwrap();
        session_link::save_operation(
            &paths,
            &operation(&record.operation_id, OpPhase::Abandoned, Some(1)),
        )
        .unwrap();
        let issues = maintain(&paths, WbVariant::Cn);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].reason.contains("已验证安全终止"), "{issues:?}");
        assert!(transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
            .unwrap()
            .is_dir());
    }

    #[test]
    fn abandoned_with_safe_terminated_marker_is_reclaimed() {
        let dir = TempDir::new("abandoned-clean");
        let paths = dir.paths();
        let mut record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        mark_protected(&paths, &mut record).unwrap();
        let mut log = operation(&record.operation_id, OpPhase::Abandoned, Some(1));
        log.cleanup_state = Some(CLEANUP_STATE_SAFE_TERMINATED.to_string());
        session_link::save_operation(&paths, &log).unwrap();
        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(
            !transaction_dir(&paths, WbVariant::Cn, &record.operation_id)
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn legacy_timestamp_backup_dirs_are_not_swept() {
        let dir = TempDir::new("legacy-dirs");
        let paths = dir.paths();
        let old_sessions = paths
            .backup_root()
            .join("sessions")
            .join("2026-01-01T00-00-00Z");
        let old_sync = paths
            .backup_root()
            .join("session-sync")
            .join(WbVariant::Cn.as_str())
            .join("op-old");
        std::fs::create_dir_all(&old_sessions).unwrap();
        std::fs::write(old_sessions.join("workbuddy.db"), b"legacy").unwrap();
        std::fs::create_dir_all(&old_sync).unwrap();
        std::fs::write(old_sync.join("manifest.json"), b"{}").unwrap();

        let issues = maintain(&paths, WbVariant::Cn);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(old_sessions.join("workbuddy.db").exists());
        assert!(old_sync.join("manifest.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_under_store_root_is_rejected() {
        let dir = TempDir::new("ancestor-link");
        let paths = dir.paths();
        let record = begin_operation(&paths, WbVariant::Cn, "copy", None, None).unwrap();
        let target = transaction_dir(&paths, WbVariant::Cn, &record.operation_id).unwrap();
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();

        let backups = paths.backup_root();
        let outside = dir.0.join("outside-backups");
        std::fs::rename(&backups, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &backups).unwrap();

        let outcome = cleanup_operation(&paths, WbVariant::Cn, &record);
        match outcome {
            CleanupOutcome::Protected { reason } => {
                assert!(reason.contains("符号链接"), "{reason}")
            }
            other => panic!("祖先符号链接必须拒绝：{other:?}"),
        }
        assert!(
            outside
                .join(TRANSACTIONS_DIR_NAME)
                .join(WbVariant::Cn.as_str())
                .join(&record.operation_id)
                .join("keep.txt")
                .exists(),
            "链接目标内的文件不得被删除"
        );
    }
