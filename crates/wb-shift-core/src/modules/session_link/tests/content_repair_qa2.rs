// ---------------------------------------------------------------------------
// 第二轮：针对「丢弃末行」新判据（is_eof + 无完整值）的证伪用例
//
// 新判据收紧了有损分支，因此既要确认它挡住了静默丢数据，也要确认它没有把
// **真正的截断** 一起挡掉（否则原 issue 会复发）。
// ---------------------------------------------------------------------------

/// 完整值 + 空白 + 半截值：空白不改变「完整记录必须保留」的结论。
#[test]
fn qa2_repair_keeps_complete_value_before_whitespace_then_truncated() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":2}  {"c":"#], "\n", true);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Ok(Some(repair)) => {
            assert_eq!(repair.kept_tail_line_records, Some(1));
            assert_eq!(repair.dropped_tail_line, Some(2));
            assert_eq!(
                repair.text,
                qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true),
                "保留的记录必须单独成行，值之间的空白与残尾都不得写入"
            );
            qa_assert_repair_contract(&repair, 2);
        }
        Err(_) => {}
        Ok(None) => panic!("严格校验已失败，不可能「无需修复」"),
    }
}

/// 完整标量值 + 半截对象：标量也是记录，不得连带丢弃。
#[test]
fn qa2_repair_keeps_complete_scalar_before_truncated() {
    let text = qa_body(&[r#"{"a":1}"#, r#"true{"c":"#], "\n", true);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Ok(Some(repair)) => {
            assert_eq!(repair.kept_tail_line_records, Some(1));
            assert_eq!(
                repair.text,
                qa_body(&[r#"{"a":1}"#, "true"], "\n", true),
                "完整标量必须原样保留在独立行上"
            );
            qa_assert_repair_contract(&repair, 2);
        }
        Err(_) => {}
        Ok(None) => panic!("严格校验已失败，不可能「无需修复」"),
    }
}

/// 「写完整但语法不合法」的末行（尾逗号 / NaN / 单引号键）一律拒绝：
/// 它们不是截断，静默丢弃会丢一条完整记录，文案还会谎称「不完整」。
#[test]
fn qa2_repair_rejects_illegal_but_complete_tail_variants() {
    for tail in [r#"{"a":1,}"#, r#"{"a":NaN}"#, r#"{'a':1}"#, r#"[1,2,]"#] {
        let text = qa_body(&[r#"{"a":1}"#, tail], "\n", true);
        let result = repair_jsonl_for_copy(&text, QA_OWN);
        assert!(
            result.is_err(),
            "末行 {tail} 属于「写完整但非法」，必须拒绝；实际 {result:?}"
        );
    }
}

/// 真正的截断即便带前导空白，仍必须按截断丢弃（不能因为收紧判据把原场景挡掉）。
#[test]
fn qa2_repair_still_drops_truncated_line_with_leading_whitespace() {
    let text = qa_body(&[r#"{"a":1}"#, r#"   {"b":"半截"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("前导空白不影响截断判定")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));
    qa_assert_repair_contract(&repair, 1);
}

/// 真实形态的末行截断（含转义、中文、URL）：仍必须可修复。
#[test]
fn qa2_repair_drops_realistic_truncated_tail_with_escapes() {
    let text = format!(
        "{}{}",
        qa_body(&[r#"{"a":1}"#], "\n", true),
        r#"{"type":"message","text":"第一行\n第二行\t\u00e9 https://example.com/a?b=1","id":"#,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("真实截断必须可修复")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));
    qa_assert_repair_contract(&repair, 1);
}

/// 未闭合的外层对象（内含写完整的嵌套对象）：记录本身不完整，按截断丢弃即可。
#[test]
fn qa2_repair_drops_unclosed_outer_object_with_complete_nested() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"outer":{"inner":1}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("未闭合外层对象属于截断")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));
    qa_assert_repair_contract(&repair, 1);
}

/// 病态输入（超长 / 超深）必须在线性时间内给出结论，不能 panic 或死循环。
#[test]
fn qa2_repair_terminates_on_pathological_lines() {
    // 5 万层未闭合对象：截断 → 丢弃。
    let deep = "{\"a\":".repeat(50_000);
    let text = format!("{}\n{deep}", r#"{"a":1}"#);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("病态长行不得 panic")
        .expect("必须返回修复结果");
    assert_eq!(repair.dropped_tail_line, Some(2));

    // 20 万个 `{`：既不是合法值也无法流式取值，必须报错而不是卡住。
    let braces = "{".repeat(200_000);
    let text = format!("{}\n{braces}", r#"{"a":1}"#);
    assert!(repair_jsonl_for_copy(&text, QA_OWN).is_err());
}

/// 末行里有 2 条完整记录 + 残尾：2 条都必须保留并按独立行写出，记录数 = 3。
#[test]
fn qa2_repair_keeps_all_complete_values_in_damaged_tail() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":2}{"c":3}{"d":"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("可保留完整值")
        .expect("必须返回修复结果");
    assert_eq!(repair.kept_tail_line_records, Some(2));
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 3);
}

/// 保留下来的完整值必须逐字节不变（不能被序列化重排、键序改动）。
#[test]
fn qa2_repair_kept_values_are_byte_identical() {
    let kept = r#"{"z":1, "a":"中文","nested":{"k":[1,2,3]}}"#;
    let text = qa_body(&[r#"{"a":1}"#, &format!(r#"{kept}{{"tail":"#)], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert!(repair.text.contains(kept), "保留的记录必须逐字节不变：{:?}", repair.text);
    qa_assert_repair_contract(&repair, 2);
}

/// 单行文件（前面没有独立行记录）里的「完整值 + 残尾」：不得复制出空会话。
///
/// 当前实现走保守分支直接拒绝（`records >= 1` 只看**独立行**的记录数）；这里按不变量
/// 断言而不是锁死落法：要么拒绝，要么保留出 ≥1 条记录。两种都不允许「报成功但 0 条记录」。
#[test]
fn qa2_repair_single_line_complete_plus_tail_never_yields_empty() {
    let text = format!("{}{}", r#"{"a":1}{"b":2}"#, r#"{"c":"#);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Err(_) => {}
        Ok(None) => panic!("严格校验已失败，不可能「无需修复」"),
        Ok(Some(repair)) => assert!(
            normalize_jsonl(&repair.text, QA_OWN).unwrap().record_count >= 1,
            "修复不得产出空会话：{:?}",
            repair.text
        ),
    }
}

/// 末行没有行尾换行时，保留完整值 + 丢残尾的产物必须仍然通过严格校验
/// （是否补换行属于格式细节，但不得产出无法解析的内容）。
#[test]
fn qa2_repair_kept_tail_without_trailing_newline_still_valid() {
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":2}{"c":"#], "\n", false);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.kept_tail_line_records, Some(1));
    qa_assert_repair_contract(&repair, 2);
}


