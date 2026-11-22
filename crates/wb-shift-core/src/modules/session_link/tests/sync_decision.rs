// -----------------------------------------------------------------------
// 同步判定（design §3.2）
// -----------------------------------------------------------------------

/// `count` 条有序记录（index 递增，便于构造严格有序追加）。
fn records(count: usize, from: usize) -> String {
    (from..from + count)
        .map(|index| format!("{{\"type\":\"assistant\",\"index\":{index}}}\n"))
        .collect()
}

/// 按给定 index 顺序构造正文（制造重排用）。
fn records_in_order(order: &[usize]) -> String {
    order
        .iter()
        .map(|index| format!("{{\"type\":\"assistant\",\"index\":{index}}}\n"))
        .collect()
}

fn normalized_from(text: &str) -> NormalizedContent {
    normalize_jsonl(text, "sess").expect("测试正文必须可归一化")
}

fn content_from(text: &str) -> ContentState {
    ContentState::Ready(ContentSnapshot {
        text: text.to_string(),
        full_digest: full_digest_of(text.as_bytes()),
        normalized: normalized_from(text),
    })
}

fn baseline_from(text: &str) -> BaselineRecord {
    let normalized = normalized_from(text);
    BaselineRecord {
        version: BASELINE_VERSION,
        baseline_ref: "base-x".to_string(),
        normalization_version: NORMALIZATION_VERSION,
        created_at: 1,
        record_count: normalized.record_count,
        total_digest: normalized.total_digest,
        line_digests: normalized.line_digests,
    }
}

/// A 与 B 有序内容一致 → identical（无需基线；有基线也一样）。
#[test]
fn verdict_identical_does_not_write_body() {
    let body = records(4, 0);
    for baseline in [
        BaselineState::Missing,
        BaselineState::Ready(baseline_from(&body)),
    ] {
        let decision = decide_sync(&content_from(&body), &content_from(&body), &baseline);
        assert_eq!(decision.verdict, SyncVerdict::Identical);
        assert!(!decision.default_checked);
        assert!(decision.verdict.available_modes().is_empty());
        assert!(
            decision.reason.contains("不需要同步"),
            "{}",
            decision.reason
        );
    }
}

/// 快进样例：共同基线 X、B = X、A = X + 3 条有序记录 → 默认勾选，extraB = 0。
/// 该场景由「B 是 A 的严格有序前缀」直接命中（不依赖基线，对齐 git fast-forward）。
#[test]
fn verdict_fast_forward_when_target_keeps_baseline_and_source_appends() {
    let base = records(5, 0);
    let source = records(8, 0);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&base),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::FastForward);
    assert!(decision.default_checked, "只有快进默认勾选");
    assert_eq!(decision.extra_a, 3);
    assert_eq!(decision.extra_b, 0);
    assert_eq!(decision.common, 5);
    assert_eq!(
        decision.verdict.available_modes(),
        vec![SyncMode::FastForward]
    );
    assert!(decision.reason.contains("新增 3 条"), "{}", decision.reason);
}

/// 祖先快进不依赖基线：无基线 / 基线不可验证时，B 是 A 的严格有序前缀照样可快进。
///
/// 对齐 git fast-forward 的 ancestor 语义：B 的每条记录（含顺序）都被 A 完整包含，
/// 同步只是把 A 多出的尾部追加给 B，**零覆盖**，因此无需历史记录佐证。
#[test]
fn fast_forward_without_baseline_when_target_is_ordered_prefix() {
    let target = records(5, 0);
    let source = records(8, 0);
    for baseline in [
        BaselineState::Missing,
        BaselineState::Unverifiable("上次同步的记录缺失或已损坏".to_string()),
    ] {
        let decision = decide_sync(&content_from(&source), &content_from(&target), &baseline);
        assert_eq!(decision.verdict, SyncVerdict::FastForward, "{baseline:?}");
        assert!(decision.default_checked, "只有快进默认勾选");
        assert_eq!(
            decision.verdict.available_modes(),
            vec![SyncMode::FastForward]
        );
        assert_eq!(decision.extra_a, 3);
        assert_eq!(decision.extra_b, 0);
        assert_eq!(decision.common, 5);
        assert!(decision.reason.contains("新增 3 条"), "{}", decision.reason);
    }
}

