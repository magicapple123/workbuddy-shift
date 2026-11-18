/// 归一只动末行的 `status` 值与 `_meta` 留痕：其它行、嵌套同名键、CRLF 都不动。
#[test]
fn normalize_last_incomplete_preserves_unrelated_bytes() {
    let head = r#"{"type":"user","sessionId":"c1","text":"正文里提到 \"status\": \"incomplete\" 也不算"}"#;
    let tail = r#"{"type":"message","role":"assistant","status":"incomplete","content":[{"type":"output_text","status":"incomplete","text":"x"}],"_meta":{"stage":"x","at":"t0"}}"#;
    let text = format!("{head}\r\n{tail}\r\n");

    let out = normalize_last_incomplete(&text, "20250101-000000", "copy normalize")
        .expect("末条 incomplete 必须被归一");
    assert!(out.ends_with("\r\n"), "行尾换行风格必须保留：{out:?}");
    let lines: Vec<&str> = out.trim_end_matches(['\r', '\n']).split("\r\n").collect();
    assert_eq!(lines.len(), 2, "不得增删行");
    assert_eq!(lines[0], head, "非末行必须逐字不变");

    let last: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(last["status"], "completed");
    assert_eq!(
        last["content"][0]["status"], "incomplete",
        "嵌套里的同名 status 不得被改"
    );
    assert_eq!(last["_meta"]["stage"], "x", "已有的 _meta 必须合并保留");
    assert_eq!(
        last["_meta"]["incompleteNormalized"]["at"],
        "20250101-000000"
    );
    assert_eq!(
        last["_meta"]["incompleteNormalized"]["reason"],
        "copy normalize"
    );

    // 幂等：已经归一过的正文再归一一次不产生任何变化。
    assert!(normalize_last_incomplete(&out, "20250101-000000", "copy normalize").is_none());
    // 正文里没有 assistant 记录时不动（用户消息里的 status 文本诱导不了它）。
    let no_assistant = format!(
        "{}\r\n{}\r\n",
        head, r#"{"type":"function_call_result","output":"ok"}"#
    );
    assert!(
        normalize_last_incomplete(&no_assistant, "20250101-000000", "copy normalize").is_none()
    );
}

/// 最后一条 assistant 消息后面还跟着 tool 行：仍要归一那一行，tool 行逐字不变。
///
/// 定位口径是「最后一条 assistant 消息」而不是「最后一条记录」：一轮结束后正文
/// 常跟 `function_call_result`，客户端照样显示「正在执行工具」。留痕必须插在
/// assistant 那一行自己的 `}` 前，不能跑到文件末尾的 tool 行上。
#[test]
fn normalize_last_incomplete_reaches_past_trailing_tool_rows() {
    let head = r#"{"type":"user","sessionId":"c1","text":"跑一下"}"#;
    let assistant = r#"{"type":"message","role":"assistant","status":"incomplete","content":[{"type":"output_text","text":"Interrupted by user"}],"_meta":{"stage":"x"}}"#;
    let tool =
        r#"{"type":"function_call_result","sessionId":"c1","name":"read_file","output":"ok"}"#;
    let text = format!("{head}\r\n{assistant}\r\n{tool}\r\n");

    let out = normalize_last_incomplete(&text, "20250102-030405", "copy normalize")
        .expect("尾部跟 tool 行时也必须归一最后一条 assistant 消息");
    let lines: Vec<&str> = out.trim_end_matches(['\r', '\n']).split("\r\n").collect();
    assert_eq!(lines.len(), 3, "不得增删行");
    assert_eq!(lines[0], head, "前置行必须逐字不变");
    assert_eq!(lines[2], tool, "尾部 tool 行必须逐字不变");
    assert!(!lines[2].contains("_meta"), "留痕不能跑到 tool 行上");

    let patched: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(patched["status"], "completed");
    assert_eq!(patched["_meta"]["stage"], "x", "已有 _meta 必须合并保留");
    assert_eq!(
        patched["_meta"]["incompleteNormalized"]["at"],
        "20250102-030405"
    );
    assert_eq!(
        patched["_meta"]["incompleteNormalized"]["reason"],
        "copy normalize"
    );
    assert_eq!(
        patched["content"],
        serde_json::json!([{"type": "output_text", "text": "Interrupted by user"}]),
        "正文内容不得被改写"
    );

    // 幂等：已归一过的正文不会再被改一次。
    assert!(normalize_last_incomplete(&out, "20250102-030405", "copy normalize").is_none());
}

