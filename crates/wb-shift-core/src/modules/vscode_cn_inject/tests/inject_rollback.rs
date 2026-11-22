    // 本文件由 `mod.rs` 的 `mod tests` 与 `tests/inject.rs` 一起 `include!`，可直接复用
    // 那边的 `temp_state_db` / `create_item_table` / `open_state_db` 夹具。
    //
    // 为什么单开一个文件：`tests/inject.rs` 已在 `docs/ARCHITECTURE.md` 的 >500 行豁免表里
    // 按**精确行数**登记（锁冲突与重试用例共用 hold_exclusive/insert_row 夹具），
    // 往里追加会同时破坏门禁与那份登记。回滚用例本来也是另一个主题。

    /// 造一个已有 `ItemTable` 的临时库；`previous = Some(v)` 时预置该 key 的旧值。
    fn db_with_previous_row(
        label: &str,
        key: &str,
        previous: Option<&str>,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let (dir, db) = temp_state_db(label);
        create_item_table(&db);
        if let Some(previous) = previous {
            let conn = open_state_db(&db, false).unwrap();
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, previous],
            )
            .unwrap();
        }
        (dir, db)
    }

    /// 读回该 key 的行数（0 = 该行不存在）。
    fn row_count(conn: &Connection, key: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM ItemTable WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// 缺陷一 #2/#3：校验判定不一致 → 旧值**逐字节**恢复，且报出的是校验错误本身
    /// （不是回滚错误，也不附加「库可能不一致」）。
    ///
    /// 校验器是注入的：生产校验器只有在真能解密/比对时才判不一致，真实库上无法稳定构造。
    #[test]
    fn content_check_failure_restores_previous_value_byte_for_byte() {
        let key = secret_storage_item_key();
        let old_value = r#"{"type":"Buffer","data":[1,2,3,4]}"#;
        let (dir, db) = db_with_previous_row("rollback-restore", &key, Some(old_value));
        let conn = open_state_db(&db, false).unwrap();

        let write = SecretWrite {
            key: &key,
            buffer_json: r#"{"type":"Buffer","data":[9,9,9,9]}"#,
            ciphertext: &[9, 9, 9, 9],
            plaintext: b"brand-new-secret",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
            previous: Some(old_value),
        };
        let failure = commit_secret_value(&conn, &write, |_| {
            Err("写后校验失败：注入的不一致".to_string())
        })
        .expect_err("校验器判定不一致时必须失败");

        assert_eq!(
            failure_text(&failure),
            "写后校验失败：注入的不一致",
            "回滚成功时应原样报校验错误"
        );

        let stored: String = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key = ?1",
                [key.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, old_value, "回滚必须逐字节还原写入前的旧值");

        std::fs::remove_dir_all(dir).ok();
    }

    /// 缺陷一 #3：原本没有这一行 → 失败后该行必须被**删除**（不是留下新值、也不是留空值）。
    #[test]
    fn content_check_failure_deletes_row_that_did_not_exist() {
        let key = secret_storage_item_key();
        let (dir, db) = db_with_previous_row("rollback-delete", &key, None);
        let conn = open_state_db(&db, false).unwrap();

        let write = SecretWrite {
            key: &key,
            buffer_json: r#"{"type":"Buffer","data":[7,7]}"#,
            ciphertext: &[7, 7],
            plaintext: b"x",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
            previous: None,
        };
        commit_secret_value(&conn, &write, |_| {
            Err("写后校验失败：注入的不一致".to_string())
        })
        .expect_err("校验器判定不一致时必须失败");

        assert_eq!(
            row_count(&conn, &key),
            0,
            "写入前没有该行 → 回滚必须 DELETE 该行"
        );

        std::fs::remove_dir_all(dir).ok();
    }

    /// 缺陷一 #4：回滚**也**失败时，两条错误都要报出来，并明确「库可能不一致，请用备份恢复」。
    ///
    /// 构造方式：在校验回调里把表删掉（真实世界里对应「表被锁死 / 结构被改 / 文件损坏」）。
    /// 校验回调正好在「写入之后、回滚之前」执行，所以这是最小且确定性的构造。
    #[test]
    fn rollback_failure_reports_both_errors_and_warns_inconsistency() {
        let key = secret_storage_item_key();
        let (dir, db) = db_with_previous_row("rollback-fail", &key, Some("old-value"));
        let conn = open_state_db(&db, false).unwrap();

        let write = SecretWrite {
            key: &key,
            buffer_json: r#"{"type":"Buffer","data":[2]}"#,
            ciphertext: &[2],
            plaintext: b"x",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
            previous: Some("old-value"),
        };
        let failure = commit_secret_value(&conn, &write, |_| {
            conn.execute_batch("DROP TABLE ItemTable").unwrap();
            Err("写后校验失败：注入的不一致".to_string())
        })
        .expect_err("回滚失败时必须报错");

        let message = failure_text(&failure);
        assert!(message.contains("写后校验失败：注入的不一致"), "{message}");
        assert!(message.contains("回滚到切换前的值也失败了"), "{message}");
        assert!(message.contains("no such table"), "第二条错误也要给出: {message}");
        assert!(message.contains("数据库可能处于不一致状态"), "{message}");
        assert!(message.contains("请用备份恢复"), "{message}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// 缺陷一 #2：默认校验器必须识破三种「库里存的不是我们写的那份」。
    #[test]
    fn content_check_rejects_bad_shape_and_mismatched_ciphertext() {
        let dir = std::env::temp_dir().join(format!("wb-content-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // ① 行值不是 Buffer：文案与改造前逐字一致
        let check = ContentCheck {
            raw_value: "\"raw-string\"",
            ciphertext: &[1, 2],
            plaintext: b"x",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
        };
        assert_eq!(
            verify_secret_content(&check).unwrap_err(),
            "写后校验失败：value 不是 Buffer"
        );

        // ② 读回的密文与刚写入的密文不同（且本次解密不可用）→ 必须判不一致
        let check = ContentCheck {
            raw_value: r#"{"type":"Buffer","data":[1,2,3]}"#,
            ciphertext: &[9, 9, 9],
            plaintext: b"x",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
        };
        let error = verify_secret_content(&check).unwrap_err();
        assert!(error.contains("写后校验失败"), "{error}");

        // ③ 行值根本不是 JSON → 同样是失败，不能当成通过
        let check = ContentCheck {
            raw_value: "not-json",
            ciphertext: &[1],
            plaintext: b"x",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
        };
        let error = verify_secret_content(&check).unwrap_err();
        assert!(error.contains("写后校验 JSON 失败"), "{error}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// 缺陷一 #2 的退化分支：解密在当前上下文拿不到时，退化为「密文逐字节相等」。
    ///
    /// 用空 Buffer 是刻意的：三个平台的 `decrypt_secret_payload` 对空密文都必然失败
    /// （Windows 长度不足 / macOS·Linux 前缀不匹配），于是确定性地走退化分支，
    /// 不会因为「本机钥匙串恰好有密钥」而变成 flaky。
    #[test]
    fn content_check_falls_back_to_ciphertext_comparison() {
        let dir = std::env::temp_dir().join(format!("wb-content-fallback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let check = ContentCheck {
            raw_value: r#"{"type":"Buffer","data":[]}"#,
            ciphertext: &[],
            plaintext: b"",
            data_root: &dir,
            target: &CODEBUDDY_CN_TARGET,
        };
        verify_secret_content(&check).expect("解密不可用时，密文逐字节一致即视为通过");

        std::fs::remove_dir_all(dir).ok();
    }
