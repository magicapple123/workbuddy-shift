// -----------------------------------------------------------------------
// 预览凭据（design §6）
// -----------------------------------------------------------------------

fn sample_binding(group: &LinkGroup) -> PreviewBinding {
    PreviewBinding {
        variant: group.variant,
        group_id: group.id.clone(),
        group_fingerprint: group_fingerprint(group),
        source: PreviewMemberBinding {
            member_id: "m-a".to_string(),
            account_id: None,
            uid: "uid-a".to_string(),
            session_id: "sess-1".to_string(),
            raw_digest: "raw-a".to_string(),
            normalized_digest: "norm-a".to_string(),
            record_count: 3,
        },
        target: PreviewMemberBinding {
            member_id: "m-b".to_string(),
            account_id: None,
            uid: "uid-b".to_string(),
            session_id: "sess-b".to_string(),
            raw_digest: "raw-b".to_string(),
            normalized_digest: "norm-b".to_string(),
            record_count: 2,
        },
        baseline_ref: Some("base-ab".to_string()),
        baseline_total_digest: Some("base-digest".to_string()),
        baseline_record_count: Some(2),
        verdict: SyncVerdict::FastForward,
        source_status: None,
        target_status: None,
        archive_action: None,
    }
}

/// 凭据读写：只有本模块生成的 UUID 能命中，伪造/穿越形状的 id 一律读不到。
#[test]
fn preview_token_round_trips_and_rejects_foreign_ids() {
    let dir = TempDir::new("preview-token");
    let paths = temp_paths(&dir);
    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);

    let id = save_preview_token(&paths, binding.clone()).unwrap();
    let loaded = load_preview_token(&paths, &id).expect("刚保存的凭据必须能读回");
    assert_eq!(loaded.binding, binding);
    assert_eq!(loaded.version, PREVIEW_TOKEN_VERSION);

    // 未知 id / 路径穿越形状 / 空值一律视为过期，且不读取任何文件。
    for bad in [
        "11111111-2222-3333-4444-555555555555",
        "../../../../etc/passwd",
        "sess-1.json",
        "",
        "   ",
    ] {
        assert!(load_preview_token(&paths, bad).is_none(), "非法 id：{bad}");
    }
    assert!(!valid_preview_id("../../etc/passwd"));

    // 版本不符视为过期。
    let mut version_bumped = loaded.clone();
    version_bumped.version = PREVIEW_TOKEN_VERSION + 1;
    std::fs::write(
        paths.preview_tokens_dir().join(format!("{id}.json")),
        serde_json::to_string(&version_bumped).unwrap(),
    )
    .unwrap();
    assert!(load_preview_token(&paths, &id).is_none());

    // 清理保留最近 N 条，且不影响其它文件；保存流程自身不会删掉刚写的凭据。
    std::fs::write(paths.preview_tokens_dir().join("keep.txt"), "x").unwrap();
    for _ in 0..4 {
        let saved = save_preview_token(&paths, binding.clone()).unwrap();
        assert!(
            load_preview_token(&paths, &saved).is_some(),
            "刚保存的凭据必须立即可用"
        );
    }
    assert!(prune_preview_tokens(&paths, 2) >= 3);
    assert!(paths.preview_tokens_dir().join("keep.txt").exists());
}

/// 目录里的 `.json` 条目数（凭据文件数量）。
fn json_entry_count(paths: &SessionPaths) -> usize {
    std::fs::read_dir(paths.preview_tokens_dir())
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// issue #19 回归：一次大批量预览在保存过程中不能把自己的凭据挤掉。
///
/// 跨组批量同步会先为**所有**目标调用预览、保存凭据，再逐条执行。原来的实现每保存一条就
/// 裁剪到 `KEEP_PREVIEW_TOKENS - 1` 条，于是第 200 条保存时第 1 条已被删掉，执行到最早的目标
/// 就报「预览凭据已过期」并 `break`，整批只做完一部分。
#[test]
fn bulk_preview_tokens_survive_their_own_prune() {
    let dir = TempDir::new("preview-bulk");
    let paths = temp_paths(&dir);
    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);

    // 比单档保留条数多 50 条（当前为 250）：足以越过旧的裁剪水位。
    let total = KEEP_PREVIEW_TOKENS + 50;
    let mut ids = Vec::with_capacity(total);
    for _ in 0..total {
        ids.push(save_preview_token(&paths, binding.clone()).unwrap());
    }

    assert_eq!(json_entry_count(&paths), total, "未达高水位时一条凭据都不该被删");
    // 第 1 条正是旧实现最先删掉的那条；最后一条用来确认「刚保存的凭据立即可用」。
    for (index, id) in ids.iter().enumerate() {
        assert!(
            load_preview_token(&paths, id).is_some(),
            "第 {} 条凭据在保存过程中被自己清理掉了",
            index + 1
        );
    }
}

