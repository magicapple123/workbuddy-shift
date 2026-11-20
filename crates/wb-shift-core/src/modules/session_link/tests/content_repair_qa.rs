// ---------------------------------------------------------------------------
// QA 独立回归：复制路径容错修复的边界与「必须拒绝」形态（issue #1）
//
// 本片段只断言**契约**（PRD/设计里的两类可修复形态 + 其余一律拒绝），断言不参考
// `content_repair.rs` 的实现细节，目的是证伪而不是复述实现。
// ---------------------------------------------------------------------------

/// 本副本自己的会话 id（用于归一化口径）。
const QA_OWN: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// 逐行构造正文，行尾风格可控。
fn qa_body(lines: &[&str], ending: &str, tail_newline: bool) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push_str(ending);
    }
    if !tail_newline {
        out.truncate(out.len() - ending.len());
    }
    out
}

/// 任何返回 `Ok(Some(_))` 的修复结果都必须满足两条硬约束：
/// ① 修复后能重新通过严格校验；② 记录数与独立统计的完整记录数一致。
fn qa_assert_repair_contract(repair: &JsonlRepair, expected_records: usize) {
    let normalized = normalize_jsonl(&repair.text, QA_OWN)
        .unwrap_or_else(|error| panic!("修复后的正文必须能通过严格校验：{error}"));
    assert_eq!(
        normalized.record_count, expected_records,
        "修复后记录数必须等于源里完整记录的条数（不能凭空增删）：{}",
        repair.text
    );
}

/// 复现线上形态：2 条完整记录 + 第 3 行半截 -> 丢弃末行，前两行逐字保留。
#[test]
fn qa_repair_truncated_tail_keeps_prefix_byte_identical() {
    let text = format!(
        "{}{}",
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true),
        r#"{"c":"半截"#
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("末行截断且前面有完整记录必须可修复")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(3));
    assert!(repair.split_lines.is_empty());
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);
}

