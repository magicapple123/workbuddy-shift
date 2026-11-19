use super::ledger::cached_scan;
use super::ledger::cli_logs_root;
use super::ledger::cli_state_path;
use super::ledger::invalidate_scan_cache;
use super::ledger::resolve_by_uid;
use super::ledger::save_rate_limit_config_at;
use super::ledger::scan_scope;
use super::ledger::ActiveAccount;
use super::ledger::ScanCache;
use super::ledger::ScanRoots;
use super::ledger::ScanScope;
use super::ledger::ScanSource;
use super::ledger::CLI_ROTATE_DIR;
use super::ledger::CLI_STATE_FILE;
use super::ledger::IDE_STATE_FILE;
use super::ledger::SCAN_CACHE;
use super::ledger::SCAN_MIN_INTERVAL_MS;
use super::parse::ide_conversation_id;
use super::parse::ide_json_model;
use super::parse::json_field;
use super::parse::line_timestamp;
use super::parse::parse_reset_at;
use super::parse::request_pair;
use super::parse::scan_text;
use super::parse::scan_text_scoped;
use super::parse::session_from_file_name;
use super::parse::AuthMarker;
use super::parse::Event;
use super::parse::Hit;
use super::parse::LogFormat;
use super::parse::MODEL_UNKNOWN;
use super::parse::QUOTA_MARKERS;
use super::payload::build_payload;
use super::payload::Resolved;
use super::scan::collect_events;
use super::scan::contains;
use super::scan::dedupe;
use super::scan::ide_log_files;
use super::scan::windowed_log_files;
use super::scan::CLI_DATA_DIR;
use super::scan::DAY_MS;
use super::scan::IDE_LOG_DIR_NAME;
use super::scan::LOG_DIR_NAME;
use super::scan::LONG_LINE_SNIFF_BYTES;
use super::scan::WINDOW_DAYS;
use crate::modules::config::now_ms;
use crate::modules::rate_limit_hook::SETTINGS_FILE_NAME;
use crate::modules::variant::WbVariant;
use crate::modules::vscode_cn_inject::{codebuddy_ide_data_dir, CodeBuddyIdeFlavor};
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// 本机实测的重置时刻：`2026-09-17 17:59:27 UTC+8`。
const CN_RESET_AT: i64 = 1_789_639_167_000;
/// 国际版文案样例的重置时刻：`2026-09-14 10:59:21 UTC+8`。
const EN_RESET_AT: i64 = 1_789_354_761_000;
/// 本机实测的 SDK 行时间戳：`2026-09-16T16:20:31.523Z`。
const SDK_OCCURRED_AT: i64 = 1_789_575_631_523;

const CN_SESSION: &str = "387a486d-b5c9-473d-b823-121db59f0084";
const CN_REQUEST: &str = "34fccb8a1ed74da39dea5481eba85b46";
const EN_SESSION: &str = "9b9130df-3665-4fdc-9ae7-65c23c8f8edd";
const EN_REQUEST: &str = "4d83788eb9de41d08424eb2b64c08bd3";

fn business_line(timestamp: &str, body: &str) -> String {
    format!("[{timestamp}] [Error] [pid=1] {body}")
}

/// SDK 会话日志：模型线索来自 `method:sendPrompt`，会话 id 来自文件名。
///
/// 回归：修复前这类事件被标成「未知模型」（归因只认业务日志的 requestId / resolved model）。
#[test]
fn sdk_send_prompt_supplies_the_model_via_file_session() {
    let session = "b3df1149-8a3f-4d73-b7a4-c71c46e15762";
    let quota = cn_quota_with(session, CN_REQUEST, "2026-09-19 13:49:50");
    let text = [
        "2026-09-18T07:37:17.657Z method:sendPrompt {\"instanceId\":\"ci-3\",\"modelId\":\"hy4-preview-f\"}"
            .to_string(),
        format!("2026-09-18T07:40:22.997Z runtime.applyStopReason {{\"preview\":\"{quota}\"}}"),
    ]
    .join("\n");
    let events = dedupe(scan_text_scoped(
        &text,
        LogFormat::WorkBuddy,
        AuthMarker::None,
        Some(session),
    ));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model.as_deref(), Some("hy4-preview-f"));
    // 不给文件级会话 id 时保持上游行为（未知），避免把别的会话的模型误归因过来。
    let without = dedupe(scan_text(&text, LogFormat::WorkBuddy, AuthMarker::None));
    assert_eq!(without[0].model, None);
}

/// SDK 文件会话身份必须落在 `logs/<日期>/sdk/conversations/<UUID>.log` 形态上：
/// 别的目录下的 UUID 文件名（以及 SDK 目录里的非 UUID 名）都不得当会话 id 用。
#[test]
fn sdk_file_session_identity_requires_the_conversations_path() {
    let uuid = "b3df1149-8a3f-4d73-b7a4-c71c46e15762";
    let conversations = Path::new("/u/.codebuddy/logs/2026-09-18/sdk/conversations");
    assert_eq!(
        session_from_file_name(&conversations.join(format!("{uuid}.log"))).as_deref(),
        Some(uuid),
        "SDK conversations 下的 UUID 文件名就是会话 id"
    );
    for path in [
        // 业务日志（日期目录下）。
        PathBuf::from(format!("/u/.codebuddy/logs/2026-09-18/{uuid}.log")),
        // 少了 `conversations` 一层。
        PathBuf::from(format!("/u/.codebuddy/logs/2026-09-18/sdk/{uuid}.log")),
        // SDK 目录里但不是会话 UUID 的文件名。
        conversations.join("workspace__abc123.log"),
        conversations.join("not-a-uuid.log"),
        // 没有上级目录的裸文件名。
        PathBuf::from(format!("{uuid}.log")),
    ] {
        assert_eq!(
            session_from_file_name(&path),
            None,
            "{path:?} 不得当会话 id"
        );
    }
}

/// 回归 2026-09-20 审查反例 (a)：SDK 的 `modelId` 为 `auto`（未选定模型）时
/// 不得把 `auto` 显示成模型名，也不得覆盖此前已知的模型。
#[test]
fn sdk_auto_is_not_a_model_name() {
    let session = "b3df1149-8a3f-4d73-b7a4-c71c46e15762";
    let quota = format!(
        "2026-09-18T07:40:22.997Z runtime.applyStopReason {{\"preview\":\"{}\"}}",
        cn_quota_with(session, CN_REQUEST, "2026-09-19 13:49:50")
    );
    let sdk_line = |model: &str| {
        format!(
            "2026-09-18T07:37:17.657Z method:sendPrompt {{\"instanceId\":\"ci-3\",\"modelId\":\"{model}\"}}"
        )
    };
    let scan = |text: &str| {
        dedupe(scan_text_scoped(
            text,
            LogFormat::WorkBuddy,
            AuthMarker::None,
            Some(session),
        ))
    };

    // ① 只有 auto 线索：未知模型，不得字面展示 `auto`。
    let only_auto = scan(&[sdk_line("auto"), quota.clone()].join("\n"));
    assert_eq!(only_auto.len(), 1);
    assert_eq!(only_auto[0].model, None, "auto 是未知哨兵，不是模型名");

    // ② auto 出现在已知模型之后：不覆盖，仍然是顺序扫描取到的最近一次**已知**模型。
    let after_known =
        scan(&[sdk_line("hy4-preview-f"), sdk_line("auto"), quota.clone()].join("\n"));
    assert_eq!(after_known.len(), 1);
    assert_eq!(
        after_known[0].model.as_deref(),
        Some("hy4-preview-f"),
        "未知值不得覆盖已知的模型线索"
    );
}

/// 业务日志有两种行首时间戳：12 小时制（`9/17/2026, 12:20:31 AM.232`）与
/// 24 小时制（`2026/9/18 15:46:30.242`，2026-09-18 本机实测）。只认前者时，
/// 后者会被整行跳过 —— 现象是台账始终为空。
#[test]
fn accepts_both_business_timestamp_styles() {
    for ts in ["9/17/2026, 12:20:31 AM.232", "2026/9/18 15:46:30.242"] {
        let events = dedupe(workbuddy_hits(&business_line(ts, &cn_quota())));
        assert_eq!(events.len(), 1, "时间戳 {ts} 未被解析");
    }
}

/// WorkBuddy 限额行没有事件身份（无 `(requestId/sessionId)`、无 `sessionId=`）时
/// 必须丢弃 —— 哪怕前面有分类器行把「当前会话」指向了别处，也不能让回显文案
/// 借道分类器兜底入账（2026-09-18 glm 假 chip 实证）。
#[test]
fn workbuddy_quota_lines_without_event_identity_are_dropped() {
    let text = [
        // 分类器行把「当前会话」指向 glm 会话（修复前回显行借道它入账）。
        "[2026/9/18 20:09:00.000] [Info] [pid=9999] [AgentClassifier] refusal classified category=quota sessionId=aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        &config_line("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "glm-5.3-flash"),
        // 无身份的限额文案行（BashTool 命令回显的截断形态）。
        "[2026/9/18 20:10:00.000] [Info] [pid=9999] [BashTool] execute start | command=\"grep 429 您的使用量已超出频率限制，将在 2026-09-19 13:49:50 UTC+8 重置 logs/\"",
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert!(events.is_empty(), "无身份的限额文案行不得入账");
}

