    // -----------------------------------------------------------------------
    // 并发合并（single-flight）用例
    //
    // 这些用例断言的是「完整统计跑了几次」，用 `collect_count(home)`（按 home 计数）
    // 而不是 `cache::PARSE_COUNTS`：文件解析缓存会让第二遍统计也零解析，解析次数
    // 区分不出「合并成一次」与「扫了两遍但都命中文件缓存」。
    //
    // memo 是进程级单格全局状态，所以每个用例都要先取 `memo_test_guard()` 串行，
    // 否则并行用例会把对方刚写进去的那一格覆盖掉，断言随机变红。
    // -----------------------------------------------------------------------

    use std::sync::Barrier;

    /// 造一个最小家目录：`<home>/.workbuddy/projects/<项目>/<会话>.jsonl`。
    ///
    /// 只喂国内版一条源：统计口径已由 `cache.rs` / `ide_roots.rs` 覆盖，这里只关心
    /// 并发合并行为，而单源 fixture 能用 `summary.input` 直接看出结果来自哪个 home。
    fn fixture_home(tag: &str, input_tokens: i64) -> PathBuf {
        let now = crate::modules::config::now_ms();
        let home = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-memo-{tag}-{}-{now}",
            std::process::id()
        ));
        let project = home
            .join(".workbuddy")
            .join("projects")
            .join("fixture-project");
        fs::create_dir_all(&project).expect("create memo fixture dirs");
        let record = json!({
            "timestamp": now,
            "cwd": "/fixture/fixture-project",
            "message": { "usage": { "input_tokens": input_tokens, "output_tokens": 1 } }
        });
        fs::write(project.join("session-a.jsonl"), record.to_string() + "\n")
            .expect("write memo fixture");
        home
    }

    /// 两个线程并发调用同一 `(home, days)`：只做一次完整统计，且拿到同一份结果。
    #[test]
    fn concurrent_same_key_scans_once() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let home = fixture_home("concurrent", 10);
        // 用屏障让两个线程尽量同时进入统计入口（真正的并发起点）。
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let home = home.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    get_statistics_at(&home, Some(7))
                })
            })
            .collect();
        let values: Vec<Value> = handles
            .into_iter()
            .map(|handle| handle.join().expect("join statistics thread"))
            .collect();

        assert_eq!(
            collect_count(&home),
            1,
            "同一 (home, days) 的并发调用只允许做一次完整统计"
        );
        assert_eq!(
            values[0]["generatedAt"], values[1]["generatedAt"],
            "后来者必须复用同一份结果（同一个 generatedAt）"
        );
        assert_eq!(values[0]["sources"][0]["summary"]["input"], 10);
        assert_eq!(values[1]["sources"][0]["summary"]["input"], 10);
        assert_eq!(values[0]["rangeDays"], 7);
        assert_eq!(values[1]["rangeDays"], 7);

        clear_statistics_memo();
        fs::remove_dir_all(home).expect("remove memo fixture");
    }

    /// 四个线程并发调用同一 `(home, days)`：仍然只算一次（加分项）。
    #[test]
    fn four_concurrent_callers_scan_once() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let home = fixture_home("four", 10);
        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let home = home.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    get_statistics_at(&home, Some(90))["generatedAt"].clone()
                })
            })
            .collect();
        let generated: Vec<Value> = handles
            .into_iter()
            .map(|handle| handle.join().expect("join statistics thread"))
            .collect();

        assert_eq!(
            collect_count(&home),
            1,
            "四个并发调用只允许做一次完整统计"
        );
        assert!(
            generated.windows(2).all(|pair| pair[0] == pair[1]),
            "四个并发调用必须拿到同一个 generatedAt：{generated:?}"
        );

        clear_statistics_memo();
        fs::remove_dir_all(home).expect("remove memo fixture");
    }

    /// 同一 key 的连续调用命中 memo；换 `days` 必须重算（不跨 `days` 复用）。
    ///
    /// memo 只有一格（最近一次），所以「7 天 → 30 天 → 7 天」会重算两次：
    /// 这是有意的，T2.6 只合并并发，不做多 key 长期缓存。
    #[test]
    fn different_windows_are_not_merged() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let home = fixture_home("windows", 10);

        let week = get_statistics_at(&home, Some(7));
        let week_again = get_statistics_at(&home, Some(7));
        assert_eq!(collect_count(&home), 1, "同 key 的第二次调用应命中 memo");
        assert_eq!(week_again["generatedAt"], week["generatedAt"]);

        let month = get_statistics_at(&home, Some(30));
        assert_eq!(month["rangeDays"], 30);
        assert_eq!(
            collect_count(&home),
            2,
            "不同 days 不得复用同一份结果，必须重算一次"
        );

        let week_third = get_statistics_at(&home, Some(7));
        assert_eq!(week_third["rangeDays"], 7);
        assert_eq!(
            collect_count(&home),
            3,
            "memo 只有一格，被 30 天覆盖后回到 7 天要重算（不做多 key 长期缓存）"
        );

        clear_statistics_memo();
        fs::remove_dir_all(home).expect("remove memo fixture");
    }

    /// `days` 归一化后才进 key：`Some(15)` 与 `None` 都是「全部」，共用同一份结果。
    #[test]
    fn unlisted_days_share_the_all_window_entry() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let home = fixture_home("normalized", 10);

        let all = get_statistics_at(&home, None);
        let unlisted = get_statistics_at(&home, Some(15));
        assert_eq!(
            collect_count(&home),
            1,
            "Some(15) 与 None 口径相同（全部），应命中同一份 memo"
        );
        assert_eq!(all["rangeDays"], Value::Null);
        assert_eq!(unlisted["rangeDays"], Value::Null);
        assert_eq!(unlisted["generatedAt"], all["generatedAt"]);

        clear_statistics_memo();
        fs::remove_dir_all(home).expect("remove memo fixture");
    }

    /// 超过合并窗口（生产窗口 1500 ms）后必须重算：memo 只合并并发，不是长期缓存。
    ///
    /// 这里真的等过期，而不注入测试专用窗口——生产窗口常量的语义不该为测试让步。
    #[test]
    fn expired_window_rescans() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let home = fixture_home("expiry", 10);

        let first = get_statistics_at(&home, Some(7));
        assert_eq!(collect_count(&home), 1);

        std::thread::sleep(Duration::from_millis(1_600));

        let second = get_statistics_at(&home, Some(7));
        assert_eq!(collect_count(&home), 2, "窗口过期后必须重新统计");
        assert_ne!(
            second["generatedAt"], first["generatedAt"],
            "重算后的 generatedAt 必须是新的时间点"
        );
        assert_eq!(second["sources"][0]["summary"]["input"], 10);

        clear_statistics_memo();
        fs::remove_dir_all(home).expect("remove memo fixture");
    }

    /// 两个临时 home 不串用：结果各自来自自己的家目录。
    #[test]
    fn different_homes_do_not_share_results() {
        let _guard = memo_test_guard();
        clear_statistics_memo();
        let first_home = fixture_home("home-a", 10);
        let second_home = fixture_home("home-b", 20);

        let first = get_statistics_at(&first_home, Some(7));
        let second = get_statistics_at(&second_home, Some(7));
        assert_eq!(first["sources"][0]["summary"]["input"], 10);
        assert_eq!(
            second["sources"][0]["summary"]["input"], 20,
            "不同 home 不得复用同一份统计结果"
        );
        assert_eq!(collect_count(&first_home), 1);
        assert_eq!(collect_count(&second_home), 1);

        // 回到第一个 home：memo 已被第二个覆盖，必须重算，但结果仍是它自己的数据。
        let first_again = get_statistics_at(&first_home, Some(7));
        assert_eq!(collect_count(&first_home), 2);
        assert_eq!(first_again["sources"][0]["summary"]["input"], 10);

        clear_statistics_memo();
        fs::remove_dir_all(first_home).expect("remove first memo fixture");
        fs::remove_dir_all(second_home).expect("remove second memo fixture");
    }