/// 第二轮轮换：基线 X = 4 条、B = 6 条（停在中间状态）、A = 9 条且以 B 为前缀
/// → 快进（改造前 B ≠ X 会判 diverge）。
#[test]
fn fast_forward_with_baseline_when_target_is_mid_chain_prefix() {
    let baseline = records(4, 0);
    let target = records(6, 0);
    let source = records(9, 0);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&baseline)),
    );
    assert_eq!(decision.verdict, SyncVerdict::FastForward);
    assert!(decision.default_checked);
    assert_eq!(decision.extra_a, 3, "来源比目标多的 3 条才是本次要追加的");
    assert_eq!(decision.extra_b, 0);
    assert_eq!(decision.common, 6);
    assert!(decision.reason.contains("新增 3 条"), "{}", decision.reason);
}

/// 目标变化样例：A = X、B = X + 5 条 → ahead，仅目标变化，不写目标。
#[test]
fn verdict_ahead_when_only_target_changed() {
    let base = records(5, 0);
    let target = records(10, 0);
    let decision = decide_sync(
        &content_from(&base),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Ahead);
    assert!(!decision.default_checked);
    assert_eq!(decision.extra_a, 0);
    assert_eq!(decision.extra_b, 5);
    assert!(
        decision.verdict.available_modes().is_empty(),
        "ahead 不可勾选"
    );
    assert!(
        decision.reason.contains("只有目标账号新增"),
        "{}",
        decision.reason
    );
}

/// 反向前缀不得判快进（回归保护）：A 是 B 的严格前缀（目标领先）时前缀规则不适用。
/// 来源等于基线 → 原有文案的 ahead；无基线时镜像规则同样判 ahead（口径 A：
/// 镜像关系不依赖基线，改造前无基线回落 unknown），两种情形都不授权任何写入。
#[test]
fn ahead_stays_when_target_extends_source() {
    let source = records(5, 0);
    let target = records(9, 0);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&source)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Ahead);
    assert!(!decision.default_checked);
    assert!(decision.verdict.available_modes().is_empty());
    assert_eq!(decision.extra_a, 0);
    assert_eq!(decision.extra_b, 4);

    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Missing,
    );
    assert_eq!(
        decision.verdict,
        SyncVerdict::Ahead,
        "镜像前缀不依赖基线，无基线时同样判 ahead 且不得写入"
    );
    assert!(!decision.default_checked);
    assert!(decision.verdict.available_modes().is_empty());
    assert_eq!(decision.extra_a, 0);
    assert_eq!(decision.extra_b, 4);
}

/// 镜像前缀 + 有基线 + 目标已偏离基线（本机复现态：来源 7 条 ⊊ 目标 9 条、配对基线 4 条）
/// → ahead，本次没有可写内容（改造前落入兜底 diverge，会给出把目标新记录回滚掉的覆盖入口）。
#[test]
fn ahead_when_source_is_ordered_prefix_of_target_with_baseline() {
    let baseline = records(4, 0);
    let source = records(7, 0);
    let target = records(9, 0);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&baseline)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Ahead);
    assert!(!decision.default_checked);
    assert!(decision.verdict.available_modes().is_empty());
    assert_eq!(decision.extra_a, 0);
    assert_eq!(decision.extra_b, 2);
    assert_eq!(decision.common, 7);
    assert!(
        decision.reason.contains("已包含") && decision.reason.contains("2 条"),
        "{}",
        decision.reason
    );
}

