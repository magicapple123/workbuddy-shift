    /// design §6.2 步 5：重建与复算核验都成功、只有「提交基线」失败 → 目标整目录从备份恢复。
    ///
    /// 与 `overwrite_failure_keeps_target_intact`（重建就被挡住）互补：这里必须走
    /// `commit_baseline` 失败后的 `restore_dir` 分支，而不是重建失败分支。
    #[test]
    fn overwrite_baseline_commit_failure_restores_whole_dir() {
        let fixture = Fixture::new("overwrite-rollback-baseline");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        let target_dir = fixture.dst_conv_dir(&target_conv);

        // 副本侧本地改动 + 源侧新增一轮 → 分叉，需要显式覆盖。
        let assistant = target_dir.join(format!("messages/{}.json", records(&target_dir)[1]));
        let text = std::fs::read_to_string(&assistant)
            .unwrap()
            .replace("在的", "在的（副本本地改动）");
        std::fs::write(&assistant, text).unwrap();
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");
        // 会话级索引备份文件（实测形态）：重建不会产出它，整目录恢复必须把它带回来。
        let target_index = std::fs::read_to_string(target_dir.join("index.json")).unwrap();
        write(&target_dir.join(".index_bak.json"), &target_index);

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "diverge");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 让关联存储锁无法建立（路径被目录占住）→ 只有「提交基线」这一步会失败。
        let lock = fixture.store.join("locks/vscode-session-links.lock");
        let _ = std::fs::remove_file(&lock);
        std::fs::create_dir_all(&lock).unwrap();

        // 断言范围取目标账号的整个数据根：内容逐字节一致 + 不留 `.tmp-*-overwrite` 残骸。
        let target_root = vscode_session::history_root(&fixture.root, DST_UID);
        let before_root = dir_snapshot(&target_root);
        let before_conv = dir_snapshot(&target_dir);
        let before_messages = dir_snapshot(&target_dir.join("messages"));
        assert_eq!(before_messages.len(), 2, "造数：操作前副本有 2 条消息文件");
        assert!(
            before_conv
                .keys()
                .any(|key| key.ends_with(".index_bak.json")),
            "造数：操作前副本已有索引备份文件"
        );

        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::Overwrite)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
        assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
        let error = report["errors"][0]["error"].as_str().unwrap();
        assert!(
            error.contains("同步记录"),
            "失败必须来自提交基线（而非重建）：{error}"
        );

        assert_eq!(
            dir_snapshot(&target_root),
            before_root,
            "基线提交失败后目标目录必须整目录恢复（含 messages/ 与索引备份文件）"
        );
        assert_eq!(
            dir_snapshot(&target_dir.join("messages")),
            before_messages,
            "messages/ 必须与操作前逐字节一致"
        );
        assert_eq!(records(&target_dir).len(), 2, "索引必须回到操作前");
        let tmp = target_dir.with_file_name(format!(".tmp-{target_conv}{OVERWRITE_SUFFIX}"));
        assert!(!tmp.exists(), "重建用的临时目录不得残留：{tmp:?}");

        // 覆盖前的整目录备份必须保留（恢复的唯一依据），内容 = 操作前的目标会话目录。
        let backup = find_overwrite_backup(&fixture, &target_conv);
        assert_eq!(
            dir_snapshot(&backup),
            before_conv,
            "备份内容必须等于操作前的目标会话目录"
        );
    }

    /// design §6.1 相邻窗口：「文件已写、索引未改」被中断后重跑必须收敛。
    ///
    /// 造数：先正常同步一次（产出派生 id 的消息文件），再把会话索引与关联存储回滚到操作前，
    /// 但保留已写入的消息文件——等价于进程恰好在「写完新增文件、更新索引」之间被 kill。
    /// 重跑时 [`plan_message_file`] 必须走 `already_applied` 分支复用同名同内容文件，
    /// 而不是换 salt 再写一份（那会留下索引引用不到的孤儿文件）。
    #[test]
    fn fast_forward_converges_after_interrupted_index_write() {
        let fixture = Fixture::new("already-applied");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");
        let target_dir = fixture.dst_conv_dir(&target_conv);
        let orphans = interrupted_index_write_scene(&fixture, &target_conv);

        // 重跑（凭据必须重新取得）：目标内容仍等于旧基线 → 判定还是可快进。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        assert_eq!(
            group["recordCount"],
            json!({"source": 4, "target": 2, "baseline": 2})
        );
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
        assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");

        // 收敛：索引补齐这批记录、无重复、复用已落盘文件、无孤儿文件、摘要与源逐条一致。
        let after = records(&target_dir);
        assert_eq!(after.len(), 4, "索引必须补齐这批记录");
        assert_eq!(
            after.iter().collect::<BTreeSet<_>>().len(),
            4,
            "不得出现重复 id"
        );
        let source_digests = digests(&fixture.src_conv_dir(), CONV_SRC);
        assert_eq!(
            after[2],
            vscode_session_link::derive_message_id(DST_UID, &target_conv, 2, &source_digests[2], 0)
        );
        assert_eq!(
            after[3],
            vscode_session_link::derive_message_id(DST_UID, &target_conv, 3, &source_digests[3], 0)
        );
        let final_files = message_files(&target_dir);
        assert!(
            orphans.iter().all(|name| final_files.contains(name)),
            "已落盘的新增文件必须被复用（不得换 salt 重写）：{orphans:?}"
        );
        assert_eq!(
            final_files,
            after
                .iter()
                .map(|id| format!("{id}.json"))
                .collect::<BTreeSet<String>>(),
            "落盘文件必须与索引引用的消息一一对应（不产生多余的 id / 孤儿文件）"
        );
        assert_eq!(digests(&target_dir, &target_conv), source_digests);

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(preview["groups"][0]["verdict"], "identical");
    }

    /// AC4：预览之后源侧再追加一条 → 旧凭据失效，该项按 `previewStale` 跳过且目标零写入。
    ///
    /// 与 `fast_forward_sync_is_idempotent`（同步成功后再用旧凭据）互补：这里的变化发生在
    /// **源侧**、且发生在预览与执行之间（用户在确认切换前又聊了一轮）。
    #[test]
    fn sync_skips_when_source_appended_after_preview() {
        let fixture = Fixture::new("stale-after-append");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 预览之后源侧再追加一轮 → 源内容与预览时不一致（目标侧没有任何变化）。
        append_round(&fixture, MSG_5, MSG_6, REQ_3, "预览之后又来一条");

        let target_dir = fixture.dst_conv_dir(&target_conv);
        let before = dir_snapshot(&target_dir);
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
        assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["skipped"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
        assert_eq!(report["skipped"][0]["verdict"], "fastForward");
        assert_eq!(records(&target_dir).len(), 2, "目标记录数不得变化");
        assert_eq!(
            dir_snapshot(&target_dir),
            before,
            "目标目录必须完全未被写入"
        );

        // 失效只是「需要重新检查」而不是死局：重新预览仍是可快进，同步后与源逐条一致。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        assert_eq!(group["recordCount"]["source"], 6);
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(records(&target_dir).len(), 6);
        assert_eq!(
            digests(&target_dir, &target_conv),
            digests(&fixture.src_conv_dir(), CONV_SRC)
        );
    }

    /// AC4 的另一半：预览之后**副本侧**再被改 → 旧凭据失效，两种模式都被跳过且目标零写入。
    ///
    /// 与 `sync_skips_when_source_appended_after_preview`（变化在源侧）互补，对齐 WorkBuddy 侧
    /// `sync_skips_when_target_changed_after_preview_even_with_overwrite`：显式覆盖也不能绕过
    /// 版本校验，更不得据此回写目标。
    #[test]
    fn sync_skips_when_target_changed_after_preview_even_with_overwrite() {
        let fixture = Fixture::new("stale-target");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        let target_dir = fixture.dst_conv_dir(&target_conv);

        // 副本本地改动 + 源侧新增一轮 → 分叉：预览会发放覆盖凭据（该项可勾选）。
        let assistant = target_dir.join(format!("messages/{}.json", records(&target_dir)[1]));
        let text = std::fs::read_to_string(&assistant)
            .unwrap()
            .replace("在的", "在的（副本本地改动）");
        std::fs::write(&assistant, text).unwrap();
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "diverge");
        assert_eq!(group["availableModes"], json!(["overwrite"]));
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 预览之后副本侧又被改（用户在确认切换前又动过副本）→ 与凭据里的目标内容不一致。
        let user_message = target_dir.join(format!("messages/{}.json", records(&target_dir)[0]));
        let text = std::fs::read_to_string(&user_message)
            .unwrap()
            .replace("你好", "你好（预览之后又改）");
        std::fs::write(&user_message, text).unwrap();

        let before = dir_snapshot(&target_dir);
        for mode in [SyncMode::FastForward, SyncMode::Overwrite] {
            let report = sync_selected_at(
                &fixture.root,
                &fixture.paths(),
                SRC_UID,
                &target_acc(),
                &[selection(&group_id, &token, mode)],
            )
            .unwrap();
            assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
            assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
            assert_eq!(report["skipped"].as_array().unwrap().len(), 1, "{report}");
            let skipped = &report["skipped"][0];
            assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE, "{report}");
            assert_eq!(skipped["verdict"], "diverge", "{report}");
            assert!(
                skipped["message"]
                    .as_str()
                    .unwrap()
                    .contains("目标账号的内容已变化"),
                "{skipped}"
            );
        }
        assert_eq!(
            dir_snapshot(&target_dir),
            before,
            "被跳过时目标目录必须完全未被写入"
        );
        assert_eq!(records(&target_dir).len(), 2, "目标记录数不得变化");
    }