/// 传输行（`[SandboxShell]` 回显 / `ProcessOutput` 载体）里即使嵌着 429 原文，
/// 也不得当成真事件，更不得污染 requestId → model 的归因映射（2026-09-18 glm 假 chip 实证）。
/// 判据只看**外层载体**（见 `is_transport_line`），不看正文里的 `stdout(` / `content=`。
#[test]
fn transport_lines_are_neither_events_nor_model_context() {
    let other_session = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let text = [
        // 真事件：requestId → hy4-preview-f。
        provider_line(CN_REQUEST, "hy4-preview-f"),
        business_line("2026/9/18 15:46:30.242", &cn_quota()),
        // 回显 A：截断的 429 行（requestId 不完整 → 会错落归因到回显会话的模型）。
        // 回显会话此前「刚用过」glm —— 修复前会多出一条 glm 事件。
        config_line(other_session, "glm-5.3-flash"),
        format!(
            "[2026/9/18 20:10:00.000] [Info] [pid=9999] [SandboxShell] ProcessOutput | processId=pipe-1 | stream=stdout | dataLen=1980 | content= 2026-09-19 13:49:50 UTC+8 重置，您也可以切换其他模型继续使用。 ({}/{}",
            &CN_REQUEST[..12], other_session
        ),
        // 回显 B：完整 429 原文整行嵌在 content= 里（会被二次计数）。
        format!(
            "[2026/9/18 20:10:01.000] [Info] [pid=9999] [SandboxShell] ProcessOutput | processId=pipe-2 | stream=stdout | dataLen=900 | content=\"{}\"",
            business_line("2026/9/18 15:46:30.242", &cn_quota())
        ),
        // 回显 C：模型上下文行也被回显 —— 不得写进 requestId → model 映射。
        format!(
            "[2026/9/18 20:10:02.000] [Info] [pid=9999] [SandboxShell] ProcessOutput | processId=pipe-3 | stream=stdout | dataLen=220 | content=[ModelProvider] Sending request: agent=cli, model=glm-5.3-flash, requestId=zzz{}, stream=true",
            &CN_REQUEST[3..]
        ),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert_eq!(events.len(), 1, "回显行不得产生事件");
    assert_eq!(
        events[0].model.as_deref(),
        Some("hy4-preview-f"),
        "归因必须来自真实 Sending request 行，而不是回显"
    );
}

/// 回归 2026-09-20 审查反例：真实限额行的**正文**里出现 `stdout(` / `stderr(` /
/// `content=` 时不得被当成传输行整行丢掉（IDE 的 `Agent execution failed` 带完整请求体，
/// CLI / WorkBuddy 的错误行也会回显工具输出；误删会直接丢真事件）。
#[test]
fn real_quota_lines_mentioning_stdout_are_not_dropped() {
    let workbuddy = business_line(
        "2026/9/18 15:46:30.242",
        &format!(
            "[Error] [Interruption] httpStatus=429, tool echo truncated: stdout(…) stderr(…) content=truncated {}",
            cn_quota()
        ),
    );
    let events = dedupe(workbuddy_hits(&workbuddy));
    assert_eq!(
        events.len(),
        1,
        "正文里的 stdout(/content= 不得让真事件消失"
    );
    assert_eq!(events[0].reset_at, CN_RESET_AT);

    // IDE 的报错行（带完整请求体）同样如此。
    let ide = ide_line(
        "2026-09-17 10:28:26.741",
        &format!(
            "[AgentReporter] [{IDE_TRACE}] Agent execution failed: {{\"statusCode\":429,\"requestBodyValues\":{{\"model\":\"{IDE_MODEL}\"}},\"responseHeaders\":{{\"x-request-id\":\"{IDE_REQUEST}\"}},\"toolOutput\":\"stdout(truncated) stderr(truncated) content=truncated\",\"message\":\"429 {IDE_QUOTA}\"}}"
        ),
    );
    let events = dedupe(scan_text(
        &ide,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events.len(), 1, "IDE 正文里的 stdout( 不得让真事件消失");
    assert_eq!(events[0].reset_at, CN_RESET_AT);
    assert_eq!(events[0].model.as_deref(), Some(IDE_MODEL));
}

/// 回归 2026-09-20 审查：无 `session_id` 不得**统一短路** —— CLI 与 WorkBuddy 共用
/// WorkBuddy 格式，但 CLI 还有日志内 uid / 状态文件归因链，缺事件身份的真实限额仍要入账；
/// 纯 WorkBuddy 两档位没有别的线索，仍按原判据丢弃（见上一条测试）。
#[test]
fn cli_quota_lines_without_event_identity_are_kept() {
    // 行内既没有行尾 `(requestId/sessionId)`，也没有 `sessionId=`。
    let text = [
        cli_auth_line("9/15/2026, 2:07:59 PM.057", UID_A),
        business_line(
            "9/15/2026, 2:08:10 PM.000",
            "429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置，您也可以切换其他模型继续使用。",
        ),
    ]
    .join("\n");

    let cli = dedupe(scan_text(
        &text,
        LogFormat::WorkBuddy,
        AuthMarker::AuthDoInitProbe,
    ));
    assert_eq!(cli.len(), 1, "CLI 缺 session_id 不得被丢弃");
    assert_eq!(cli[0].session_id, None);
    assert_eq!(cli[0].reset_at, CN_RESET_AT);
    assert_eq!(cli[0].uid.as_deref(), Some(UID_A), "uid 归因链照常工作");

    // 同一份文本按 WorkBuddy 两档位解析（无 uid 线索）→ 按原判据丢弃。
    assert!(
        dedupe(workbuddy_hits(&text)).is_empty(),
        "WorkBuddy 两档位无事件身份仍要丢弃"
    );
}

/// 中文限额文案（`session` / `request` / `reset` 可替换，便于构造多条事件）。
fn cn_quota_with(session: &str, request: &str, reset: &str) -> String {
    format!(
        "429 您的使用量已超出频率限制，将在 {reset} UTC+8 重置，您也可以切换其他模型继续使用。 ({request}/{session})"
    )
}

fn cn_quota() -> String {
    cn_quota_with(CN_SESSION, CN_REQUEST, "2026-09-17 17:59:27")
}

fn en_quota() -> String {
    format!(
        "429 usage exceeds frequency limit, please try later, will reset at 2026-09-14 10:59:21 UTC+8, switch to another model. ({EN_REQUEST}/{EN_SESSION})"
    )
}

fn config_line(session: &str, model: &str) -> String {
    format!("[ModelConfig] sessionId={session}, resolved model={model} for agent=cli, requestOptions.model={model}")
}

fn provider_line(request: &str, model: &str) -> String {
    format!(
        "[ModelProvider] Sending request: agent=cli, model={model}, requestId={request}, stream=true"
    )
}

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "workbuddy-shift-limits-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

/// 把文件 mtime 设为 N 天前（收窗测试用；`File::set_modified` 需要可写句柄）。
fn set_days_old_mtime(path: &Path, days: u64) {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("打开文件以设置 mtime")
        .set_modified(old)
        .expect("设置文件 mtime");
}

/// 现有两档位（WorkBuddy 格式、无鉴权行）的解析入口。
fn workbuddy_hits(text: &str) -> Vec<Hit> {
    scan_text(text, LogFormat::WorkBuddy, AuthMarker::None)
}

// -----------------------------------------------------------------------
// CodeBuddy CLI 与两个 IDE 的样本（行骨架取自本机真实日志，id/uid 已替换）
// -----------------------------------------------------------------------

/// IDE 会话 id（`conversationId`，32 位 hex）。
const IDE_CONV: &str = "b3cca2525ba544debd0abc63ca69f8b9";
/// 另一次会话（验证模型归因按会话隔离）。
const IDE_OTHER_CONV: &str = "a12075d16ada4fbba38047b0d879db2b";
const IDE_REQUEST: &str = "0ffa4f2d001e4aee92e1cd6ce6a709c2";
const IDE_TRACE: &str = "8e090575f6032a82163c94dc3d65e9ba";
const IDE_MODEL: &str = "deepseek-v4.1-flash";
const UID_A: &str = "f0ae9eeb-8476-4ef5-9a3e-1d6339d545da";
const UID_B: &str = "2e649415-05c1-4c8a-8736-6b8e3aac0f9a";

/// IDE 限额文案（与研究样本逐字一致）。
const IDE_QUOTA: &str =
    "您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置，您也可以切换其他模型继续使用。";

/// IDE 行首时间格式（本地时间，无方括号）。
fn ide_line(timestamp: &str, body: &str) -> String {
    format!("{timestamp} [error] {body}")
}

fn ide_auth_line(uid: &str) -> String {
    format!(
        "2026-09-17 10:26:01.772 [info] [PulseServiceLifecycle] Auth session changed: hasSession=true, initialized=true, uid={uid}"
    )
}

fn cli_auth_line(timestamp: &str, uid: &str) -> String {
    business_line(
        timestamp,
        &format!(
            "[AuthenticationManager]  [FirstScreen] [AuthDoInitProbe] stage=fastPathBeforeEmit totalMs=591 sinceLastMs=0 uid={uid} hasAccessToken=true"
        ),
    )
}

/// 本机真实事件的行集合：一次 429 写 8 行带文案 + 1 行 `[handleAuthError]`。
fn ide_event_lines(conversation: &str, uid: &str) -> String {
    let quota = IDE_QUOTA;
    let notify_step_error = ide_line(
        "2026-09-17 10:28:26.730",
        &format!(
            "[BaseAgent:craft] [{IDE_REQUEST}]  notifyStepError responseBody: \"{{\\\"code\\\":6004,\\\"msg\\\":\\\"{quota}\\\",\\\"requestId\\\":\\\"{IDE_REQUEST}\\\"}}\""
        ),
    );
    // 超长行：请求体已截断，但 `"model":`（行首）与 `"x-user-id"`（行尾）保留原位置。
    let agent_execution_failed = ide_line(
        "2026-09-17 10:28:26.741",
        &format!(
            "[AgentReporter] [{IDE_TRACE}]  Agent execution failed: {{\"name\":\"AI_APICallError\",\"url\":\"https://copilot.tencent.com/v2/chat/completions\",\"requestBodyValues\":{{\"model\":\"{IDE_MODEL}\"}},\"statusCode\":429,\"responseHeaders\":{{\"x-request-id\":\"{IDE_REQUEST}\",\"x-user-id\":\"{uid}\"}},\"responseBody\":\"{{\\\"code\\\":6004,\\\"msg\\\":\\\"{quota}\\\"}}\",\"message\":\"{quota}\",\"code\":6004}}"
        ),
    );
    let lines = [
        notify_step_error,
        agent_execution_failed,
        ide_line(
            "2026-09-17 10:28:26.746",
            &format!("[CraftInvokableAgent] [{IDE_TRACE}]  Execution failed: {quota}"),
        ),
        ide_line(
            "2026-09-17 10:28:26.746",
            &format!("[CraftInvokableAgent] [{IDE_TRACE}]  Agent call failed: {quota}"),
        ),
        ide_line(
            "2026-09-17 10:28:26.748",
            &format!("[handleAuthError] modelId={IDE_MODEL}, baseUrl=undefined, officialEndpoint=https://copilot.tencent.com, isCustomAuthFailure=false"),
        ),
        ide_line(
            "2026-09-17 10:28:26.749",
            &format!(
                "[ResultHandler.handleError]  errorMessage {{\"requestId\":\"acp-{conversation}-1789612105908\",\"complete\":true,\"error\":{{\"code\":6004,\"message\":\"{quota}\",\"traceId\":\"{IDE_TRACE}\",\"model\":\"{IDE_MODEL}\"}},\"isEnd\":true,\"conversationId\":\"{conversation}\"}}"
            ),
        ),
        ide_line(
            "2026-09-17 10:28:26.751",
            &format!("[AcpAgent:{conversation}] Session error: {quota}"),
        ),
        ide_line(
            "2026-09-17 10:28:26.751",
            &format!(
                "[AgentSessionManager] executeAsync FAILED: conversationId={conversation}, errorCode=6004, traceId={IDE_TRACE}, message={quota}"
            ),
        ),
        ide_line(
            "2026-09-17 10:28:26.751",
            &format!(
                "[[acp-conn]] [AgentCraftError: {quota}]  [AcpConnection:{conversation}] Error handling request:"
            ),
        ),
    ];
    lines.join("\n")
}

fn ide_model_selection_line(conversation: &str, model: &str) -> String {
    ide_line(
        "2026-09-17 10:28:25.919",
        &format!(
            "[ModelSelection] conversationId={conversation}, mode=craft, modelId={model}, source=user-selected"
        ),
    )
}

/// IDE 限额文案行（只有文案与事件 id，不含模型线索）。
fn ide_quota_line(timestamp: &str, conversation: &str) -> String {
    ide_line(
        timestamp,
        &format!("[AcpAgent:{conversation}] Session error: {IDE_QUOTA}"),
    )
}

#[test]
fn byte_filter_matches_multibyte_and_ascii_markers() {
    assert!(contains(cn_quota().as_bytes(), QUOTA_MARKERS[0].as_bytes()));
    assert!(contains(
        b"x usage exceeds frequency limit y",
        QUOTA_MARKERS[1].as_bytes()
    ));
    assert!(!contains(b"nothing to see", QUOTA_MARKERS[1].as_bytes()));
    assert!(!contains(b"", QUOTA_MARKERS[0].as_bytes()));
    assert!(!contains(b"", b""));
    // Horspool 跳表的边界：needle 比 haystack 长、needle 内含重复字节、needle 在末尾命中。
    assert!(!contains(b"aaa", b"aaaa"));
    assert!(contains(b"aaaa", b"aaaa"));
    assert!(contains(b"abcabcabd", b"abcabd"));
    assert!(contains(b"xxababacyy", b"ababac"));
    assert!(!contains(b"xxabababyy", b"ababac"));
}

#[test]
fn parses_chinese_and_english_reset_times() {
    assert_eq!(parse_reset_at(&cn_quota()), Some(CN_RESET_AT));
    assert_eq!(parse_reset_at(&en_quota()), Some(EN_RESET_AT));
    // 缺时区偏移的文案不猜时间。
    assert_eq!(
        parse_reset_at("429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 重置"),
        None
    );
}

#[test]
fn parses_utc_offsets_the_official_text_may_use() {
    let at = |offset: &str| {
        parse_reset_at(&format!(
            "将在 2026-09-17 17:59:27 UTC{offset} 重置，您也可以切换其他模型继续使用。"
        ))
    };
    assert_eq!(at("+8"), Some(CN_RESET_AT));
    assert_eq!(at("+08"), Some(CN_RESET_AT));
    assert_eq!(at("+08:00"), Some(CN_RESET_AT));
    assert_eq!(at("+0800"), Some(CN_RESET_AT));
    assert_eq!(at("+7"), Some(CN_RESET_AT + 3_600_000));
    assert_eq!(at(""), None);
}

#[test]
fn business_log_timestamp_is_local_while_sdk_log_timestamp_is_utc() {
    let local = Local
        .with_ymd_and_hms(2026, 9, 17, 0, 20, 31)
        .earliest()
        .expect("测试时间必须存在");
    assert_eq!(
        line_timestamp(
            &business_line("9/17/2026, 12:20:31 AM.232", "[Info] x"),
            LogFormat::WorkBuddy,
        ),
        Some(local.timestamp_millis() + 232)
    );
    assert_eq!(
        line_timestamp(
            "2026-09-16T16:20:31.523Z runtime.applyStopReason {}",
            LogFormat::WorkBuddy,
        ),
        Some(SDK_OCCURRED_AT)
    );
    assert_eq!(
        line_timestamp("no timestamp here", LogFormat::WorkBuddy),
        None
    );
}

#[test]
fn parses_the_trailing_request_id_pair() {
    assert_eq!(
        request_pair(&cn_quota()),
        Some((CN_REQUEST.to_string(), CN_SESSION.to_string()))
    );
    assert_eq!(
        request_pair(&format!(
            "x ({CN_REQUEST}/{CN_SESSION}), lastPendingTool=(none)"
        )),
        Some((CN_REQUEST.to_string(), CN_SESSION.to_string())),
        "取行尾括号，忽略 `(none)` 这种非时间戳括号"
    );
    assert_eq!(request_pair("no pair here"), None);
    assert_eq!(
        request_pair(&format!("({CN_REQUEST}/{CN_REQUEST})")),
        None,
        "会话 id 不是 36 位 uuid 时不认"
    );
}

#[test]
fn model_attribution_falls_back_request_id_then_session_then_unknown() {
    // ① 限额行给出 requestId，同文件有 `model=` + `requestId=` 的行。
    let with_request = [
        business_line(
            "9/17/2026, 12:20:03 AM.349",
            &config_line(CN_SESSION, "hy3"),
        ),
        business_line(
            "9/17/2026, 12:20:30 AM.838",
            &provider_line(CN_REQUEST, "deepseek-v4.1-flash"),
        ),
        business_line("9/17/2026, 12:20:31 AM.232", &cn_quota()),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&with_request));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model.as_deref(), Some("deepseek-v4.1-flash"));

    // ② 没有 requestId → model 行，回退到该会话最近一次 `resolved model=`。
    let with_session = [
        business_line(
            "9/17/2026, 12:20:03 AM.349",
            &config_line(CN_SESSION, "hy3"),
        ),
        business_line("9/17/2026, 12:20:31 AM.232", &cn_quota()),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&with_session));
    assert_eq!(events[0].model.as_deref(), Some("hy3"));

    // ③ 都取不到 → None（前端显示「未知模型」，绝不猜）。
    let without_model = business_line("9/17/2026, 12:20:31 AM.232", &cn_quota());
    let events = dedupe(workbuddy_hits(&without_model));
    assert_eq!(events[0].model, None);
}

