// ---------------------------------------------------------------------------
// 复制路径的容错修复（严格校验不放宽，修复只发生在复制路径上）
// ---------------------------------------------------------------------------

/// 两条完整记录 + 末行半截：丢弃末行，前两行逐字保留。
#[test]
fn repair_drops_truncated_tail_after_complete_records() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let head = format!(
        "{}\n{}\n",
        r#"{"sessionId":"a","text":"1"}"#, r#"{"sessionId":"a","text":"2"}"#
    );
    let text = format!("{head}{{\"sessionId\":\"a\",\"text\":\"");

    // 严格校验仍然拒绝（语义不变，见 `content_state_rejects_empty_invalid_and_truncated`）。
    assert!(normalize_jsonl(&text, own).is_err());

    let repair = repair_jsonl_for_copy(&text, own)
        .expect("末行截断且前面有完整记录时必须可修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.dropped_tail_line, Some(3));
    assert!(repair.split_lines.is_empty());
    assert_eq!(repair.text, head, "前两行必须逐字保留");
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 2);
}

/// 整份内容只有一条不完整记录：必须报错，不能复制出空会话。
#[test]
fn repair_rejects_truncated_only_record() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let truncated = "{\"sessionId\":\"a\",\"text\":";
    assert!(repair_jsonl_for_copy(truncated, own).is_err());

    // 末尾多一个换行也一样：剔除末尾空行后仍只有这一条坏记录。
    assert!(repair_jsonl_for_copy(&format!("{truncated}\n"), own).is_err());

    // 对照：前面有一条完整记录就可修复，证明拒绝条件是「前面没有完整记录」。
    let mixed = format!("{}\n{truncated}\n", r#"{"sessionId":"a","text":"1"}"#);
    assert!(repair_jsonl_for_copy(&mixed, own).unwrap().is_some());
}

/// 两个 JSON 对象粘在同一行（上次退出没写换行）：无损拆成两行，CRLF 风格保留。
#[test]
fn repair_splits_glued_records_without_losing_bytes() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let first = r#"{"sessionId":"a","text":"1"}"#;
    let second = r#"{"sessionId":"a","text":"2"}"#;
    let third = r#"{"sessionId":"a","text":"3"}"#;
    let text = format!("{first}\r\n{second}{third}\r\n");

    let repair = repair_jsonl_for_copy(&text, own)
        .expect("粘连行必须可无损拆分")
        .expect("必须给出修复结果");
    assert_eq!(repair.split_lines, vec![(2usize, 2usize)]);
    assert_eq!(repair.dropped_tail_line, None, "无损拆分不得丢记录");
    assert_eq!(repair.text, format!("{first}\r\n{second}\r\n{third}\r\n"));
    for line in repair.text.trim_end().split("\r\n") {
        assert!(
            serde_json::from_str::<serde::de::IgnoredAny>(line).is_ok(),
            "拆出的每一行都必须能独立解析：{line}"
        );
    }
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 3);
}

/// 中间行的纯垃圾（`hello`）不是可修复形态：报错并指到具体行。
#[test]
fn repair_rejects_garbage_line_in_the_middle() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!(
        "{}\nhello\n{}\n",
        r#"{"sessionId":"a","text":"1"}"#, r#"{"sessionId":"a","text":"2"}"#
    );
    let error = repair_jsonl_for_copy(&text, own).unwrap_err();
    assert!(error.starts_with("第 2 行的格式无法识别（"), "{error}");
    assert!(error.contains("不是末行"), "{error}");
}

/// 空行仍然是异常（不放宽 `normalize_jsonl` 的空行语义）。
#[test]
fn repair_rejects_empty_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!(
        "{}\n\n{}\n",
        r#"{"sessionId":"a","text":"1"}"#, r#"{"sessionId":"a","text":"2"}"#
    );
    let error = repair_jsonl_for_copy(&text, own).unwrap_err();
    assert!(error.contains("第 2 行为空"), "{error}");
}

/// 已经合法的正文不需要修复。
#[test]
fn repair_is_noop_for_valid_body() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    assert!(repair_jsonl_for_copy(&sample_body(own), own)
        .unwrap()
        .is_none());
}