/// 末行截断但该行以 CRLF 结尾：仍然可修复，且剩下的行仍是 CRLF 风格。
#[test]
fn qa_repair_truncated_tail_with_crlf_ending() {
    let text = qa_body(
        &[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":"半截"#],
        "\r\n",
        true,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("末行截断必须可修复")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(3));
    let expected = qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\r\n", true);
    assert_eq!(repair.text, expected, "行尾风格必须保留");
    qa_assert_repair_contract(&repair, 2);
}

/// 末行截断后面还跟着空行：行号必须指向真正的坏行，而不是末尾空行。
#[test]
fn qa_repair_truncated_tail_reports_real_line_number() {
    let mut text = qa_body(
        &[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":"半截"#],
        "\n",
        true,
    );
    text.push_str("\n\n");
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("末行截断必须可修复")
        .expect("必须返回修复结果");
    assert_eq!(
        repair.dropped_tail_line,
        Some(3),
        "行号必须指向第 3 行（真正的坏行）"
    );
    qa_assert_repair_contract(&repair, 2);
}

/// 整份内容只有一条不完整记录 -> 必须拒绝（不能复制出空会话）。
#[test]
fn qa_repair_rejects_when_only_record_is_truncated() {
    assert!(repair_jsonl_for_copy(r#"{"c":"半截"#, QA_OWN).is_err());
    assert!(repair_jsonl_for_copy("{\"c\":", QA_OWN).is_err());
    assert!(repair_jsonl_for_copy("{\"c\":\"半截\"\n", QA_OWN).is_err());
    // 空白文件同样是「内容为空」而不是「可修复」。
    assert!(repair_jsonl_for_copy("", QA_OWN).is_err());
    assert!(repair_jsonl_for_copy("\n\n", QA_OWN).is_err());
    assert!(repair_jsonl_for_copy("   \n", QA_OWN).is_err());
}

/// 中间行是纯垃圾（`hello`）-> 必须拒绝，且指出行号与「不是末行」。
#[test]
fn qa_repair_rejects_garbage_middle_line() {
    let text = qa_body(&[r#"{"a":1}"#, "hello", r#"{"b":2}"#], "\n", true);
    let error = repair_jsonl_for_copy(&text, QA_OWN).expect_err("中间垃圾行不得被修复");
    assert!(error.contains("第 2 行"), "必须指出行号：{error}");
    assert!(error.contains("不是末行"), "必须说明不是末行：{error}");
}

/// 半截行不在末行 -> 必须拒绝（不能丢弃中间记录）。
#[test]
fn qa_repair_rejects_truncated_middle_line() {
    let text = qa_body(
        &[r#"{"a":1}"#, r#"{"b":"半截"#, r#"{"c":3}"#],
        "\n",
        true,
    );
    let error = repair_jsonl_for_copy(&text, QA_OWN).expect_err("中间半截行不得被修复");
    assert!(error.contains("第 2 行"), "{error}");
    assert!(error.contains("不是末行"), "{error}");
}

/// 空行（中间）仍然异常：不得当作可修复形态吞掉。
#[test]
fn qa_repair_rejects_empty_middle_line() {
    let text = qa_body(&[r#"{"a":1}"#, "", r#"{"b":2}"#], "\n", true);
    let error = repair_jsonl_for_copy(&text, QA_OWN).expect_err("中间空行不得被修复");
    assert!(error.contains("第 2 行为空"), "{error}");
}

/// 完整 JSON 值后跟非法尾部（末行）-> 必须拒绝：
/// 丢弃它会连带丢掉**已经写完整**的那条记录，设计里没有任何一类修复允许这样做。
#[test]
fn qa_repair_rejects_complete_value_with_illegal_tail() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":2}garbage"#], "\n", true);
    let result = repair_jsonl_for_copy(&text, QA_OWN);
    assert!(
        result.is_err(),
        "「完整记录 + 非法尾部」不属于任何一类可修复形态，必须拒绝；实际得到 {result:?}"
    );
}

/// 一行里「完整记录 + 半截记录」：允许「保留完整值、只丢残尾」或直接拒绝，
/// 但**不允许**把完整记录一起丢掉。
///
/// 实现选择的是保留：`kept_tail_line_records = Some(1)`、`dropped_tail_line = Some(2)`，
/// 修复后的正文包含 `{"b":2}`。断言按契约写（两种落法都接受），而不是锁死某一种落法。
#[test]
fn qa_repair_keeps_complete_value_when_tail_is_truncated() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":2}{"c":"#], "\n", true);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Ok(Some(repair)) => {
            assert_eq!(repair.kept_tail_line_records, Some(1), "必须声明保留了 1 条");
            assert_eq!(repair.dropped_tail_line, Some(2), "必须指出残尾所在行");
            assert_eq!(
                repair.text,
                qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true),
                "保留下来的完整记录必须落成独立一行，残尾不得写入"
            );
            qa_assert_repair_contract(&repair, 2);
        }
        // 另一种可接受落法：拒绝，让用户看到真实损坏。
        Err(_) => {}
        Ok(None) => panic!("严格校验已经判为失败，不可能返回「无需修复」"),
    }
}

/// 深嵌套（>128 层）不得 panic；完整记录也不得被当成「末行截断」悄悄丢掉。
///
/// 实测（serde_json 1.0.151）：`normalize_jsonl` 用的是 `from_str::<IgnoredAny>`，
/// 它**不套用** 128 层递归上限，因此 200 层嵌套被视为合法单行并原样通过——这是本次
/// 改动之前就有的行为，与容错修复无关。这里锁住两条底线：① 不 panic；
/// ② 修复不得凭空丢掉一条完整记录（只允许「不修复」或「报错」）。
#[test]
fn qa_repair_deep_nesting_does_not_panic_or_drop_record() {
    let deep = format!("{}{}{}", "{\"a\":".repeat(200), "1", "}".repeat(200));
    let alone = format!("{deep}\n");
    let alone_result = repair_jsonl_for_copy(&alone, QA_OWN).expect("深嵌套单行不得 panic");
    assert!(alone_result.is_none(), "严格校验能通过的内容不得触发修复");
    // 前面有完整记录时：要么不修复，要么报错；不允许只保留第 1 行。
    let text = qa_body(&[r#"{"a":1}"#, &deep], "\n", true);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Ok(None) => {}
        Ok(Some(repair)) => assert_eq!(
            normalize_jsonl(&repair.text, QA_OWN).unwrap().record_count,
            2,
            "不得丢掉深嵌套那条完整记录"
        ),
        Err(_) => {}
    }
    // 截断到一半的深嵌套（未闭合）仍按「末行截断」处理，且前面必须有完整记录。
    let truncated = "{\"a\":".repeat(200);
    let text = qa_body(&[r#"{"a":1}"#, &truncated], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.dropped_tail_line, Some(2));
    qa_assert_repair_contract(&repair, 1);
}

/// 一行 2 个对象粘连 -> 无损拆成两行，内容一个字节不丢。
#[test]
fn qa_repair_splits_two_glued_objects() {
    let text = qa_body(&[r#"{"a":1}{"b":2}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("粘连行必须可无损拆分")
        .expect("必须返回修复结果");
    assert_eq!(repair.split_lines, vec![(1usize, 2usize)]);
    assert_eq!(repair.dropped_tail_line, None);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);
}

/// 一行 3 个对象粘连（CRLF 文件）-> 拆成 3 行，行尾仍是 CRLF。
#[test]
fn qa_repair_splits_three_glued_objects_with_crlf() {
    let glued = r#"{"a":1}{"b":2}{"c":3}"#;
    let text = qa_body(&[glued], "\r\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("粘连行必须可无损拆分")
        .expect("必须返回修复结果");
    assert_eq!(repair.split_lines, vec![(1usize, 3usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\r\n", true),
        "CRLF 行尾风格必须保留"
    );
    qa_assert_repair_contract(&repair, 3);
}

/// 粘连行不在末行：后面的行必须原样保留、行号不受影响。
#[test]
fn qa_repair_splits_glued_line_in_the_middle() {
    let text = qa_body(
        &[r#"{"a":1}"#, r#"{"b":2}{"c":3}"#, r#"{"d":4}"#],
        "\n",
        true,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("粘连行必须可无损拆分")
        .expect("必须返回修复结果");
    assert_eq!(repair.split_lines, vec![(2usize, 2usize)]);
    assert_eq!(
        repair.text,
        qa_body(
            &[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#, r#"{"d":4}"#],
            "\n",
            true
        )
    );
    qa_assert_repair_contract(&repair, 4);
}

/// 字符串值里含 `}{` 与转义引号：不得被误拆。
#[test]
fn qa_repair_does_not_mis_split_braces_inside_strings() {
    let line = r#"{"a":"}{","b":"say \"}\" \\"}{"c":"中文😀"}"#;
    let text = qa_body(&[line], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("粘连行必须可无损拆分")
        .expect("必须返回修复结果");
    assert_eq!(repair.split_lines, vec![(1usize, 2usize)]);
    for part in repair.text.lines() {
        serde_json::from_str::<serde::de::IgnoredAny>(part)
            .unwrap_or_else(|error| panic!("拆出的每一行都必须能独立解析（{error}）：{part}"));
    }
    qa_assert_repair_contract(&repair, 2);
}

/// 一行 1 个合法对象 + 尾部空白：既不算粘连，也不算损坏（不需要修复）。
#[test]
fn qa_repair_trailing_whitespace_is_not_damage() {
    let text = qa_body(&[r#"{"a":1}   "#, r#"{"b":2}"#], "\n", true);
    assert!(
        repair_jsonl_for_copy(&text, QA_OWN)
            .expect("尾部空白不应被判为损坏")
            .is_none(),
        "合法正文不得触发修复"
    );
    // 制表符 + 多个空格 + CRLF 也一样。
    let text = qa_body(&[r#"{"a":1} 	 "#, r#"{"b":2}"#], "\r\n", true);
    assert!(repair_jsonl_for_copy(&text, QA_OWN).unwrap().is_none());
}

/// 干净正文不触发修复（`Ok(None)`），保证复制路径不会无谓改写内容。
#[test]
fn qa_repair_clean_body_is_noop() {
    assert!(repair_jsonl_for_copy(&sample_body(QA_OWN), QA_OWN)
        .unwrap()
        .is_none());
}

/// 修复后的正文必须以换行结尾（末行截断丢弃后不得出现「末尾无换行」的残形）。
#[test]
fn qa_repair_dropped_tail_keeps_trailing_newline() {
    for ending in ["\n", "\r\n"] {
        let text = format!(
            "{}{}",
            qa_body(&[r#"{"a":1}"#], ending, true),
            r#"{"b":"半截"#
        );
        let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
        assert!(
            repair.text.ends_with(ending),
            "丢弃末行后正文仍应以换行结束：{:?}",
            repair.text
        );
    }
}

/// 同一份正文里既有粘连行、又有末行截断：两类修复必须同时报告，行号各自准确。
#[test]
fn qa_repair_reports_split_and_dropped_tail_together() {
    let text = qa_body(
        &[r#"{"a":1}{"b":2}"#, r#"{"c":3}"#, r#"{"d":"半截"#],
        "\n",
        true,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("混合形态必须可修复")
        .expect("必须返回修复结果");
    assert_eq!(repair.split_lines, vec![(1usize, 2usize)]);
    assert_eq!(repair.dropped_tail_line, Some(3));
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 3);
}