/// 镜像前缀不依赖基线：无基线 / 基线不可验证时，A 是 B 的严格有序前缀同样判 ahead
/// （改造前为 unknown；两者都不授权任何写入，只是提示更准确）。
#[test]
fn ahead_without_baseline_when_source_is_ordered_prefix_of_target() {
    let source = records(5, 0);
    let target = records(9, 0);
    for baseline in [
        BaselineState::Missing,
        BaselineState::Unverifiable("上次同步的记录缺失或已损坏".to_string()),
    ] {
        let decision = decide_sync(&content_from(&source), &content_from(&target), &baseline);
        assert_eq!(decision.verdict, SyncVerdict::Ahead, "{baseline:?}");
        assert!(!decision.default_checked);
        assert!(decision.verdict.available_modes().is_empty());
        assert_eq!(decision.extra_a, 0);
        assert_eq!(decision.extra_b, 4);
        assert!(
            decision.reason.contains("已包含") && decision.reason.contains("4 条"),
            "{}",
            decision.reason
        );
    }
}

/// 双方都变化、且互不为前缀（真分叉）→ diverge：默认不勾，但可显式覆盖。
#[test]
fn verdict_diverge_when_both_sides_changed() {
    let base = records(5, 0);
    // 目标写的是另一批新增记录：与来源的尾部互不相同，任一侧都不是对方的前缀。
    let target = format!("{}{}", records(5, 0), records(5, 100));
    let decision = decide_sync(
        &content_from(&records(7, 0)),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Diverge);
    assert!(!decision.default_checked);
    assert_eq!(
        decision.extra_a, 2,
        "真分叉：来源尾部独有，不得被镜像前缀吞掉"
    );
    assert_eq!(decision.extra_b, 5, "真分叉：目标尾部独有");
    assert_eq!(
        decision.verdict.available_modes(),
        vec![SyncMode::Overwrite]
    );
    assert!(
        decision.verdict.allows(SyncMode::Overwrite),
        "有效可比较的冲突才允许显式覆盖"
    );
    assert!(!decision.verdict.allows(SyncMode::FastForward));
    assert!(
        decision.reason.contains("替换目标账号的完整内容"),
        "{}",
        decision.reason
    );
}

/// 来源重写/压缩 → diverge，不得当成快进。
#[test]
fn verdict_diverge_when_source_rewritten_or_compacted() {
    let base = records(5, 0);
    let ready = BaselineState::Ready(baseline_from(&base));

    // 压缩：来源比基线还短。
    let compacted = decide_sync(&content_from(&records(3, 0)), &content_from(&base), &ready);
    assert_eq!(compacted.verdict, SyncVerdict::Diverge);

    // 重写：同样条数但内容不同。
    let rewritten_text = records(5, 100);
    let rewritten = decide_sync(&content_from(&rewritten_text), &content_from(&base), &ready);
    assert_eq!(rewritten.verdict, SyncVerdict::Diverge);

    // 目标侧被重写、来源等于基线 → ahead（只报目标变化，不写目标）。
    let target_rewritten =
        decide_sync(&content_from(&base), &content_from(&rewritten_text), &ready);
    assert_eq!(target_rewritten.verdict, SyncVerdict::Ahead);
}

/// 相同多重集、顺序不同 → 不得判快进 / ahead（extraB 为 0 也不代表安全）。
#[test]
fn verdict_diverge_for_same_multiset_in_different_order() {
    let base_order = vec![0usize, 1, 2, 3];
    let base = records_in_order(&base_order);
    let source = records_in_order(&[0, 1, 3, 2]);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&base),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Diverge);
    assert!(!decision.default_checked);
    assert_ne!(
        decision.verdict,
        SyncVerdict::Ahead,
        "重排后的等价多重集不得判 ahead"
    );
    assert_eq!(decision.extra_a, 0, "多重集相同：差集为 0 只是解释信息");
    assert_eq!(decision.extra_b, 0);
    assert_eq!(decision.common, 4);

    // 目标另有新增（多重集包含来源，但顺序被重排、目标已偏离基线）：镜像分支只认有序前缀，
    // 不得按多重集包含判 ahead。
    let target = records_in_order(&[0, 1, 2, 3, 100]);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Diverge);
    assert_ne!(decision.verdict, SyncVerdict::Ahead);
    assert!(!decision.verdict.allows(SyncMode::FastForward));
    assert_eq!(decision.extra_a, 0);
    assert_eq!(decision.extra_b, 1);
    assert_eq!(decision.common, 4);
}