/// 高水位清理：未达阈值一条都不删，达到阈值才裁到 `keep` 条。
#[test]
fn prune_over_threshold_only_prunes_at_high_water_mark() {
    let dir = TempDir::new("preview-watermark");
    let paths = temp_paths(&dir);
    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);

    for _ in 0..4 {
        save_preview_token(&paths, binding.clone()).unwrap();
    }
    // 4 < 5：不做任何删除（批量预览保存到一半时走的正是这条路径）。
    assert_eq!(prune_preview_tokens_over(&paths, 2, 5), 0);
    assert_eq!(json_entry_count(&paths), 4);

    // 补到 5 条：达到阈值，清理后保留最近 2 条。
    save_preview_token(&paths, binding.clone()).unwrap();
    assert_eq!(prune_preview_tokens_over(&paths, 2, 5), 3);
    assert_eq!(json_entry_count(&paths), 2);
}

/// 未达阈值时不碰任何文件（`.json` 之外的条目同样不受影响）。
#[test]
fn prune_over_threshold_leaves_other_files_alone_below_threshold() {
    let dir = TempDir::new("preview-watermark-keep");
    let paths = temp_paths(&dir);
    let tokens_dir = paths.preview_tokens_dir();
    std::fs::create_dir_all(&tokens_dir).unwrap();
    std::fs::write(tokens_dir.join("keep.txt"), "x").unwrap();
    // 扩展名不是 `json`：计数与清理都不该把它算成凭据。
    std::fs::write(tokens_dir.join("notes.json.bak"), "y").unwrap();

    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);
    for _ in 0..2 {
        save_preview_token(&paths, binding.clone()).unwrap();
    }

    // 计数只算 `.json`（2 条），因此未达阈值。
    assert_eq!(json_entry_count(&paths), 2);
    assert_eq!(prune_preview_tokens_over(&paths, 1, 5), 0);
    assert_eq!(json_entry_count(&paths), 2);
    assert!(tokens_dir.join("keep.txt").exists());
    assert!(tokens_dir.join("notes.json.bak").exists());
}

/// 阈值语义的边界：目录不存在、`threshold = 0`、`keep >= count` 都必须返回 0、
/// 不 panic、不删错文件。
#[test]
fn prune_over_threshold_handles_missing_dir_and_zero_bounds() {
    let dir = TempDir::new("preview-watermark-edge");
    let paths = temp_paths(&dir);
    let tokens_dir = paths.preview_tokens_dir();

    // 目录不存在（`prune_preview_tokens_over` 是公开函数，不能假设调用方先建目录）。
    assert!(!tokens_dir.exists());
    assert_eq!(prune_preview_tokens_over(&paths, 2, 5), 0);
    assert!(!tokens_dir.exists(), "只读检查不该顺手建目录");

    // `threshold = 0`：比较条件恒成立，但空目录没有可删的条目。
    std::fs::create_dir_all(&tokens_dir).unwrap();
    assert_eq!(prune_preview_tokens_over(&paths, 0, 0), 0);

    // `keep >= count`：条数没超过保留数，一条都不删。
    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);
    for _ in 0..3 {
        save_preview_token(&paths, binding.clone()).unwrap();
    }
    assert_eq!(prune_preview_tokens_over(&paths, 3, 0), 0);
    assert_eq!(json_entry_count(&paths), 3);

    // 高水位必须明显高于单档保留数：一次批量超过 `keep` 时，只有把清理推迟到高水位
    // 才能保证本批凭据不被自己删掉（issue #19 的修复前提）。
    // 用 const 块写成编译期断言：阈值被调小到 `keep` 附近会直接编译失败。
    const {
        assert!(
            PREVIEW_TOKEN_PRUNE_THRESHOLD > KEEP_PREVIEW_TOKENS,
            "预览凭据高水位必须高于单档保留数"
        )
    };
}

