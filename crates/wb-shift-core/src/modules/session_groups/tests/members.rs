    #[test]
    fn removing_a_member_drops_its_pair_bases_and_keeps_the_rest() {
        let store = TempStore::new("remove-member");
        let mut group = LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![
                stored_member("m-a", "uid-a", MemberState::Active),
                stored_member("m-b", "uid-b", MemberState::Active),
                stored_member("m-c", "uid-c", MemberState::Stale),
            ],
            pair_bases: Vec::new(),
        };
        for (left, right, ref_name) in [
            ("m-a", "m-b", "base-ab"),
            ("m-a", "m-c", "base-ac"),
            ("m-b", "m-c", "base-bc"),
        ] {
            let (first, second) = session_link::pair_key(left, right);
            group
                .pair_bases
                .push(crate::modules::session_link::PairBase {
                    member_ids: [first, second],
                    baseline_ref: ref_name.to_string(),
                    normalization_version: 1,
                });
        }
        store.seed(vec![group]);

        let result = remove_member_at(&store.paths, "g-1", "m-a");

        assert_eq!(result["status"], "removed");
        assert_eq!(result["remaining"], 2);
        let saved = store.store();
        assert_eq!(saved.groups.len(), 1);
        let members = &saved.groups[0].members;
        assert_eq!(members.len(), 2);
        assert!(members.iter().all(|member| member.member_id != "m-a"));
        let refs = saved.groups[0]
            .pair_bases
            .iter()
            .map(|pair| pair.baseline_ref.as_str())
            .collect::<Vec<_>>();
        assert_eq!(refs, vec!["base-bc"]);
        assert_eq!(saved.revision, 2);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn removing_a_superseded_member_keeps_the_group() {
        // 共同旧版 / 已替代成员同样可移除：组保留，remaining 是剩余人数，引用它的配对基线被清掉。
        let store = TempStore::new("remove-superseded");
        let mut group = LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![
                stored_member("m-a", "uid-a", MemberState::Active),
                stored_member("m-b", "uid-b", MemberState::Active),
                stored_member("m-old", "uid-old", MemberState::Superseded),
            ],
            pair_bases: Vec::new(),
        };
        for (left, right, ref_name) in [
            ("m-a", "m-b", "base-ab"),
            ("m-a", "m-old", "base-a-old"),
            ("m-b", "m-old", "base-b-old"),
        ] {
            let (first, second) = session_link::pair_key(left, right);
            group
                .pair_bases
                .push(crate::modules::session_link::PairBase {
                    member_ids: [first, second],
                    baseline_ref: ref_name.to_string(),
                    normalization_version: 1,
                });
        }
        store.seed(vec![group]);

        let result = remove_member_at(&store.paths, "g-1", "m-old");

        assert_eq!(result["status"], "removed");
        assert_eq!(result["remaining"], 2);
        let saved = store.store();
        assert_eq!(saved.groups.len(), 1);
        assert_eq!(saved.groups[0].id, "g-1");
        assert!(saved.groups[0]
            .members
            .iter()
            .all(|member| member.member_id != "m-old"));
        let refs = saved.groups[0]
            .pair_bases
            .iter()
            .map(|pair| pair.baseline_ref.as_str())
            .collect::<Vec<_>>();
        assert_eq!(refs, vec!["base-ab"]);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn removing_the_last_member_deletes_the_group() {
        let store = TempStore::new("remove-last");
        store.seed(vec![
            LinkGroup {
                id: "g-1".to_string(),
                variant: WbVariant::Cn,
                created_at: 1,
                members: vec![stored_member("m-a", "uid-a", MemberState::Active)],
                pair_bases: Vec::new(),
            },
            LinkGroup {
                id: "g-2".to_string(),
                variant: WbVariant::Cn,
                created_at: 1,
                members: vec![stored_member("m-z", "uid-z", MemberState::Active)],
                pair_bases: Vec::new(),
            },
        ]);

        let result = remove_member_at(&store.paths, "g-1", "m-a");

        assert_eq!(result["status"], "groupRemoved");
        assert_eq!(result["remaining"], 0);
        let saved = store.store();
        assert_eq!(saved.groups.len(), 1);
        assert_eq!(saved.groups[0].id, "g-2");
        assert_eq!(saved.revision, 2);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn removing_an_unknown_group_or_member_reports_a_readable_error() {
        let store = TempStore::new("remove-unknown");
        store.seed(vec![LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![stored_member("m-a", "uid-a", MemberState::Active)],
            pair_bases: Vec::new(),
        }]);

        let missing_group = session_link::with_link_store_write(&store.paths, |link_store| {
            super::remove_member_in(link_store, SessionClient::Workbuddy, None, "missing", "m-a")
        })
        .unwrap_err();
        assert_eq!(missing_group, "会话关联组不存在或不属于当前客户端");

        let missing_member = session_link::with_link_store_write(&store.paths, |link_store| {
            super::remove_member_in(link_store, SessionClient::Workbuddy, None, "g-1", "nope")
        })
        .unwrap_err();
        assert_eq!(missing_member, "成员不存在，请刷新后重试");

        // 失败的读改写不得落盘，更不得推进 revision。
        let saved = store.store();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.groups[0].members.len(), 1);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn deleting_a_group_drops_all_members_and_keeps_the_others() {
        let store = TempStore::new("delete-group");
        let mut group = LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![
                stored_member("m-a", "uid-a", MemberState::Active),
                stored_member("m-b", "uid-b", MemberState::Stale),
            ],
            pair_bases: Vec::new(),
        };
        let (first, second) = session_link::pair_key("m-a", "m-b");
        group
            .pair_bases
            .push(crate::modules::session_link::PairBase {
                member_ids: [first, second],
                baseline_ref: "base-ab".to_string(),
                normalization_version: 1,
            });
        store.seed(vec![
            group,
            LinkGroup {
                id: "g-2".to_string(),
                variant: WbVariant::Cn,
                created_at: 1,
                members: vec![stored_member("m-z", "uid-z", MemberState::Active)],
                pair_bases: Vec::new(),
            },
        ]);

        let removed = delete_group_at(&store.paths, "g-1").unwrap();

        assert_eq!(removed, 2);
        let saved = store.store();
        assert_eq!(saved.groups.len(), 1);
        assert_eq!(saved.groups[0].id, "g-2");
        assert_eq!(saved.groups[0].members.len(), 1);
        assert_eq!(saved.revision, 2);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn deleting_an_unknown_group_reports_a_readable_error_without_writing() {
        let store = TempStore::new("delete-unknown");
        store.seed(vec![LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![stored_member("m-a", "uid-a", MemberState::Active)],
            pair_bases: Vec::new(),
        }]);

        let error = delete_group_at(&store.paths, "missing").unwrap_err();

        assert_eq!(error, "会话关联组不存在或不属于当前客户端");
        // 失败的读改写不得落盘，更不得推进 revision。
        let saved = store.store();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.groups.len(), 1);
        assert!(session_link::validate_store(&saved).is_ok());
    }

    #[test]
    fn delete_group_rejects_invalid_scope_combinations() {
        let vscode =
            delete_group(SessionClient::VscodeExt, Some(WbVariant::Ai), "g-1").unwrap_err();
        assert!(vscode.contains("只有 CodeBuddy IDE"));
        let workbuddy =
            delete_group(SessionClient::Workbuddy, Some(WbVariant::Cn), "g-1").unwrap_err();
        assert!(workbuddy.contains("只有 CodeBuddy IDE"));
        let ide = delete_group(SessionClient::CodebuddyIde, None, "g-1").unwrap_err();
        assert!(ide.contains("必须指定 cn 或 ai"));
    }

    #[test]
    fn unlink_rejects_invalid_scope_combinations() {
        let vscode =
            remove_member(SessionClient::VscodeExt, Some(WbVariant::Ai), "g-1", "m-a").unwrap_err();
        assert!(vscode.contains("只有 CodeBuddy IDE"));
        let workbuddy =
            remove_member(SessionClient::Workbuddy, Some(WbVariant::Cn), "g-1", "m-a").unwrap_err();
        assert!(workbuddy.contains("只有 CodeBuddy IDE"));
        let ide = remove_member(SessionClient::CodebuddyIde, None, "g-1", "m-a").unwrap_err();
        assert!(ide.contains("必须指定 cn 或 ai"));
    }