/// `role = "assistant"` 但 `type != "message"` 的记录不能当候选（与存量脚本口径一致）。
///
/// 真实数据里目前没有，但 `{"type":"event","role":"assistant","status":"incomplete"}`
/// 一旦排在真正的末条消息之后，只看 role 就会改到错误的那一行。
#[test]
fn normalize_last_incomplete_skips_non_message_assistant_rows() {
    let message = r#"{"type":"message","role":"assistant","status":"incomplete","content":[{"type":"output_text","text":"Interrupted by user"}],"sessionId":"c1"}"#;
    let event = r#"{"type":"event","role":"assistant","status":"incomplete","sessionId":"c1"}"#;
    let text = format!("{message}\n{event}\n");

    let out = normalize_last_incomplete(&text, "20250103-000000", "copy normalize")
        .expect("必须命中前面那条 type=message 的 assistant 消息");
    let lines: Vec<&str> = out.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 2, "不得增删行");
    assert_eq!(
        lines[1], event,
        "非 message 记录必须逐字不变（它才是文件末行）"
    );
    let patched: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        patched["status"], "completed",
        "被归一的必须是 message 那一行"
    );
    assert_eq!(
        patched["_meta"]["incompleteNormalized"]["reason"],
        "copy normalize"
    );

    // 正文里只有非 message 的 assistant 记录 → 一律不动。
    let only_event = format!("{event}\n");
    assert!(
        normalize_last_incomplete(&only_event, "20250103-000000", "copy normalize").is_none()
    );
}

// -----------------------------------------------------------------------
// 内容版本
// -----------------------------------------------------------------------