/// 逐项核对：任一绑定字段变化都必须报出可读原因。
#[test]
fn verify_preview_reports_every_binding_mismatch() {
    let group = group_with_members("g-1", vec![]);
    let binding = sample_binding(&group);
    let token = PreviewToken {
        version: PREVIEW_TOKEN_VERSION,
        preview_id: "p-1".to_string(),
        created_at: 1,
        binding: binding.clone(),
    };
    assert!(verify_preview(&token, &binding).is_empty());

    let cases: [(&str, PreviewBinding); 7] = [
        (
            "当前应用",
            PreviewBinding {
                variant: WbVariant::Ai,
                ..binding.clone()
            },
        ),
        (
            "关联关系",
            PreviewBinding {
                group_id: "g-2".to_string(),
                ..binding.clone()
            },
        ),
        (
            "关联关系或同步记录",
            PreviewBinding {
                group_fingerprint: "changed".to_string(),
                ..binding.clone()
            },
        ),
        (
            "来源账号",
            PreviewBinding {
                source: PreviewMemberBinding {
                    raw_digest: "changed".to_string(),
                    ..binding.source.clone()
                },
                ..binding.clone()
            },
        ),
        (
            "目标账号",
            PreviewBinding {
                target: PreviewMemberBinding {
                    session_id: "sess-b2".to_string(),
                    ..binding.target.clone()
                },
                ..binding.clone()
            },
        ),
        (
            "上次同步的记录",
            PreviewBinding {
                baseline_ref: Some("base-other".to_string()),
                ..binding.clone()
            },
        ),
        (
            "检查结果",
            PreviewBinding {
                verdict: SyncVerdict::Diverge,
                ..binding.clone()
            },
        ),
    ];
    for (name, mutated) in cases {
        let stale = verify_preview(&token, &mutated);
        assert!(!stale.is_empty(), "{name} 不一致必须报过期");
        assert!(
            stale.iter().any(|reason| reason.contains(name)),
            "{name} 的原因文案缺失：{stale:?}"
        );
    }

    // 基线内容变化（引用不变）同样必须被报出。
    let drifted = PreviewBinding {
        baseline_total_digest: Some("other".to_string()),
        ..binding.clone()
    };
    let stale = verify_preview(&token, &drifted);
    assert!(
        stale.iter().any(|reason| reason.contains("上次同步的内容")),
        "{stale:?}"
    );

    // 凭据版本不符直接报过期。
    let mut old = token.clone();
    old.version = PREVIEW_TOKEN_VERSION + 1;
    assert!(!verify_preview(&old, &binding).is_empty());
}

/// 配对基线状态解析：无引用 → Missing，引用读不出来 → Unverifiable。
#[test]
fn pair_baseline_state_distinguishes_missing_from_unverifiable() {
    let dir = TempDir::new("pair-baseline-state");
    let paths = temp_paths(&dir);
    let text = sample_body("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let normalized = normalized_from(&text);

    let mut group = group_with_members(
        "g-1",
        vec![
            member("uid-a", "sess-1", MemberState::Active),
            member("uid-b", "sess-b", MemberState::Active),
        ],
    );
    assert_eq!(
        load_pair_baseline(&paths, &group, "m-uid-a-sess-1", "m-uid-b-sess-b"),
        BaselineState::Missing
    );

    save_baseline(&paths, "base-ab", &normalized).unwrap();
    set_pair_base(&mut group, "m-uid-a-sess-1", "m-uid-b-sess-b", "base-ab", 1);
    let ready = load_pair_baseline(&paths, &group, "m-uid-b-sess-b", "m-uid-a-sess-1");
    assert_eq!(ready.ready().unwrap().record_count, normalized.record_count);

    // 归一化版本不符 → 不可验证，不得当成没有基线而「重新建立」。
    set_pair_base(
        &mut group,
        "m-uid-a-sess-1",
        "m-uid-b-sess-b",
        "base-ab",
        NORMALIZATION_VERSION + 1,
    );
    match load_pair_baseline(&paths, &group, "m-uid-a-sess-1", "m-uid-b-sess-b") {
        BaselineState::Unverifiable(reason) => {
            assert!(reason.contains("记录格式版本"), "{reason}")
        }
        other => panic!("期望 Unverifiable，实际 {other:?}"),
    }

    // 引用在但文件损坏 → 不可验证。
    set_pair_base(&mut group, "m-uid-a-sess-1", "m-uid-b-sess-b", "base-ab", 1);
    std::fs::write(paths.baselines_dir().join("base-ab.json"), "not-json").unwrap();
    assert!(matches!(
        load_pair_baseline(&paths, &group, "m-uid-a-sess-1", "m-uid-b-sess-b"),
        BaselineState::Unverifiable(_)
    ));
}