#[test]
fn attributes_the_limited_model_not_the_one_switched_to_afterwards() {
    let text = [
        business_line(
            "9/17/2026, 12:20:03 AM.349",
            &config_line(CN_SESSION, "hy3"),
        ),
        business_line(
            "9/17/2026, 12:20:30 AM.838",
            &provider_line(CN_REQUEST, "deepseek-v4.1-flash"),
        ),
        business_line("9/17/2026, 12:20:31 AM.232", &cn_quota()),
        // 被限之后用户切到了 hy3：不得归因成 hy3。
        business_line(
            "9/17/2026, 12:21:54 AM.388",
            &config_line(CN_SESSION, "hy3"),
        ),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model.as_deref(), Some("deepseek-v4.1-flash"));
}

#[test]
fn request_options_model_is_not_mistaken_for_the_request_model() {
    let text = [
        business_line(
            "9/17/2026, 12:20:03 AM.349",
            &config_line("other", "kimi-k3-1"),
        ),
        business_line(
            "9/17/2026, 12:20:30 AM.838",
            &format!("[ModelProvider] requestId={CN_REQUEST}, requestOptions.model=glm-5.2"),
        ),
        business_line("9/17/2026, 12:20:31 AM.232", &cn_quota()),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert_eq!(
        events[0].model, None,
        "带点号的 `requestOptions.model=` 不是请求模型字段"
    );
}

#[test]
fn classifier_line_supplies_the_session_when_the_pair_is_missing() {
    let text = [
        business_line(
            "9/17/2026, 12:20:31 AM.467",
            &format!(
                "[ACP Agent] refusal classified: sessionId={CN_SESSION}, rpcCode=-32003, httpStatus=429, bizCode=6004, category=quota"
            ),
        ),
        business_line(
            "9/17/2026, 12:20:31 AM.468",
            "429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置，您也可以切换其他模型继续使用。",
        ),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id.as_deref(), Some(CN_SESSION));
}

/// 分类器兜底只在 `CLASSIFIER_FALLBACK_MS`（2 秒）窗内有效：回显行与分类器行相隔
/// 分钟级，不得借道旧分类器会话入账（2026-09-18 glm 假 chip 实证；R6「超过时间窗
/// 不沿用旧会话」）。
#[test]
fn classifier_fallback_does_not_outlive_its_time_window() {
    const BARE_QUOTA: &str = "429 您的使用量已超出频率限制，将在 2026-09-17 17:59:27 UTC+8 重置，您也可以切换其他模型继续使用。";
    let classifier = business_line(
        "9/17/2026, 12:20:31 AM.467",
        &format!(
            "[ACP Agent] refusal classified: sessionId={CN_SESSION}, rpcCode=-32003, httpStatus=429, bizCode=6004, category=quota"
        ),
    );

    // 紧邻（1 ms）：兜底生效，会话来自分类器行。
    let near = [
        classifier.clone(),
        business_line("9/17/2026, 12:20:31 AM.468", BARE_QUOTA),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&near));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id.as_deref(), Some(CN_SESSION));

    // 超窗（约 5 秒后）：不得沿用旧分类器会话 —— 该行既无事件身份又借不到兜底，
    // 按「宁可少显示」丢弃。
    let far = [
        classifier,
        business_line("9/17/2026, 12:20:36.500", BARE_QUOTA),
    ]
    .join("\n");
    assert!(
        dedupe(workbuddy_hits(&far)).is_empty(),
        "超过时间窗的裸文案行不得沿用旧会话"
    );
}

