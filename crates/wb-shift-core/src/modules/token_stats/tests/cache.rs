    /// mtime 早于窗口起点的文件整份跳过读盘；`filesScanned` 仍按发现的文件数返回，
    /// 窗口内的记录口径不变。
    #[test]
    fn files_older_than_the_window_are_not_read() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-window-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        let record = |timestamp| {
            json!({
                "timestamp": timestamp,
                "cwd": "/fixture/fixture-project",
                "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } }
            })
            .to_string()
                + "\n"
        };

        // 老文件：记录与 mtime 都远早于窗口。
        let old = project.join("session-old.jsonl");
        fs::write(&old, record(now - 120 * 86_400_000)).expect("write old fixture");
        pin_mtime(
            old.clone(),
            std::time::SystemTime::now() - std::time::Duration::from_secs(120 * 86_400),
        );
        // 新文件：正常计入。
        let fresh = project.join("session-new.jsonl");
        fs::write(&fresh, record(now - 1_000)).expect("write new fixture");

        let result = source(root.clone(), "fixture", Some(now - 30 * 86_400_000), false);
        assert_eq!(parse_count(&old), 0, "窗口外的文件不该被读盘解析");
        assert_eq!(parse_count(&fresh), 1);

        assert_eq!(
            result["filesScanned"], 2,
            "filesScanned 仍按发现的文件总数返回"
        );
        assert_eq!(result["summary"]["records"], 1);
        assert_eq!(result["summary"]["input"], 10);
        assert_eq!(result["sessions"].as_array().map(Vec::len), Some(1));

        // 不限定窗口时，老文件必须被计入（裁剪只在有窗口时发生）。
        let all = source(root.clone(), "fixture", None, false);
        assert_eq!(parse_count(&old), 1);
        assert_eq!(all["summary"]["records"], 2);
        assert_eq!(all["summary"]["input"], 20);

        fs::remove_dir_all(root).expect("remove fixture");
    }
