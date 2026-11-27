//! 诊断包的行为测试：脱敏不泄漏、形状稳定、落盘校验。
//!
//! 全部用临时目录与手写数据，**不读真实数据目录**（`WB_SHIFT_HOME` 也不依赖）：
//! 只有 [`probe_store_writable`](super::report::probe_store_writable) 这类生产专用探测
//! 不在测试路径上。

use serde_json::json;
use std::path::{Path, PathBuf};

use super::report::{
    build_report, save_report, ClientProbe, HookState, HostForm, ReportInput, ReportSources,
    DEFAULT_LOG_LINES, MAX_REPORT_BYTES,
};
use super::{account_bucket, redact_text};

/// 临时目录 + `error.log` 路径。
fn temp_log(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-diagnostics-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let path = dir.join("error.log");
    (dir, path)
}

fn write_log(path: &Path, lines: &[serde_json::Value]) {
    let text = lines
        .iter()
        .map(|line| serde_json::to_string(line).expect("日志行可序列化"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{text}\n")).expect("写入临时日志");
}

fn input() -> ReportInput {
    ReportInput {
        host_form: HostForm::Desktop,
        locale: "zh-CN".to_string(),
        account_count: 3,
        clients: vec![
            ClientProbe::new("WorkBuddy", true, Some("1.2.3".to_string())),
            ClientProbe::new("CodeBuddy CLI", false, None),
        ],
    }
}

/// `report` 的固定形状：分段标题、宿主标识、账号区间都在。
#[test]
fn report_contains_expected_sections() {
    let (dir, log) = temp_log("shape");
    write_log(
        &log,
        &[json!({
            "ts": 1_700_000_000_123i64,
            "kind": "backend",
            "message": "切换失败",
            "detail": "",
            "appVersion": "0.9.3",
            "os": "macos",
        })],
    );

    let report = build_report(
        &input(),
        &ReportSources {
            error_log: log,
            error_log_limit: DEFAULT_LOG_LINES,
            settings: Some(json!({"configs": {"checkin": {"enabled": true, "days": 7}}})),
            hook: Some(HookState {
                installed: true,
                registered_targets: 1,
                total_targets: 3,
            }),
            store_writable: Some(true),
            generated_at_ms: 1_700_000_000_123,
        },
    );

    assert!(report.starts_with("WorkBuddy Shift 诊断包"));
    for needle in [
        "[环境]",
        "[账号]",
        "[客户端]",
        "[权限]",
        "[限额 hook]",
        "[设置项（只有键名与类型，不含取值）]",
        "[最近错误日志]",
        "宿主标识: desktop",
        "账号数量: 3 个（区间 1-5）",
        "WorkBuddy: 已安装（版本 1.2.3）",
        "CodeBuddy CLI: 未安装",
        "已注册配置: 1/3 处",
        "enabled: boolean",
        "days: integer",
        // 毫秒时间戳渲染成人能读的 UTC，而不是一串会被脱敏掉的数字。
        "2023-11-14",
    ] {
        assert!(report.contains(needle), "报告缺少 {needle}：\n{report}");
    }

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// **核心回归**：往 `error.log` 与设置里注入真实形态的令牌 / 邮箱 / 路径 / uid，
/// 断言脱敏输出里**一个原文片段都不出现**，同时键名与类型仍然可读。
#[test]
fn report_never_leaks_secrets_from_log_or_settings() {
    let (dir, log) = temp_log("secrets");
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlLWFiYw";
    let email = "alex.chen@example.com";
    let home_path = "/Users/alex/Library/Application Support/WorkBuddy/auth.json";
    let win_path = r"C:\Users\alex\AppData\Roaming\WorkBuddy\info.json";
    let uuid = "3f2a1b4c-5d6e-7f80-9a1b-2c3d4e5f6071";
    let gateway = "https://user:pass@gateway.internal/proxy/secret-path?token=abc";

    write_log(
        &log,
        &[json!({
            "ts": 1_700_000_000_123i64,
            "kind": "backend",
            "message": format!("读取 {home_path} 失败（{email}）"),
            "detail": format!("Authorization: {jwt}\n写入 {win_path}\nsession {uuid}\nPOST {gateway}"),
            "appVersion": "0.9.3",
            "os": "macos",
        })],
    );

    let settings = json!({
        "version": 1,
        "exportedAt": 1_700_000_000_123i64,
        "configs": {
            "checkin": {"enabled": true, "lastUid": 1_700_000_000_123i64},
            "travel": {"note": email, "path": home_path},
            "rotate": {"token": jwt},
            "rateLimit": {"threshold": 1_700_000_000_123i64},
        }
    });

    let report = build_report(
        &input(),
        &ReportSources {
            error_log: log,
            error_log_limit: DEFAULT_LOG_LINES,
            settings: Some(settings),
            hook: Some(HookState {
                installed: false,
                registered_targets: 0,
                total_targets: 3,
            }),
            store_writable: Some(false),
            generated_at_ms: 1_700_000_000_123,
        },
    );

    for secret in [
        jwt,
        email,
        home_path,
        win_path,
        uuid,
        gateway,
        // URL 的 scheme + host 是**刻意保留**的（用户据此判断是哪家网关），
        // 但 userinfo 里的口令与路径必须抹掉。
        "user:pass",
        "secret-path",
        "alex.chen",
        "Application Support",
        "1700000000123",
    ] {
        assert!(
            !report.contains(secret),
            "诊断包泄漏了 {secret}：\n{report}"
        );
    }

    // 脱敏不能把报告抹成空白：键名与类型这类「无机密」信息必须保留。
    for needle in [
        "checkin",
        "enabled: boolean",
        "threshold: integer",
        "token: string",
    ] {
        assert!(report.contains(needle), "报告缺少 {needle}：\n{report}");
    }
    // 报告头必须自证「已脱敏」并给出替换计数。
    assert!(report.contains("脱敏: 已替换 "), "\n{report}");

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 读不到日志 / 读不到设置时也要出一份完整报告，只是对应分段写一行说明。
#[test]
fn report_survives_missing_log_and_settings() {
    let report = build_report(
        &ReportInput::new(HostForm::WebUi, "en-US", 0),
        &ReportSources {
            error_log: std::env::temp_dir().join("workbuddy-shift-does-not-exist.log"),
            error_log_limit: DEFAULT_LOG_LINES,
            settings: None,
            hook: None,
            store_writable: None,
            generated_at_ms: 0,
        },
    );
    assert!(report.contains("（没有日志文件，或读不出来）"));
    assert!(report.contains("（读取失败）"));
    assert!(report.contains("未探测"));
    assert!(report.contains("（未提供探测结果）"));
    assert!(report.contains("宿主标识: webui"));
    assert!(report.contains("账号数量: 0 个（区间 0）"));
}

/// 日志条数上限生效：只回看最后 N 条（最老的那条不出现）。
#[test]
fn report_keeps_only_the_tail_of_the_log() {
    let (dir, log) = temp_log("tail");
    let lines: Vec<serde_json::Value> = (0..5)
        .map(|index| {
            json!({
                "ts": 1_700_000_000_000i64 + index,
                "kind": "backend",
                "message": format!("第 {index} 条"),
                "detail": "",
                "appVersion": "0.9.3",
                "os": "macos",
            })
        })
        .collect();
    write_log(&log, &lines);

    let report = build_report(
        &input(),
        &ReportSources {
            error_log: log,
            error_log_limit: 2,
            settings: None,
            hook: None,
            store_writable: Some(true),
            generated_at_ms: 0,
        },
    );
    assert!(report.contains("第 4 条"));
    assert!(report.contains("第 3 条"));
    assert!(!report.contains("第 0 条"));
    assert!(!report.contains("第 1 条"));

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 账号数量分桶（报告里只出现区间，不出明细）。
#[test]
fn account_bucket_covers_boundaries() {
    assert_eq!(account_bucket(0), "0");
    assert_eq!(account_bucket(1), "1-5");
    assert_eq!(account_bucket(5), "1-5");
    assert_eq!(account_bucket(6), "6-10");
    assert_eq!(account_bucket(11), "11-20");
    assert_eq!(account_bucket(21), "21-50");
    assert_eq!(account_bucket(51), "50+");
}

/// 落盘：扩展名白名单 / 空内容 / 父目录必须存在 / 超限，全部返回可读错误。
#[test]
fn save_report_validates_path_and_size() {
    let (dir, _) = temp_log("save");
    let target = dir.join("diagnostics.txt");

    let saved = save_report(&target.to_string_lossy(), "诊断内容").expect("正常保存");
    assert_eq!(saved["ok"], json!(true));
    assert_eq!(saved["bytes"], json!("诊断内容".len()));
    assert_eq!(std::fs::read_to_string(&target).expect("读回"), "诊断内容");

    // `.md` 也允许（同样是纯文本）。
    assert!(save_report(&dir.join("d.md").to_string_lossy(), "x").is_ok());

    assert!(save_report("", "x").is_err());
    assert!(save_report(&dir.join("d.csv").to_string_lossy(), "x")
        .expect_err("csv 不在白名单")
        .contains(".txt"));
    assert!(save_report(&target.to_string_lossy(), "").is_err());
    assert!(save_report(&dir.join("missing").join("d.txt").to_string_lossy(), "x").is_err());
    let huge = "x".repeat(MAX_REPORT_BYTES + 1);
    assert!(save_report(&dir.join("big.txt").to_string_lossy(), &huge).is_err());

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 脱敏入口对整份报告文本同样成立（防止将来新增分段时忘了调用）。
#[test]
fn redact_text_is_exposed_for_report_level_masking() {
    let masked = redact_text("路径 /Users/alex/x 与 alex@example.com");
    assert!(!masked.contains("/Users/alex"));
    assert!(!masked.contains("alex@example.com"));
}
