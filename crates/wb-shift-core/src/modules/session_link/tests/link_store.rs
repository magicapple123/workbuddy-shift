// -----------------------------------------------------------------------
// 关联存储
// -----------------------------------------------------------------------

#[test]
fn store_initializes_missing_then_round_trips_with_revision() {
    let dir = TempDir::new("store-round-trip");
    let paths = temp_paths(&dir);
    assert!(matches!(load_store(&paths), StoreState::Missing));

    let revision = with_link_store_write(&paths, |store| {
        store.groups.push(group_with_members(
            "g-1",
            vec![member("uid-a", "sess-1", MemberState::Active)],
        ));
        Ok(store.revision)
    })
    .unwrap();
    assert_eq!(revision, 0, "首次写入前 revision 为 0");

    match load_store(&paths) {
        StoreState::Ready(store) => {
            assert_eq!(store.version, LINK_STORE_VERSION);
            assert_eq!(store.revision, 1);
            assert_eq!(store.groups.len(), 1);
        }
        other => panic!("期望 Ready，实际 {other:?}"),
    }
}

#[test]
fn store_corrupt_or_unknown_version_is_unavailable_and_preserved() {
    let dir = TempDir::new("store-corrupt");
    let paths = temp_paths(&dir);
    std::fs::create_dir_all(&paths.store_root).unwrap();

    for (name, content) in [
        ("corrupt", "not-json".to_string()),
        (
            "unknown-version",
            r#"{"version":99,"revision":1,"groups":[]}"#.to_string(),
        ),
    ] {
        std::fs::write(paths.session_links_file(), &content).unwrap();
        assert!(
            matches!(load_store(&paths), StoreState::Unavailable(_)),
            "{name} 必须判为不可用"
        );
        let write = with_link_store_write(&paths, |store| {
            store.groups.push(group_with_members("g-x", vec![]));
            Ok(())
        });
        assert!(write.is_err(), "{name} 时必须禁写");
        assert_eq!(
            std::fs::read_to_string(paths.session_links_file()).unwrap(),
            content,
            "{name}：必须保留现场，不得覆盖"
        );
    }
}