#[test]
fn recognizes_english_quota_text() {
    let text = [
        business_line(
            "9/14/2026, 10:59:00 AM.000",
            &config_line(EN_SESSION, "kimi-k3-1"),
        ),
        business_line("9/14/2026, 10:59:21 AM.523", &en_quota()),
    ]
    .join("\n");
    let events = dedupe(workbuddy_hits(&text));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reset_at, EN_RESET_AT);
    assert_eq!(events[0].model.as_deref(), Some("kimi-k3-1"));
    assert_eq!(events[0].session_id.as_deref(), Some(EN_SESSION));
}

/// AC4：本机最近 3 天 14 条限额行只是 2 次事件（每次事件业务日志 6 行 + SDK 冗余 1 行）。
#[test]
fn fourteen_duplicate_lines_collapse_into_two_events() {
    let root = temp_dir("dedupe");
    let business = root.join("2026-09-16");
    let sdk = root.join("2026-09-17").join("sdk").join("conversations");
    std::fs::create_dir_all(&business).expect("业务日志目录");
    std::fs::create_dir_all(&sdk).expect("SDK 日志目录");

    let event_lines = |session: &str,
                       request: &str,
                       reset: &str,
                       provider_at: &str,
                       quota_at: &str,
                       switched_at: &str| {
        let mut lines = vec![
            business_line(provider_at, &config_line(session, "deepseek-v4.1-flash")),
            business_line(provider_at, &provider_line(request, "deepseek-v4.1-flash")),
        ];
        for ms in ["232", "233", "233", "462", "467", "468"] {
            lines.push(business_line(
                &format!("{quota_at}.{ms}"),
                &cn_quota_with(session, request, reset),
            ));
        }
        lines.push(business_line(switched_at, &config_line(session, "hy3")));
        lines
    };

    // 与真机一致：两次事件的**重置时刻相同**，靠发生时刻相差 805s（≫15s 合并窗口）
    // 保持为两次事件——若这里用不同的重置时刻区分，就测不到「同 reset 不得误合并」。
    let first = event_lines(
        CN_SESSION,
        CN_REQUEST,
        "2026-09-17 17:59:27",
        "9/17/2026, 12:20:03 AM.349",
        "9/17/2026, 12:20:31 AM",
        "9/17/2026, 12:21:54 AM.388",
    );
    std::fs::write(business.join("event-one.log"), first.join("\n")).expect("写入业务日志");
    let second = event_lines(
        EN_SESSION,
        EN_REQUEST,
        "2026-09-17 17:59:27",
        "9/17/2026, 12:33:56 AM.520",
        "9/17/2026, 12:33:56 AM",
        "9/17/2026, 1:25:24 AM.523",
    );
    std::fs::write(business.join("event-two.log"), second.join("\n")).expect("写入业务日志");

    // SDK 侧对同一事件再写一份（UTC 时间戳、无模型行），会话 id 仍取得到。
    // 两次书写各随其业务日志的时刻（真机相差约 0.3s），否则两条 SDK 冗余会把
    // 相隔 13 分钟的事件拉进同一个 15s 窗口。
    for (session, quota, sdk_at) in [
        (CN_SESSION, &first[2], "2026-09-16T16:20:31.523Z"),
        (EN_SESSION, &second[2], "2026-09-16T16:33:56.520Z"),
    ] {
        std::fs::write(
            sdk.join(format!("{session}.log")),
            format!("{sdk_at} runtime.applyStopReason {{\"errorMessageMetaPreview\":\"{quota}\"}}"),
        )
        .expect("写入 SDK 日志");
    }

    let events = collect_events(&root, LogFormat::WorkBuddy, AuthMarker::None);
    assert_eq!(events.len(), 2, "14 条限额行必须聚合为 2 次事件");
    assert_eq!(events[0].hit_count, 7);
    assert_eq!(events[1].hit_count, 7);
    assert_eq!(events[0].model.as_deref(), Some("deepseek-v4.1-flash"));
    assert_eq!(events[1].model.as_deref(), Some("deepseek-v4.1-flash"));
    assert_eq!(events[0].reset_at, CN_RESET_AT);
    assert_eq!(
        events[1].reset_at, CN_RESET_AT,
        "两次事件重置时刻相同，仍必须是两次事件"
    );
    assert_eq!(events[0].session_id.as_deref(), Some(CN_SESSION));
    assert_eq!(events[1].session_id.as_deref(), Some(EN_SESSION));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn dedupe_keeps_sessions_apart_and_merges_only_within_fifteen_seconds() {
    let hit = |session: &str, occurred_at: i64, model: Option<&str>| Hit {
        session_id: Some(session.to_string()),
        model: model.map(str::to_string),
        reset_at: CN_RESET_AT,
        occurred_at,
        uid: None,
    };
    let events = dedupe(vec![
        // ① 步：同一会话的重复书写先合并。
        hit("session-a", 1_000_000, None),
        hit("session-a", 1_014_000, None),
        // ② 步：不同会话、重置时刻相同且相差 ≤15s，属同一次事件的另一次书写。
        hit("session-b", 1_003_000, Some("hy3")),
        // 相差 20s（超出合并窗口）的另一次限流不得被吞。
        hit("session-c", 1_020_000, None),
    ]);
    assert_eq!(events.len(), 2, "相差 20s 的第二次限流不得被合并");
    assert_eq!(events[0].hit_count, 3);
    assert_eq!(events[0].session_id.as_deref(), Some("session-a"));
    assert_eq!(
        events[0].model.as_deref(),
        Some("hy3"),
        "合并时保留有归因的那条"
    );
    assert_eq!(events[1].hit_count, 1);
    assert_eq!(events[1].session_id.as_deref(), Some("session-c"));
}

/// 收窗判据是文件 mtime 而非目录名：目录名很旧但文件在窗口内（跨天追加）必须纳入，
/// 目录名很新但文件在窗口外必须排除；非日期目录整体忽略。
#[test]
fn window_selects_files_by_mtime_not_directory_name_and_tolerates_missing_roots() {
    let root = temp_dir("window");
    // 目录名很旧、文件刚写：真机「9/15 目录写到 9/16 07:59」的形态，必须纳入。
    let cross_day = root.join("2020-01-01");
    std::fs::create_dir_all(&cross_day).expect("跨天目录");
    std::fs::write(cross_day.join("cross-day.log"), "x").expect("跨天日志");
    // 目录名很新、文件 mtime 在窗口外：必须排除。
    let old_file = root.join("2099-12-31").join("old.log");
    std::fs::create_dir_all(old_file.parent().expect("父目录")).expect("未来目录");
    std::fs::write(&old_file, "x").expect("旧日志");
    set_days_old_mtime(&old_file, 30);
    // 非日期目录整体忽略（即使里面有窗口内的 .log）。
    let plain = root.join("memwatch");
    std::fs::create_dir_all(&plain).expect("非日期目录");
    std::fs::write(plain.join("mem.log"), "x").expect("非日期目录内日志");

    let cutoff = now_ms() - WINDOW_DAYS as i64 * DAY_MS;
    let names: BTreeSet<String> = windowed_log_files(&root, cutoff)
        .iter()
        .filter_map(|path| Some(path.file_name()?.to_string_lossy().to_string()))
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["cross-day.log".to_string()]),
        "旧目录里的新文件要收，新目录里的旧文件要排除，非日期目录整体忽略"
    );
    // 目录名不参与判定：cutoff 推到未来 → 空集；拉到 0 → 两个日期目录下的文件都收。
    assert!(windowed_log_files(&root, now_ms() + DAY_MS).is_empty());
    assert_eq!(windowed_log_files(&root, 0).len(), 2);
    assert!(
        windowed_log_files(&root.join("missing"), 0).is_empty(),
        "档位日志根缺失时返回空集，不是错误"
    );
    assert!(collect_events(
        &root.join("missing"),
        LogFormat::WorkBuddy,
        AuthMarker::None
    )
    .is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn payload_drops_expired_entries_and_sorts_by_reset_time() {
    let now = 1_000_000;
    let resolved = vec![
        Resolved {
            account_id: "a".to_string(),
            model: Some("late".to_string()),
            reset_at: now + 7_200_000,
            first_seen_at: 0,
            hit_count: 1,
            source: "workbuddy-cn".to_string(),
        },
        Resolved {
            account_id: "a".to_string(),
            model: Some("soon".to_string()),
            reset_at: now + 600_000,
            first_seen_at: 0,
            hit_count: 3,
            source: "workbuddy-cn".to_string(),
        },
        Resolved {
            account_id: "a".to_string(),
            model: Some("expired".to_string()),
            reset_at: now - 1,
            first_seen_at: 0,
            hit_count: 1,
            source: "workbuddy-cn".to_string(),
        },
        Resolved {
            account_id: "b".to_string(),
            model: None,
            reset_at: now + 60_000,
            first_seen_at: 0,
            hit_count: 2,
            source: "workbuddy-cn".to_string(),
        },
    ];
    let payload = build_payload(resolved, now, now, true);
    assert_eq!(payload["windowDays"], 2);
    assert_eq!(payload["scannedAt"], now);
    let accounts = payload["accounts"].as_array().expect("accounts 数组");
    assert_eq!(accounts.len(), 2, "无受限模型的账号不出现在结果里");
    assert_eq!(accounts[0]["accountId"], "a");
    let limited = accounts[0]["limited"].as_array().expect("limited 数组");
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0]["model"], "soon", "按恢复时间升序");
    assert_eq!(limited[1]["model"], "late");
    assert_eq!(accounts[1]["limited"][0]["model"], Value::Null);
}

