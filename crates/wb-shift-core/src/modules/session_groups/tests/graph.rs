    #[test]
    fn aggregate_selects_a_safe_dominant_source_and_marks_only_fast_forward_targets_behind() {
        let mut views = vec![
            member_view(
                "source",
                "account-a",
                MemberState::Active,
                ready_content("a"),
            ),
            member_view(
                "behind",
                "account-b",
                MemberState::Active,
                ready_content("b"),
            ),
        ];
        let mut matrix = HashMap::new();
        pair(&mut matrix, 0, 1, SyncVerdict::FastForward);
        pair(&mut matrix, 1, 0, SyncVerdict::Ahead);

        let active = active_indexes(&views);
        let (safe_source, summary) = aggregate_group_state(&mut views, &active, &matrix);

        assert_eq!(safe_source, Some(0));
        assert_eq!(summary, "behind");
        assert_eq!(views[0].version_status, "latest");
        assert_eq!(views[1].version_status, "behind");
    }

    #[test]
    fn aggregate_keeps_equal_latest_copies_and_uses_stable_account_order_for_operations() {
        // Input order differs from the stable account ordering on purpose.
        let mut views = vec![
            member_view(
                "latest-z",
                "account-z",
                MemberState::Active,
                ready_content("z"),
            ),
            member_view(
                "latest-a",
                "account-a",
                MemberState::Active,
                ready_content("a"),
            ),
            member_view(
                "behind",
                "account-m",
                MemberState::Active,
                ready_content("m"),
            ),
        ];
        let mut matrix = HashMap::new();
        for (source, target, verdict) in [
            (0, 1, SyncVerdict::Identical),
            (1, 0, SyncVerdict::Identical),
            (0, 2, SyncVerdict::FastForward),
            (1, 2, SyncVerdict::FastForward),
            (2, 0, SyncVerdict::Ahead),
            (2, 1, SyncVerdict::Ahead),
        ] {
            pair(&mut matrix, source, target, verdict);
        }

        let active = active_indexes(&views);
        let (safe_source, summary) = aggregate_group_state(&mut views, &active, &matrix);

        assert_eq!(safe_source, Some(1));
        assert_eq!(views[0].version_status, "latest");
        assert_eq!(views[1].version_status, "latest");
        assert_eq!(views[2].version_status, "behind");
        assert_eq!(summary, "behind");
    }

    #[test]
    fn aggregate_reports_divergence_without_inventing_a_safe_source() {
        let mut views = vec![
            member_view("a", "account-a", MemberState::Active, ready_content("a")),
            member_view("b", "account-b", MemberState::Active, ready_content("b")),
        ];
        let mut matrix = HashMap::new();
        pair(&mut matrix, 0, 1, SyncVerdict::Diverge);
        pair(&mut matrix, 1, 0, SyncVerdict::Diverge);

        let active = active_indexes(&views);
        let (safe_source, summary) = aggregate_group_state(&mut views, &active, &matrix);

        assert_eq!(safe_source, None);
        assert_eq!(summary, "diverge");
        assert_eq!(views[0].version_status, "diverge");
        assert_eq!(views[1].version_status, "diverge");
    }

    #[test]
    fn recognizes_three_equal_old_copies_and_two_independent_extensions() {
        let views = vec![
            member_view("old-a", "a", MemberState::Active, ready_lines(&["base"])),
            member_view(
                "tip-a",
                "b",
                MemberState::Active,
                ready_lines(&["base", "alice"]),
            ),
            member_view("old-b", "c", MemberState::Active, ready_lines(&["base"])),
            member_view(
                "tip-b",
                "d",
                MemberState::Active,
                ready_lines(&["base", "bob"]),
            ),
            member_view("old-c", "e", MemberState::Active, ready_lines(&["base"])),
        ];
        assert_eq!(
            common_base_branches(&views, &active_indexes(&views)),
            Some(CommonBaseBranches {
                common: vec![0, 2, 4],
                branches: vec![vec![1], vec![3]]
            })
        );
    }

    #[test]
    fn does_not_call_a_linear_history_independent_branches() {
        let views = vec![
            member_view("old", "a", MemberState::Active, ready_lines(&["base"])),
            member_view(
                "mid",
                "b",
                MemberState::Active,
                ready_lines(&["base", "alice"]),
            ),
            member_view(
                "new",
                "c",
                MemberState::Active,
                ready_lines(&["base", "alice", "bob"]),
            ),
        ];
        assert_eq!(common_base_branches(&views, &active_indexes(&views)), None);
    }

    /// 由**原始 JSONL 行**构造内容快照。
    ///
    /// 与 `ready_lines` 的区别：后者把入参当成 id 再包一层 `{"id":...}`，造不出
    /// 「assistant 消息 + 归一留痕」这类真实记录形态，而这正是本用例要验的摘要口径。
    fn ready_jsonl(lines: &[&str]) -> ContentState {
        let text = lines.join("\n");
        ContentState::Ready(ContentSnapshot {
            full_digest: full_digest_of(text.as_bytes()),
            normalized: normalize_jsonl(&text, "unrelated-session-id").unwrap(),
            text,
        })
    }

    /// 末条 `incomplete` 归一后的副本与来源是**同一个版本**，不是独立分支。
    ///
    /// 三成员组：老副本（base）、来源（base + `incomplete` 尾）、新副本（base + 归一后的尾）。
    /// 两两判定分别是 identical / fastForward（老副本可安全快进），组徽标必须是 `behind`。
    /// 改造前 `common_base_branches` 比原始摘要，会把来源与归一后的新副本当成两条独立分支
    /// 返回 `Some(...)`，`view.rs` 随后把 `summaryStatus` 强行改成 `diverge`，与同一组两两
    /// `decide_sync` 的结论自相矛盾（issue #3 最后一条：「存在分歧」消不掉）。
    #[test]
    fn normalized_copy_is_not_an_independent_branch() {
        let base = [
            r#"{"type":"user","text":"你好"}"#,
            r#"{"type":"message","role":"assistant","status":"completed","text":"hi"}"#,
        ];
        // 来源末条停在 incomplete（用户中断 / 限流 / 报错）。
        let source_lines = [
            base[0],
            base[1],
            r#"{"type":"message","role":"assistant","status":"incomplete","text":"被打断"}"#,
        ];
        // 复制 / 同步写目标时的归一形态：`status` 改 completed + `_meta.incompleteNormalized` 留痕。
        let normalized_lines = [
            base[0],
            base[1],
            r#"{"type":"message","role":"assistant","status":"completed","text":"被打断","_meta":{"incompleteNormalized":{"at":"20260101-000000","reason":"copy normalize"}}}"#,
        ];

        let mut views = vec![
            member_view("old", "a", MemberState::Active, ready_jsonl(&base)),
            member_view(
                "source",
                "b",
                MemberState::Active,
                ready_jsonl(&source_lines),
            ),
            member_view(
                "copy",
                "c",
                MemberState::Active,
                ready_jsonl(&normalized_lines),
            ),
        ];
        let baseline = session_link::BaselineState::Missing;
        let decide = |source: usize, target: usize| {
            session_link::decide_sync(&views[source].content, &views[target].content, &baseline)
        };
        // 前提：两两判定本来就认为「来源与归一副本一致、老副本可快进」。
        assert_eq!(decide(1, 2).verdict, SyncVerdict::Identical);
        assert_eq!(decide(2, 1).verdict, SyncVerdict::Identical);
        assert_eq!(decide(1, 0).verdict, SyncVerdict::FastForward);
        assert_eq!(decide(0, 1).verdict, SyncVerdict::Ahead);

        let active = active_indexes(&views);
        assert_eq!(
            common_base_branches(&views, &active),
            None,
            "归一后的副本不是独立分支：否则组徽标被强行改成 diverge"
        );

        let mut matrix = HashMap::new();
        for (source, target) in [(0, 1), (1, 0), (0, 2), (2, 0), (1, 2), (2, 1)] {
            let decision = session_link::decide_sync(
                &views[source].content,
                &views[target].content,
                &baseline,
            );
            matrix.insert((source, target), (decision.verdict, decision.reason));
        }
        let (safe_source, summary) = aggregate_group_state(&mut views, &active, &matrix);
        assert_eq!(
            safe_source,
            Some(1),
            "来源是唯一安全的操作来源：老副本可快进、归一副本内容一致"
        );
        assert_eq!(summary, "behind");
        assert_eq!(views[0].version_status, "behind");
        assert_eq!(views[1].version_status, "latest");
        assert_eq!(views[2].version_status, "latest");
    }

    #[test]
    fn aggregate_keeps_unknown_and_missing_distinct() {
        let mut unknown_views = vec![
            member_view("a", "account-a", MemberState::Active, ready_content("a")),
            member_view(
                "unknown",
                "account-b",
                MemberState::Active,
                ContentState::Unavailable("正文读取失败".to_string()),
            ),
        ];
        let mut unknown_matrix = HashMap::new();
        pair(&mut unknown_matrix, 0, 1, SyncVerdict::Unknown);
        pair(&mut unknown_matrix, 1, 0, SyncVerdict::Unknown);
        let active = active_indexes(&unknown_views);
        let (safe_source, summary) =
            aggregate_group_state(&mut unknown_views, &active, &unknown_matrix);
        assert_eq!(safe_source, None);
        assert_eq!(summary, "unknown");
        assert_eq!(unknown_views[1].version_status, "unknown");

        let mut missing_views = vec![
            member_view("a", "account-a", MemberState::Active, ready_content("a")),
            member_view(
                "missing",
                "account-b",
                MemberState::Active,
                ContentState::Missing,
            ),
        ];
        let mut missing_matrix = HashMap::new();
        pair(&mut missing_matrix, 0, 1, SyncVerdict::Unknown);
        pair(&mut missing_matrix, 1, 0, SyncVerdict::Unknown);
        let active = active_indexes(&missing_views);
        let (safe_source, summary) =
            aggregate_group_state(&mut missing_views, &active, &missing_matrix);
        assert_eq!(safe_source, None);
        assert_eq!(summary, "missing");
        assert_eq!(missing_views[1].version_status, "missing");
    }

    #[test]
    fn aggregate_preserves_stale_and_superseded_members_outside_active_source_selection() {
        let mut views = vec![
            member_view(
                "source",
                "account-a",
                MemberState::Active,
                ready_content("a"),
            ),
            member_view(
                "behind",
                "account-b",
                MemberState::Active,
                ready_content("b"),
            ),
            member_view(
                "stale",
                "account-c",
                MemberState::Stale,
                ContentState::Missing,
            ),
            member_view(
                "superseded",
                "account-d",
                MemberState::Superseded,
                ContentState::Missing,
            ),
        ];
        let mut matrix = HashMap::new();
        pair(&mut matrix, 0, 1, SyncVerdict::FastForward);
        pair(&mut matrix, 1, 0, SyncVerdict::Ahead);

        let active = active_indexes(&views);
        let (safe_source, summary) = aggregate_group_state(&mut views, &active, &matrix);

        assert_eq!(safe_source, Some(0));
        assert_eq!(summary, "behind");
        assert_eq!(views[2].version_status, "stale");
        assert_eq!(views[3].version_status, "superseded");
    }

    /// 汇总对象初始化形状：与 `sync_safe_batch` / `sync_unify_batch_vscode` 一致。
    fn combined_report() -> Value {
        json!({
            "client": "workbuddy",
            "groupId": "g-1",
            "synced": [],
            "skipped": [],
            "errors": [],
            "needsRecovery": false,
            "temporaryFiles": [],
        })
    }

    /// 单个目标项的报告：只给 `key` 灌入一条记录，其余合并键为空。
    fn sync_report_item(key: &str) -> Value {
        let mut item = json!({
            "synced": [],
            "skipped": [],
            "errors": [],
            "temporaryFiles": [],
        });
        item[key] = json!([{"targetMemberId": "m-1"}]);
        item
    }

    #[test]
    fn merge_sync_report_merges_every_key_and_propagates_needs_recovery() {
        let mut combined = combined_report();
        combined["synced"] = json!([{"targetMemberId": "m-0"}]);
        combined["temporaryFiles"] = json!([{"targetMemberId": "m-0", "path": "/tmp/first"}]);

        merge_sync_report(
            &mut combined,
            json!({
                "synced": [{"targetMemberId": "m-1"}],
                "skipped": [{"targetMemberId": "m-1", "reasonCode": "previewStale"}],
                "errors": [{"error": "m-1：写入失败"}],
                "needsRecovery": true,
                "temporaryFiles": [{"targetMemberId": "m-1", "path": "/tmp/second"}],
            }),
        );

        assert_eq!(combined["synced"].as_array().map(Vec::len), Some(2));
        assert_eq!(combined["skipped"].as_array().map(Vec::len), Some(1));
        assert_eq!(combined["errors"].as_array().map(Vec::len), Some(1));
        assert_eq!(combined["temporaryFiles"].as_array().map(Vec::len), Some(2));
        assert_eq!(combined["temporaryFiles"][0]["path"], json!("/tmp/first"));
        assert_eq!(combined["temporaryFiles"][1]["path"], json!("/tmp/second"));
        assert_eq!(combined["needsRecovery"], json!(true));
    }

    #[test]
    fn merge_sync_report_initializes_missing_key_instead_of_panicking() {
        for key in ["synced", "skipped", "errors", "temporaryFiles"] {
            let mut combined = combined_report();
            combined.as_object_mut().unwrap().remove(key);
            assert!(combined.get(key).is_none());

            merge_sync_report(&mut combined, sync_report_item(key));

            assert_eq!(
                combined[key],
                json!([{"targetMemberId": "m-1"}]),
                "{key} 缺失时应补成数组并保留合并内容"
            );
        }
    }

    #[test]
    fn merge_sync_report_replaces_null_key_with_merged_array() {
        for key in ["synced", "skipped", "errors", "temporaryFiles"] {
            let mut combined = combined_report();
            combined[key] = Value::Null;
            assert!(combined[key].is_null());

            merge_sync_report(&mut combined, sync_report_item(key));

            assert_eq!(
                combined[key],
                json!([{"targetMemberId": "m-1"}]),
                "{key} 为 Null 时应补成数组并保留合并内容"
            );
        }
    }

    // -----------------------------------------------------------------------
    // issue #231：跨组安全批量同步（`sync_all`）
    // -----------------------------------------------------------------------

    /// 空候选（前端在 N=0 时不会放按钮，但接口必须自己兜住）直接拒绝，不去读任何存储。
    ///
    /// 只断言「空入参」这一条：非空入参会去解析真实客户端存储，用例不得读写用户数据。
    #[test]
    fn sync_all_safe_rejects_empty_group_list() {
        let err = sync_all_safe(SessionClient::Workbuddy, None, &[], false).unwrap_err();
        assert_eq!(err, "没有需要同步的会话组");
    }

    /// 跨组汇总必须给子报告条目补 `groupId`；内核已带 `groupId` 的条目不能被覆盖。
    #[test]
    fn stamp_group_id_backfills_missing_ids_only() {
        let mut item = json!({
            "synced": [{ "targetMemberId": "m-1" }, { "groupId": "g-inner", "targetMemberId": "m-2" }],
            "skipped": [{ "targetMemberId": "m-3", "reasonCode": "previewStale" }],
            "errors": [{ "error": "写入失败" }],
            "temporaryFiles": [{ "reason": "待恢复" }],
        });

        // 助手是 `sync_all` 的私有项（`pub(super)`）：用例按模块路径调用，不为测试放宽可见性。
        sync_all::stamp_group_id(&mut item, "g-1");
        assert_eq!(item["synced"][0]["groupId"], "g-1", "缺失 groupId 的条目要补上");
        assert_eq!(
            item["synced"][1]["groupId"], "g-inner",
            "内核已给的 groupId 不得被覆盖"
        );
        assert_eq!(item["skipped"][0]["groupId"], "g-1");
        assert_eq!(item["errors"][0]["groupId"], "g-1");
        assert!(
            item["temporaryFiles"][0].get("groupId").is_none(),
            "临时文件条目不是会话条目，不补 groupId"
        );
    }
