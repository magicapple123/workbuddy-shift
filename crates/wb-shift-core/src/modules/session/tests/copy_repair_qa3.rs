// ---------------------------------------------------------------------------
// 第三轮 QA 独立回归：裸 NUL 填充（issue #1 用户实测的第 1000 行）端到端
//
// 只断言用户可见契约：能复制 / 不能复制、报告里的行号与字节数、副本严格可读且逐字节
// 保留记录、源文件一字不改、记录数不增不减。断言不依赖 `content_repair.rs` 的内部字段。
// ---------------------------------------------------------------------------

/// `count` 个裸 NUL + `tail`：复现「行首填充 + 一条完整 JSON」的线上形态。
fn qa3_pad(count: usize, tail: &str) -> String {
    let mut out = "\0".repeat(count);
    out.push_str(tail);
    out
}

/// 5 条记录、第 3 行行首 8 974 字节裸 NUL（issue #1 实测）：复制成功、副本 5 条、
/// 报告说明行号与字节数、源文件逐字节不变、每条记录内容逐字节保留。
#[test]
fn qa3_copy_nul_padded_line_is_stripped_losslessly() {
    let env = ready_env("qa3-repair-nul");
    const FILLER: usize = 8_974;
    let records: Vec<String> = (1..=5)
        .map(|index| {
            let role = if index % 2 == 0 { "assistant" } else { "user" };
            qa_line(
                "sess-1",
                role,
                &format!("第{index}条：中文 + 转义 \\ \"q\" https://example.com/{index}"),
            )
        })
        .collect();
    let mut source = String::new();
    for (offset, record) in records.iter().enumerate() {
        if offset + 1 == 3 {
            source.push_str(&"\0".repeat(FILLER));
        }
        source.push_str(record);
        source.push('\n');
    }
    env.add_body("sess-1", &source);
    let source_bytes = std::fs::read(env.body_path("sess-1")).unwrap();

    // 严格校验语义不变：坏行仍然判为不可验证，容错只发生在复制路径上。
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path("sess-1"), "sess-1"),
            session_link::ContentState::Unavailable(_)
        ),
        "严格校验不得因为新增修复而放宽"
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 3 行"), "必须指出填充所在行号：{note}");
    assert!(
        note.contains(&format!("{FILLER} 字节空填充（NUL）")),
        "必须给出剥离的字节数：{note}"
    );
    assert!(note.contains("已剥离"), "{note}");
    assert!(note.contains("记录内容未丢失"), "{note}");
    assert!(!note.contains("已跳过"), "无损剥离不得报「已跳过」：{note}");
    assert!(!note.contains("粘连"), "填充不得报成粘连：{note}");

    // 副本：5 条记录、引用新 id、无残留 NUL、严格可读；每条记录逐字节保留。
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert!(!copied.contains('\0'), "副本不得残留 NUL：{copied:?}");
    let copied_lines: Vec<&str> = copied.lines().collect();
    assert_eq!(copied_lines.len(), 5, "{copied}");
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            copied_lines[index],
            record.replace("sess-1", &new_id),
            "第 {} 条记录内容必须逐字节保留（只换 id 引用）",
            index + 1
        );
    }
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 5)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    let operation = session_link::scan_operations(&env.paths)
        .operations
        .first()
        .cloned()
        .expect("操作日志必须保留");
    assert_eq!(
        operation.expected_record_count, 5,
        "操作日志的记录数必须来自修复后的正文（不是源文件的 5 行 + 填充）"
    );

    // 源文件逐字节不变（修复只在内存里做，副本另写）。
    assert_eq!(
        std::fs::read(env.body_path("sess-1")).unwrap(),
        source_bytes,
        "源文件必须一个字节都没变"
    );
}

/// NUL 把一条记录劈成两半 → 必须仍然失败，且不写出目标行/正文。
#[test]
fn qa3_copy_nul_split_record_still_fails() {
    let env = ready_env("qa3-repair-nul-split");
    let source = format!(
        "{}\n{}\0\0{}\n{}\n",
        qa_line("sess-1", "user", "一"),
        r#"{"type":"user","sessionId":"sess-1","text":"这条"#,
        r#"被劈开了"}"#,
        qa_line("sess-1", "assistant", "三")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert!(error.contains("第 2 行"), "必须指出坏行行号：{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出目标行");
    assert_eq!(env.body_files().len(), 1, "不得写出目标正文");
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source,
        "源文件不得被改动"
    );
}