#[test]
fn payload_keeps_the_latest_reset_time_per_account_and_model() {
    let now = 1_000_000;
    let resolved = vec![
        Resolved {
            account_id: "a".to_string(),
            model: Some("hy3".to_string()),
            reset_at: now + 600_000,
            first_seen_at: 10,
            hit_count: 1,
            source: "workbuddy-cn".to_string(),
        },
        Resolved {
            account_id: "a".to_string(),
            model: Some("hy3".to_string()),
            reset_at: now + 3_600_000,
            first_seen_at: 20,
            hit_count: 2,
            source: "workbuddy-cn".to_string(),
        },
    ];
    let payload = build_payload(resolved, now, now, true);
    let limited = payload["accounts"][0]["limited"]
        .as_array()
        .expect("limited 数组");
    assert_eq!(limited.len(), 1, "同 (账号, 模型) 只保留一条");
    assert_eq!(limited[0]["resetAt"], now + 3_600_000);
    assert_eq!(limited[0]["hitCount"], 2);
}

/// 扫描结果缓存 5 分钟：缓存期间到达 resetAt 的条目必须立即消失。
///
/// 过滤若继续用 `scannedAt`，已解锁的模型会在下一次扫描前一直挂在台账里；hook 通路
/// 按当前时刻清理，两条通路口径不一致。
#[test]
fn payload_drops_entries_reset_between_scan_and_now() {
    let scanned_at = 1_000_000;
    let now = scanned_at + 120_000;
    let resolved = vec![Resolved {
        account_id: "a".to_string(),
        model: Some("expired-after-scan".to_string()),
        reset_at: scanned_at + 60_000,
        first_seen_at: 0,
        hit_count: 1,
        source: "workbuddy-cn".to_string(),
    }];
    let payload = build_payload(resolved, scanned_at, now, true);
    assert_eq!(payload["scannedAt"], scanned_at);
    assert!(payload["accounts"]
        .as_array()
        .expect("accounts 数组")
        .is_empty());
}

// -----------------------------------------------------------------------
// CodeBuddy CLI
// -----------------------------------------------------------------------

/// CLI 的事件账号取「该事件前最近一次鉴权行」的 uid：切换账号后不串号、也不回溯。
#[test]
fn cli_uid_comes_from_the_last_auth_line_before_the_event() {
    let text = [
        cli_auth_line("9/15/2026, 2:07:59 PM.057", UID_A),
        business_line(
            "9/15/2026, 2:08:10 PM.000",
            &cn_quota_with(CN_SESSION, CN_REQUEST, "2026-09-15 15:55:40"),
        ),
        // 切换账号：之后的事件必须归到新账号。
        cli_auth_line("9/15/2026, 2:09:00 PM.000", UID_B),
        business_line(
            "9/15/2026, 2:09:10 PM.000",
            &cn_quota_with(EN_SESSION, EN_REQUEST, "2026-09-15 16:55:40"),
        ),
    ]
    .join("\n");
    let hits = scan_text(&text, LogFormat::WorkBuddy, AuthMarker::AuthDoInitProbe);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].uid.as_deref(), Some(UID_A));
    assert_eq!(hits[1].uid.as_deref(), Some(UID_B));

    // 鉴权行在事件之后：不得回溯使用。
    let later = [
        business_line("9/15/2026, 2:08:10 PM.000", &cn_quota()),
        cli_auth_line("9/15/2026, 2:08:20 PM.000", UID_A),
    ]
    .join("\n");
    let hits = scan_text(&later, LogFormat::WorkBuddy, AuthMarker::AuthDoInitProbe);
    assert_eq!(hits[0].uid, None, "事件后的 uid 行不得回溯归因");

    // WorkBuddy 两档位不采集 uid（归因仍走 sessions 表）。
    let hits = workbuddy_hits(&later);
    assert_eq!(hits[0].uid, None);
}

/// `uid=none` 是「无会话 / 未登录」的哨兵值（CLI 与 IDE 都会写），必须忽略。
#[test]
fn auth_lines_with_uid_none_are_ignored() {
    let cli = [
        cli_auth_line("9/15/2026, 2:07:59 PM.057", "none"),
        business_line("9/15/2026, 2:08:10 PM.000", &cn_quota()),
    ]
    .join("\n");
    let hits = scan_text(&cli, LogFormat::WorkBuddy, AuthMarker::AuthDoInitProbe);
    assert_eq!(hits[0].uid, None);

    let ide = [
        ide_auth_line("none"),
        ide_quota_line("2026-09-17 10:28:26.751", IDE_CONV),
    ]
    .join("\n");
    let hits = scan_text(&ide, LogFormat::Ide, AuthMarker::AuthSessionChanged);
    assert_eq!(hits[0].uid, None);
}

/// CLI 日志目录复用日期目录枚举 + 递归收 `.log`（含 `sdk/conversations/` 一层）。
#[test]
fn cli_source_reuses_the_date_directory_enumeration() {
    let root = temp_dir("cli");
    let day = root.join("2026-09-15");
    let sdk = day.join("sdk").join("conversations");
    std::fs::create_dir_all(&sdk).expect("SDK 日志目录");
    std::fs::write(
        day.join("session.log"),
        [
            cli_auth_line("9/15/2026, 2:07:59 PM.057", UID_A),
            business_line("9/15/2026, 2:08:10 AM.000", &cn_quota()),
        ]
        .join("\n"),
    )
    .expect("业务日志");
    // SDK 侧对同一事件再写一份（同一会话 id）。
    std::fs::write(
        sdk.join("session.log"),
        business_line("9/15/2026, 2:08:10 AM.500", &cn_quota()),
    )
    .expect("SDK 日志");

    let events = collect_events(&root, LogFormat::WorkBuddy, AuthMarker::AuthDoInitProbe);
    assert_eq!(events.len(), 1, "业务日志与 SDK 冗余书写聚合为 1 条");
    assert_eq!(events[0].hit_count, 2);
    assert_eq!(events[0].reset_at, CN_RESET_AT);
    assert_eq!(events[0].uid.as_deref(), Some(UID_A));
    std::fs::remove_dir_all(&root).ok();
}

/// CLI 的日志根与状态文件都挂在用户主目录下（跨平台由 `home_dir()` 兜住）。
#[test]
fn cli_source_paths_live_under_the_home_directory() {
    assert!(cli_logs_root().ends_with(Path::new(CLI_DATA_DIR).join(LOG_DIR_NAME)));
    assert!(cli_state_path().ends_with(Path::new(CLI_ROTATE_DIR).join(CLI_STATE_FILE)));
}

// -----------------------------------------------------------------------
// CodeBuddy IDE / CodeBuddy CN IDE
// -----------------------------------------------------------------------

/// IDE 行首时间戳是 23 字节定长前缀（本地时间，保留毫秒）；不是该形态就不猜时间。
#[test]
fn ide_timestamp_is_a_23_byte_local_prefix() {
    let at = Local
        .with_ymd_and_hms(2026, 9, 17, 10, 28, 26)
        .earliest()
        .expect("测试时间必须存在");
    let line = ide_quota_line("2026-09-17 10:28:26.730", IDE_CONV);
    assert_eq!(
        line_timestamp(&line, LogFormat::Ide),
        Some(at.timestamp_millis() + 730)
    );
    assert_eq!(
        line_timestamp("2026-09-17 10:28:26", LogFormat::Ide),
        None,
        "不足 23 字节的行首不得猜时间"
    );
}

