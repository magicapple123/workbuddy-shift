    #[test]
    fn secret_key_matches_verified_format() {
        let key = secret_storage_item_key();
        assert_eq!(
            key,
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessTokencn"}"#
        );
        assert_eq!(
            secret_storage_item_key_for(&CODEBUDDY_INTL_TARGET),
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessToken"}"#
        );
        assert_eq!(
            CodeBuddyIdeFlavor::Intl.keychain_service(),
            "CodeBuddy Safe Storage"
        );
        assert_ne!(
            CodeBuddyIdeFlavor::Cn.secret_key(),
            CodeBuddyIdeFlavor::Intl.secret_key()
        );
    }

    #[test]
    fn flavor_target_matches_flavor_fields() {
        for flavor in [CodeBuddyIdeFlavor::Cn, CodeBuddyIdeFlavor::Intl] {
            let target = flavor.target();
            assert_eq!(target.secret_key, flavor.secret_key());
            assert_eq!(target.macos_keychain_service, flavor.keychain_service());
            assert_eq!(
                target.linux_secret_tool_app_names,
                flavor.linux_secret_apps()
            );
            assert_eq!(target.display_name, flavor.product_label());
        }
    }

    #[test]
    fn secret_key_for_custom_target_uses_its_fields() {
        let target = VscodeSafeStorageTarget {
            data_dir_resolver: codebuddy_cn_data_dir,
            display_name: "Test",
            secret_item_prefix_extension_id: "tencent-cloud.coding-copilot",
            secret_key: "Tencent-Cloud.coding-copilot.new.accessToken",
            macos_keychain_service: "Code Safe Storage",
            linux_secret_tool_app_names: &["Code"],
        };
        assert_eq!(
            secret_storage_item_key_for(&target),
            r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"Tencent-Cloud.coding-copilot.new.accessToken"}"#
        );
    }

    #[test]
    fn data_dir_contains_codebuddy_cn() {
        let Some(dir) = codebuddy_cn_data_dir() else {
            return;
        };
        let s = dir.to_string_lossy();
        assert!(
            s.contains("CodeBuddy CN"),
            "data dir should contain CodeBuddy CN: {s}"
        );
        let Some(intl) = codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Intl) else {
            return;
        };
        let intl_s = intl.to_string_lossy();
        assert!(
            intl_s.contains("CodeBuddy") && !intl_s.contains("CodeBuddy CN"),
            "intl data dir should be CodeBuddy not CN: {intl_s}"
        );
    }

    #[test]
    fn state_db_path_ends_with_state_vscdb() {
        let Some(db) = codebuddy_cn_state_db_path() else {
            return;
        };
        assert!(db.ends_with("state.vscdb"));
        assert!(db.to_string_lossy().contains("globalStorage"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn cbc_roundtrip_with_known_password() {
        let key = pbkdf2_sha1_key("test-password", 1003);
        let plain = br#"{"token":"abc","accessToken":"uid+abc"}"#;
        let encrypted = encrypt_cbc_prefixed(V10_PREFIX, &key, plain).unwrap();
        assert!(encrypted.starts_with(V10_PREFIX));
        let decrypted = decrypt_cbc_prefixed(&encrypted, V10_PREFIX, &key).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn resolve_prefers_existing_candidate() {
        let dir =
            std::env::temp_dir().join(format!("wb-cn-ide-path-test-{}", uuid::Uuid::new_v4()));
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, b"").unwrap();
        let resolved = resolve_state_db_path(Some(&dir)).unwrap();
        assert_eq!(resolved, db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `has_secret_row_for`：只看 key 是否存在（有行 / 无行 / 无文件三态），
    /// 且不读值、不解密、不创建任何目录或文件。
    #[test]
    fn has_secret_row_detects_row_without_creating_files() {
        let dir =
            std::env::temp_dir().join(format!("wb-cn-secret-row-test-{}", uuid::Uuid::new_v4()));
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();

        // ① 库文件不存在 → false，且不得顺手创建目录/文件
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());
        assert!(!db.exists(), "只读探测不得创建 state.vscdb");

        let missing_root = dir.join("no-such-data-dir");
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&missing_root)).unwrap());
        assert!(!missing_root.exists(), "只读探测不得创建数据目录");

        // ①b 空库（0 字节文件）：ItemTable 尚未初始化 → false，且不得报错
        std::fs::write(&db, b"").unwrap();
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        // ② 表存在但无该 key → false
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('other', 'x')",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(!has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        // ③ 有该 key → true（值是不可解密的占位串，证明只查存在性、不走解密）
        let key = secret_storage_item_key();
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, 'not-a-ciphertext')",
            [key.as_str()],
        )
        .unwrap();
        drop(conn);
        assert!(has_secret_row_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap());

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 回归 issue #91：只读探测（`read_secret_for`）不得有任何建目录副作用，
    /// 否则 `installed` 会把探测自建的空数据目录误判为「已接入」，且用户删掉后
    /// 下次探测又会重建，形成自我维持的误报。国内版与国际版 target 一并断言。
    #[test]
    fn read_secret_probe_creates_nothing() {
        let base =
            std::env::temp_dir().join(format!("wb-cn-readonly-probe-{}", uuid::Uuid::new_v4()));

        for (label, target, root) in [
            (
                "cn/数据目录整体缺失",
                &CODEBUDDY_CN_TARGET,
                base.join("cn-missing"),
            ),
            (
                "cn/目录存在但无 globalStorage",
                &CODEBUDDY_CN_TARGET,
                base.join("cn-empty-root"),
            ),
            (
                "intl/数据目录整体缺失",
                &CODEBUDDY_INTL_TARGET,
                base.join("intl-missing"),
            ),
        ] {
            if label.ends_with("无 globalStorage") {
                std::fs::create_dir_all(&root).unwrap();
            }
            assert_eq!(
                read_secret_for(target, Some(&root)).unwrap(),
                None,
                "{label}: 无 db 时应返回 Ok(None)"
            );
            assert!(!root.join("User").exists(), "{label}: 不得创建 User/");
            assert!(
                !root.join("User").join("globalStorage").exists(),
                "{label}: 不得创建 User/globalStorage/"
            );
            assert!(
                !root.join("globalStorage").exists(),
                "{label}: 不得创建 globalStorage/"
            );
            assert!(
                !root.join("state.vscdb").exists(),
                "{label}: 不得创建 state.vscdb"
            );
        }

        std::fs::remove_dir_all(base).ok();
    }

    /// 回归 issue #91（写入路径不受影响）：db 不存在时注入仍会自动准备目录并写库成功。
    #[test]
    fn inject_still_prepares_directory() {
        let dir = std::env::temp_dir().join(format!("wb-cn-inject-mkdir-{}", uuid::Uuid::new_v4()));
        let plaintext = r#"{"token":"tok","accessToken":"uid+tok"}"#;
        let result = inject_secret_for(&CODEBUDDY_CN_TARGET, plaintext, Some(&dir));
        // Windows 上加密依赖系统 Safe Storage，无该条目会失败；只断言目录已准备好。
        let db = dir.join("User").join("globalStorage").join("state.vscdb");
        if result.is_ok() {
            assert!(db.exists(), "注入应创建 state.vscdb");
            let read = read_secret_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap();
            assert_eq!(read.as_deref(), Some(plaintext));
        } else {
            assert!(
                db.parent().map(|p| p.exists()).unwrap_or(false),
                "注入路径解析应准备 globalStorage 目录"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// 回归 issue #80：密钥环里确实有密码时，读本机登录信息不能再报
    /// 「无法加载 Linux secret storage key（v11）」。
    ///
    /// 只在本机既有密钥环条目、又有 CodeBuddy CN 数据目录时才验证，
    /// 无桌面环境（CI）直接跳过。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_v11_secret_is_readable_when_keyring_has_password() {
        let Some(_password) = crate::modules::linux_keyring::find_password(
            CODEBUDDY_CN_TARGET.linux_secret_tool_app_names,
        ) else {
            return;
        };
        let Some(db_path) = codebuddy_cn_state_db_path() else {
            return;
        };
        if !db_path.exists() {
            return;
        }
        read_secret_for(&CODEBUDDY_CN_TARGET, None)
            .expect("密钥环里有密码时，读取 CodeBuddy CN secret 不应失败");
    }

    /// Linux 写入 → 读回必须还原原文：有密钥环走 v11、没有则退回 peanuts 的 v10，
    /// 两条路都要能自洽。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_inject_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wb-linux-inject-{}", uuid::Uuid::new_v4()));
        let plaintext = r#"{"token":"tok","accessToken":"uid+tok"}"#;
        inject_secret_for(&CODEBUDDY_CN_TARGET, plaintext, Some(&dir)).unwrap();
        let read = read_secret_for(&CODEBUDDY_CN_TARGET, Some(&dir)).unwrap();
        assert_eq!(read.as_deref(), Some(plaintext));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 临时库辅助：返回 (目录, state.vscdb 路径)，目录由调用方清理。
    use std::time::Duration;

    fn temp_state_db(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "wb-state-db-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("state.vscdb");
        (dir, db)
    }

    /// 建好 `ItemTable` 的临时库。
    fn create_item_table(db: &std::path::Path) {
        let conn = open_state_db(db, false).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
    }

    /// 让调用方所在的线程**持有**独占写锁，直到显式 `COMMIT`。
    ///
    /// 用 `EXCLUSIVE` 而不是 `IMMEDIATE`：独占锁会把**第一条**语句就挡住
    /// （`IMMEDIATE` 只挡写、读仍可过），这样「生产写入路径撞锁」的位置是确定的，
    /// 不受「`CREATE TABLE IF NOT EXISTS` 是否真的需要写」这类细节影响。
    ///
    /// 刻意**不**放到后台线程里 sleep 到期释放：那样测试结果取决于调度与时序，
    /// 会是 flaky 的。释放时机由测试自己控制（在两次重试之间 `COMMIT`）。
    fn hold_exclusive(db: &std::path::Path) -> Connection {
        let holder = open_state_db(db, false).unwrap();
        holder
            .execute_batch(
                "BEGIN EXCLUSIVE; INSERT OR REPLACE INTO ItemTable (key, value) VALUES ('holder', 'x');",
            )
            .unwrap();
        holder
    }

    /// 一次「打开 + 写入」尝试：与生产路径同一套失败分类（`classify_db_error`）。
    fn insert_row(
        db: &std::path::Path,
        timeout: Duration,
        key: &str,
    ) -> Result<(), InjectFailure> {
        let conn = open_state_db_with_timeout(db, false, timeout)
            .map_err(|e| classify_db_error("打开 state.vscdb 失败", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, 'v')",
            [key],
        )
        .map_err(|e| classify_db_error("写入 state.vscdb 失败", e))?;
        Ok(())
    }

    /// `PRAGMA busy_timeout` 的当前值（毫秒）。
    fn busy_timeout_ms(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap()
    }

    /// `open_state_db` 必须给两类连接都设上 `STATE_DB_BUSY_TIMEOUT`（读写 + 只读）。
    ///
    /// **诚实的边界**：rusqlite 0.32 在 `sqlite3_open` 后自己就设了 5000ms，
    /// 所以这条断言今天无法区分「我们设的」与「库默认」。它的价值是钉住**文档化的取值**
    /// （依赖升级把默认改小、或有人调大常量都会被发现）。真正验证等待行为的是
    /// [`write_retry_survives_a_lock_held_longer_than_one_timeout`]。
    #[test]
    fn state_db_connections_report_expected_busy_timeout() {
        let (dir, db) = temp_state_db("busy-timeout");
        let write_conn = open_state_db(&db, false).unwrap();
        assert_eq!(
            busy_timeout_ms(&write_conn),
            STATE_DB_BUSY_TIMEOUT.as_millis() as i64,
            "读写连接必须带 busy_timeout"
        );
        drop(write_conn);

        let read_conn = open_state_db(&db, true).unwrap();
        assert_eq!(
            busy_timeout_ms(&read_conn),
            STATE_DB_BUSY_TIMEOUT.as_millis() as i64,
            "只读连接同样要带 busy_timeout（读也会撞 SQLITE_BUSY）"
        );
        drop(read_conn);

        std::fs::remove_dir_all(dir).ok();
    }

    /// 生产写入入口（`inject_secret_for_with_policy`）撞锁时必须给出锁冲突文案，
    /// 而不是把它混进「加密/校验失败」。
    ///
    /// 这里能在**所有平台**稳定断言：`BEGIN EXCLUSIVE` 会让第一条语句（建表）就拿到
    /// `SQLITE_BUSY`，走不到平台相关的加解密步骤。
    #[test]
    fn inject_secret_reports_lock_contention_from_the_real_write_path() {
        let (dir, db) = temp_state_db("inject-lock");
        create_item_table(&db);
        let policy = BusyRetryPolicy {
            timeout: Duration::from_millis(100),
            attempts: 1,
            backoff: Duration::from_millis(0),
        };
        let lock = hold_exclusive(&db);
        let error = inject_secret_for_with_policy(&CODEBUDDY_CN_TARGET, "{}", Some(&dir), policy)
            .expect_err("库被独占时必须失败");
        lock.execute_batch("COMMIT;").unwrap();
        assert!(error.contains("写入 state.vscdb 失败"), "{error}");
        assert!(error.contains("已完全退出"), "{error}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// 回归（本次故障的正题）：写锁没在第一次尝试时释放，只有「整轮重试」能救回来。
    ///
    /// 两个用例用同一把独占锁、同一套路径，唯一变量是 `attempts`。这正是「VS Code 主进程
    /// 刚退出、子进程还持着写锁」时用户看到失败的机理，也是「只把 busy_timeout 补上」
    /// 并不够的证据（rusqlite 本来就有 5 秒默认）：
    ///
    /// - `attempts = 1`（等价于只加 busy_timeout、不重试）→ 必然 `Busy` 失败；
    /// - `attempts = 3` 且锁在两次尝试之间释放 → 第二次写入成功。
    #[test]
    fn write_retry_survives_a_lock_released_between_attempts() {
        let (dir, db) = temp_state_db("lock-retry");
        create_item_table(&db);

        // ① 只尝试一次（= 只有 busy_timeout、没有重试）：锁一直不放 → 必然 Busy。
        let single_shot = BusyRetryPolicy {
            timeout: Duration::from_millis(50),
            attempts: 1,
            backoff: Duration::from_millis(0),
        };
        let holder = hold_exclusive(&db);
        let failure = retry_on_busy(single_shot, |timeout| insert_row(&db, timeout, "single"))
            .expect_err("锁未释放、只尝试一次时必须失败");
        assert!(
            matches!(failure, InjectFailure::Busy(_)),
            "失败原因应为锁冲突（而不是被误判成其它错误）"
        );
        holder.execute_batch("COMMIT;").unwrap();
        drop(holder);

        // ② 允许重试，且锁在**两次尝试之间**释放：第一次撞锁、第二次成功。
        //    释放时机由测试控制（不靠 sleep），所以这条用例没有时序 flaky。
        let retrying = BusyRetryPolicy {
            timeout: Duration::from_millis(50),
            attempts: 3,
            backoff: Duration::from_millis(1),
        };
        let mut holder = Some(hold_exclusive(&db));
        let mut calls = 0usize;
        retry_on_busy(retrying, |timeout| {
            calls += 1;
            if calls == 2 {
                // 模拟「编辑器的收尾子进程终于释放了写锁」。
                if let Some(holder) = holder.take() {
                    holder.execute_batch("COMMIT;").unwrap();
                }
            }
            insert_row(&db, timeout, "retried")
        })
        .expect("锁释放后应当写入成功");
        assert_eq!(calls, 2, "第一次撞锁、第二次成功：重试必须真的发生一次");

        let conn = open_state_db(&db, false).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ItemTable", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2, "持锁方的行与重试写入的行都应在");

        std::fs::remove_dir_all(dir).ok();
    }

    /// 重试循环的边界：只在锁冲突时重试；次数受 policy 约束；非锁失败立刻返回。
    #[test]
    fn retry_on_busy_retries_only_lock_conflicts() {
        let policy = BusyRetryPolicy {
            timeout: Duration::from_millis(1),
            attempts: 3,
            backoff: Duration::from_millis(1),
        };
        // 构造 Busy 只需要「有个 rusqlite 错误」：这里考的是循环语义，
        // 分类是否正确由带真实锁的用例覆盖。
        let busy = || InjectFailure::Busy(rusqlite::Error::InvalidQuery);

        // ① 前两次撞锁、第三次成功 → 共 3 次调用
        let mut calls = 0;
        retry_on_busy(policy, |_| {
            calls += 1;
            if calls < 3 {
                Err(busy())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(calls, 3);

        // ② 一直撞锁 → 用尽 attempts 后返回最后一个 Busy
        let mut calls = 0;
        let failure = retry_on_busy(policy, |_| {
            calls += 1;
            Err(busy())
        })
        .unwrap_err();
        assert_eq!(calls, 3, "尝试次数必须等于 policy.attempts");
        assert!(matches!(failure, InjectFailure::Busy(_)));

        // ③ 非锁失败立刻返回，不浪费重试
        let mut calls = 0;
        let failure = retry_on_busy(policy, |_| {
            calls += 1;
            Err(InjectFailure::Fatal("加密失败".to_string()))
        })
        .unwrap_err();
        assert_eq!(calls, 1, "非锁失败不得重试");
        assert!(matches!(failure, InjectFailure::Fatal(_)));

        // ④ attempts = 0 也要至少跑一次（配置写错不能变成「一次都不试」）
        let mut calls = 0;
        retry_on_busy(
            BusyRetryPolicy {
                attempts: 0,
                ..policy
            },
            |_| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
    }

    /// 锁冲突文案必须点名原因与可操作动作；非锁错误保持改造前的原样文案。
    ///
    /// 用**真实的** SQLite 错误验证分类（不手工构造 `SqliteFailure`）。
    #[test]
    fn lock_conflict_message_names_cause_and_action() {
        let (dir, db) = temp_state_db("lock-msg");
        create_item_table(&db);

        let lock = hold_exclusive(&db);
        let busy = open_state_db_with_timeout(&db, false, Duration::from_millis(50))
            .and_then(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO ItemTable (key, value) VALUES ('w', 'y')",
                    [],
                )
            })
            .expect_err("持写锁时短 timeout 的连接必须失败");
        let message = classify_db_error("写入 state.vscdb 失败", busy).into_message(DEFAULT_WRITE_POLICY);
        lock.execute_batch("COMMIT;").unwrap();
        drop(lock);
        assert!(
            message.starts_with("写入 state.vscdb 失败: "),
            "{message}"
        );
        assert!(message.contains("已完全退出"), "{message}");
        assert!(message.contains("已重试 3 次"), "{message}");

        // 非锁冲突（表不存在）：文案与改造前一致，不追加误导性提示。
        let conn = open_state_db(&db, false).unwrap();
        let other = conn
            .execute("INSERT INTO no_such_table (a) VALUES (1)", [])
            .expect_err("表不存在必须报错");
        assert!(!is_db_busy_error(&other), "非锁错误不得判为锁冲突: {other}");
        let message = classify_db_error("写入 state.vscdb 失败", other).into_message(DEFAULT_WRITE_POLICY);
        assert!(
            message.starts_with("写入 state.vscdb 失败: "),
            "{message}"
        );
        assert!(
            !message.contains("已完全退出"),
            "非锁错误不得追加锁提示: {message}"
        );

        std::fs::remove_dir_all(dir).ok();
    }
