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

// ---------------------------------------------------------------------------
// 第三类形态：裸 NUL 填充（issue #1 用户实测的第 1000 行：行首 8 974 个 NUL）
// ---------------------------------------------------------------------------

/// `count` 个裸 NUL + `tail`：构造线上那种「行首填充 + 一条完整 JSON」的行。
fn nul_pad(count: usize, tail: &str) -> String {
    let mut out = "\0".repeat(count);
    out.push_str(tail);
    out
}

/// 行首 NUL + 一条完整记录：剥掉填充后该记录照常复制，`nul_stripped` 报出行号与字节数。
#[test]
fn repair_strips_leading_nul_filler_before_a_complete_record() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let padded = nul_pad(5, r#"{"a":1}"#);
    let text = format!("{padded}\n{}\n", r#"{"b":2}"#);
    // 严格校验仍然拒绝（语义不变）：NUL 不是空白，第 1 列就报「expected value」。
    let strict = normalize_jsonl(&text, own).unwrap_err();
    assert!(strict.contains("第 1 行的格式无法识别"), "{strict}");

    let repair = repair_jsonl_for_copy(&text, own)
        .expect("行首 NUL 填充必须可无损修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(1usize, 5usize)]);
    assert_eq!(repair.dropped_tail_line, None, "剥离填充不得算丢记录");
    assert!(repair.split_lines.is_empty(), "这不是「粘连拆行」");
    assert_eq!(repair.text, format!("{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#));
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 2);
}

/// 行尾 NUL + 完整记录：同样是填充，剥掉即可（行尾风格原样保留）。
#[test]
fn repair_strips_trailing_nul_filler_after_a_complete_record() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let padded = nul_pad(3, r#"{"b":2}"#);
    let text = format!("{}\r\n{padded}\r\n", r#"{"a":1}"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("行尾 NUL 填充必须可无损修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(2usize, 3usize)]);
    assert_eq!(
        repair.text,
        format!("{}\r\n{}\r\n", r#"{"a":1}"#, r#"{"b":2}"#),
        "CRLF 行尾不得被悄悄改掉"
    );
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 2);
}

/// NUL 夹在两条记录之间：两段各是一条完整记录，按原行尾风格拆成独立两行。
#[test]
fn repair_splits_two_records_separated_by_nul() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let glued = nul_pad(2, r#"{"c":3}"#);
    let text = format!("{}\n{}\0{glued}\n", r#"{"a":1}"#, r#"{"b":2}"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("NUL 分隔的两条完整记录必须可无损拆行")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(2usize, 3usize)]);
    assert_eq!(
        repair.text,
        format!("{}\n{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#)
    );
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 3);
}

/// 中间行整行都是 NUL：连行尾一起移除，前后记录都保留（留空行会被判异常）。
#[test]
fn repair_removes_a_nul_only_line_in_the_middle() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let filler = "\0".repeat(9);
    let text = format!("{}\n{filler}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("整行 NUL 填充必须可移除")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(2usize, 9usize)]);
    assert_eq!(repair.text, format!("{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#));
    assert_eq!(
        repair.text.lines().count(),
        2,
        "整行填充移除后行数要少一行，且不得留下空行"
    );
    assert_eq!(normalize_jsonl(&repair.text, own).unwrap().record_count, 2);
}

/// NUL 落在一条记录内部（把记录劈成两段）：无法证明无损 → 拒绝，不得拼接猜测。
#[test]
fn repair_rejects_nul_inside_a_record() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let split = format!(
        "{}\0\0\0{}",
        r#"{"sessionId":"a","text":"he"#, r#"llo"}"#
    );
    let text = format!("{}\n{split}\n{}\n", r#"{"a":1}"#, r#"{"d":4}"#);
    let error = repair_jsonl_for_copy(&text, own).expect_err("被 NUL 劈开的记录不得被修复");
    assert!(error.contains("第 2 行"), "必须指出坏行行号：{error}");
    assert!(error.contains("不是末行"), "{error}");
}

/// 行首 NUL + 末行截断（前面已有完整记录）：按末行规则丢弃该行，同时报告剥离了填充。
#[test]
fn repair_handles_leading_nul_on_a_truncated_last_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let truncated = nul_pad(4, r#"{"b":"半"#);
    let text = format!("{}\n{truncated}\n", r#"{"a":1}"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("末行 NUL + 截断必须按末行规则修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(
        repair.kept_tail_line_records, None,
        "整行都是半截，没有可保留的完整值"
    );
    assert_eq!(repair.nul_stripped, vec![(2usize, 4usize)]);
    assert_eq!(repair.text, format!("{}\n", r#"{"a":1}"#));
}

/// 行首 NUL + 截断，但该行**不是**末行：后面还有内容 → 拒绝。
#[test]
fn repair_rejects_leading_nul_with_truncation_when_not_last_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let truncated = nul_pad(4, r#"{"b":"半"#);
    let text = format!("{}\n{truncated}\n{}\n", r#"{"a":1}"#, r#"{"d":4}"#);
    let error = repair_jsonl_for_copy(&text, own).expect_err("非末行的截断不得被修复");
    assert!(error.contains("第 2 行"), "{error}");
    assert!(error.contains("不是末行"), "{error}");
}

/// 中间行的 NUL 填充（非整行 NUL、也不是末行）：这一类**无损**，因此允许修复。
#[test]
fn repair_allows_nul_filler_on_a_middle_line() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let padded = nul_pad(7, r#"{"b":2}"#);
    let text = format!("{}\n{padded}\n{}\n", r#"{"a":1}"#, r#"{"c":3}"#);
    let repair = repair_jsonl_for_copy(&text, own)
        .expect("中间行的 NUL 填充必须可无损修复")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(2usize, 7usize)]);
    assert_eq!(
        repair.text,
        format!("{}\n{}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#)
    );
    // 修复后记录数必须与严格校验的独立统计一致（沿用现有防御性校验）。
    let normalized = normalize_jsonl(&repair.text, own).expect("修复后的正文必须能通过严格校验");
    assert_eq!(normalized.record_count, 3);
    assert_eq!(repair.text.lines().count(), normalized.record_count);
}

/// 只有填充、没有任何记录的文件：修复后仍是空内容 → 拒绝（不得复制出空会话）。
#[test]
fn repair_rejects_a_body_made_only_of_nul_filler() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let filler = "\0".repeat(32);
    let filler_line = format!("{filler}\n");
    assert!(repair_jsonl_for_copy(&filler, own).is_err());
    assert!(repair_jsonl_for_copy(&filler_line, own).is_err());
}

/// 干净正文 → 无需修复。
#[test]
fn repair_is_noop_for_valid_body_without_filler() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    assert!(repair_jsonl_for_copy(&sample_body(own), own)
        .unwrap()
        .is_none());
}