/// 有共同前缀但尾部各自分叉（真分叉）→ 不得判快进：无基线时 unknown、不可勾选。
#[test]
fn unknown_without_baseline_when_sides_share_no_prefix() {
    let target = records_in_order(&[0, 1, 2, 3, 100, 101]);
    let source = records_in_order(&[0, 1, 2, 3, 200, 201, 202]);
    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Missing,
    );
    assert_eq!(decision.verdict, SyncVerdict::Unknown);
    assert!(!decision.default_checked);
    assert!(decision.verdict.available_modes().is_empty());
    assert!(!decision.verdict.allows(SyncMode::FastForward));
    assert!(
        !decision.verdict.allows(SyncMode::Overwrite),
        "unknown 不得被覆盖绕过"
    );
    assert!(
        decision.reason.contains("找不到双方上次一致的内容"),
        "{}",
        decision.reason
    );
}

/// 缺少可验证基线 / 内容不可验证 → unknown，禁止任何模式（含显式覆盖）。
#[test]
fn verdict_unknown_without_verifiable_baseline_or_content() {
    // 双方互不为前缀（来源重写了前段）：无法用内容关系判定，无基线时只能 unknown。
    let base = records(5, 0);
    let source = records(5, 100);

    for (name, baseline) in [
        ("缺少基线引用", BaselineState::Missing),
        (
            "基线不可验证",
            BaselineState::Unverifiable("上次同步的记录缺失或已损坏".to_string()),
        ),
    ] {
        let decision = decide_sync(&content_from(&source), &content_from(&base), &baseline);
        assert_eq!(decision.verdict, SyncVerdict::Unknown, "{name}");
        assert!(!decision.default_checked, "{name}");
        assert!(decision.verdict.available_modes().is_empty(), "{name}");
        assert!(
            !decision.verdict.allows(SyncMode::Overwrite),
            "{name}：unknown 不得被覆盖"
        );
        assert!(!decision.reason.is_empty(), "{name}");
    }
    assert_eq!(
        BaselineState::Ready(baseline_from(&base)).unusable_reason(),
        None
    );
    assert!(BaselineState::Missing
        .unusable_reason()
        .unwrap()
        .contains("找不到双方上次一致的内容"));

    // 正文不可验证：缺失/截断/非法一律不得默认快进。
    let ready = BaselineState::Ready(baseline_from(&base));
    for (name, source, target) in [
        ("来源缺失", ContentState::Missing, content_from(&base)),
        (
            "来源不可验证",
            ContentState::Unavailable("第 3 行的格式无法识别".to_string()),
            content_from(&base),
        ),
        (
            "目标不可验证",
            content_from(&source),
            ContentState::Unavailable("内容读取失败".to_string()),
        ),
    ] {
        let decision = decide_sync(&source, &target, &ready);
        assert_eq!(decision.verdict, SyncVerdict::Unknown, "{name}");
        assert!(!decision.default_checked, "{name}");
        assert!(decision.verdict.available_modes().is_empty(), "{name}");
        assert!(
            !decision.verdict.allows(SyncMode::Overwrite),
            "{name}：内容不可验证时禁止覆盖"
        );
    }
}

/// 长时间正常追加（远超 50%）不得因「共同占比低」被误判。
#[test]
fn verdict_fast_forward_survives_large_ordered_append() {
    let base = records(5, 0);
    let decision = decide_sync(
        &content_from(&records(205, 0)),
        &content_from(&base),
        &BaselineState::Ready(baseline_from(&base)),
    );
    assert_eq!(decision.verdict, SyncVerdict::FastForward);
    assert!(decision.default_checked);
    assert_eq!(decision.extra_a, 200);
    assert_eq!(decision.extra_b, 0);
    assert_eq!(decision.common, 5, "共同记录只占 2%，仍应判快进");
}