/// 权限错误（读不了主文件）→ 不可用且禁写。
#[cfg(unix)]
#[test]
fn store_permission_denied_is_unavailable() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("store-perm");
    let paths = temp_paths(&dir);
    std::fs::create_dir_all(&paths.store_root).unwrap();
    std::fs::write(paths.session_links_file(), json_store(vec![])).unwrap();
    std::fs::set_permissions(
        paths.session_links_file(),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    if std::fs::read_to_string(paths.session_links_file()).is_ok() {
        // root / 特殊 ACL 环境读得到，跳过（不误报）。
        std::fs::set_permissions(
            paths.session_links_file(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        return;
    }
    assert!(matches!(load_store(&paths), StoreState::Unavailable(_)));
    assert!(with_link_store_write(&paths, |_| Ok(())).is_err());
    std::fs::set_permissions(
        paths.session_links_file(),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
}

/// 主文件缺失但残留未完成操作 → 不可用（不得当成空表初始化）。
#[test]
fn store_missing_file_with_unfinished_operation_is_unavailable() {
    let dir = TempDir::new("store-residual");
    let paths = temp_paths(&dir);
    save_operation(
        &paths,
        &operation("op-1", OpPhase::BodyWritten, "uid-b", "sess-b"),
    )
    .unwrap();
    assert!(matches!(load_store(&paths), StoreState::Unavailable(_)));

    // 已完成的痕迹不算残留：可以按首次初始化。
    save_operation(
        &paths,
        &operation("op-2", OpPhase::Completed, "uid-b", "sess-b"),
    )
    .unwrap();
    std::fs::remove_file(paths.operations_dir().join("op-1.json")).unwrap();
    assert!(matches!(load_store(&paths), StoreState::Missing));
}

/// 主文件缺失但残留基线文件 → 不可用（不得当成空表初始化并覆盖现场）。
#[test]
fn store_missing_file_with_orphan_baseline_is_unavailable() {
    let dir = TempDir::new("store-baseline-residual");
    let paths = temp_paths(&dir);
    let normalized = normalize_jsonl("{\"a\":1}\n", "sess").unwrap();
    save_baseline(&paths, "base-orphan", &normalized).unwrap();

    assert!(matches!(load_store(&paths), StoreState::Unavailable(_)));
    assert!(with_link_store_write(&paths, |_| Ok(())).is_err());
    assert!(!paths.session_links_file().exists(), "不得重建空表覆盖现场");
    assert!(
        paths.baselines_dir().join("base-orphan.json").exists(),
        "必须保留基线现场"
    );
}

#[test]
fn store_rejects_invariant_violations() {
    let dir = TempDir::new("store-invariant");
    let paths = temp_paths(&dir);
    std::fs::create_dir_all(&paths.store_root).unwrap();

    // 同账号两个 active 成员。
    let two_active = json_store(vec![group_with_members(
        "g-1",
        vec![
            member("uid-b", "sess-b1", MemberState::Active),
            member("uid-b", "sess-b2", MemberState::Active),
        ],
    )]);
    std::fs::write(paths.session_links_file(), two_active).unwrap();
    assert!(matches!(load_store(&paths), StoreState::Unavailable(_)));

    // 同一会话身份出现在两个组。
    let duplicated = json_store(vec![
        group_with_members("g-1", vec![member("uid-a", "sess-1", MemberState::Active)]),
        group_with_members("g-2", vec![member("uid-a", "sess-1", MemberState::Active)]),
    ]);
    std::fs::write(paths.session_links_file(), duplicated).unwrap();
    assert!(matches!(load_store(&paths), StoreState::Unavailable(_)));
}

fn json_store(groups: Vec<LinkGroup>) -> String {
    let store = LinkStore {
        version: LINK_STORE_VERSION,
        revision: 1,
        groups,
    };
    serde_json::to_string_pretty(&store).unwrap()
}

/// 旧存储（成员没有 `variant` 字段）载入时用组级档位回填，行为与改造前一致。
#[test]
fn load_backfills_member_variant_from_group() {
    let dir = TempDir::new("member-variant-backfill");
    let paths = temp_paths(&dir);
    std::fs::create_dir_all(&paths.store_root).unwrap();

    // 手工写入"旧版"存储：AI 档位的组，成员没有 variant 字段。
    let legacy = serde_json::json!({
        "version": LINK_STORE_VERSION,
        "revision": 3,
        "groups": [{
            "id": "g-legacy",
            "variant": "ai",
            "createdAt": 1,
            "members": [{
                "memberId": "m-1",
                "uid": "uid-a",
                "sessionId": "sess-1",
                "state": "active",
                "linkedAt": 1
            }],
            "pairBases": []
        }]
    });
    std::fs::write(paths.session_links_file(), legacy.to_string()).unwrap();

    let StoreState::Ready(store) = load_store(&paths) else {
        panic!("旧存储应当可读（成员档位由组级回填）");
    };
    let group = &store.groups[0];
    assert_eq!(group.members[0].variant, Some(WbVariant::Ai));
    assert_eq!(member_variant(group, &group.members[0]), WbVariant::Ai);
}

/// 跨档组：成员各自保存档位；身份查找与一致性校验按成员档位匹配。
#[test]
fn cross_variant_group_members_keep_own_variant() {
    let mut cn = member("uid-cn", "sess-cn", MemberState::Active);
    cn.variant = Some(WbVariant::Cn);
    let mut ai = member("uid-ai", "sess-ai", MemberState::Active);
    ai.variant = Some(WbVariant::Ai);
    let store = LinkStore {
        version: LINK_STORE_VERSION,
        revision: 1,
        groups: vec![group_with_members("g-cross", vec![cn, ai])],
    };

    validate_store(&store).expect("跨档组应通过一致性校验");
    assert!(find_group_for_identity(&store, WbVariant::Ai, "uid-ai", "sess-ai").is_some());
    assert!(find_group_for_identity(&store, WbVariant::Cn, "uid-cn", "sess-cn").is_some());
    // 身份必须带档位：拿另一档身份查同一成员不命中。
    assert!(find_group_for_identity(&store, WbVariant::Cn, "uid-ai", "sess-ai").is_none());
    assert!(find_group_for_identity(&store, WbVariant::Ai, "uid-cn", "sess-cn").is_none());
}

/// 显式档位随存储往返保留（写回后重新载入仍是成员自己的档位）。
/// 旧版操作记录（没有 `sourceVariant` 字段）可正常载入：一律按同档处理。
#[test]
fn legacy_operation_without_source_variant_deserializes() {
    let legacy = serde_json::json!({
        "version": OPERATION_VERSION,
        "operationId": "op-legacy",
        "kind": "copy",
        "variant": "cn",
        "groupId": "g-1",
        "source": {"uid": "uid-a", "sessionId": "sess-1"},
        "target": {"uid": "uid-b", "sessionId": "sess-b"},
        "expectedContentDigest": "d",
        "expectedRecordCount": 1,
        "phase": "prepared",
        "backup": null,
        "createdAt": 1,
        "updatedAt": 1
    });
    let operation: Operation = serde_json::from_value(legacy).unwrap();
    assert_eq!(operation.source_variant, None);
    assert_eq!(operation.variant, WbVariant::Cn);
}

#[test]
fn explicit_member_variant_round_trips() {
    let dir = TempDir::new("member-variant-roundtrip");
    let paths = temp_paths(&dir);
    let mut ai = member("uid-ai", "sess-ai", MemberState::Active);
    ai.variant = Some(WbVariant::Ai);
    let mut cn = member("uid-cn", "sess-cn", MemberState::Active);
    cn.variant = Some(WbVariant::Cn);
    with_link_store_write(&paths, |store| {
        store
            .groups
            .push(group_with_members("g-cross", vec![ai, cn]));
        Ok(())
    })
    .unwrap();

    let StoreState::Ready(store) = load_store(&paths) else {
        panic!("写回后应可重新载入");
    };
    let members = &store.groups[0].members;
    assert_eq!(members[0].variant, Some(WbVariant::Ai));
    assert_eq!(members[1].variant, Some(WbVariant::Cn));
}

#[test]
fn add_active_member_supersedes_previous_active_only() {
    let mut group =
        group_with_members("g-1", vec![member("uid-b", "sess-b1", MemberState::Active)]);
    add_active_member(&mut group, member("uid-b", "sess-b2", MemberState::Active));
    assert_eq!(
        group
            .members
            .iter()
            .filter(|m| m.uid == "uid-b" && m.state == MemberState::Active)
            .count(),
        1
    );
    assert_eq!(
        group
            .members
            .iter()
            .find(|m| m.session_id == "sess-b1")
            .unwrap()
            .state,
        MemberState::Superseded
    );

    // 不自动复活：显式置回 active 才恢复。
    assert!(set_member_state(
        &mut group,
        "m-uid-b-sess-b1",
        MemberState::Active
    ));
    assert_eq!(
        group
            .members
            .iter()
            .filter(|m| m.state == MemberState::Active)
            .count(),
        2
    );
}

#[test]
fn pair_bases_are_per_member_pair_and_updates_are_directed() {
    let mut group = group_with_members(
        "g-1",
        vec![
            member("uid-a", "sess-1", MemberState::Active),
            member("uid-b", "sess-b", MemberState::Active),
            member("uid-c", "sess-c", MemberState::Active),
        ],
    );
    set_pair_base(&mut group, "m-uid-a-sess-1", "m-uid-b-sess-b", "base-ab", 1);
    set_pair_base(&mut group, "m-uid-a-sess-1", "m-uid-c-sess-c", "base-ac", 1);
    assert_eq!(group.pair_bases.len(), 2);

    // 无序查找：任一方向都能定位同一对。
    assert_eq!(
        find_pair_base(&group, "m-uid-b-sess-b", "m-uid-a-sess-1")
            .unwrap()
            .baseline_ref,
        "base-ab"
    );

    // 定向更新 A/B 不触碰 A/C。
    set_pair_base(
        &mut group,
        "m-uid-a-sess-1",
        "m-uid-b-sess-b",
        "base-ab2",
        1,
    );
    assert_eq!(
        find_pair_base(&group, "m-uid-a-sess-1", "m-uid-b-sess-b")
            .unwrap()
            .baseline_ref,
        "base-ab2"
    );
    assert_eq!(
        find_pair_base(&group, "m-uid-a-sess-1", "m-uid-c-sess-c")
            .unwrap()
            .baseline_ref,
        "base-ac"
    );
    assert_eq!(group.pair_bases.len(), 2);
}

#[test]
fn baseline_inheritance_requires_verified_prefix() {
    let dir = TempDir::new("baseline-inherit");
    let paths = temp_paths(&dir);
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let body = sample_body(own);
    let full = normalize_jsonl(&body, own).unwrap();

    let record = save_baseline(&paths, "base-ab", &full).unwrap();
    assert!(record.is_self_consistent());
    assert_eq!(
        load_baseline(&paths, "base-ab").unwrap().record_count,
        full.record_count
    );

    let pair = PairBase {
        member_ids: ["m-a".to_string(), "m-b".to_string()],
        baseline_ref: "base-ab".to_string(),
        normalization_version: NORMALIZATION_VERSION,
    };
    // 来源内容包含该基线（等价）→ 可继承。
    assert!(inheritable_baseline(&paths, &pair, &full.line_digests).is_some());

    // 来源内容以该基线为前缀（有序追加）→ 可继承。
    let mut extended = full.line_digests.clone();
    extended.push("later".to_string());
    assert!(inheritable_baseline(&paths, &pair, &extended).is_some());

    // 来源内容短于基线 / 顺序不同 → 不可证明，不继承。
    assert!(inheritable_baseline(&paths, &pair, &full.line_digests[..1]).is_none());
    let mut reordered = full.line_digests.clone();
    reordered.reverse();
    if reordered != full.line_digests {
        assert!(inheritable_baseline(&paths, &pair, &reordered).is_none());
    }

    // 基线文件不存在 → 不可验证（Unknown），不继承。
    let missing_pair = PairBase {
        member_ids: ["m-a".to_string(), "m-c".to_string()],
        baseline_ref: "base-missing".to_string(),
        normalization_version: NORMALIZATION_VERSION,
    };
    assert!(inheritable_baseline(&paths, &missing_pair, &full.line_digests).is_none());

    // 基线文件损坏 / 版本不符 → 不可验证。
    std::fs::write(paths.baselines_dir().join("base-ab.json"), "not-json").unwrap();
    assert!(inheritable_baseline(&paths, &pair, &full.line_digests).is_none());
}
