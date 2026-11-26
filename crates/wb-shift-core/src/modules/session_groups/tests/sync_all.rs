    // -----------------------------------------------------------------------
    // issue #231：跨组安全批量同步的「组计划」筛选（纯函数，不读真实存储）
    //
    // `plan_from_detail` 决定一个组该不该进批量、进哪些目标。这里的四条用例是这条判断的
    // 全部出入口：没有安全来源 → 整组跳过；有来源 → 只取 behind 且非来源的成员；
    // 没有 behind → 跳过；标题缺失 → 空串（前端回退显示 groupId）。
    // -----------------------------------------------------------------------

    use super::sync_all::{plan_from_detail, GroupPlan};

    /// 构造 `plan_from_detail` 真正读到的字段子集（`title` / `safeSourceMemberId` / `members`）。
    fn group_detail(source: Option<&str>, members: &[(&str, &str)]) -> Value {
        json!({
            "title": "会话标题",
            "groupId": "g1",
            "safeSourceMemberId": source,
            "members": members
                .iter()
                .map(|(member_id, version_status)| json!({
                    "memberId": member_id,
                    "versionStatus": version_status,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// 无安全来源（分歧组）→ 整组跳过，文案必须告诉用户出路（打开该组手动选）。
    #[test]
    fn plan_skips_group_without_a_safe_source() {
        let detail = group_detail(None, &[("a", "latest"), ("b", "diverge")]);

        match plan_from_detail(&detail) {
            GroupPlan::Skip {
                title,
                reason_code,
                message,
            } => {
                assert_eq!(reason_code, "noSafeSource");
                assert!(
                    message.contains("手动选择保留哪一份"),
                    "文案要给用户出路，实际：{message}"
                );
                assert_eq!(title, "会话标题", "跳过原因要能对上组名");
            }
            other => panic!("无安全来源的组必须整组跳过，实际：{other:?}"),
        }
    }

    /// 有来源 → 只取 behind 且非来源自己的成员（latest / stale / superseded 一律排除）。
    #[test]
    fn plan_selects_only_behind_members_excluding_the_source() {
        let detail = group_detail(
            Some("src"),
            &[
                ("src", "latest"),
                ("a", "behind"),
                ("b", "behind"),
                ("c", "latest"),
                ("d", "stale"),
            ],
        );

        match plan_from_detail(&detail) {
            GroupPlan::Ready {
                title,
                source_member_id,
                targets,
            } => {
                assert_eq!(title, "会话标题");
                assert_eq!(source_member_id, "src", "来源来自载荷，不在这里重算");
                assert_eq!(targets, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("有来源且有落后副本的组必须可执行，实际：{other:?}"),
        }
    }

    /// 有来源但没有 behind 成员 → 跳过（该组已是最新，不该出现在执行清单里）。
    #[test]
    fn plan_skips_group_without_behind_members() {
        let detail = group_detail(Some("src"), &[("src", "latest"), ("a", "latest")]);

        match plan_from_detail(&detail) {
            GroupPlan::Skip { reason_code, .. } => assert_eq!(reason_code, "noBehind"),
            other => panic!("没有落后副本的组必须跳过，实际：{other:?}"),
        }
    }

    /// 详情缺 `title`（老载荷 / 组被删一半）→ 标题为空串，前端回退显示 `groupId`。
    #[test]
    fn plan_uses_empty_title_when_detail_has_none() {
        // 跳过分支：没有安全来源，同时缺 title。
        let skipped = json!({
            "safeSourceMemberId": Value::Null,
            "members": [{ "memberId": "a", "versionStatus": "diverge" }],
        });
        match plan_from_detail(&skipped) {
            GroupPlan::Skip { title, .. } => assert_eq!(title, ""),
            other => panic!("缺 title 且无来源时应跳过，实际：{other:?}"),
        }

        // 可执行分支：同样缺 title，但来源与落后副本齐全。
        let ready = json!({
            "safeSourceMemberId": "src",
            "members": [
                { "memberId": "src", "versionStatus": "latest" },
                { "memberId": "a", "versionStatus": "behind" },
            ],
        });
        match plan_from_detail(&ready) {
            GroupPlan::Ready { title, targets, .. } => {
                assert_eq!(title, "");
                assert_eq!(targets, vec!["a".to_string()]);
            }
            other => panic!("缺 title 但有可同步目标时应可执行，实际：{other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // issue #231：QA 独立复核（`plan_from_detail` 的边界载荷）
    //
    // 这些载荷**都不是**正常产出的形态，但它们决定「一个坏组会不会拖垮整次批量」：
    // 该函数只读载荷、不写盘，所以任何输入都必须给出确定的 GroupPlan，不能 panic。
    // -----------------------------------------------------------------------

    /// 来源成员自己被标成 `behind`（理论不可能，但载荷可被篡改/构造）：仍不得被选成目标。
    ///
    /// 来源→来源的「同步」在 `resolve_pair` 会被拒（来源与目标必须是不同成员），
    /// 但那是一条目标级 skipped；不该出现的更早，故这里钉住筛选口径。
    #[test]
    fn plan_never_targets_the_source_even_when_it_is_marked_behind() {
        let detail = group_detail(
            Some("src"),
            &[("src", "behind"), ("a", "behind"), ("b", "latest")],
        );

        match plan_from_detail(&detail) {
            GroupPlan::Ready {
                source_member_id,
                targets,
                ..
            } => {
                assert_eq!(source_member_id, "src");
                assert_eq!(targets, vec!["a".to_string()], "来源自身绝不进目标清单");
            }
            other => panic!("有来源且有落后副本时必须可执行，实际：{other:?}"),
        }
    }

    /// 来源 id 不在 `members` 里（组被并发改写 / 载荷截断）：不 panic，照常给出目标，
    /// 由下游 `resolve_pair` 报「来源成员已失效」并计为该目标 skipped（不写任何东西）。
    ///
    /// 这里不把「来源不存在」提前判成整组跳过，是为了不在这一步复制 `resolve_pair` 的判据；
    /// 但一旦 `plan_from_detail` 改成 panic 或 `unwrap`，整次跨组批量会被一个坏组带崩。
    #[test]
    fn plan_tolerates_source_missing_from_members() {
        let detail = group_detail(Some("ghost"), &[("a", "behind"), ("b", "latest")]);

        match plan_from_detail(&detail) {
            GroupPlan::Ready {
                source_member_id,
                targets,
                ..
            } => {
                assert_eq!(source_member_id, "ghost");
                assert_eq!(targets, vec!["a".to_string()]);
            }
            other => panic!("来源缺失也不应 panic，实际：{other:?}"),
        }
    }

    /// `versionStatus` 缺失 / null / 非字符串（老载荷或第三方写入）一律不算 `behind`。
    #[test]
    fn plan_ignores_members_without_a_string_version_status() {
        let detail = json!({
            "title": "残缺载荷",
            "safeSourceMemberId": "src",
            "members": [
                { "memberId": "src", "versionStatus": "latest" },
                { "memberId": "no-status" },
                { "memberId": "null-status", "versionStatus": Value::Null },
                { "memberId": "num-status", "versionStatus": 1 },
                { "memberId": "ok", "versionStatus": "behind" },
            ],
        });

        match plan_from_detail(&detail) {
            GroupPlan::Ready { targets, .. } => {
                assert_eq!(targets, vec!["ok".to_string()], "只认字符串 `behind`");
            }
            other => panic!("有合法 behind 成员时应可执行，实际：{other:?}"),
        }
    }

    /// `members` 缺失 / 不是数组（载荷截断）：按「没有落后副本」跳过，不 panic。
    #[test]
    fn plan_skips_when_members_is_missing_or_not_an_array() {
        for detail in [
            json!({ "title": "无成员键", "safeSourceMemberId": "src" }),
            json!({ "title": "成员不是数组", "safeSourceMemberId": "src", "members": "x" }),
            json!({ "title": "成员为空", "safeSourceMemberId": "src", "members": [] }),
        ] {
            match plan_from_detail(&detail) {
                GroupPlan::Skip { reason_code, .. } => assert_eq!(reason_code, "noBehind"),
                other => panic!("无成员可同步时必须跳过，实际：{other:?}"),
            }
        }
    }

    /// `safeSourceMemberId` 不是字符串（null / 数字 / 缺失）：整组跳过且原因说清出路。
    #[test]
    fn plan_skips_when_safe_source_is_not_a_string() {
        for detail in [
            json!({ "title": "t", "safeSourceMemberId": Value::Null, "members": [{ "memberId": "a", "versionStatus": "behind" }] }),
            json!({ "title": "t", "safeSourceMemberId": 7, "members": [{ "memberId": "a", "versionStatus": "behind" }] }),
            json!({ "title": "t", "members": [{ "memberId": "a", "versionStatus": "behind" }] }),
        ] {
            match plan_from_detail(&detail) {
                GroupPlan::Skip {
                    reason_code,
                    message,
                    ..
                } => {
                    assert_eq!(reason_code, "noSafeSource");
                    assert!(
                        message.contains("手动选择"),
                        "跳过原因必须告诉用户出路：{message}"
                    );
                }
                other => panic!("无可用来源时必须跳过，实际：{other:?}"),
            }
        }
    }

    /// 载荷出现重复 `memberId`（存储层保证不会发生）：只钉住「不 panic、不静默吞掉」。
    ///
    /// 去重责任在存储层（`LinkGroup.members` 的 member id 唯一）；这里若改成 `unwrap` /
    /// 索引访问，一个被篡改的载荷就能让整次跨组批量 panic 掉其它组。
    #[test]
    fn plan_passes_duplicate_member_ids_through_without_panicking() {
        let detail = group_detail(Some("src"), &[("a", "behind"), ("a", "behind")]);

        match plan_from_detail(&detail) {
            GroupPlan::Ready { targets, .. } => {
                assert_eq!(targets.len(), 2, "重复 id 原样透传（去重由存储层保证）");
            }
            other => panic!("重复成员不应改变可执行结论，实际：{other:?}"),
        }
    }

    /// 档位校验必须在**任何存储读取之前**完成：错档位请求不得去碰真实数据目录。
    ///
    /// 这三条都是 `validate_scope` 的既有文案，跨组入口必须与单组入口逐字一致 ——
    /// 否则前端按文案分支的提示会失配。
    #[test]
    fn sync_all_safe_validates_scope_before_touching_any_store() {
        let ids = vec!["g-1".to_string()];

        assert_eq!(
            sync_all_safe(SessionClient::Workbuddy, Some(WbVariant::Cn), &ids, false).unwrap_err(),
            "只有 CodeBuddy IDE 支持档位筛选"
        );
        assert_eq!(
            sync_all_safe(SessionClient::CodebuddyIde, None, &ids, false).unwrap_err(),
            "CodeBuddy IDE 请求必须指定 cn 或 ai 档位"
        );
        // 空列表 + 错档位：报档位错误，说明校验发生在「入参非空」检查之前、也在读存储之前。
        assert_eq!(
            sync_all_safe(SessionClient::Jetbrains, Some(WbVariant::Ai), &[], false).unwrap_err(),
            "只有 CodeBuddy IDE 支持档位筛选"
        );
    }

    /// 空入参在四个客户端上都按同一句文案拒绝（前端 N=0 时不放按钮，接口仍要自己兜住）。
    ///
    /// 只跑「空列表」这一条：非空入参会去读客户端存储，用例不得触碰真实数据。
    #[test]
    fn sync_all_safe_rejects_empty_group_list_for_every_client() {
        for client in [
            SessionClient::Workbuddy,
            SessionClient::CodebuddyIde,
            SessionClient::VscodeExt,
            SessionClient::Jetbrains,
        ] {
            // CodeBuddy IDE 必须先过档位校验，单独给它一个合法档位。
            let scope = (client == SessionClient::CodebuddyIde).then_some(WbVariant::Cn);
            assert_eq!(
                sync_all_safe(client, scope, &[], false).unwrap_err(),
                "没有需要同步的会话组",
                "{client:?} 的空入参应被拒绝"
            );
        }
    }