/// 归一化只替换本副本自己的 sessionId；其它 sessionId 保留 → 产生保守差异。
#[test]
fn normalize_replaces_only_own_session_id() {
    let other = "11111111-2222-3333-4444-555555555555";
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    let left = format!(
        "{}\n",
        format_args!(r#"{{"sessionId":"{own}","parentSessionId":"{other}"}}"#)
    );
    let right = format!(
        "{}\n",
        format_args!(
            r#"{{"sessionId":"{own}","parentSessionId":"99999999-8888-7777-6666-555555555555"}}"#
        )
    );
    let left = normalize_jsonl(&left, own).unwrap();
    let right = normalize_jsonl(&right, own).unwrap();
    assert_ne!(
        left.total_digest, right.total_digest,
        "第二个 sessionId 必须保留并参与摘要"
    );

    // 只有本副本 id 不同 → 归一化后完全一致。
    let copy = format!(
        "{}\n",
        format_args!(r#"{{"sessionId":"{other}","text":"x"}}"#)
    );
    let original = format!(
        "{}\n",
        format_args!(r#"{{"sessionId":"{own}","text":"x"}}"#)
    );
    let copy = normalize_jsonl(&copy, other).unwrap();
    let original = normalize_jsonl(&original, own).unwrap();
    assert_eq!(copy.total_digest, original.total_digest);
    assert_eq!(copy.line_digests, original.line_digests);
}

/// 顺序与重复次数参与摘要，不能按 id 去重。
#[test]
fn normalize_keeps_order_and_duplicates() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let line_a = r#"{"sessionId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","text":"a"}"#;
    let line_b = r#"{"sessionId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","text":"b"}"#;

    let ab = normalize_jsonl(&format!("{line_a}\n{line_b}\n"), own).unwrap();
    let ba = normalize_jsonl(&format!("{line_b}\n{line_a}\n"), own).unwrap();
    assert_ne!(ab.total_digest, ba.total_digest, "重排必须产生不同摘要");

    let duplicated = normalize_jsonl(&format!("{line_a}\n{line_a}\n{line_b}\n"), own).unwrap();
    assert_eq!(duplicated.record_count, 3);
    assert_ne!(ab.total_digest, duplicated.total_digest);
    // 重复行不去重：逐行摘要允许出现重复项。
    assert_eq!(duplicated.line_digests[0], duplicated.line_digests[1]);
}

/// 键序变化是保守差异（本工具不重排用户内容）。
#[test]
fn normalize_treats_key_order_change_as_difference() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let a = normalize_jsonl(&format!(r#"{{"sessionId":"{own}","text":"x"}}"#), own).unwrap();
    let b = normalize_jsonl(&format!(r#"{{"text":"x","sessionId":"{own}"}}"#), own).unwrap();
    assert_ne!(a.total_digest, b.total_digest, "键序变化应保守视为差异");
}

/// 空 / 非法 / 截断一律 Unavailable；文件缺失才是 Missing。
#[test]
fn content_state_rejects_empty_invalid_and_truncated() {
    let dir = TempDir::new("content");
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    let missing = dir.path().join("missing.jsonl");
    assert_eq!(read_content_snapshot(&missing, own), ContentState::Missing);

    for (name, text) in [
        ("empty", ""),
        ("blank", "   \n"),
        ("not-json", "hello\n"),
        ("truncated", "{\"sessionId\":\"x\"\n"),
        ("blank-line", "{\"a\":1}\n\n{\"b\":2}\n"),
    ] {
        let path = dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&path, text).unwrap();
        assert!(
            matches!(
                read_content_snapshot(&path, own),
                ContentState::Unavailable(_)
            ),
            "{name} 必须判为不可验证"
        );
    }

    let ok = dir.path().join("ok.jsonl");
    std::fs::write(&ok, sample_body(own)).unwrap();
    assert!(matches!(
        read_content_snapshot(&ok, own),
        ContentState::Ready(_)
    ));

    // 末尾多几个换行不算截断；中间空行仍算异常。
    let trailing = dir.path().join("trailing.jsonl");
    std::fs::write(&trailing, format!("{}\n\n", sample_body(own).trim_end())).unwrap();
    match read_content_snapshot(&trailing, own) {
        ContentState::Ready(snapshot) => assert_eq!(snapshot.normalized.record_count, 2),
        other => panic!("末尾换行应容忍，实际 {other:?}"),
    }
}

/// 全文摘要覆盖空白变化；归一化摘要不受空白影响。
#[test]
fn full_digest_covers_raw_bytes_while_normalized_ignores_spacing() {
    let own = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let base = format!(r#"{{"sessionId":"{own}","text":"x"}}"#);
    let padded = format!("{{ \"sessionId\":\"{own}\", \"text\":\"x\" }}");
    let a = normalize_jsonl(&format!("{base}\n"), own).unwrap();
    let b = normalize_jsonl(&format!("{padded}\n"), own).unwrap();
    assert_ne!(a.total_digest, b.total_digest);
    assert_ne!(
        full_digest_of(base.as_bytes()),
        full_digest_of(padded.as_bytes())
    );
    assert_eq!(
        full_digest_of(base.as_bytes()),
        full_digest_of(base.as_bytes())
    );
}

/// 摘要算法与归一化版本固定：值变化必须显式提升 normalizationVersion。
///
/// 总摘要的**域前缀**为 `workbuddy-shift-lines-v1`，不做前缀回退：本产品使用
/// 全新数据目录 `~/.workbuddy-shift`，不存在需要兼容的旧摘要基线；行摘要口径未变。
#[test]
fn digest_is_stable_for_fixed_content() {
    let normalized = normalize_jsonl("{\"a\":1}\n", "sess").unwrap();
    assert_eq!(normalized.record_count, 1);
    assert_eq!(NORMALIZATION_VERSION, 1);
    assert_eq!(
        normalized.line_digests[0],
        "37b3e6066c98ad57f90d4c840524b677383e4bc21f1e562e1426b753eb1e08f8"
    );
    assert_eq!(
        normalized.total_digest,
        "163df63dfa31cbc1b4bd8ff318c6beb2f82ee7b83eb7c35885d093bd48cd6828"
    );
    // 总摘要可由行摘要重算，篡改后的基线因此自曝不一致。
    assert_eq!(
        total_digest_of(&normalized.line_digests),
        normalized.total_digest
    );
    assert_ne!(
        total_digest_of(&["00".to_string()]),
        normalized.total_digest
    );
    assert_eq!(
        full_digest_of(b"{\"a\":1}\n"),
        "e346432021b04179518d9614f3560ccd71354a4ee101ddcb893d6959a9d6301c"
    );
}

#[test]
fn ordered_prefix_helpers() {
    let base = vec!["a".to_string(), "b".to_string()];
    let extended = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let reordered = vec!["b".to_string(), "a".to_string()];
    assert!(is_ordered_prefix(&base, &extended));
    assert!(is_ordered_prefix(&base, &base));
    assert!(!is_ordered_prefix(&extended, &base));
    assert!(!is_ordered_prefix(&reordered, &extended));
    assert!(is_strict_ordered_extension(&base, &extended));
    assert!(!is_strict_ordered_extension(&base, &base));
}
