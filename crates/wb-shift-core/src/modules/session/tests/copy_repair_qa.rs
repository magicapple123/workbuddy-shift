// ---------------------------------------------------------------------------
// QA 独立回归：复制路径的容错修复（issue #1）端到端
//
// 只断言用户可见契约：能复制 / 不能复制、报告里的行号、目标副本严格可读、源文件不变、
// 记录数不增不减。断言独立于实现（不看 `content_repair.rs` 内部字段）。
// ---------------------------------------------------------------------------

/// 一条完整记录（`sessionId` 由调用方指定）。
fn qa_line(cid: &str, role: &str, text: &str) -> String {
    json!({"type": role, "sessionId": cid, "text": text}).to_string()
}

/// 复现线上形态：2 条完整记录 + 第 3 行半截 → 必须复制成功并报告行号。
#[test]
fn qa_copy_truncated_tail_succeeds_and_keeps_source_intact() {
    let env = ready_env("qa-repair-tail");
    let source = format!(
        "{}\n{}\n{}",
        qa_line("sess-1", "user", "你好"),
        qa_line("sess-1", "assistant", "hi"),
        r#"{"type":"user","sessionId":"sess-1","text":"半"#
    );
    env.add_body("sess-1", &source);
    let source_bytes = std::fs::read(env.body_path("sess-1")).unwrap();

    // 严格校验必须仍然拒绝：修复只发生在复制路径上。
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path("sess-1"), "sess-1"),
            session_link::ContentState::Unavailable(_)
        ),
        "严格校验语义不得放宽"
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        note, "源会话末尾有 1 条不完整记录（第 3 行），已跳过",
        "整行丢弃的文案必须逐字稳定（前端直接展示）"
    );

    // 目标副本：2 条记录、引用新 id、能通过严格校验。
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
    assert!(copied.contains(&new_id), "{copied}");
    assert!(!copied.contains("sess-1"), "{copied}");
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 2);
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
    // 数据库行与操作记录同步：期望记录数 = 修复后的记录数（不是源文件的 3）。
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    let operation = session_link::scan_operations(&env.paths)
        .operations
        .first()
        .cloned()
        .expect("操作日志必须保留");
    assert_eq!(
        operation.expected_record_count, 2,
        "操作日志的记录数必须来自修复后的正文"
    );

    // 源文件逐字节不变：不信报告，直接读文件内容比对（含长度与坏行仍在原处）。
    let source_after = std::fs::read(env.body_path("sess-1")).unwrap();
    assert_eq!(source_after, source_bytes, "源文件必须一个字节都没变");
    assert_eq!(source_after.len(), source.len(), "源文件长度必须不变");
    let source_text = String::from_utf8(source_after).expect("源文件仍是 UTF-8");
    assert!(
        source_text.ends_with(r#""text":"半"#),
        "源文件里必须仍保留那截坏行（没有被就地修复）：{source_text:?}"
    );
    assert_eq!(
        source_text,
        format!(
            "{}\n{}\n{}",
            qa_line("sess-1", "user", "你好"),
            qa_line("sess-1", "assistant", "hi"),
            r#"{"type":"user","sessionId":"sess-1","text":"半"#
        )
    );
}

/// 整份文件只有一条不完整记录 → 必须仍然失败（不能复制出空会话）。
#[test]
fn qa_copy_single_truncated_record_still_fails() {
    let env = ready_env("qa-repair-only-broken");
    env.add_body("sess-1", r#"{"type":"user","sessionId":"sess-1","text":"半"#);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出目标行");
    assert_eq!(env.body_files().len(), 1, "不得写出目标正文");
}

/// CRLF 源文件里一行粘连了 3 条记录 → 拆成 3 条，行尾风格保留，内容不丢。
#[test]
fn qa_copy_glued_crlf_records_are_split_without_loss() {
    let env = ready_env("qa-repair-glued-crlf");
    let source = format!(
        "{}{}{}\r\n{}\r\n",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二"),
        qa_line("sess-1", "user", "三"),
        qa_line("sess-1", "assistant", "四"),
    );
    env.add_body("sess-1", &source);
    let source_bytes = std::fs::read(env.body_path("sess-1")).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        note, "源会话第 1 行有 3 条记录粘连，已拆分为独立记录（内容未丢失）",
        "无损拆分的文案必须说清条数且声明内容未丢失"
    );

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.matches("\r\n").count(), 4, "CRLF 行尾必须保留：{copied:?}");
    let lines: Vec<&str> = copied.split("\r\n").filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 4, "{copied:?}");
    for line in &lines {
        serde_json::from_str::<serde::de::IgnoredAny>(line)
            .unwrap_or_else(|error| panic!("拆出的每一行都必须能独立解析（{error}）：{line}"));
        assert!(line.contains(&new_id), "引用必须换成新 id：{line}");
    }
    assert_eq!(std::fs::read(env.body_path("sess-1")).unwrap(), source_bytes);
}