/// 判定/模式的字符串契约与 serde 输出一致，且 unknown 不接受任何模式。
#[test]
fn verdict_and_mode_string_contract() {
    for verdict in [
        SyncVerdict::Identical,
        SyncVerdict::FastForward,
        SyncVerdict::Ahead,
        SyncVerdict::Diverge,
        SyncVerdict::Unknown,
    ] {
        assert_eq!(
            serde_json::to_value(verdict).unwrap().as_str().unwrap(),
            verdict.as_str()
        );
        assert_eq!(
            verdict.allows(SyncMode::FastForward) || verdict.allows(SyncMode::Overwrite),
            verdict.is_actionable()
        );
    }
    for mode in [SyncMode::FastForward, SyncMode::Overwrite] {
        assert_eq!(
            serde_json::to_value(mode).unwrap().as_str().unwrap(),
            mode.as_str()
        );
        assert_eq!(SyncMode::parse(mode.as_str()).unwrap(), mode);
    }
    assert!(SyncMode::parse(" overwrite ").is_ok());
    assert!(SyncMode::parse("force").unwrap_err().contains("force"));

    assert!(SyncVerdict::FastForward.allows(SyncMode::FastForward));
    assert!(!SyncVerdict::FastForward.allows(SyncMode::Overwrite));
    assert!(SyncVerdict::Diverge.allows(SyncMode::Overwrite));
    assert!(SyncVerdict::Ahead.allows(SyncMode::UnifyOverwrite));
    assert!(!SyncVerdict::Diverge.allows(SyncMode::UnifyOverwrite));
    assert!(!SyncVerdict::Unknown.allows(SyncMode::UnifyOverwrite));
    for verdict in [
        SyncVerdict::Unknown,
        SyncVerdict::Ahead,
        SyncVerdict::Identical,
    ] {
        assert!(!verdict.allows(SyncMode::FastForward), "{verdict:?}");
        assert!(!verdict.allows(SyncMode::Overwrite), "{verdict:?}");
    }
}

/// 文案用「条」，不得把 JSONL 行数叫「消息数」。
#[test]
fn verdict_reasons_use_record_wording() {
    let base = records(5, 0);
    let ready = BaselineState::Ready(baseline_from(&base));
    let decisions = [
        decide_sync(&content_from(&base), &content_from(&base), &ready),
        decide_sync(&content_from(&records(8, 0)), &content_from(&base), &ready),
        decide_sync(&content_from(&base), &content_from(&records(9, 0)), &ready),
        decide_sync(
            &content_from(&records(7, 0)),
            &content_from(&records(9, 0)),
            &ready,
        ),
        decide_sync(
            &content_from(&records(7, 0)),
            &content_from(&format!("{}{}", records(5, 0), records(5, 100))),
            &ready,
        ),
        decide_sync(
            &content_from(&base),
            &content_from(&base),
            &BaselineState::Missing,
        ),
    ];
    for decision in &decisions {
        assert!(
            !decision.reason.contains("消息"),
            "不得把条数叫消息数：{}",
            decision.reason
        );
    }
    for decision in &decisions[..5] {
        assert!(decision.reason.contains("条"), "{}", decision.reason);
    }
}

/// 成员状态字符串契约与 serde 输出一致。
#[test]
fn member_state_string_contract() {
    for state in [
        MemberState::Active,
        MemberState::Stale,
        MemberState::Superseded,
    ] {
        assert_eq!(
            serde_json::to_value(state).unwrap().as_str().unwrap(),
            state.as_str()
        );
    }
}

