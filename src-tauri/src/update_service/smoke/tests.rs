//! `smoke.rs` 的单测：闸门矩阵、整参相等边界、日志契约、退出码映射、时间格式。
//!
//! 硬约束：**不触发真实网络 / 安装 / 窗口**。所有用例只碰纯函数与本地临时文件，
//! 因此可以在 CI 的任何平台上跑（含 macOS 的 `cargo test --lib`）。

use super::*;

// ---------------------------------------------------------------------------
// 闸门矩阵（设计 §3.3 的 6 种组合）
// ---------------------------------------------------------------------------

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn gate_opens_only_with_arg_env_and_no_demo() {
    let with_arg = args(&["--hidden", "--smoke-self-update"]);

    let opened = gate(&with_arg, Some("1"), false);
    assert_eq!(
        opened,
        Gate {
            arg: true,
            env: true,
            demo: false
        }
    );
    assert!(opened.is_open(), "三条闸门都满足时必须放行");
    assert!(opened.blocked_by().is_empty());
}

#[test]
fn gate_stays_closed_when_any_condition_missing() {
    let with_arg = args(&["--smoke-self-update"]);
    let without_arg = args(&["--hidden"]);

    // 1) 普通启动：无参数无 env
    let plain = gate(&without_arg, None, false);
    assert!(!plain.is_open());
    assert_eq!(plain.blocked_by(), vec!["arg", "env"]);

    // 2) 只有参数（普通用户误加）
    let arg_only = gate(&with_arg, None, false);
    assert!(!arg_only.is_open());
    assert_eq!(arg_only.blocked_by(), vec!["env"]);

    // 3) 只有 env（环境变量残留）
    let env_only = gate(&without_arg, Some("1"), false);
    assert!(!env_only.is_open());
    assert_eq!(env_only.blocked_by(), vec!["arg"]);

    // 4) 双闸门 + 截图演示模式：演示态禁止真实网络/安装，必须否决
    let demo = gate(&with_arg, Some("1"), true);
    assert!(!demo.is_open());
    assert_eq!(demo.blocked_by(), vec!["demo"]);

    // 5) env 值不是 "1" 的近似写法都不算满足
    for value in ["", "0", "true", "yes", " 1", "1 "] {
        assert!(
            !gate(&with_arg, Some(value), false).is_open(),
            "env={value:?} 不该放行"
        );
    }
    // 未设置（None）同样不放行
    assert!(!gate(&with_arg, None, false).is_open());
}

/// 默认启动零行为变化：`maybe_spawn` 的第一条语句是闸门判定，而普通启动的
/// `(args, env, demo)` 组合在上面已证明 `is_open() == false`，因此它会立刻 return。
/// 这里把「普通启动的参数形态」逐个钉住，防止将来有人把闸门改成子串匹配。
#[test]
fn default_startup_combinations_never_open_the_gate() {
    let ordinary: [&[&str]; 6] = [
        &[],
        &["--hidden"],
        &["--smoke-self-update-x"],
        &["--smoke-self-update=1"],
        &["--no-smoke-self-update"],
        &["--smoke", "self-update"],
    ];
    for case in ordinary {
        let decision = gate(&args(case), None, false);
        assert!(!decision.is_open(), "普通启动形态 {case:?} 不应打开闸门");
    }
    // 截图演示模式无论如何都不放行
    for case in [&["--smoke-self-update"][..], &["--hidden"][..]] {
        assert!(
            !gate(&args(case), Some("1"), true).is_open(),
            "演示模式 {case:?} 不应打开闸门"
        );
    }
}

// ---------------------------------------------------------------------------
// 整参相等（禁止子串匹配）
// ---------------------------------------------------------------------------

#[test]
fn exact_arg_matching_rejects_prefixes_and_suffixes() {
    assert!(has_exact_arg([SELF_UPDATE_ARG]));
    assert!(has_exact_arg(["--hidden", SELF_UPDATE_ARG]));
    assert!(has_exact_arg([SELF_UPDATE_ARG.to_string()]));
    assert!(has_exact_arg(["x", "--smoke-self-update"]));

    for value in [
        "--smoke-self-update-x",
        "--smoke-self-update=1",
        "--smoke-self-updateX",
        "x--smoke-self-update",
        "--no-smoke-self-update",
        "--smoke-self-update ",
        " --smoke-self-update",
        "--SMOKE-SELF-UPDATE",
    ] {
        assert!(!has_exact_arg([value]), "{value:?} 不应命中整参相等");
    }
    assert!(!has_exact_arg(Vec::<String>::new()));
}