/// CRLF 源文件末行截断 → 复制成功，目标仍是 CRLF、2 条记录。
#[test]
fn qa_copy_truncated_tail_with_crlf_body() {
    let env = ready_env("qa-repair-tail-crlf");
    let source = format!(
        "{}\r\n{}\r\n{}",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二"),
        r#"{"type":"user","sessionId":"sess-1","text":"三"#
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.matches("\r\n").count(), 2, "{copied:?}");
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 2)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
}

/// 中间行是纯垃圾 → 必须仍然失败（不得悄悄修好）。
#[test]
fn qa_copy_middle_garbage_line_still_fails() {
    let env = ready_env("qa-repair-garbage-middle");
    let source = format!(
        "{}\nhello\n{}",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert!(error.contains("第 2 行"), "必须指出坏行行号：{error}");
}

/// 中间空行 → 必须仍然失败。
#[test]
fn qa_copy_middle_empty_line_still_fails() {
    let env = ready_env("qa-repair-empty-middle");
    let source = format!(
        "{}\n\n{}",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("第 2 行"), "{error}");
}

/// 末行「完整记录 + 非法尾部」→ 必须失败。
///
/// 设计只允许两种修复：粘连拆行（无损）与末行截断丢弃（丢的是**没写完**的那条）。
/// 这一形态里第 3 行已经写完整，丢弃它等于悄悄丢掉一条完整记录，而报告还会说
/// 「末尾有 1 条不完整记录，已跳过」——用户与后续排障都会被误导。
#[test]
fn qa_copy_complete_record_with_garbage_tail_must_fail() {
    let env = ready_env("qa-repair-garbage-tail");
    let source = format!(
        "{}\n{}garbage",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let copied = report["copied"].as_array().map(Vec::len).unwrap_or(0);
    let note = report["copied"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["sourceRepaired"].as_str())
        .unwrap_or_default();
    // 失败信息必须能直接看出「丢了几条完整记录」，便于定位数据损失。
    let lost = if copied == 0 {
        0
    } else {
        let new_id = env.first_copy_id(&report);
        let body = std::fs::read_to_string(env.body_path(&new_id)).unwrap_or_default();
        let target_records = session_link::normalize_jsonl(&body, &new_id)
            .map(|normalized| normalized.record_count)
            .unwrap_or(0);
        2 - target_records
    };
    assert_eq!(
        copied, 0,
        "「完整记录 + 非法尾部」不属于任何一类可修复形态，必须拒绝；实际报告：{report}（修复说明：{note}，目标副本丢了 {lost} 条完整记录）"
    );
}

/// 干净源会话复制：报告里不得出现 `sourceRepaired`。
#[test]
fn qa_copy_clean_source_has_no_repair_note() {
    let env = ready_env("qa-repair-clean");
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(
        report["copied"][0].get("sourceRepaired").is_none(),
        "干净源会话不得出现修复说明：{report}"
    );
}

/// 第二轮：末行「写完整但语法不合法」（尾逗号）必须仍然失败，且不产生副本与目标行。
///
/// 这类行不是截断，静默丢弃会丢一条完整记录；用户必须看到真实损坏并自行处理。
#[test]
fn qa2_copy_illegal_but_complete_tail_still_fails() {
    let env = ready_env("qa2-repair-trailing-comma");
    let source = format!(
        "{}\n{},",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert!(error.contains("第 2 行"), "必须指出坏行行号：{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出目标行");
    assert_eq!(env.body_files().len(), 1, "不得写出目标正文");
    // 源文件不得被改动。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source
    );
}

/// 第二轮：末行「完整记录 + 残尾」→ 复制成功，完整记录必须保留，报告不得说成整行丢弃。
#[test]
fn qa2_copy_kept_tail_records_are_reported_as_kept() {
    let env = ready_env("qa2-repair-kept-tail");
    let kept = qa_line("sess-1", "assistant", "这条写完了");
    let source = format!(
        "{}\n{}{}",
        qa_line("sess-1", "user", "一"),
        kept,
        r#"{"type":"user","sessionId":"sess-1","text":"这条没写完"#
    );
    env.add_body("sess-1", &source);
    let source_bytes = std::fs::read(env.body_path("sess-1")).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        note, "源会话第 2 行末尾有 1 条不完整记录，已跳过（同行的 1 条完整记录已保留）",
        "「保留 + 丢残尾」的文案必须同时给出丢弃数与保留数（前端直接展示）"
    );

    // 目标副本：2 条记录，被保留的那条内容与源逐字一致（只换了 id 引用）。
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
    assert!(copied.contains("这条写完了"), "{copied}");
    assert!(!copied.contains("这条没写完"), "残尾不得写入副本：{copied}");
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 2)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
    // 源文件逐字节不变（含坏行仍在原处）：直接读文件比对，不依赖报告。
    let source_after = std::fs::read(env.body_path("sess-1")).unwrap();
    assert_eq!(source_after, source_bytes, "源文件必须一个字节都没变");
    let source_text = String::from_utf8(source_after).expect("源文件仍是 UTF-8");
    assert!(
        source_text.contains("这条没写完"),
        "源文件里必须仍保留那截残尾（没有被就地修复）：{source_text:?}"
    );
    assert!(source_text.contains(kept.as_str()), "被保留的记录在源里也必须原样存在");
}