/// 一次 429 的 8 行书写聚合为 1 条，且会话 id / 模型 / 重置时刻 / uid 全部正确。
#[test]
fn ide_event_lines_collapse_into_one_event_with_conversation_and_model() {
    let text = [
        ide_auth_line(UID_A),
        ide_model_selection_line(IDE_CONV, IDE_MODEL),
        ide_event_lines(IDE_CONV, UID_A),
    ]
    .join("\n");
    let events = dedupe(scan_text(
        &text,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events.len(), 1, "8 行带限额文案只是一次事件");
    assert_eq!(events[0].hit_count, 8);
    assert_eq!(events[0].session_id.as_deref(), Some(IDE_CONV));
    assert_eq!(events[0].model.as_deref(), Some(IDE_MODEL));
    assert_eq!(events[0].reset_at, CN_RESET_AT);
    assert_eq!(events[0].uid.as_deref(), Some(UID_A));
    // 发生时刻取最早的限额行（10:28:26.730）。
    let at = Local
        .with_ymd_and_hms(2026, 9, 17, 10, 28, 26)
        .earliest()
        .expect("测试时间必须存在");
    assert_eq!(events[0].first_seen_at, at.timestamp_millis() + 730);
}

/// 模型归因按会话隔离，且只取「事件之前」最近一次选用的模型。
#[test]
fn ide_model_is_attributed_per_conversation_and_before_the_event() {
    // 别的会话选过模型：不得归给本会话。
    let other = [
        ide_auth_line(UID_A),
        ide_model_selection_line(IDE_OTHER_CONV, "glm-5.2"),
        ide_quota_line("2026-09-17 10:28:26.751", IDE_CONV),
    ]
    .join("\n");
    let events = dedupe(scan_text(
        &other,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events[0].session_id.as_deref(), Some(IDE_CONV));
    assert_eq!(events[0].model, None, "不得把别的会话的模型归给本会话");

    // 被限之后用户切了模型：不得归因成切换后的模型。
    let switched = [
        ide_model_selection_line(IDE_CONV, IDE_MODEL),
        ide_quota_line("2026-09-17 10:28:26.751", IDE_CONV),
        ide_line(
            "2026-09-17 10:30:00.000",
            &format!(
                "[AcpAgent:{IDE_CONV}] Model cache synced: mode=craft, modelId=glm-5.2, source=user-selected"
            ),
        ),
    ]
    .join("\n");
    let events = dedupe(scan_text(
        &switched,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events[0].model.as_deref(), Some(IDE_MODEL));
}

/// `modelId=auto` / `undefined` / `null`（会话尚未选定模型或字段缺失）按未知处理。
#[test]
fn ide_unknown_model_sentinels_are_treated_as_unknown() {
    for sentinel in MODEL_UNKNOWN {
        let text = [
            ide_model_selection_line(IDE_CONV, sentinel),
            ide_quota_line("2026-09-17 10:28:26.751", IDE_CONV),
        ]
        .join("\n");
        let events = dedupe(scan_text(
            &text,
            LogFormat::Ide,
            AuthMarker::AuthSessionChanged,
        ));
        assert_eq!(events[0].model, None, "`{sentinel}` 不得作为模型名展示");
    }
    // `[handleAuthError] modelId=undefined` 作为文件级兜底时同样不得展示。
    let text = [
        ide_line(
            "2026-09-17 10:28:26.748",
            "[handleAuthError] modelId=undefined, baseUrl=undefined, officialEndpoint=https://copilot.tencent.com",
        ),
        ide_quota_line("2026-09-17 10:28:26.751", IDE_CONV),
    ]
    .join("\n");
    let events = dedupe(scan_text(
        &text,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events[0].model, None);
}

/// 鉴权行缺失时，由事件行内的 `x-user-id` 补位（同一事件的其余行随之归到该账号）。
#[test]
fn x_user_id_fills_the_uid_when_no_auth_line_exists() {
    let text = ide_event_lines(IDE_CONV, UID_A);
    let events = dedupe(scan_text(
        &text,
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged,
    ));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].uid.as_deref(), Some(UID_A));
}

/// 约 350 KB 的 `Agent execution failed` 行（含完整请求体）：只在定长窗口内嗅探字段。
#[test]
fn ide_long_error_line_is_sniffed_within_bounded_windows() {
    let filler = "x".repeat(350 * 1024);
    let line = format!(
        "2026-09-17 10:28:26.741 [error] [AgentReporter] [{IDE_TRACE}]  Agent execution failed: {{\"name\":\"AI_APICallError\",\"requestBodyValues\":{{\"model\":\"{IDE_MODEL}\",\"messages\":[{{\"content\":\"{filler}\"}}]}},\"statusCode\":429,\"responseHeaders\":{{\"x-user-id\":\"{UID_A}\"}},\"responseBody\":\"{{\\\"msg\\\":\\\"{IDE_QUOTA}\\\"}}\",\"code\":6004}}"
    );
    assert!(line.len() > 350 * 1024);

    let hits = scan_text(&line, LogFormat::Ide, AuthMarker::AuthSessionChanged);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].uid.as_deref(),
        Some(UID_A),
        "行尾窗口内的 x-user-id"
    );
    assert_eq!(
        hits[0].model.as_deref(),
        Some(IDE_MODEL),
        "行首窗口内的 model"
    );
    assert_eq!(hits[0].reset_at, CN_RESET_AT);
}

/// JSON 形态的事件 id 覆盖两种书写：未转义的 `"conversationId":"…"`（本机 `ResultHandler`
/// 行）与嵌在字符串里的转义形式 `\"conversationId\":\"…\"`（`responseBody: "…"` 这类行）。
#[test]
fn ide_conversation_id_covers_quoted_json_in_both_escapes() {
    let plain = ide_line(
        "2026-09-17 10:28:26.749",
        &format!(
            "[ResultHandler.handleError]  errorMessage {{\"isEnd\":true,\"conversationId\":\"{IDE_CONV}\"}}"
        ),
    );
    assert_eq!(ide_conversation_id(&plain), Some(IDE_CONV));

    let escaped = ide_line(
        "2026-09-17 10:28:26.749",
        &format!(
            "[BaseAgent:craft] responseBody: \"{{\\\"code\\\":6004,\\\"conversationId\\\":\\\"{IDE_CONV}\\\"}}\""
        ),
    );
    assert_eq!(ide_conversation_id(&escaped), Some(IDE_CONV));
}

/// 载荷被截断（值没有闭合引号）时不得把半个值当成结果：宁可「未知模型」，也不能展示
/// 掐头去尾的模型名。定长窗口的边界同理——`"modelId"` 之类相似键也不得误命中。
#[test]
fn json_sniffing_rejects_values_without_a_closing_quote() {
    assert_eq!(json_field("{\"model\":\"deepseek-v4.1-fl", "model"), None);
    assert_eq!(json_field("{\"modelId\":\"auto\"}", "model"), None);

    // `"model":"` 落在行尾窗口里、值被写出方截断（超长行）。补齐闭合引号后才可用。
    let mut line = String::from("2026-09-17 10:28:26.741 [error] [AgentReporter]  ");
    line.push_str(IDE_QUOTA);
    line.push_str(&"y".repeat(LONG_LINE_SNIFF_BYTES));
    line.push_str("\"model\":\"deepseek-v4.1-fl");
    assert!(line.len() > LONG_LINE_SNIFF_BYTES);
    assert_eq!(json_field(&line, "model"), None);
    assert_eq!(ide_json_model(&line), None);

    let hits = scan_text(&line, LogFormat::Ide, AuthMarker::AuthSessionChanged);
    assert_eq!(hits.len(), 1, "截断的行仍是一次限额命中");
    assert_eq!(hits[0].model, None, "截断的半个模型名不得作为模型展示");
}

/// IDE 枚举：只认 `exthost/Tencent-Cloud.coding-copilot/` 下的文件（不限文件名），
/// 并按**文件 mtime** 收窗——会话目录名是启动时间，按目录名收窗会漏事件。
#[test]
fn ide_enumeration_filters_by_plugin_dir_and_file_mtime() {
    let root = temp_dir("ide-enum");
    // 会话目录名是「旧的启动时间」，但文件是刚刚写的：真机那次 429 正是这种情形。
    let window = root.join("20260916T193500").join("window9");
    let copilot = window.join("exthost").join(IDE_LOG_DIR_NAME);
    std::fs::create_dir_all(&copilot).expect("插件日志目录");
    // 三种文件名（含 `.1.log` 轮转与国际版命名）都必须收——文件名不写死。
    let names = [
        "腾讯云代码助手.log",
        "腾讯云代码助手.1.log",
        "Tencent Cloud CodeBuddy.log",
    ];
    for name in names {
        std::fs::write(copilot.join(name), IDE_QUOTA).expect("插件日志");
    }
    // 插件目录之外的同级日志（渲染进程 / exthost 主日志）不得收。
    std::fs::write(window.join("renderer.log"), IDE_QUOTA).expect("渲染进程日志");
    std::fs::write(window.join("exthost").join("exthost.log"), IDE_QUOTA).expect("exthost 主日志");

    let files = ide_log_files(&root, now_ms() - WINDOW_DAYS as i64 * DAY_MS);
    let found: BTreeSet<String> = files
        .iter()
        .filter_map(|path| Some(path.file_name()?.to_string_lossy().to_string()))
        .collect();
    assert_eq!(
        found,
        names.map(str::to_string).into_iter().collect(),
        "只收插件目录下的文件，且不限文件名/扩展名"
    );

    // mtime 收窗：窗口起点在未来 → 一个都不收（证明是按文件 mtime，而不是目录名）。
    assert!(ide_log_files(&root, now_ms() + DAY_MS).is_empty());
    // 日志根缺失（该 IDE 未安装/未使用）不报错。
    assert!(ide_log_files(&root.join("missing"), 0).is_empty());
    std::fs::remove_dir_all(&root).ok();
}

/// IDE 来源端到端（枚举 → 粗筛 → 解析 → 去重），空根/无候选文件都不报错。
#[test]
fn ide_source_collects_events_and_tolerates_empty_roots() {
    let root = temp_dir("ide-source");
    let copilot = root
        .join("20260917T102952")
        .join("window3")
        .join("exthost")
        .join(IDE_LOG_DIR_NAME);
    std::fs::create_dir_all(&copilot).expect("插件日志目录");
    std::fs::write(
        copilot.join("腾讯云代码助手.log"),
        [
            ide_auth_line(UID_A),
            ide_model_selection_line(IDE_CONV, IDE_MODEL),
            ide_event_lines(IDE_CONV, UID_A),
        ]
        .join("\n"),
    )
    .expect("插件日志");

    let events = collect_events(&root, LogFormat::Ide, AuthMarker::AuthSessionChanged);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].hit_count, 8);
    assert_eq!(events[0].session_id.as_deref(), Some(IDE_CONV));
    assert!(collect_events(
        &root.join("missing"),
        LogFormat::Ide,
        AuthMarker::AuthSessionChanged
    )
    .is_empty());

    let empty = temp_dir("ide-empty");
    std::fs::create_dir_all(&empty).expect("空目录");
    assert!(collect_events(&empty, LogFormat::Ide, AuthMarker::AuthSessionChanged).is_empty());
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&empty).ok();
}

