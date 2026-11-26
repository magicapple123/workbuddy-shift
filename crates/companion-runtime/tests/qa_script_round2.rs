//! QA 第 2 轮独立验证（fresh eyes）：issue #20 回归修复——注册轻量 shell 脚本。
//!
//! 工程师自报「脚本 100–141ms、stdout 恒 `{}`、解码器四态正确」。这里不复用他的断言：
//!
//! 1. **真跑注册命令**：从 `hook_script::command()` 取宿主实际会执行的字符串，用真正的
//!    `sh` / `bash` 喂 stdin，逐字节核对 stdout / stderr / spool；
//! 2. **敌意主目录**：空格 + 中文 + 单引号（`o'brien`）同时出现，验证 POSIX 引号配对；
//! 3. **解码器极端输入**：半截帧、坏行、超长 payload、新旧混排、跨行 payload；
//! 4. **并发写入**：脚本没有文件锁（旧 exe 有 fs2 排他锁），所以不再追加共享文件，而是
//!    「一事件一文件」（`hook-inbox.d/`：私有 `.part` + `mv` 发布，见 `hook_inbox`）。
//!    末尾两条用例原本是 BUG-QA-1 的复现，修复后已去掉 `#[ignore]`，作为常驻回归。

use companion_runtime::hook_frame::{Frame, HookFrameDecoder, SOURCE_DIRECTIVE};
use companion_runtime::hook_inbox::inbox_dir;
use companion_runtime::hook_script;
use companion_runtime::hook_spool::{spool_path, HookSpool};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// 本机可用的 sh 解释器；CI 的裸 Windows runner 没有 PortableGit 时跳过执行类用例。
fn interpreter() -> Option<String> {
    ["bash", "sh"]
        .into_iter()
        .find(|name| {
            Command::new(name)
                .arg("-c")
                .arg("exit 0")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .map(str::to_string)
}

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("qa-r2-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// 空格 + 中文 + 单引号同时出现在主目录里。
fn hostile_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join("qa r2 中文 o'brien 空格")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// 按宿主的方式执行注册命令：把 `bash '<脚本>'` 这一整串交给解释器。
fn run_registered(name: &str, registered: &str, stdin: &[u8]) -> (String, String, bool) {
    let mut child = Command::new(name)
        .arg("-c")
        .arg(registered)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(stdin).unwrap();
    let output = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

fn drain(home: &std::path::Path) -> Vec<Value> {
    let mut spool = HookSpool::new(home);
    let mut events = Vec::new();
    spool.drain(|value| events.push(value)).unwrap();
    events
}

fn event(frame: Frame) -> Value {
    match frame {
        Frame::Event(value) => value,
        other => panic!("期望完整事件，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1. 脚本本体：敌意路径下真跑
// ---------------------------------------------------------------------------

#[test]
fn script_runs_from_a_home_with_spaces_cjk_and_a_single_quote() {
    let Some(name) = interpreter() else {
        eprintln!("跳过：本机没有 sh / bash");
        return;
    };
    let home = hostile_home();
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    let registered =
        hook_script::command(&hook_script::script_path(&home, "workbuddy", "domestic"));
    let payload = br#"{"hook_event_name":"Stop","session_id":"o'brien"}"#;
    let (stdout, stderr, ok) = run_registered(&name, &registered, payload);
    assert!(ok, "脚本执行失败：{stderr}");
    assert_eq!(stdout, "{}", "宿主只接受 `{{}}`；实际 {stdout:?}");
    assert!(stderr.is_empty(), "stderr 必须为空：{stderr:?}");
    // 事件文件必须逐字节等于「指令行 + 原样 payload」，一条事件只占一个文件。
    let published: Vec<_> = std::fs::read_dir(inbox_dir(&home))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(published.len(), 1, "一条事件只发布一个文件：{published:?}");
    let raw = std::fs::read(&published[0]).unwrap();
    let expected = format!(
        "{SOURCE_DIRECTIVE} workbuddy domestic\n{}\n",
        String::from_utf8_lossy(payload)
    );
    assert_eq!(String::from_utf8_lossy(&raw), expected);
    assert!(
        !spool_path(&home).exists(),
        "脚本不得再写共享 spool 文件（并发会交错）"
    );
    let events = drain(&home);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["agent_edition"], "domestic");
    assert_eq!(events[0]["session_id"], "o'brien");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn script_preserves_a_multi_line_payload_verbatim() {
    let Some(name) = interpreter() else {
        eprintln!("跳过：本机没有 sh / bash");
        return;
    };
    let home = temp_home("multiline");
    hook_script::ensure(&home, "codebuddy-ide", "international").unwrap();
    let registered = hook_script::command(&hook_script::script_path(
        &home,
        "codebuddy-ide",
        "international",
    ));
    let payload = b"{\n  \"session_id\": \"s-multi\",\n  \"hook_event_name\": \"Stop\"\n}";
    let (stdout, stderr, ok) = run_registered(&name, &registered, payload);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "{}");
    assert!(stderr.is_empty(), "{stderr:?}");
    let events = drain(&home);
    assert_eq!(events.len(), 1, "跨行 payload 必须还原成一条事件");
    assert_eq!(events[0]["session_id"], "s-multi");
    assert_eq!(events[0]["agent_source"], "codebuddy-ide");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn empty_stdin_writes_nothing_and_still_prints_an_empty_object() {
    let Some(name) = interpreter() else {
        eprintln!("跳过：本机没有 sh / bash");
        return;
    };
    let home = temp_home("empty-stdin");
    hook_script::ensure(&home, "workbuddy", "international").unwrap();
    let registered = hook_script::command(&hook_script::script_path(
        &home,
        "workbuddy",
        "international",
    ));
    let (stdout, stderr, ok) = run_registered(&name, &registered, b"");
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "{}");
    assert!(stderr.is_empty(), "{stderr:?}");
    assert!(
        !spool_path(&home).exists() || std::fs::read(spool_path(&home)).unwrap().is_empty(),
        "空 stdin 不得写出任何字节"
    );
    assert_eq!(
        std::fs::read_dir(inbox_dir(&home)).unwrap().count(),
        0,
        "空 stdin 不得发布事件文件"
    );
    assert!(drain(&home).is_empty());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn the_registered_command_never_mentions_the_executable() {
    let home = temp_home("no-exe");
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    let script = hook_script::script_path(&home, "workbuddy", "domestic");
    let registered = hook_script::command(&script);
    assert!(
        !registered.contains("companion-runtime"),
        "注册命令不得再拉起 5MB exe：{registered}"
    );
    let body = std::fs::read_to_string(&script).unwrap();
    assert!(
        !body.contains("companion-runtime"),
        "脚本正文不得拉起 exe：{body}"
    );
    for forbidden in ["curl", "http", "runtime-v1.json", "127.0.0.1"] {
        assert!(!body.contains(forbidden), "脚本不得含 {forbidden}：{body}");
    }
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn the_script_is_rewritten_only_when_its_body_changes() {
    // 幂等：重复安装不得刷新 mtime（宿主可能监听配置文件）。
    let home = temp_home("idempotent");
    let script = hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    let first = std::fs::metadata(&script).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    assert_eq!(
        std::fs::metadata(&script).unwrap().modified().unwrap(),
        first
    );
    std::fs::remove_dir_all(&home).unwrap();
}

// ---------------------------------------------------------------------------
// 2. 解码器边界四态
// ---------------------------------------------------------------------------

#[test]
fn a_legacy_line_is_never_absorbed_by_a_pending_frame() {
    let mut decoder = HookFrameDecoder::new();
    assert!(matches!(decoder.push("{oops"), Frame::Invalid));
    decoder.push(&format!("{SOURCE_DIRECTIVE} workbuddy domestic"));
    let first = event(decoder.push("{\"session_id\":\"a\"}"));
    assert_eq!(first["agent_source"], "workbuddy");
    let legacy = event(decoder.push("{\"agent_source\":\"codex\",\"session_id\":\"b\"}"));
    assert_eq!(legacy["agent_source"], "codex", "旧格式自带来源不得被改写");
    assert!(legacy["agent_edition"].is_null());
}

#[test]
fn an_oversized_multi_line_payload_is_abandoned_without_blocking_later_events() {
    let mut decoder = HookFrameDecoder::new();
    decoder.push(&format!("{SOURCE_DIRECTIVE} workbuddy domestic"));
    assert!(matches!(decoder.push("{"), Frame::Incomplete));
    let huge = "x".repeat(300 * 1024);
    assert!(matches!(decoder.push(&huge), Frame::Invalid));
    decoder.push(&format!("{SOURCE_DIRECTIVE} workbuddy domestic"));
    let recovered = event(decoder.push("{\"session_id\":\"after-huge\"}"));
    assert_eq!(recovered["session_id"], "after-huge");
}

#[test]
fn a_truncated_directive_line_falls_back_to_the_legacy_rule() {
    let mut decoder = HookFrameDecoder::new();
    assert!(matches!(decoder.push(SOURCE_DIRECTIVE), Frame::Skipped));
    let legacy = event(decoder.push("{\"agent_source\":\"workbuddy\",\"session_id\":\"s\"}"));
    assert_eq!(legacy["agent_source"], "workbuddy");
    assert!(matches!(
        decoder.push("@@workbuddy-shift-hook-sour"),
        Frame::Invalid
    ));
    let after = event(decoder.push("{\"agent_source\":\"workbuddy\",\"session_id\":\"t\"}"));
    assert_eq!(after["session_id"], "t");
}

#[test]
fn a_frame_left_open_by_a_killed_hook_is_closed_by_the_next_directive() {
    let mut decoder = HookFrameDecoder::new();
    decoder.push(&format!("{SOURCE_DIRECTIVE} workbuddy domestic"));
    assert!(matches!(
        decoder.push("{\"session_id\":"),
        Frame::Incomplete
    ));
    decoder.push(&format!("{SOURCE_DIRECTIVE} workbuddy international"));
    let fresh = event(decoder.push("{\"session_id\":\"after-kill\"}"));
    assert_eq!(fresh["session_id"], "after-kill");
    assert_eq!(fresh["agent_edition"], "international");
}

// ---------------------------------------------------------------------------
// 3. 端到端：v0.4.2 遗留 spool 与新脚本写在同一队列里
// ---------------------------------------------------------------------------

#[test]
fn a_v042_spool_is_still_consumed_after_the_switch_to_scripts() {
    let Some(name) = interpreter() else {
        eprintln!("跳过：本机没有 sh / bash");
        return;
    };
    let home = temp_home("legacy-spool");
    // 升级现场：spool 里已有 v0.4.2 exe 写的两条单行 JSON（daemon 当时没跑）。
    std::fs::create_dir_all(spool_path(&home).parent().unwrap()).unwrap();
    std::fs::write(
        spool_path(&home),
        concat!(
            "{\"agent_source\":\"workbuddy\",\"agent_edition\":\"domestic\",\"session_id\":\"old-1\"}\n",
            "{\"agent_source\":\"codebuddy-ide\",\"agent_edition\":\"international\",\"session_id\":\"old-2\"}\n"
        ),
    )
    .unwrap();
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    let registered =
        hook_script::command(&hook_script::script_path(&home, "workbuddy", "domestic"));
    let (stdout, stderr, ok) = run_registered(
        &name,
        &registered,
        br#"{"session_id":"new-1","hook_event_name":"Stop"}"#,
    );
    assert!(ok && stdout == "{}", "{stdout} / {stderr}");
    let events = drain(&home);
    let ids: Vec<&str> = events
        .iter()
        .filter_map(|event| event["session_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["old-1", "old-2", "new-1"],
        "升级后旧队列事件不得丢：{ids:?}"
    );
    assert_eq!(events[2]["agent_source"], "workbuddy");
    assert_eq!(events[2]["agent_edition"], "domestic");
    std::fs::remove_dir_all(&home).unwrap();
}

// ---------------------------------------------------------------------------
// 4. BUG-QA-1 复现：并发写入没有锁，一帧不是一次 write
// ---------------------------------------------------------------------------
//
// 脚本用 `printf '%s %s %s\n%s\n' … >> spool` 写一帧。bash 的 printf 会把「指令行」和
// 「payload」分多次 write() 落到文件（本机实测 2KB 就已分成多次），而脚本路径**没有**
// v0.4.2 exe 用的 fs2 排他锁。两个 hook 同时写就会交错，产生：
//
//     @@… domestic\n            <- A 的指令行
//     {"session_id":"w-3","blob":"xxx@@… domestic\n   <- B 的指令行粘在 A 的 payload 中间
//     xxx"}\n                    <- A 的 payload 尾巴
//     {"session_id":"w-1",…}\n  <- C 完整的一帧
//
// 解码器遇到这种 spool 时会把 C 也当成残帧续写（`pending` 非空就一直累积），
// 于是一条**字节完好**的事件也被吞掉。

/// 按上面 dump 的顺序构造交错后的 spool 内容（确定性复现，不依赖线程调度）。
fn interleaved_spool(tag: &str) -> PathBuf {
    let home = temp_home(tag);
    std::fs::create_dir_all(spool_path(&home).parent().unwrap()).unwrap();
    let directive = format!("{SOURCE_DIRECTIVE} workbuddy domestic");
    let torn_prefix = r#"{"session_id":"w-3","blob":"xxx"#;
    let intact = r#"{"session_id":"w-1","blob":"yyy"}"#;
    std::fs::write(
        spool_path(&home),
        format!("{directive}\n{torn_prefix}{directive}\nxxx\"}}\n{intact}\n"),
    )
    .unwrap();
    home
}

#[test]
fn an_interleaved_frame_must_not_swallow_the_next_intact_event() {
    let home = interleaved_spool("interleaved");
    let ids: Vec<String> = drain(&home)
        .iter()
        .filter_map(|event| event["session_id"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.contains(&"w-1".to_string()),
        "被撕裂的是 w-3（可以放弃），但字节完好的 w-1 必须照常投递；实际 {ids:?}"
    );
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn concurrent_script_writes_keep_every_event_intact() {
    let Some(name) = interpreter() else {
        eprintln!("跳过：本机没有 sh / bash");
        return;
    };
    const WRITERS: usize = 16;
    let home = temp_home("concurrent");
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    let registered =
        hook_script::command(&hook_script::script_path(&home, "workbuddy", "domestic"));
    let blob = "x".repeat(8 * 1024);
    // stdin 落成文件后重定向，让 16 个进程真正同时起跑。
    let mut inputs = Vec::new();
    for id in 0..WRITERS {
        let path = home.join(format!("in-{id}.json"));
        std::fs::write(
            &path,
            format!("{{\"session_id\":\"w-{id}\",\"blob\":\"{blob}\"}}"),
        )
        .unwrap();
        inputs.push(path);
    }
    let children: Vec<_> = inputs
        .iter()
        .map(|path| {
            Command::new(&name)
                .arg("-c")
                .arg(&registered)
                .stdin(Stdio::from(std::fs::File::open(path).unwrap()))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let events = drain(&home);
    let intact = events
        .iter()
        .filter(|event| event["blob"].as_str().map(str::len) == Some(blob.len()))
        .count();
    assert_eq!(
        intact, WRITERS,
        "并发写入丢事件：只收到 {intact}/{WRITERS} 条完整事件"
    );
    std::fs::remove_dir_all(&home).unwrap();
}
