    // issue #202 的验收用例（凭据写入互斥）：抢锁失败立即返回且零写入、磁盘更新时
    // 放弃写回、正常路径仍成功、提前返回路径锁已释放。
    //
    // 并发用例不用 `sleep` 赌时序：持锁方是自己持有并在代码里显式 `drop` 的 `FileLock`，
    // 「谁先谁后」由语句顺序决定。

    /// 夹具时间基准：13 位毫秒（现实的 `refreshedAt` / `expiresAt`）。
    ///
    /// 不能用几百几千这种小数字：小于 `SECONDS_UPPER_BOUND_MS` 会被当成秒级再乘 1000，
    /// 断言就变成在测归一化而不是在测判定（该归一化本身有单独用例）。
    const FRESHNESS_BASE_MS: i64 = 1_700_000_000_000;

    /// 临时目录夹具：绝不触碰真实 `~/.workbuddy-shift` 或客户端数据目录。
    struct TempCredentialDir(PathBuf);

    impl TempCredentialDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "workbuddy-shift-credentials-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self(path)
        }

        fn store_root(&self) -> PathBuf {
            self.0.join("store")
        }

        fn auth_path(&self) -> PathBuf {
            self.0.join("workbuddy-desktop.info")
        }
    }

    impl Drop for TempCredentialDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 账号库记录夹具：`expires_at` / `refreshed_at` 是新鲜度判定的全部依据，
    /// 入参是相对 [`FRESHNESS_BASE_MS`] 的偏移。
    fn account_record(id: &str, expires_at: i64, refreshed_at: i64) -> Value {
        let expires_at = FRESHNESS_BASE_MS + expires_at;
        let refreshed_at = FRESHNESS_BASE_MS + refreshed_at;
        json!({
            "id": id,
            "uid": "uid-202",
            "access_token": format!("at-{refreshed_at}"),
            "refresh_token": format!("rt-{refreshed_at}"),
            "expiresAt": expires_at,
            "refreshedAt": refreshed_at,
        })
    }

    /// ① 抢锁失败：立即返回、不读账号库、不写任何文件、锁未泄漏。
    #[test]
    fn write_back_fails_fast_without_reading_or_writing_when_lock_is_held() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let held = try_acquire_credential_write_lock_at(&store).expect("首次抢锁应当成功");

        let error = write_back_credentials_at(
            &account_record("acc-202", 10_000, 0),
            &auth,
            &store,
            || panic!("抢锁失败时不得读取账号库：锁必须挡在所有读取与写入之前"),
        )
        .expect_err("抢不到锁必须失败");

        assert_eq!(
            error, CREDENTIAL_LOCK_BUSY_MESSAGE,
            "失败文案必须直接告诉用户稍后重试且没有写入任何凭据"
        );
        assert!(!auth.exists(), "抢锁失败不得创建/改写认证文件");
        assert!(
            !logout_marker_path(&auth).exists(),
            "抢锁失败不得创建/清理退出标记"
        );

        drop(held);
        assert!(
            try_acquire_credential_write_lock_at(&store).is_ok(),
            "释放后必须能重新抢锁（否则前一条用例的守卫泄漏了）"
        );
    }

    /// ① 反向证明：真的有人持锁时第二个获取者一定失败（不是恒真断言）。
    #[test]
    fn credential_lock_is_exclusive_and_released_on_drop() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();

        let held = try_acquire_credential_write_lock_at(&store).expect("首次抢锁应当成功");
        let second = try_acquire_credential_write_lock_at(&store);
        assert_eq!(
            second.err(),
            Some(CREDENTIAL_LOCK_BUSY_MESSAGE.to_string()),
            "同一把锁必须互斥，且失败文案与常量一致"
        );

        drop(held);
        assert!(
            try_acquire_credential_write_lock_at(&store).is_ok(),
            "守卫 Drop 必须释放锁"
        );
    }

    /// ② 账号库记录比快照新（切换期间被刷新过）→ 放弃写回，认证文件保持原样。
    #[test]
    fn write_back_abandons_when_store_record_is_newer() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = account_record("acc-202", 10_000, 0);
        let refreshed = account_record("acc-202", 30_000, 3_000);

        let error = write_back_credentials_at(&snapshot, &auth, &store, || Some(refreshed.clone()))
            .expect_err("磁盘更新时必须放弃写回");

        assert!(error.contains("已被刷新"), "{error}");
        assert!(error.contains("放弃写回"), "{error}");
        assert!(
            error.contains("磁盘刷新时间"),
            "必须说明是哪个时间戳更新了：{error}"
        );
        assert!(
            error.contains(&(FRESHNESS_BASE_MS + 3_000).to_string()),
            "必须带出磁盘上的刷新时间，便于用户/日志核对：{error}"
        );
        assert!(!auth.exists(), "放弃写回不得写入认证文件");
        assert!(
            try_acquire_credential_write_lock_at(&store).is_ok(),
            "提前返回（?）路径必须已释放锁"
        );
    }

    /// ② 官方认证文件更新（同名账号、客户端自己刚刷过）→ 同样放弃写回。
    #[test]
    fn write_back_abandons_when_auth_file_is_newer_for_same_account() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = account_record("acc-202", 10_000, 0);
        let disk_refresh_time = FRESHNESS_BASE_MS + 9_000;
        let disk = json!({
            "account": {"uid": "uid-202"},
            "auth": {
                "accessToken": "fresh-at",
                "refreshToken": "fresh-rt",
                "lastRefreshTime": disk_refresh_time,
                "expiresAt": FRESHNESS_BASE_MS + 50_000,
            },
        });
        std::fs::write(&auth, serde_json::to_string_pretty(&disk).expect("序列化"))
            .expect("写入磁盘认证文件");

        let error = write_back_credentials_at(&snapshot, &auth, &store, || Some(snapshot.clone()))
            .expect_err("官方文件更新时必须放弃写回");

        assert!(error.contains("官方登录态"), "{error}");
        assert!(error.contains("放弃写回"), "{error}");
        // 现场必须原样保留：写回被放弃时一行都不许动。
        let after = read_auth_file_at(&auth).expect("认证文件仍可读");
        assert_eq!(after["auth"]["accessToken"], "fresh-at");
        assert_eq!(
            after["auth"]["lastRefreshTime"],
            json!(disk_refresh_time),
            "放弃写回时不得改动认证文件的刷新时间"
        );
    }

    /// ② 官方文件属于**别的账号**：按 uid 判据跳过比较，正常写入（防误伤）。
    #[test]
    fn write_back_ignores_auth_file_of_another_account() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = account_record("acc-202", 10_000, 0);
        let other = json!({
            "account": {"uid": "uid-other"},
            "auth": {
                "accessToken": "other-at",
                "lastRefreshTime": FRESHNESS_BASE_MS + 9_000,
                "expiresAt": FRESHNESS_BASE_MS + 99_000,
            },
        });
        std::fs::write(&auth, serde_json::to_string_pretty(&other).expect("序列化"))
            .expect("写入磁盘认证文件");

        write_back_credentials_at(&snapshot, &auth, &store, || Some(snapshot.clone()))
            .expect("切到另一个账号不能被源账号的时间戳挡住");

        let after = read_auth_file_at(&auth).expect("认证文件可读");
        assert_eq!(after["account"]["uid"], "uid-202");
        assert_eq!(
            after["auth"]["accessToken"],
            format!("at-{FRESHNESS_BASE_MS}")
        );
    }

    /// ③ 正常路径：无人持锁、磁盘未更新 → 写回成功且内容正确。
    #[test]
    fn write_back_succeeds_when_nothing_changed() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = account_record("acc-202", 10_000, 0);

        write_back_credentials_at(&snapshot, &auth, &store, || Some(snapshot.clone()))
            .expect("正常路径必须成功");

        let written = read_auth_file_at(&auth).expect("认证文件可读");
        assert_eq!(written["account"]["uid"], "uid-202");
        assert_eq!(
            written["auth"]["accessToken"],
            format!("at-{FRESHNESS_BASE_MS}")
        );
        assert_eq!(
            written["auth"]["refreshToken"],
            format!("rt-{FRESHNESS_BASE_MS}")
        );
        assert_eq!(
            written["auth"]["lastRefreshTime"],
            json!(FRESHNESS_BASE_MS),
            "写回必须沿用账号库记录的刷新时刻（不是写回时刻）"
        );
        assert!(
            try_acquire_credential_write_lock_at(&store).is_ok(),
            "成功路径同样必须释放锁"
        );
    }

    /// ③ 账号库里查不到记录（切换期间被删）按「没有更新证据」放行：
    /// 不能因为读不到就误判成「磁盘更新」而拦住切号。
    #[test]
    fn write_back_allows_missing_store_record() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = account_record("acc-202", 10_000, 0);

        write_back_credentials_at(&snapshot, &auth, &store, || None).expect("无记录时应当放行");

        assert!(auth.exists());
    }

    /// ③ 双方都没有时间戳（历史数据）时不得误伤。
    #[test]
    fn write_back_without_timestamps_is_not_blocked() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();
        let snapshot = json!({"id": "acc-legacy", "uid": "uid-legacy", "access_token": "at"});

        write_back_credentials_at(&snapshot, &auth, &store, || Some(snapshot.clone()))
            .expect("没有时间戳不算「磁盘更新」");

        assert!(auth.exists());
    }

    /// 新鲜度只认续期时间：`expiresAt` 前进但 `refreshedAt` 没变时同样要放弃
    /// （网关只回 `expiresIn` 的路径会写出这种记录）。
    #[test]
    fn newer_than_detects_advanced_expiry_only() {
        let snapshot = CredentialFreshness::of_account(&account_record("acc-202", 10_000, 0));
        let disk = CredentialFreshness::of_account(&account_record("acc-202", 20_000, 0));
        let reason = disk.newer_than(&snapshot).expect("到期时间前进也算更新");
        assert!(reason.contains("到期时间"), "{reason}");

        // 相等 / 任一侧未知 → 不算更新（正常切号与历史数据都不能被拦住）。
        assert!(snapshot.newer_than(&snapshot).is_none());
        let unknown = CredentialFreshness::default();
        assert!(disk.newer_than(&unknown).is_none());
        assert!(unknown.newer_than(&snapshot).is_none());
    }

    /// `of_account` 取 `refreshedAt` 与 `auth_raw` 两种结构的最大值，秒级按毫秒归一。
    #[test]
    fn freshness_reads_both_auth_raw_shapes() {
        let nested = json!({
            "id": "acc-nested",
            "expiresAt": FRESHNESS_BASE_MS + 5_000,
            "auth_raw": {"auth": {"lastRefreshTime": FRESHNESS_BASE_MS + 4_000}},
        });
        let flat = json!({
            "id": "acc-flat",
            "auth_raw": {"lastRefreshTime": FRESHNESS_BASE_MS + 3_000},
        });
        assert_eq!(
            CredentialFreshness::of_account(&nested).refreshed_at(),
            Some(FRESHNESS_BASE_MS + 4_000)
        );
        assert_eq!(
            CredentialFreshness::of_account(&flat).refreshed_at(),
            Some(FRESHNESS_BASE_MS + 3_000)
        );
        // 秒级时间戳（4 秒）按毫秒归一：否则与毫秒值比较会被无条件判旧。
        let seconds = json!({"id": "acc-sec", "auth_raw": {"lastRefreshTime": 4}});
        assert_eq!(
            CredentialFreshness::of_account(&seconds).refreshed_at(),
            Some(4_000)
        );
    }

    /// 「同一个账号」判据覆盖官方文件的两种结构；取不到身份时按「不是同一个」。
    #[test]
    fn same_account_matches_uid_in_both_shapes() {
        let acc = json!({"uid": "uid-1"});
        assert!(same_account(&acc, &json!({"account": {"uid": "uid-1"}})));
        assert!(same_account(&acc, &json!({"uid": "uid-1"})));
        assert!(!same_account(&acc, &json!({"account": {"uid": "uid-2"}})));
        assert!(!same_account(&acc, &json!({"account": {}})));
        assert!(!same_account(
            &json!({}),
            &json!({"account": {"uid": "uid-1"}})
        ));
    }

    /// 独立复现（QA 复核，issue #202 核心不变量）：**快照落后于磁盘** → 放弃写回，
    /// 且官方认证文件**逐字节未变**。
    ///
    /// 与上面的用例差别：这里用的是「真实切号会写出的那种认证文件」原始字节
    /// （含 `allAccounts`、缩进与行尾空白），断言的是**整份文件的字节**，而不是某几个
    /// JSON 字段——只要写回逻辑哪怕只做一次「读出来重新序列化」，字节就会变（键序、
    /// 缩进、尾随换行都会变），这条用例就会红。
    #[test]
    fn write_back_abandoned_when_snapshot_is_stale_keeps_file_bytes_identical() {
        let dir = TempCredentialDir::new();
        let store = dir.store_root();
        let auth = dir.auth_path();

        // 磁盘：源账号（uid-other）当前登录着，且刚刚续期过。
        let original = concat!(
            "{\n",
            "  \"account\": {\n",
            "    \"uid\": \"uid-other\",\n",
            "    \"nickname\": \"源账号\"\n",
            "  },\n",
            "  \"auth\": {\n",
            "    \"accessToken\": \"fresh-at\",\n",
            "    \"refreshToken\": \"fresh-rt\",\n",
            "    \"lastRefreshTime\": 1700000009000,\n",
            "    \"expiresAt\": 1700005000000\n",
            "  },\n",
            "  \"allAccounts\": []\n",
            "}\n"
        );
        std::fs::write(&auth, original.as_bytes()).expect("写入磁盘认证文件");
        let before = std::fs::read(&auth).expect("读回原始字节");

        // 手上快照：本次切号开头读到的那份，`refreshedAt`/`expiresAt` 都更早。
        let stale_snapshot = account_record("acc-202", 10_000, 0);
        // 账号库里的同一条记录已经被刷新推进（`refreshedAt` 更晚）。
        let refreshed_record = account_record("acc-202", 30_000, 3_000);

        let error =
            write_back_credentials_at(&stale_snapshot, &auth, &store, || Some(refreshed_record))
                .expect_err("磁盘更新时必须放弃写回");
        assert!(error.contains("放弃写回"), "{error}");

        // ① 字节级不变：没有重排键、没有重新缩进、没有多/少尾随换行。
        let after = std::fs::read(&auth).expect("读回写回后的字节");
        assert_eq!(after, before, "放弃写回时认证文件必须逐字节不变");
        assert_eq!(
            String::from_utf8(after).unwrap(),
            original,
            "内容与原始文本必须完全一致"
        );
        // ② 没写退出标记、锁已释放。
        assert!(!logout_marker_path(&auth).exists());
        assert!(try_acquire_credential_write_lock_at(&store).is_ok());

        // ③ 正控：同一份原始字节、快照**没有**落后时，写回会真的改写它。
        // 没有这条断言，上面「字节未变」可能只是因为写回永远不写（恒真）。
        let control = TempCredentialDir::new();
        let control_auth = control.auth_path();
        std::fs::write(&control_auth, original.as_bytes()).expect("写入控制组认证文件");
        let snapshot = account_record("acc-202", 10_000, 0);
        write_back_credentials_at(&snapshot, &control_auth, &control.store_root(), || {
            Some(snapshot.clone())
        })
        .expect("未过期时必须写回成功");
        assert_ne!(
            std::fs::read(&control_auth).expect("读回控制组字节"),
            before,
            "正控失败：写回成功时文件字节必须发生变化"
        );
    }