/// 组指纹：成员身份/状态与配对基线引用参与，组内顺序无关；其它组的成员不影响它。
#[test]
fn group_fingerprint_tracks_members_and_pair_bases() {
    let mut group = group_with_members(
        "g-1",
        vec![
            member("uid-a", "sess-1", MemberState::Active),
            member("uid-b", "sess-b", MemberState::Active),
        ],
    );
    set_pair_base(&mut group, "m-uid-a-sess-1", "m-uid-b-sess-b", "base-ab", 1);
    let baseline = group_fingerprint(&group);

    // 顺序无关：同一集合换个书写顺序指纹不变。
    group.members.reverse();
    assert_eq!(group_fingerprint(&group), baseline);
    group.members.reverse();

    // 状态变化 / 新增成员 / 基线变化都会改变指纹。
    set_member_state(&mut group, "m-uid-b-sess-b", MemberState::Stale);
    let stale = group_fingerprint(&group);
    assert_ne!(stale, baseline);
    set_member_state(&mut group, "m-uid-b-sess-b", MemberState::Active);
    assert_eq!(group_fingerprint(&group), baseline);

    let mut added = group.clone();
    add_active_member(&mut added, member("uid-c", "sess-c", MemberState::Active));
    assert_ne!(group_fingerprint(&added), baseline, "新增成员必须改变指纹");

    let mut rebased = group.clone();
    set_pair_base(
        &mut rebased,
        "m-uid-a-sess-1",
        "m-uid-b-sess-b",
        "base-ab2",
        1,
    );
    assert_ne!(
        group_fingerprint(&rebased),
        baseline,
        "基线引用变化必须改变指纹"
    );

    let mut other_variant = group.clone();
    other_variant.variant = WbVariant::Ai;
    assert_ne!(group_fingerprint(&other_variant), baseline);
}

// -----------------------------------------------------------------------
// 末条归一等价（issue #3 最后一条：副本永久显示「存在分歧」消不掉）
// -----------------------------------------------------------------------
//
// 复制与同步写目标时都会把末条 assistant 消息的 `incomplete` 归一到 `completed`
// 并留一条 `_meta.incompleteNormalized` 痕迹。两侧因此「行数相同、末行摘要必然不同」，
// 按原始摘要逐行硬比会永远判 diverge，用户点多少次「以此为准」都消不掉。

/// 末条 assistant 消息行；`normalized` 为真时带上归一留痕（时间戳刻意固定，
/// 用例只关心「留痕内容不同也不影响判定」）。
fn tail_record(index: usize, status: &str, text: &str, normalized: bool) -> String {
    let marker = if normalized {
        r#","_meta":{"incompleteNormalized":{"at":"20260101-000000","reason":"copy normalize"}}"#
    } else {
        ""
    };
    format!(
        "{{\"type\":\"message\",\"role\":\"assistant\",\"index\":{index},\"status\":\"{status}\",\"text\":\"{text}\"{marker}}}\n"
    )
}

/// 两边只差末条归一签名 → identical（原始摘要不同，比较口径相同）。
#[test]
fn identical_when_only_tail_normalization_signature_differs() {
    let prefix = records(2, 0);
    let source = format!("{prefix}{}", tail_record(2, "incomplete", "被打断", false));
    let target = format!("{prefix}{}", tail_record(2, "completed", "被打断", true));

    // 先证明这个用例不是「正文本来就一样」：存储口径必须不同，否则证明不了新规则。
    let source_normalized = normalized_from(&source);
    let target_normalized = normalized_from(&target);
    assert_ne!(
        source_normalized.line_digests, target_normalized.line_digests,
        "末条签名不同的原始摘要必须不同，否则用例失去意义"
    );
    assert_eq!(
        source_normalized.comparable_line_digests, target_normalized.comparable_line_digests,
        "比较口径必须把归一签名折算成等价形式"
    );

    for baseline in [
        BaselineState::Missing,
        BaselineState::Ready(baseline_from(&prefix)),
    ] {
        let decision = decide_sync(&content_from(&source), &content_from(&target), &baseline);
        assert_eq!(decision.verdict, SyncVerdict::Identical, "{baseline:?}");
        assert!(!decision.default_checked, "一致不默认勾选");
        assert!(decision.verdict.available_modes().is_empty(), "一致不可勾选");
        assert!(
            decision.reason.contains("不需要同步"),
            "{}",
            decision.reason
        );
        assert!(
            decision.reason.contains("归一"),
            "新增的等价路径要自解释为什么算一致：{}",
            decision.reason
        );
        assert!(
            !decision.verdict.allows(SyncMode::Overwrite),
            "不得因为新规则放开覆盖入口"
        );
    }
}