/// 修不出来的坏行：最终 error 里同一句诊断只能各出现一次（早先拼了两遍）。
#[test]
fn qa3_copy_error_message_does_not_repeat_diagnosis() {
    let env = ready_env("qa3-repair-dedup");
    let source = format!(
        "{}\nhello\n{}\n",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert_eq!(
        error.matches("第 2 行的格式无法识别").count(),
        1,
        "行诊断不得重复：{error}"
    );
    assert_eq!(
        error.matches("会话内容无法验证").count(),
        1,
        "错误外壳不得重复：{error}"
    );
    assert!(error.contains("第 2 行"), "{error}");
}

/// 严格校验的首个坏行**可修复**、真正修不出来的行在后面：复制必须仍然失败，
/// 且两处坏行的行诊断都要出现（不同句不得互相吞掉，同一句也不得重复）。
///
/// 判据是「两句是不是同一句」，不是「有没有修复层诊断」：这里第 1 行是粘连（修复层
/// 拆得开），第 2 行 `hello` 修不出来。若把修复层诊断无条件丢掉，用户会被告知去修一个
/// 本来可修的行，真正的阻断点从文案里消失；反之若只看 `contains` 就丢，结论同样错。
/// 实测文案两处行号都在：
/// 「会话内容无法验证（第 1 行的格式无法识别（第 50 列，trailing characters；该行
/// 103 字节；不是末行），内容可能不完整），未复制；第 2 行的格式无法识别（第 1 列，
/// expected value；该行 5 字节；不是末行），内容可能不完整」。
#[test]
fn qa3_copy_repairable_first_error_plus_unfixable_later_still_fails_once() {
    let env = ready_env("qa3-repair-mixed-error");
    let source = format!(
        "{}{}\nhello\n{}\n",
        qa_line("sess-1", "user", "一"),
        qa_line("sess-1", "assistant", "二"),
        qa_line("sess-1", "user", "三")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert_eq!(
        error.matches("会话内容无法验证").count(),
        1,
        "错误外壳不得重复：{error}"
    );
    assert_eq!(
        error.matches("的格式无法识别").count(),
        2,
        "两处坏行必须各报一句（同一句不重复，不同句不吞掉）：{error}"
    );
    assert!(
        error.contains("第 1 行"),
        "首个（可修复的）坏行行号必须保留：{error}"
    );
    assert!(
        error.contains("第 2 行"),
        "真正阻断的坏行行号必须可见：{error}"
    );
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出目标行");
    assert_eq!(env.body_files().len(), 1, "不得写出目标正文");
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source,
        "源文件不得被改动"
    );
}

/// 同一份正文里同时有 NUL 填充、粘连、末行截断：三类都要生效，报告说明齐全，记录数正确。
#[test]
fn qa3_copy_nul_glued_truncated_combination_reports_all() {
    let env = ready_env("qa3-repair-mixed");
    let first = qa_line("sess-1", "user", "一");
    let second = qa_line("sess-1", "assistant", "二");
    let third = qa_line("sess-1", "user", "三");
    let source = format!(
        "{}\n{}{}\n{}\n{}",
        qa3_pad(3, &first),
        // 第 2 行：两条记录粘连（客户端退出时没写换行）。
        second,
        third,
        qa_line("sess-1", "assistant", "四"),
        // 第 4 行：末行截断。
        r#"{"type":"user","sessionId":"sess-1","text":"五没写完"#
    );
    env.add_body("sess-1", &source);
    let source_bytes = std::fs::read(env.body_path("sess-1")).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(
        note.contains("第 1 行有 3 字节空填充（NUL），已剥离"),
        "{note}"
    );
    assert!(note.contains("第 2 行有 2 条记录粘连"), "{note}");
    assert!(note.contains("第 4 行"), "必须指出被跳过的末行行号：{note}");
    assert!(note.contains("已跳过"), "{note}");

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert!(!copied.contains('\0'), "{copied:?}");
    assert_eq!(copied.lines().count(), 4, "{copied}");
    assert_eq!(
        std::fs::read(env.body_path("sess-1")).unwrap(),
        source_bytes,
        "源文件必须一个字节都没变"
    );
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 4, "{copied}")
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
}

/// 整行 NUL 位于文件中间：复制成功，前后记录都保留，副本里**不得留下空行**。
#[test]
fn qa3_copy_nul_only_line_in_middle_succeeds_without_blank_line() {
    let env = ready_env("qa3-repair-nul-blank");
    let source = format!(
        "{}\n{}\n{}\n",
        qa_line("sess-1", "user", "一"),
        "\0".repeat(64),
        qa_line("sess-1", "assistant", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        note, "源会话第 2 行有 64 字节空填充（NUL），已剥离（记录内容未丢失）",
        "整行填充的文案必须说清行号与字节数（前端直接展示）"
    );

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "不得留下空行：{copied:?}");
    assert!(!copied.contains("\n\n"), "不得出现空行：{copied:?}");
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 2)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
}

/// 整份正文只有 NUL 填充 → 必须仍然失败（不能复制出空会话）。
#[test]
fn qa3_copy_body_of_only_nul_filler_still_fails() {
    let env = ready_env("qa3-repair-only-filler");
    env.add_body("sess-1", &format!("{}\n", "\0".repeat(4096)));

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出目标行");
    assert_eq!(env.body_files().len(), 1, "不得写出目标正文");
}

/// 修复与既有「末条 incomplete 归一」必须能叠加：被填充撑大的那行若是最后一条
/// assistant 消息，复制时仍要按原口径把 `status` 归一为 `completed` 并留痕。
///
/// 这条用例盯的是两条路径的接缝：修复层产出的是逐字节原文，归一化层必须还能在它上面
/// 定位并改写；反过来，归一化不得让记录数变化（否则写后校验会失败）。
#[test]
fn qa3_copy_nul_repaired_body_still_normalizes_incomplete_tail() {
    let env = ready_env("qa3-repair-incomplete");
    let assistant = json!({
        "type": "message",
        "role": "assistant",
        "sessionId": "sess-1",
        "status": "incomplete",
        "text": "被中断的一轮",
    })
    .to_string();
    let source = format!(
        "{}\n{}\n{}\n",
        qa_line("sess-1", "user", "一"),
        qa3_pad(5, &assistant),
        qa_line("sess-1", "user", "二")
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert!(!copied.contains('\0'), "副本不得残留 NUL：{copied:?}");
    assert!(
        copied.contains("\"status\":\"completed\""),
        "末条 assistant 消息仍要归一：{copied}"
    );
    assert!(copied.contains("incompleteNormalized"), "{copied}");
    assert!(copied.contains(&new_id) && !copied.contains("sess-1"), "{copied}");
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 3, "{copied}")
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
}
