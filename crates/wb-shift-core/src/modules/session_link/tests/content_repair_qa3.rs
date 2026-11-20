// ---------------------------------------------------------------------------
// 第三轮 QA 独立回归：裸 NUL 填充（issue #1 第三类形态）
//
// 断言只写契约（PRD/设计里的三类可修复形态 + 「必须拒绝」清单），不复述实现内部判定顺序。
// 重点是把「无损」与「有损」分开证伪：
//   * 无损（剥离填充 / 粘连拆行）→ 记录内容逐字节不变、与位置无关；
//   * 有损（末行截断）→ 只允许末行 + 前面已有完整记录；
//   * 其余形态 → 必须报错，不允许「悄悄修好」。
// ---------------------------------------------------------------------------

/// `count` 个裸 NUL + `tail`：线上形态是「行首填充 + 一条完整 JSON」。
fn qa3_pad(count: usize, tail: &str) -> String {
    let mut out = "\0".repeat(count);
    out.push_str(tail);
    out
}

/// 剥离 NUL 后保留下来的记录必须**逐字节**不变。
///
/// 键序、中文、emoji、转义序列、嵌套数组、浮点写法、URL 里的 `&`/`?` 全部参与比对：
/// 任何「重新序列化」或「按键重排」都会让副本与源会话悄悄分叉。
#[test]
fn qa3_nul_strip_keeps_record_bytes_identical() {
    let record = concat!(
        r#"{"z":1, "a":"中文😀","nested":{"k":[1,2,{"m":null}]},"f":1.50,"#,
        r#""e":"\u00e9\n\t\"q\"","u":"https://example.com/a?b=1&c=2","big":12345678901234567890}"#
    );
    let text = qa_body(&[&qa3_pad(9, record), r#"{"b":2}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN)
        .expect("行首 NUL 填充属于无损修复，不得报错")
        .expect("必须给出修复结果");
    assert_eq!(repair.nul_stripped, vec![(1usize, 9usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[record, r#"{"b":2}"#], "\n", true),
        "剥离填充后的记录必须逐字节不变"
    );
    qa_assert_repair_contract(&repair, 2);
}

/// 无损修复不得谎报「丢记录」或「粘连」：`dropped_tail_line` / `split_lines` 必须为空。
#[test]
fn qa3_nul_strip_does_not_claim_split_or_drop() {
    let text = qa_body(&[r#"{"a":1}"#, &qa3_pad(4, r#"{"b":2}"#)], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.dropped_tail_line, None, "剥离填充不得报「已跳过」");
    assert_eq!(repair.kept_tail_line_records, None);
    assert!(repair.split_lines.is_empty(), "填充不是粘连");
    assert_eq!(repair.nul_stripped, vec![(2usize, 4usize)]);
}

/// 位置无关：首行 / 中间行 / 末行（含末行无换行）都必须能修。
///
/// 这是与「末行截断」的关键差异：截断有损、只允许末行；剥离填充无损、与位置无关。
#[test]
fn qa3_nul_strip_is_position_independent() {
    let padded = qa3_pad(6, r#"{"pad":0}"#);
    // 首行
    let text = qa_body(&[&padded, r#"{"b":2}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(1usize, 6usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"pad":0}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);

    // 中间行
    let text = qa_body(&[r#"{"a":1}"#, &padded, r#"{"b":2}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(2usize, 6usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"pad":0}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 3);

    // 末行（带换行）
    let text = qa_body(&[r#"{"a":1}"#, &padded], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(2usize, 6usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"pad":0}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);

    // 末行（无换行）
    let text = format!("{}\n{}", r#"{"a":1}"#, qa3_pad(6, r#"{"pad":0}"#));
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(
        repair.text,
        format!("{}\n{}", r#"{"a":1}"#, r#"{"pad":0}"#),
        "末行没有换行时不得凭空补一个"
    );
    qa_assert_repair_contract(&repair, 2);
}

/// NUL 把一条记录**劈成两半**：两段都不是完整值 → 必须拒绝（不得拼接猜测）。
///
/// 末行位置同样要拒绝：这里的形态不是「没写完」，而是 NUL 落在记录内部，
/// 任何补法都可能把一条记录缝成两条或反过来。
#[test]
fn qa3_nul_rejects_record_split_in_half() {
    let splits = [
        format!("{}\0{}", r#"{"a":1"#, r#","b":2}"#),
        format!("{}\0\0\0{}", r#"{"sessionId":"x","text":"he"#, r#"llo"}"#),
        format!("{}\0{}", r#"{"outer":{"inner":"#, r#"v"}}"#),
    ];
    for split in splits {
        // 中间行
        let text = qa_body(&[r#"{"x":0}"#, &split, r#"{"y":9}"#], "\n", true);
        let error = repair_jsonl_for_copy(&text, QA_OWN).unwrap_err();
        assert!(
            error.contains("第 2 行"),
            "必须指出坏行行号（{split:?}）：{error}"
        );
        // 末行（前面已有完整记录）也不得被当作截断悄悄丢掉
        let text = qa_body(&[r#"{"x":0}"#, &split], "\n", true);
        assert!(
            repair_jsonl_for_copy(&text, QA_OWN).is_err(),
            "NUL 劈开的记录在末行也不得被修复：{split:?}"
        );
    }
}

/// NUL 段内是垃圾文本（`hello` / `garbage` / 单引号键）→ 必须拒绝。
#[test]
fn qa3_nul_rejects_garbage_segment() {
    let bodies = [
        format!("{}\0hello\0{}", qa3_pad(2, ""), r#"{"a":1}"#),
        format!("{}\0garbage", r#"{"a":1}"#),
        format!("{}\0{}", r#"{"a":1}"#, r#"{'b':2}"#),
        format!("{}\0{}", r#"{"a":1}"#, r#"{"b":NaN}"#),
    ];
    for body in bodies {
        let text = qa_body(&[&body, r#"{"c":3}"#], "\n", true);
        assert!(
            repair_jsonl_for_copy(&text, QA_OWN).is_err(),
            "段内垃圾不得被猜测修补：{body:?}"
        );
    }
}

/// 「NUL + 截断尾巴」不在末行 → 必须拒绝；在末行 → 才允许按末行规则处理。
#[test]
fn qa3_nul_rejects_truncated_tail_on_non_last_line() {
    let truncated = qa3_pad(4, r#"{"b":"半"#);
    let text = qa_body(&[&truncated, r#"{"c":3}"#], "\n", true);
    let error = repair_jsonl_for_copy(&text, QA_OWN).expect_err("非末行的截断不得被修复");
    assert!(error.contains("第 1 行"), "{error}");
    assert!(error.contains("不是末行"), "必须说明不是末行：{error}");

    // 末行 + 前面已有完整记录：按末行规则（整行都是半截 → 丢弃该行）。
    let text = qa_body(&[r#"{"a":1}"#, &truncated], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(repair.kept_tail_line_records, None, "整行都是半截，没有完整值");
    assert_eq!(repair.nul_stripped, vec![(2usize, 4usize)]);
    assert_eq!(repair.text, qa_body(&[r#"{"a":1}"#], "\n", true));
    qa_assert_repair_contract(&repair, 1);

    // 前面没有任何完整记录（只有填充行）：不得产出空会话。
    let text = format!("{}\n{truncated}", qa3_pad(3, ""));
    assert!(
        repair_jsonl_for_copy(&text, QA_OWN).is_err(),
        "只有填充 + 截断时不得复制出 0 条记录"
    );
}

/// 整份正文只有 NUL 填充 → 必须拒绝（不得产出空副本）。
#[test]
fn qa3_nul_rejects_body_made_only_of_filler() {
    let cases = [
        "\0".repeat(4096),
        format!("{}\n", "\0".repeat(4096)),
        "\0\0\n\0\0\n".to_string(),
        "\0  \0".to_string(),
    ];
    for body in cases {
        assert!(
            repair_jsonl_for_copy(&body, QA_OWN).is_err(),
            "只有填充的正文不得被修复成空会话：{body:?}"
        );
    }
}

/// 整行 NUL 位于文件中间：连行尾一起移除，前后记录都保留，**不得留下空行**。
#[test]
fn qa3_nul_only_line_is_removed_without_leaving_blank_line() {
    let filler = "\0".repeat(9);
    for (label, ending) in [("LF", "\n"), ("CRLF", "\r\n")] {
        // 中间行
        let text = qa_body(&[r#"{"a":1}"#, &filler, r#"{"b":2}"#], ending, true);
        let repair = repair_jsonl_for_copy(&text, QA_OWN)
            .unwrap_or_else(|error| panic!("整行 NUL 必须可移除（{label}）：{error}"))
            .unwrap();
        assert_eq!(repair.nul_stripped, vec![(2usize, 9usize)], "{label}");
        assert_eq!(
            repair.text,
            qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], ending, true),
            "{label}：移除整行填充后不得留下空行"
        );
        assert!(
            !repair.text.contains(&format!("{ending}{ending}")),
            "{label}：不得出现连续两个行尾（那是一个空行）：{:?}",
            repair.text
        );
        qa_assert_repair_contract(&repair, 2);

        // 首行
        let text = qa_body(&[&filler, r#"{"a":1}"#], ending, true);
        let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
        assert_eq!(
            repair.text,
            qa_body(&[r#"{"a":1}"#], ending, true),
            "{label}：首行填充移除后行号与行尾都要正常"
        );

        // 末行
        let text = qa_body(&[r#"{"a":1}"#, &filler], ending, true);
        let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
        assert_eq!(repair.text, qa_body(&[r#"{"a":1}"#], ending, true), "{label}");
    }
}

/// CRLF 正文里 NUL 行是 CRLF 结尾：修复后行尾风格不被改写（不出现裸 LF）。
#[test]
fn qa3_nul_strip_preserves_crlf_endings() {
    let text = qa_body(
        &[r#"{"a":1}"#, &qa3_pad(5, r#"{"b":2}"#), r#"{"c":3}"#],
        "\r\n",
        true,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\r\n", true),
        "剥离填充不得把 CRLF 改成 LF"
    );
    assert_eq!(
        repair.text.matches('\r').count(),
        repair.text.matches('\n').count(),
        "不得出现裸 LF：{:?}",
        repair.text
    );
    qa_assert_repair_contract(&repair, 3);
}

/// 末行「NUL + 完整值 + 残尾」：完整值必须保留（不得连带丢弃）。
#[test]
fn qa3_nul_truncated_last_line_keeps_complete_values() {
    let tail = qa3_pad(4, r#"{"b":2}{"c":"半"#);
    let text = qa_body(&[r#"{"a":1}"#, &tail], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.dropped_tail_line, Some(2));
    assert_eq!(
        repair.kept_tail_line_records,
        Some(1),
        "同行的完整记录必须保留并如实上报"
    );
    assert_eq!(repair.nul_stripped, vec![(2usize, 4usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);
}

/// 同一份正文里同时有 NUL 填充、粘连、末行截断：三类都要生效，行号与条数各自准确。
#[test]
fn qa3_nul_mixed_with_glued_and_truncated() {
    let text = qa_body(
        &[
            &qa3_pad(7, r#"{"a":1}"#),
            r#"{"b":2}{"c":3}"#,
            r#"{"d":"半截"#,
        ],
        "\n",
        true,
    );
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(1usize, 7usize)]);
    assert_eq!(repair.split_lines, vec![(2usize, 2usize)]);
    assert_eq!(repair.dropped_tail_line, Some(3));
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 3);
}

/// 同一行里既有 NUL 填充、又有粘连：修复结果必须无损（记录数不增不减）。
///
/// 已知不精确（不影响数据，只影响报告）：段内的粘连展开目前只计入 `nul_stripped`，
/// 不会同时写进 `split_lines`，因此报告只说「剥离了填充」，没提「两条记录被拆开」。
/// 这里锁「无损」这个硬约束，不固定报告口径。
#[test]
fn qa3_nul_and_glued_on_the_same_line_is_lossless() {
    let text = qa_body(&[r#"{"a":1}"#, &qa3_pad(2, r#"{"b":2}{"c":3}"#)], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(2usize, 2usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#], "\n", true),
        "同一行里的两条粘连记录必须拆开且内容不丢"
    );
    qa_assert_repair_contract(&repair, 3);
}

/// 病态输入必须在线性时间内给出结论：10 万个填充 + 尾部 JSON、1MB 全 NUL 行都不 panic。
#[test]
fn qa3_nul_filler_large_input_is_linear() {
    let started = std::time::Instant::now();
    // 一行 10 万个 NUL + 尾部 JSON。
    let text = format!("{}\n{}\n", r#"{"a":1}"#, qa3_pad(100_000, r#"{"b":2}"#));
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(2usize, 100_000usize)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);

    // 一行 1MB 全 NUL（中间行）：整行移除，不 panic。
    let huge = "\0".repeat(1024 * 1024);
    let text = format!("{}\n{huge}\n{}\n", r#"{"a":1}"#, r#"{"b":2}"#);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert_eq!(repair.nul_stripped, vec![(2usize, 1024 * 1024)]);
    assert_eq!(
        repair.text,
        qa_body(&[r#"{"a":1}"#, r#"{"b":2}"#], "\n", true)
    );
    qa_assert_repair_contract(&repair, 2);

    // 1MB 全 NUL 的单行文件：不得被修成空副本。
    assert!(repair_jsonl_for_copy(&huge, QA_OWN).is_err());
    // 二次方行为会在这里直接超时（debug 构建下 20 秒仍是非常宽松的上界）。
    assert!(
        started.elapsed().as_secs() < 20,
        "病态填充行的处理耗时 {:?}，疑似非线性",
        started.elapsed()
    );
}

/// `nul_stripped` 只在**真的剥离**了填充时出现，别的形态不得带上它。
#[test]
fn qa3_nul_stripped_only_when_filler_really_stripped() {
    // 干净正文：不需要修复。
    assert!(repair_jsonl_for_copy(&sample_body(QA_OWN), QA_OWN)
        .unwrap()
        .is_none());
    // 粘连：只有 split_lines。
    let text = qa_body(&[r#"{"a":1}{"b":2}"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert!(repair.nul_stripped.is_empty(), "粘连不得报成填充");
    assert_eq!(repair.split_lines, vec![(1usize, 2usize)]);
    // 末行截断：只有 dropped_tail_line。
    let text = qa_body(&[r#"{"a":1}"#, r#"{"b":"半"#], "\n", true);
    let repair = repair_jsonl_for_copy(&text, QA_OWN).unwrap().unwrap();
    assert!(repair.nul_stripped.is_empty(), "截断不得报成填充");
    assert_eq!(repair.dropped_tail_line, Some(2));
}

/// 已知限制（记录形态边界）：NUL 落在**标量**记录内部时，两段都能独立解析成合法值，
/// 实现无法区分「两条标量记录」与「一条被劈开的标量记录」。
///
/// 真实会话的每条记录都是 JSON 对象，因此这里按「不 panic、不丢字节」锁住底线，
/// 不把这种病态输入当成必须拒绝的形态（拒绝与否都不可证伪）。
#[test]
fn qa3_known_limit_scalar_record_split_by_nul() {
    let text = qa_body(&[r#"{"a":1}"#, "12\0 34"], "\n", true);
    match repair_jsonl_for_copy(&text, QA_OWN) {
        Ok(Some(repair)) => {
            assert!(
                repair.text.contains("12") && repair.text.contains("34"),
                "被劈开的标量字节不得丢失：{:?}",
                repair.text
            );
            assert!(repair.text.contains(r#"{"a":1}"#), "对象记录必须保留");
        }
        Ok(None) => panic!("严格校验已失败，不可能「无需修复」"),
        Err(_) => {}
    }
}