/// 末行「完整记录 + 非法尾部」不是截断（错误不是 EOF）：必须拒绝，不能静默丢掉这条记录。
#[test]
fn repair_rejects_complete_record_with_garbage_tail() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!("{}\n{}{}", r#"{"a":1}"#, r#"{"b":2}"#, "garbage");
    let error = repair_jsonl_for_copy(&text, own).unwrap_err();
    assert!(error.starts_with("第 2 行的格式无法识别（"), "{error}");
    assert!(error.contains("文件末行"), "{error}");
}

/// 末行「完整记录 + 半截记录」：保留完整的那条（拆成独立行），只丢末尾半截。
#[test]
fn repair_keeps_complete_values_of_a_truncated_last_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!("{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}{"c":"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("末行「完整 + 半截」必须按「保留完整值、丢残尾」修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2), "残尾所在行必须上报");
    assert_eq!(
        repair.kept_tail_line_records,
        Some(1),
        "同行完整记录必须保留并计数"
    );
    assert!(repair.split_lines.is_empty(), "这不是「粘连拆行」");
    assert_eq!(repair.text, format!("{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#));
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 2);
}

/// 末行「2 条完整 + 半截」：两条完整值都要保留、只丢残尾，记录数必须逐条对上
/// （`records` 把保留下来的记录数一并计入，最终还要过 `record_count` 一致性校验）。
#[test]
fn repair_keeps_two_complete_values_of_a_truncated_last_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!("{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}{"c":3}{"d":"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("末行「2 完整 + 半截」必须可修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(repair.kept_tail_line_records, Some(2));
    assert!(repair.split_lines.is_empty(), "这不是「粘连拆行」");
    assert_eq!(
        repair.text,
        format!("{}\n{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#)
    );
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 3);
}

/// 只有这一行且「完整 + 半截」：前面没有任何完整记录 → 拒绝（不拿一次没写完的写入造副本）。
#[test]
fn repair_rejects_partial_last_line_without_prior_records() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    for text in [r#"{"b":2}{"c":"#, "{\"b\":2}{\"c\":\n"] {
        assert!(
            repair_jsonl_for_copy(text, own).is_err(),
            "前面无完整记录时不得修复：{text}"
        );
    }
}

/// 非末行的「完整 + 半截」：后面还有内容，一律拒绝。
#[test]
fn repair_rejects_partial_line_in_the_middle() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!("{}\n{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}{"c":"#, r#"{"d":4}"#);
    let error = repair_jsonl_for_copy(&text, own).unwrap_err();
    assert!(error.contains("第 2 行"), "{error}");
    assert!(error.contains("不是末行"), "{error}");
}

/// 真截断（整行都写了一半）仍然走整行丢弃，且不产生「保留记录」标记。
#[test]
fn repair_drops_whole_truncated_tail_without_kept_records() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!("{}\n{}", r#"{"a":1}"#, r#"{"b":"半"#);
    let repair = repair_jsonl_for_copy(&text, own).unwrap().unwrap();
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(repair.kept_tail_line_records, None);
    assert_eq!(repair.text, format!("{}\n", r#"{"a":1}"#));
}

/// 末行「写完了但语法不合法」（尾逗号 / `NaN` / 单引号键）不是截断：
/// 丢弃它会静默丢掉一条完整记录，还会用「不完整」误导用户，因此必须报错。
#[test]
fn repair_rejects_complete_but_illegal_tail_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    for bad in [r#"{"a":1,}"#, r#"{"a":NaN}"#, r#"{'a':1}"#] {
        let text = format!("{}\n{bad}\n", r#"{"a":1}"#);
        let error = repair_jsonl_for_copy(&text, own).unwrap_err();
        assert!(error.contains("第 2 行"), "缺陷行必须被指出：{error}");
    }
}

/// 诊断文案带列号、字节数与「文件末行」标识（末行截断与中间损坏要能区分）。
#[test]
fn parse_failure_message_carries_position_and_tail_flag() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let text = format!(
        "{}\n{}\n",
        r#"{"sessionId":"a","text":"1"}"#, r#"{"sessionId":"a","text":"2""#
    );
    let error = normalize_jsonl(&text, own).unwrap_err();
    assert!(error.starts_with("第 2 行的格式无法识别（"), "{error}");
    assert!(error.contains("列"), "{error}");
    assert!(error.contains("字节"), "{error}");
    assert!(error.contains("文件末行"), "{error}");
    assert!(error.ends_with("），内容可能不完整"), "{error}");
}
