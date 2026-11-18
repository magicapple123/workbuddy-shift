    // -----------------------------------------------------------------------
    // 会话存活判定（`liveness`）：索引条目 + 会话目录两条证据，三处消费方共用
    // -----------------------------------------------------------------------
    //
    // 优先级（见 `liveness` 模块注释）：
    // 1. 索引命中（含 `Partial` 的可读分片）→ Alive；
    // 2. 索引未命中但 `history/<workspaceHash>/<conversationId>/` 还在 → Alive；
    // 3. 两者都没有：`Available` → Gone（可信消失），其余四态 → Unknown（无完整依据）。
    //
    // 第 2 条是本轮新增的更强证据：`index.json` 可能损坏 / 半截写入 / 条目被改归属，
    // 目录却还在——那种现场旧实现判 Gone（预览隐藏、清理删组），是错的。

    #[test]
    fn directory_evidence_wins_over_a_missing_index_entry() {
        let fixture = Fixture::new("liveness-dir-evidence");
        seed_source(&fixture);
        // 索引可读但没有该会话条目（半截写入 / 条目被改归属的现场），目录仍在。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Available(BTreeSet::new()),
            "造数：索引必须是「可用 + 空集」，否则测不到目录证据"
        );

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);
        assert!(
            scan.session_dirs.contains(CONV_SRC),
            "同一次遍历必须收集到会话目录：{:?}",
            scan.session_dirs
        );
        assert_eq!(session_liveness(&scan, CONV_SRC), SessionLiveness::Alive);
    }

    /// 一次遍历：目录证据与索引定位表来自同一个 `conversation_index_scan`（不额外扫盘）。
    #[test]
    fn the_scan_collects_session_directories_while_reading_the_index() {
        let fixture = Fixture::new("liveness-single-pass");
        seed_source(&fixture);

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert_eq!(scan.index_workspaces, 1);
        assert_eq!(scan.parsed, 1);
        // 定位表里有 CONV_SRC（索引条目），目录集合里也有它（磁盘证据）。
        assert!(scan.locators.contains_key(CONV_SRC));
        assert_eq!(scan.session_dirs, BTreeSet::from([CONV_SRC.to_string()]));
    }

    /// `Partial`（部分分片损坏）且会话只在损坏分片里、目录存在 → 明确存活。
    ///
    /// 改造前这一现场是「整次拒绝」：索引读不全 + 可读分片未命中 → 判不了。
    #[test]
    fn partial_shards_with_a_session_directory_are_alive() {
        let fixture = Fixture::new("liveness-partial-dir");
        // WS_A 可读但为空（条目丢了），WS_B 损坏；会话目录在 WS_A 下。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        write(
            &workspace_index(&fixture, SRC_UID, "ffffffffffffffffffffffffffffffff"),
            "{ broken",
        );
        std::fs::create_dir_all(fixture.src_conv_dir()).unwrap();
        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Partial(BTreeSet::new())
        );

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert_eq!(session_liveness(&scan, CONV_SRC), SessionLiveness::Alive);
    }

    /// `Partial` 且目录也不存在 → Unknown（没有完整依据）：读路径 fail-open、写路径拒绝。
    #[test]
    fn partial_shards_without_a_session_directory_are_unknown() {
        let fixture = Fixture::new("liveness-partial-unknown");
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        write(
            &workspace_index(&fixture, SRC_UID, "ffffffffffffffffffffffffffffffff"),
            "{ broken",
        );

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert_eq!(
            session_liveness(&scan, CONV_SRC),
            SessionLiveness::Unknown(ConversationIndexState::Partial(BTreeSet::new()))
        );
    }

    /// `Available` + 索引无条目 + 目录不存在 → Gone（唯一能当作「可信消失」的组合）。
    #[test]
    fn available_without_entry_or_directory_is_gone() {
        let fixture = Fixture::new("liveness-gone");
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert_eq!(session_liveness(&scan, CONV_SRC), SessionLiveness::Gone);
    }

    /// `Absent`（历史目录不存在）即使目录「查不到」也没有判定依据 → Unknown。
    #[test]
    fn absent_history_is_unknown_not_gone() {
        let fixture = Fixture::new("liveness-absent");

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, "uid-never-used");

        assert_eq!(
            session_liveness(&scan, CONV_SRC),
            SessionLiveness::Unknown(ConversationIndexState::Absent)
        );
    }

    /// 判定器按账号缓存扫描结果：同一请求内不重复扫盘。
    #[test]
    fn liveness_judge_caches_each_account_scan() {
        let fixture = Fixture::new("liveness-cache");
        // 造数：索引可读但空，目录还在 → Alive（靠目录证据）。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        std::fs::create_dir_all(fixture.src_conv_dir()).unwrap();
        let mut judge = LivenessJudge::new(VSCODE_STORE, &fixture.root);
        assert_eq!(judge.liveness(SRC_UID, CONV_SRC), SessionLiveness::Alive);

        // 扫描结果已缓存：删掉目录也不会改变本请求内的结论（不重复扫盘）。
        std::fs::remove_dir_all(fixture.src_conv_dir()).unwrap();
        assert_eq!(judge.liveness(SRC_UID, CONV_SRC), SessionLiveness::Alive);

        // 新的判定器才看到磁盘现状：索引可用、条目与目录都没有 → Gone。
        let mut fresh = LivenessJudge::new(VSCODE_STORE, &fixture.root);
        assert_eq!(fresh.liveness(SRC_UID, CONV_SRC), SessionLiveness::Gone);
    }

    /// 预览：索引丢了条目但目录还在的组**不得**被过滤（旧实现必挂的回归点）。
    #[test]
    fn preview_keeps_a_group_proven_alive_by_its_session_directory() {
        let fixture = Fixture::new("preview-dir-evidence");
        seed_source(&fixture);
        copy_and_register(&fixture);
        // 两侧会话目录都还在，索引条目都被清空 → 旧实现判 Gone 并隐藏整组。
        for uid in [SRC_UID, DST_UID] {
            write(
                &workspace_index(&fixture, uid, WS),
                &json!({ "conversations": [] }).to_string(),
            );
            assert_eq!(
                index_state(&fixture, uid),
                ConversationIndexState::Available(BTreeSet::new())
            );
        }

        let report =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        let groups = report["groups"].as_array().unwrap();
        assert_eq!(
            groups.len(),
            1,
            "索引条目缺失但会话目录还在时必须保留该组：{report}"
        );
        // 标题退化在预期内（索引里没有名字），组本身不能消失。
        assert_eq!(groups[0]["title"], json!("(无标题)"), "{report}");
    }
    /// 空工作区目录（只有目录、没有 `index.json`）不会让状态变成 `Partial`，
    /// 也不会盖掉「会话目录还在」这条更强证据。
    ///
    /// 旧实现把工作区子目录数当分母 → 「有一个没索引的工作区」就永远 `Partial` → 该客户端
    /// 的清理被永久拒绝。放宽分母之后，索引条目缺失的会话仍然靠目录证据判 `Alive`。
    #[test]
    fn an_empty_workspace_directory_keeps_the_session_directory_evidence() {
        let fixture = Fixture::new("liveness-empty-workspace");
        // WS_A：索引可读但条目已丢（半截写入现场）；会话目录在 WS_A 下。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        std::fs::create_dir_all(fixture.src_conv_dir()).unwrap();
        // WS_B：只有目录，客户端还没写入 index.json。
        std::fs::create_dir_all(
            vscode_session::history_root(&fixture.root, SRC_UID)
                .join("ffffffffffffffffffffffffffffffff"),
        )
        .unwrap();

        // 分母只数「含 index.json 的工作区」：WS_B 不计入，所以这里是 Available 而不是 Partial。
        assert_eq!(
            index_state(&fixture, SRC_UID),
            ConversationIndexState::Available(BTreeSet::new())
        );

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert_eq!(
            session_liveness(&scan, CONV_SRC),
            SessionLiveness::Alive,
            "目录证据必须兜底：{:?}",
            scan.session_dirs
        );
    }

    /// 会话目录是**符号链接**（Windows 上是 junction）时同样是存活证据。
    ///
    /// 漏掉链接会让证据凭空消失：索引同时丢了条目时就会误判 `Gone`（预览隐藏、清理删组）。
    /// 当前环境建不了链接（Windows 需要开发者模式 / 管理员权限）时**跳过并打印原因**，
    /// 不假装通过。
    #[test]
    fn a_symlinked_session_directory_is_still_evidence() {
        let fixture = Fixture::new("liveness-symlink");
        // 索引可读但条目已丢 → 只剩目录证据可用。
        write(
            &workspace_index(&fixture, SRC_UID, WS),
            &json!({ "conversations": [] }).to_string(),
        );
        // 真实目录放在 history 之外，工作区里放一个指向它的链接。
        let real = fixture.base.join("real-session-dir");
        std::fs::create_dir_all(&real).unwrap();
        let link =
            vscode_session::history_root(&fixture.root, SRC_UID).join(WS).join(CONV_SRC);
        if let Err(error) = link_dir(&real, &link) {
            eprintln!(
                "跳过 a_symlinked_session_directory_is_still_evidence：\
                 本环境无法创建目录符号链接（{error}）。\
                 Windows 需要开发者模式或管理员权限。"
            );
            return;
        }

        let scan = conversation_index_scan(VSCODE_STORE, &fixture.root, SRC_UID);

        assert!(
            scan.session_dirs.contains(CONV_SRC),
            "指向真实目录的链接必须被收集为证据：{:?}",
            scan.session_dirs
        );
        assert_eq!(session_liveness(&scan, CONV_SRC), SessionLiveness::Alive);
    }

    /// 创建目录链接：Windows 先试 `symlink_dir`，没有权限时退回 `mklink /J`（junction，
    /// 不需要开发者模式）；Unix 用 `symlink`。
    fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            // `symlink_dir` 需要开发者模式或管理员权限，普通用户会拿到 os error 1314。
            let first = std::os::windows::fs::symlink_dir(target, link);
            if first.is_ok() {
                return first;
            }
            let output = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()?;
            if output.status.success() {
                return Ok(());
            }
            // 两条路都不行：把第一次的错误交回去（测试据此优雅跳过）。
            first
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (target, link);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "该平台没有实现目录链接",
            ))
        }
    }
