// ---------------------------------------------------------------------------
// 复制前的内容级去重（issue #22「同步时相同会话没有跳过」）
//
// 场景：目标账号里已经有一条**标题 / 工作目录 / 正文都相同但从未建立关联**的会话
// （用户以前手动同步、或用历史版本重复复制留下的多副本）。关联表里查不到它，早期实现
// 会再写一份，于是每操作一次多一条同名会话。命中后必须「只登记、不复制」。
// ---------------------------------------------------------------------------

/// 目标账号已有同内容的未关联会话 → 不新增行，返回已关联，并把双方都登记进关联组。
#[test]
fn copy_reuses_unlinked_same_content_session_in_target() {
    let env = ready_env("same-content");
    // 目标账号里先放一份「同标题 + 同 cwd + 同正文」但没有任何关联记录的副本：
    // 正文里的 sessionId 是它自己的 id（真实副本就是这样），因此原始字节摘要与源不同。
    env.add_session("sess-dup", "uid-b", "标题一");
    env.add_body("sess-dup", &body_text("sess-dup"));

    let report = copy(&env, "uid-b", &["sess-1"]);

    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    assert_eq!(
        report["alreadyLinked"].as_array().unwrap().len(),
        1,
        "{report}"
    );
    assert_eq!(report["alreadyLinked"][0]["sessionId"], "sess-dup");
    assert_eq!(report["alreadyLinked"][0]["id"], "sess-1");
    assert!(
        report
            .get("errors")
            .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
        "{report}"
    );

    // 不新增正文、不新增行：目标账号仍然只有那一份。
    assert_eq!(
        env.body_files().len(),
        2,
        "命中同内容会话时不得写出第二份副本"
    );
    assert_eq!(env.rows_for("uid-b"), vec!["sess-dup".to_string()]);
    assert_eq!(env.rows_for("uid-a"), vec!["sess-1".to_string()]);

    // 关联组：本次新建一组（原本没有组），source / target 都是 active 成员并带配对基线。
    let store = env.store();
    assert_eq!(store.groups.len(), 1, "{store:?}");
    let group = &store.groups[0];
    assert_eq!(group.members.len(), 2);
    assert!(group
        .members
        .iter()
        .all(|member| member.state == MemberState::Active));
    assert!(group
        .members
        .iter()
        .any(|member| member.uid == "uid-a" && member.session_id == "sess-1"));
    assert!(group
        .members
        .iter()
        .any(|member| member.uid == "uid-b" && member.session_id == "sess-dup"));
    assert_eq!(
        group.pair_bases.len(),
        1,
        "必须留下共同基线，之后的同步才可比"
    );

    // 再操作一次：这一次走「组内已有有效成员」的常规幂等分支，仍然不新增副本。
    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["copied"].as_array().unwrap().len(), 0, "{again}");
    assert_eq!(again["alreadyLinked"][0]["sessionId"], "sess-dup");
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.store().groups.len(), 1, "不得再建第二个组");
}

/// 标题相同但正文不同 / 正文相同但标题不同：都不算同一条，必须正常复制一份新的。
#[test]
fn copy_does_not_reuse_session_with_different_title_or_content() {
    // 标题相同、正文不同。
    let env = ready_env("same-title-diff-body");
    env.add_session("sess-other", "uid-b", "标题一");
    env.add_body(
        "sess-other",
        &format!(
            "{}\n",
            json!({"type": "user", "sessionId": "sess-other", "text": "完全不同的开场"})
        ),
    );
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert_eq!(env.body_files().len(), 3);

    // 正文相同（只差 sessionId）、标题不同。
    let env = ready_env("same-body-diff-title");
    env.add_session("sess-other", "uid-b", "标题二");
    env.add_body("sess-other", &body_text("sess-other"));
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert_eq!(env.body_files().len(), 3);
}

/// 候选已在**别的**关联组里：关联表的身份唯一性不允许一个会话属于两个组，因此跳过它
/// 正常复制（宁可多一份，也不破坏关联表一致性）。
#[test]
fn copy_skips_same_content_session_already_in_another_group() {
    let env = ready_env("same-content-occupied");
    env.add_session("sess-dup", "uid-b", "标题一");
    env.add_body("sess-dup", &body_text("sess-dup"));
    // 别的来源（sess-else）已经把这个副本拉进了一个组。
    env.add_session("sess-else", "uid-a", "标题一");
    env.add_body("sess-else", &body_text("sess-else"));
    session_link::with_link_store_write(&env.paths, |store| {
        let mut group = LinkGroup {
            id: "group-other".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: Vec::new(),
            pair_bases: Vec::new(),
        };
        for (uid, session_id) in [("uid-a", "sess-else"), ("uid-b", "sess-dup")] {
            group.members.push(LinkMember {
                member_id: uuid::Uuid::new_v4().to_string(),
                account_id: None,
                uid: uid.to_string(),
                session_id: session_id.to_string(),
                variant: Some(WbVariant::Cn),
                state: MemberState::Active,
                linked_at: 1,
                last_synced_at: None,
            });
        }
        store.groups.push(group);
        Ok(())
    })
    .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(
        report["copied"].as_array().unwrap().len(),
        1,
        "已属其它组的同内容会话不能被拉进本组：{report}"
    );
    let new_id = env.first_copy_id(&report);
    assert_ne!(new_id, "sess-dup");
    // 原组未被改动：sess-dup 仍在 group-other 里，且没有被抢到新组。
    let store = env.store();
    let original = store
        .groups
        .iter()
        .find(|group| group.id == "group-other")
        .expect("原组保留");
    assert!(original
        .members
        .iter()
        .any(|member| member.session_id == "sess-dup"));
    let created = store
        .groups
        .iter()
        .find(|group| group.id != "group-other")
        .expect("新建了复制组");
    assert!(!created
        .members
        .iter()
        .any(|member| member.session_id == "sess-dup"));
}