/// 来源在归一副本之后又追加（末条签名仍只差归一）→ fastForward：目标仍是来源的有序前缀。
#[test]
fn fast_forward_when_source_appends_after_normalized_copy() {
    let prefix = records(2, 0);
    let target = format!("{prefix}{}", tail_record(2, "completed", "被打断", true));
    let source = format!(
        "{prefix}{}{}",
        tail_record(2, "incomplete", "被打断", false),
        records(1, 9)
    );

    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Missing,
    );
    assert_eq!(decision.verdict, SyncVerdict::FastForward);
    assert!(decision.default_checked, "只有快进默认勾选");
    assert_eq!(
        decision.verdict.available_modes(),
        vec![SyncMode::FastForward]
    );
    // 文案按记录数差给出「新增 1 条」；`extra_a` / `extra_b` 仍是原始摘要的多重集差集
    // （只用于解释，不参与判定），这里不断言它们的取值以免把解释口径固化。
    assert!(
        decision.reason.contains("新增 1 条"),
        "{}",
        decision.reason
    );
}

/// 目标在归一副本之后又追加 → ahead（目标已完整包含来源，本次没有可写内容）。
#[test]
fn ahead_when_target_appends_after_normalized_copy() {
    let prefix = records(2, 0);
    let source = format!("{prefix}{}", tail_record(2, "incomplete", "被打断", false));
    let target = format!(
        "{prefix}{}{}",
        tail_record(2, "completed", "被打断", true),
        records(1, 9)
    );

    let decision = decide_sync(&content_from(&source), &content_from(&target), &BaselineState::Missing);
    assert_eq!(decision.verdict, SyncVerdict::Ahead);
    assert!(!decision.default_checked);
    assert!(decision.verdict.available_modes().is_empty());
    assert!(
        decision.reason.contains("已包含"),
        "{}",
        decision.reason
    );
}

/// 末条内容本身不同（不只是签名）→ 仍判 diverge，不得被新规则放过。
#[test]
fn diverge_when_tail_content_differs_beyond_normalization() {
    let prefix = records(2, 0);
    let source = format!("{prefix}{}", tail_record(2, "incomplete", "我的回答", false));
    let target = format!("{prefix}{}", tail_record(2, "completed", "别人的回答", true));

    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&prefix)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Diverge);
    assert!(!decision.default_checked);
    assert_eq!(
        decision.verdict.available_modes(),
        vec![SyncMode::Overwrite],
        "有效可比较的冲突仍保留显式覆盖入口"
    );
}

/// 中间行带归一留痕、其它内容确实不同 → 仍判 diverge。
///
/// 归一改写对任意一行都会尝试（不只看末行），所以必须验证它不会把「其它真实差异」一起吃掉。
#[test]
fn diverge_when_middle_line_carries_marker_and_other_content_differs() {
    let prefix = records(1, 0);
    let source = format!(
        "{prefix}{}{}",
        tail_record(1, "incomplete", "被打断", false),
        records(1, 50)
    );
    let target = format!(
        "{prefix}{}{}",
        tail_record(1, "completed", "被打断", true),
        records(1, 60)
    );

    let decision = decide_sync(
        &content_from(&source),
        &content_from(&target),
        &BaselineState::Ready(baseline_from(&prefix)),
    );
    assert_eq!(decision.verdict, SyncVerdict::Diverge);
    assert!(!decision.default_checked);
    assert!(!decision.verdict.allows(SyncMode::FastForward));
}