/// 两个 IDE 的日志根各自指向自己的数据目录（平台分支在 `codebuddy_ide_data_dir` 内）。
#[test]
fn the_two_ide_sources_use_distinct_data_roots() {
    let (Some(intl), Some(cn)) = (
        codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Intl),
        codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Cn),
    ) else {
        // 该平台定位不到配置目录时不适用（与运行时代码同样跳过）。
        return;
    };
    assert_ne!(intl, cn);
    assert_eq!(
        intl.file_name().and_then(|name| name.to_str()),
        Some("CodeBuddy")
    );
    assert_eq!(
        cn.file_name().and_then(|name| name.to_str()),
        Some("CodeBuddy CN")
    );
}

// -----------------------------------------------------------------------
// 新来源的账号归因（uid 优先，状态文件带 updatedAt 门控回落）
// -----------------------------------------------------------------------

fn event_without_uid(first_seen_at: i64) -> Event {
    Event {
        session_id: None,
        model: None,
        reset_at: CN_RESET_AT,
        first_seen_at,
        hit_count: 1,
        uid: None,
    }
}

/// 回落门控：只有 `updatedAt ≤ 事件时刻` 才可信；缺失/过晚/损坏一律丢弃（不误归）。
#[test]
fn fallback_state_file_only_counts_when_updated_before_the_event() {
    let dir = temp_dir("fallback");
    std::fs::create_dir_all(&dir).expect("临时目录");
    let path = dir.join(CLI_STATE_FILE);
    let events = [event_without_uid(1_500)];
    let no_uid = HashMap::new();

    // ① uid 不可用 + 状态文件缺失 → 丢弃。
    assert!(resolve_by_uid(&events, &no_uid, &ActiveAccount::load(&path), "codebuddy-cli").is_empty());

    // ② `updatedAt` 晚于事件 → 不可信：IDE 的 detect 会回写状态，但那不是账号切换。
    std::fs::write(
        &path,
        json!({ "activeAccountId": "acc-1", "updatedAt": 2_000 }).to_string(),
    )
    .expect("状态文件");
    assert!(resolve_by_uid(&events, &no_uid, &ActiveAccount::load(&path), "codebuddy-cli").is_empty());

    // ③ `updatedAt` 早于事件 → 该时刻生效的账号可用。
    std::fs::write(
        &path,
        json!({ "activeAccountId": "acc-1", "updatedAt": 1_000 }).to_string(),
    )
    .expect("状态文件");
    let resolved = resolve_by_uid(&events, &no_uid, &ActiveAccount::load(&path), "codebuddy-cli");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].account_id, "acc-1");

    // ④ 缺 `updatedAt`（老状态文件）→ 无从证实「当时是谁」，丢弃。
    std::fs::write(&path, json!({ "activeAccountId": "acc-1" }).to_string()).expect("状态文件");
    assert!(resolve_by_uid(&events, &no_uid, &ActiveAccount::load(&path), "codebuddy-cli").is_empty());

    // ⑤ 内容损坏 → 回落不可用，不报错。
    std::fs::write(&path, "{ not json").expect("状态文件");
    assert!(resolve_by_uid(&events, &no_uid, &ActiveAccount::load(&path), "codebuddy-cli").is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// 日志内 uid 命中账号库时优先于状态文件回落（AC8：不按「当前 activeAccountId」误归）。
#[test]
fn uid_match_wins_over_the_state_file_fallback() {
    let dir = temp_dir("uid-wins");
    std::fs::create_dir_all(&dir).expect("临时目录");
    let path = dir.join(IDE_STATE_FILE);
    std::fs::write(
        &path,
        json!({ "activeAccountId": "acc-stale", "updatedAt": 1 }).to_string(),
    )
    .expect("状态文件");

    let uid_to_account = HashMap::from([(UID_A.to_string(), "acc-uid".to_string())]);
    let mut event = event_without_uid(1_500);
    event.uid = Some(UID_A.to_string());
    let resolved = resolve_by_uid(&[event], &uid_to_account, &ActiveAccount::load(&path), "codebuddy-cli");
    assert_eq!(resolved[0].account_id, "acc-uid");
    std::fs::remove_dir_all(&dir).ok();
}

/// 归因不到的 uid（账号库没有该账号）不得误配给任何账号。
#[test]
fn unknown_uid_is_dropped_without_a_fallback() {
    let mut event = event_without_uid(1_500);
    event.uid = Some(UID_A.to_string());
    let resolved = resolve_by_uid(
        &[event],
        &HashMap::from([(UID_B.to_string(), "acc-b".to_string())]),
        &ActiveAccount {
            account_id: None,
            updated_at: None,
        },
        "codebuddy-cli",
    );
    assert!(resolved.is_empty());
}

/// 扫描范围是位图：缓存命中要求「缓存范围 ⊇ 请求范围」，且受 5 分钟节流约束。
#[test]
fn scan_scope_and_cache_require_a_superset() {
    let mut cli = ScanScope::NONE;
    cli.insert(ScanSource::Cli);
    let mut ides = ScanScope::NONE;
    ides.insert(ScanSource::Ide(CodeBuddyIdeFlavor::Intl));
    ides.insert(ScanSource::Ide(CodeBuddyIdeFlavor::Cn));

    assert!(ScanScope::NONE.covers(ScanScope::NONE));
    assert!(!ScanScope::NONE.covers(cli));
    assert!(ScanScope::ALL.covers(ides));
    assert!(!ides.covers(ScanScope::ALL));
    assert!(!ides.covers(cli));
    // 位图按来源独立：插入 IDE 不影响 CLI。
    assert!(ides.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Cn)));
    assert!(!ides.contains(ScanSource::WorkBuddy(WbVariant::Ai)));

    let now = 1_000_000;
    let cache = ScanCache {
        at: now,
        scope: ScanScope::ALL,
        entries: Vec::new(),
    };
    assert!(
        cache.covers(ides, now + SCAN_MIN_INTERVAL_MS - 1),
        "未过期且范围是超集 → 命中（装 hook 后只扫 IDE 的请求由全量缓存满足）"
    );
    assert!(
        !cache.covers(ides, now + SCAN_MIN_INTERVAL_MS),
        "过期缓存不命中"
    );
    let narrow = ScanCache {
        at: now,
        scope: ides,
        entries: Vec::new(),
    };
    assert!(
        !narrow.covers(ScanScope::ALL, now),
        "范围不足（子集）不命中，必须重扫"
    );
    assert!(narrow.covers(ides, now), "同一范围命中");
}

/// 测试用扫描范围输入：`dir` 下按固定名字放 CLI / WorkBuddy 两档位 / 两个 IDE 的数据根。
///
/// 只拼路径，不建目录 / 文件：存在性与配置内容由各测试自己决定。
fn scan_roots_for(dir: &Path, marker: &str) -> ScanRoots {
    let cli_root = dir.join(".codebuddy");
    let wb_cn = dir.join(".workbuddy");
    let wb_ai = dir.join(".workbuddy-ai");
    ScanRoots {
        hook_sources: vec![
            (
                ScanSource::WorkBuddy(WbVariant::Cn),
                wb_cn.clone(),
                wb_cn.join(SETTINGS_FILE_NAME),
            ),
            (
                ScanSource::WorkBuddy(WbVariant::Ai),
                wb_ai.clone(),
                wb_ai.join(SETTINGS_FILE_NAME),
            ),
            (
                ScanSource::Cli,
                cli_root.clone(),
                cli_root.join(SETTINGS_FILE_NAME),
            ),
        ],
        ide_sources: vec![
            (
                ScanSource::Ide(CodeBuddyIdeFlavor::Intl),
                dir.join("CodeBuddy"),
            ),
            (
                ScanSource::Ide(CodeBuddyIdeFlavor::Cn),
                dir.join("CodeBuddy CN"),
            ),
        ],
        marker: marker.to_string(),
    }
}

/// 注册命令里的脚本路径 marker（与 `HookLayout::marker` 同构：单引号包裹的绝对路径）。
fn quoted_marker(dir: &Path) -> String {
    format!(
        "'{}'",
        dir.join(".workbuddy-shift")
            .join("hook.sh")
            .to_string_lossy()
    )
}

/// 已注册本工具 hook 的 `settings.json` 内容。
///
/// `marker` 是注册命令里脚本路径的**原样形态**（含引号，与 `hook_command` 一致）：
/// 这里按真实形态拼装，保证与 `HookLayout::marker` 的判定同构。
fn registered_settings(marker: &str) -> String {
    json!({
        "hooks": {
            "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": format!("sh {marker}") }] }],
            "FinalStop": [{ "matcher": "", "hooks": [{ "type": "command", "command": format!("sh {marker}") }] }],
        }
    })
    .to_string()
}