// ---------------------------------------------------------------------------
// phase 词表
// ---------------------------------------------------------------------------

#[test]
fn phase_vocabulary_is_fixed_and_ordered() {
    // 与设计 §3.1.3 一一对应；新增词必须同时改文档与 assert-update-log.py。
    let expected = [
        "start",
        "check",
        "check_ok",
        "up_to_date",
        "download_start",
        "download_progress",
        "download_done",
        "install_start",
        "install_ok",
        "relaunch",
        "done",
        "error",
        "abort",
    ];
    assert_eq!(PHASES.len(), expected.len(), "词表条数变化必须同步文档");
    assert_eq!(PHASES.to_vec(), expected.to_vec());

    // 每个导出的常量都在词表里，且不重复。
    for phase in [
        PHASE_START,
        PHASE_CHECK,
        PHASE_CHECK_OK,
        PHASE_UP_TO_DATE,
        PHASE_DOWNLOAD_START,
        PHASE_DOWNLOAD_PROGRESS,
        PHASE_DOWNLOAD_DONE,
        PHASE_INSTALL_START,
        PHASE_INSTALL_OK,
        PHASE_RELAUNCH,
        PHASE_DONE,
        PHASE_ERROR,
        PHASE_ABORT,
    ] {
        assert!(PHASES.contains(&phase), "{phase} 不在词表里");
    }
    let mut sorted = PHASES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), PHASES.len(), "词表不能有重复项");
}

// ---------------------------------------------------------------------------
// 退出码映射
// ---------------------------------------------------------------------------

#[test]
fn exit_codes_match_design_table() {
    // 设计 §3.1.4。CI 只用它做失败归类，不能重复、不能改值（driver 里写死了这些数字）。
    let table = [
        (EXIT_OK, 0),
        (EXIT_UP_TO_DATE, 10),
        (EXIT_CHECK_FAILED, 11),
        (EXIT_DOWNLOAD_FAILED, 12),
        (EXIT_INSTALL_FAILED, 13),
        (EXIT_UNAVAILABLE, 14),
        (EXIT_UNSUPPORTED_PLATFORM, 20),
    ];
    for (actual, expected) in table {
        assert_eq!(actual, expected);
    }
    let mut codes: Vec<i32> = table.iter().map(|(code, _)| *code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), table.len(), "退出码不能重复");
}

// ---------------------------------------------------------------------------
// 日志：字段齐全 + 序列化可解析 + 错误路径不 panic
// ---------------------------------------------------------------------------

/// 临时日志路径（按用例名隔离，避免并行测试互相踩）。
fn temp_log(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "wb-shift-smoke-test-{}-{name}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn read_lines(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).expect("日志应可读");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("每行必须是合法 JSON"))
        .collect()
}