/// 逐来源判定扫描范围（注入 tempdir 路径，不触碰真实配置）：
///
/// - 数据根不存在 → 不扫（也不因其缺失而报错）；
/// - 数据根存在且该处未注册 hook → 扫（某处注册失败只有那一处回退）；
/// - 数据根存在且已注册 hook → 不扫（交给事件通路）；
/// - IDE 数据根存在且 `scanIdeLogs` 开启 → 扫（IDE 的 429 不触发任何事件）。
#[test]
fn scan_scope_is_decided_per_source() {
    let dir = temp_dir("scan-scope");
    let marker = quoted_marker(&dir);
    let registered = registered_settings(&marker);

    let cli_root = dir.join(".codebuddy");
    let wb_cn = dir.join(".workbuddy");
    let ide_intl = dir.join("CodeBuddy");
    let ide_cn = dir.join("CodeBuddy CN");
    // 已安装：CLI（已注册）、WorkBuddy 国内版（配置里**没有** marker）、两个 IDE。
    std::fs::create_dir_all(&cli_root).expect("CLI 数据根");
    std::fs::write(cli_root.join(SETTINGS_FILE_NAME), &registered).expect("CLI 配置");
    std::fs::create_dir_all(&wb_cn).expect("WorkBuddy 数据根");
    std::fs::write(wb_cn.join(SETTINGS_FILE_NAME), "{}").expect("WorkBuddy 配置");
    std::fs::create_dir_all(&ide_intl).expect("IDE 数据根");
    std::fs::create_dir_all(&ide_cn).expect("CN IDE 数据根");

    let roots = scan_roots_for(&dir, &marker);

    let scope = scan_scope(&roots, true);
    assert!(
        !scope.contains(ScanSource::Cli),
        "已注册 hook 的来源不扫日志"
    );
    assert!(
        scope.contains(ScanSource::WorkBuddy(WbVariant::Cn)),
        "未注册的来源回退日志扫描"
    );
    assert!(
        !scope.contains(ScanSource::WorkBuddy(WbVariant::Ai)),
        "不存在的数据根不参与扫描，也不因其缺失报错"
    );
    assert!(scope.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Intl)));
    assert!(scope.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Cn)));

    // 用户手删了 CLI 的条目 → 只有 CLI 回退扫描，其余来源不受影响。
    std::fs::write(cli_root.join(SETTINGS_FILE_NAME), "{}").expect("手删条目");
    let scope = scan_scope(&roots, true);
    assert!(
        scope.contains(ScanSource::Cli),
        "条目被删的来源必须回退扫描"
    );
    assert!(scope.contains(ScanSource::WorkBuddy(WbVariant::Cn)));

    // 某处配置写坏（非法 JSON）按「未注册」处理 → 该来源回退扫描。
    std::fs::write(cli_root.join(SETTINGS_FILE_NAME), "{ not json").expect("写坏配置");
    assert!(scan_scope(&roots, true).contains(ScanSource::Cli));

    // 注册齐全 + 数据根都在 → 只剩两个 IDE。
    std::fs::write(cli_root.join(SETTINGS_FILE_NAME), &registered).expect("重新注册");
    std::fs::write(wb_cn.join(SETTINGS_FILE_NAME), &registered).expect("注册 WorkBuddy");
    let scope = scan_scope(&roots, true);
    assert!(!scope.contains(ScanSource::Cli));
    assert!(!scope.contains(ScanSource::WorkBuddy(WbVariant::Cn)));
    assert!(scope.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Intl)));
    assert!(scope.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Cn)));

    std::fs::remove_dir_all(&dir).ok();
}

/// `scanIdeLogs = false` **只关两个 IDE**：CLI / WorkBuddy 的逐来源判定完全不变。
#[test]
fn ide_scan_switch_excludes_only_the_two_ide_sources() {
    let dir = temp_dir("ide-switch");
    let marker = quoted_marker(&dir);
    let registered = registered_settings(&marker);

    let cli_root = dir.join(".codebuddy");
    let wb_cn = dir.join(".workbuddy");
    std::fs::create_dir_all(&cli_root).expect("CLI 数据根");
    std::fs::write(cli_root.join(SETTINGS_FILE_NAME), &registered).expect("CLI 配置");
    std::fs::create_dir_all(&wb_cn).expect("WorkBuddy 数据根");
    std::fs::write(wb_cn.join(SETTINGS_FILE_NAME), "{}").expect("WorkBuddy 配置");
    std::fs::create_dir_all(dir.join("CodeBuddy")).expect("IDE 数据根");
    std::fs::create_dir_all(dir.join("CodeBuddy CN")).expect("CN IDE 数据根");

    let roots = scan_roots_for(&dir, &marker);

    // 关闭：两个 IDE 一个都不扫；CLI（已注册）仍不扫、WorkBuddy（未注册）仍回退扫描。
    let off = scan_scope(&roots, false);
    assert!(!off.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Intl)));
    assert!(!off.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Cn)));
    assert!(
        !off.contains(ScanSource::Cli),
        "已注册 hook 的判定不因 IDE 开关改变"
    );
    assert!(
        off.contains(ScanSource::WorkBuddy(WbVariant::Cn)),
        "未接 hook 的档位仍回退日志扫描"
    );

    // 重新开启 → 两个 IDE 回到范围（是关闭时的超集）。
    let on = scan_scope(&roots, true);
    assert!(on.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Intl)));
    assert!(on.contains(ScanSource::Ide(CodeBuddyIdeFlavor::Cn)));
    assert!(on.covers(off), "开启是关闭的超集");

    std::fs::remove_dir_all(&dir).ok();
}

/// 缓存作废语义 + 保存配置的范围同步。
///
/// 装 / 卸 hook 与开关类变更共用同一条：**只清缓存**，下一次扫描严格按请求的 scope 走。
/// 这里用一个 `NONE` 范围的请求证明「不再有任何强制全量」——它一个来源都不扫，
/// 因此可在单测里安全调用（不读真实日志），且扫描后缓存的 scope 就是 `NONE`。
///
/// 本测试是唯一操作进程级 `SCAN_CACHE` 的测试：它是全局静态，拆成多个会并行互相干扰。
#[test]
fn cache_invalidation_keeps_the_next_scan_on_the_requested_scope() {
    let dir = temp_dir("cache-sync");
    std::fs::create_dir_all(&dir).expect("临时目录");
    let path = dir.join("rate_limit_config.json");
    crate::modules::config::save_rate_limit_config_at(&path, &json!({ "scanIdeLogs": true }))
        .expect("写入配置");

    // ① 装 / 卸 hook 后作废：缓存被清掉。
    *SCAN_CACHE.lock().unwrap() = Some(ScanCache {
        at: 1,
        scope: ScanScope::ALL,
        entries: Vec::new(),
    });
    invalidate_scan_cache();
    assert!(SCAN_CACHE.lock().unwrap().is_none(), "缓存必须清掉");

    // ② 下一次扫描按请求的 scope 走，不会被放大成「全来源」。
    let (at, entries) = cached_scan(ScanScope::NONE, 1_000_000, true);
    assert_eq!(at, 1_000_000);
    assert!(entries.is_empty(), "NONE 范围不扫任何来源");
    assert_eq!(
        SCAN_CACHE.lock().unwrap().as_ref().map(|c| c.scope),
        Some(ScanScope::NONE),
        "缓存记录的必须是请求的范围，不是全量"
    );

    // ③ 保存配置：`scanIdeLogs` 变化才作废缓存；其它字段变化不动缓存。
    *SCAN_CACHE.lock().unwrap() = Some(ScanCache {
        at: 1,
        scope: ScanScope::NONE,
        entries: Vec::new(),
    });
    save_rate_limit_config_at(&path, &json!({ "scanIdeLogs": true, "enabled": false }))
        .expect("保存其它字段");
    assert!(
        SCAN_CACHE.lock().unwrap().is_some(),
        "非范围字段变化不清缓存"
    );

    save_rate_limit_config_at(&path, &json!({ "scanIdeLogs": false })).expect("关闭 IDE 日志扫描");
    assert!(
        SCAN_CACHE.lock().unwrap().is_none(),
        "scanIdeLogs 变化必须清缓存"
    );
    assert_eq!(
        crate::modules::config::load_rate_limit_config_at(&path)
            .get("scanIdeLogs")
            .and_then(Value::as_bool),
        Some(false)
    );

    // ④ 装 / 卸 hook 的作废同样不改变范围判定：已注册 hook 的来源不会因为「刚装过 hook」
    //    被带回来（没有强制全量，也没有接入前的历史补扫）——首次查询、重启、缓存过期
    //    都由同一条 `scan_scope` 计算覆盖。
    let root = temp_dir("cache-sync-scope");
    let marker = quoted_marker(&root);
    let cli_root = root.join(".codebuddy");
    std::fs::create_dir_all(&cli_root).expect("CLI 数据根");
    std::fs::write(
        cli_root.join(SETTINGS_FILE_NAME),
        registered_settings(&marker),
    )
    .expect("CLI 配置");
    invalidate_scan_cache();
    assert!(
        !scan_scope(&scan_roots_for(&root, &marker), true).contains(ScanSource::Cli),
        "已注册 hook 的来源始终不扫日志"
    );
    std::fs::remove_dir_all(&root).ok();

    std::fs::remove_dir_all(&dir).ok();
}

// -----------------------------------------------------------------------
// 第四轮 13-B：跨客户端兜底（borrowed）只在 VS Code 无自有记录时产出
// -----------------------------------------------------------------------

#[test]
fn payload_borrows_latest_other_client_limit_only_without_vscode_record() {
    let now = 1_000_000;
    let entry = |source: &str, reset_at: i64| Resolved {
        account_id: "a".to_string(),
        model: Some("hy3".to_string()),
        reset_at,
        first_seen_at: 0,
        hit_count: 1,
        source: source.to_string(),
    };

    let payload = build_payload(
        vec![
            entry("workbuddy-cn", now + 60_000),
            entry("codebuddy-cli", now + 180_000),
        ],
        now,
        now,
        true,
    );
    let borrowed = &payload["accounts"][0]["borrowed"];
    assert_eq!(borrowed["resetAt"], now + 180_000, "取同账号最晚的恢复时刻");
    assert_eq!(borrowed["source"], "codebuddy-cli");
    assert_eq!(borrowed["sourceLabel"], "CodeBuddy CLI");

    // 关闭推断：不产出 borrowed。
    let disabled = build_payload(
        vec![entry("workbuddy-cn", now + 60_000)],
        now,
        now,
        false,
    );
    assert!(disabled["accounts"][0]["borrowed"].is_null());

    // VS Code 有自有记录：自有优先，不兜底。
    let own = build_payload(
        vec![
            entry("workbuddy-cn", now + 60_000),
            Resolved {
                model: Some("vscode-own".to_string()),
                source: "vscode-ext".to_string(),
                ..entry("vscode-ext", now + 240_000)
            },
        ],
        now,
        now,
        true,
    );
    assert!(own["accounts"][0]["borrowed"].is_null());
    let sources: Vec<&str> = own["accounts"][0]["limited"]
        .as_array()
        .expect("limited")
        .iter()
        .filter_map(|item| item["source"].as_str())
        .collect();
    assert!(sources.contains(&"vscode-ext"));
}