#[test]
fn log_line_has_every_contract_field() {
    let path = temp_log("fields");
    let mut log = SmokeLog::open(&path).expect("应能创建日志");

    log.write(
        PHASE_START,
        None,
        None,
        None,
        Some(json!({ "logPath": path.display().to_string() })),
    );
    log.write(
        PHASE_CHECK_OK,
        Some("0.2.5"),
        Some("https://github.com/o/r/releases/latest"),
        None,
        None,
    );
    log.write(PHASE_ERROR, Some("0.2.5"), None, Some("签名校验失败"), None);

    let records = read_lines(&path);
    assert_eq!(records.len(), 3, "三行都要落盘（每行 flush）");

    for (index, record) in records.iter().enumerate() {
        let object = record.as_object().expect("必须是 JSON 对象");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "currentVersion",
                "detail",
                "error",
                "phase",
                "pid",
                "seq",
                "ts",
                "url",
                "version",
            ],
            "第 {index} 行字段集合不符（契约见设计 §3.1.3）"
        );
        assert_eq!(
            object["seq"].as_u64(),
            Some(index as u64),
            "seq 必须从 0 单调递增"
        );
        assert_eq!(object["pid"].as_u64(), Some(u64::from(std::process::id())));
        assert_eq!(
            object["currentVersion"].as_str(),
            Some(APP_VERSION),
            "currentVersion 必须等于本进程版本"
        );
        assert!(
            object["ts"].as_str().is_some_and(|ts| ts.ends_with('Z')),
            "ts 必须是 ISO 8601 UTC"
        );
    }

    // 首行：start 事件的 version/url/error 都是 null，detail 带 logPath。
    assert_eq!(records[0]["phase"], json!(PHASE_START));
    assert_eq!(records[0]["version"], json!(null));
    assert_eq!(records[0]["error"], json!(null));
    assert!(records[0]["detail"]["logPath"].is_string());

    // 第二行：check_ok 带目标版本与下载页地址。
    assert_eq!(records[1]["phase"], json!(PHASE_CHECK_OK));
    assert_eq!(records[1]["version"], json!("0.2.5"));
    assert!(records[1]["url"].is_string());

    // 第三行：错误行 error 非 null —— 错误路径必须写 phase=error 且不 panic。
    assert_eq!(records[2]["phase"], json!(PHASE_ERROR));
    assert_eq!(records[2]["error"], json!("签名校验失败"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn log_appends_across_reopen_and_detects_finished() {
    let path = temp_log("append");
    {
        let mut log = SmokeLog::open(&path).expect("首次打开");
        log.write(PHASE_INSTALL_START, Some("0.2.5"), None, None, None);
        assert!(!already_finished(&path), "只有 install_start 不算终态");
    }
    {
        // 模拟「安装完成 → 进程重启 → 新进程重新打开同一个日志」。
        let mut log = SmokeLog::open(&path).expect("再次打开");
        log.write(PHASE_INSTALL_OK, Some("0.2.5"), None, None, None);
        log.write(PHASE_DONE, Some("0.2.5"), None, None, None);
    }
    assert!(already_finished(&path), "有 install_ok/done 即视为已跑完");
    let records = read_lines(&path);
    assert_eq!(records.len(), 3, "追加语义：不覆盖已有行");
    assert_eq!(records[2]["phase"], json!(PHASE_DONE));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_log_file_is_not_reported_as_finished() {
    let path = temp_log("missing");
    assert!(
        !already_finished(&path),
        "日志不存在时必须返回 false（否则新进程会误判已跑完）"
    );
}

/// 日志目录不可写 → `SmokeLog::open` 返回 `Err`（调用方据此 exit 14），不 panic。
#[test]
fn unwritable_log_path_returns_error_without_panicking() {
    // 把「文件」当目录用：任何平台都会让 create_dir_all / open 失败。
    let mut path = std::env::temp_dir();
    path.push(format!("wb-shift-smoke-blocker-{}.bin", std::process::id()));
    std::fs::write(&path, b"x").expect("写入占位文件");
    let target = path.join("nested").join("smoke.jsonl");
    assert!(
        SmokeLog::open(&target).is_err(),
        "不可写路径必须返回 Err 而不是 panic"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// 时间格式（CI 按 ts 排序，格式错了会让断言失效）
// ---------------------------------------------------------------------------

#[test]
fn iso8601_matches_known_instants() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(1), "1970-01-01T00:00:01Z");
    // 2023-11-14T22:13:20Z（GitHub API 文档里常见的示例时间）
    assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    // 2024-02-29T00:00:00Z：闰年 2 月 29 日必须存在
    assert_eq!(iso8601_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    // 2000-03-01T12:34:56Z：跨世纪闰年（2000 是闰年）
    assert_eq!(iso8601_utc(951_914_096), "2000-03-01T12:34:56Z");
    // 负值（时钟异常）不 panic，按 div_euclid 语义落到 1969 年。
    assert_eq!(iso8601_utc(-1), "1969-12-31T23:59:59Z");
}

#[test]
fn iso8601_is_monotonic_and_round_trips_today() {
    let now = now_epoch_secs();
    assert!(now > 1_700_000_000, "系统时钟明显异常：{now}");
    let text = iso8601_utc(now);
    assert_eq!(text.len(), 20, "YYYY-MM-DDTHH:MM:SSZ 固定 20 字符：{text}");
    assert!(text.ends_with('Z'));
    assert!(iso8601_utc(now + 1) > text, "同一秒内的字符串比较应单调");
}

// ---------------------------------------------------------------------------
// 平台白名单
// ---------------------------------------------------------------------------

#[test]
fn platform_gate_matches_desktop_targets() {
    // 本仓库只在这三个平台产出桌面安装包；其它目标应走 exit 20。
    let expected = cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ));
    assert_eq!(platform_supported(), expected);
}
