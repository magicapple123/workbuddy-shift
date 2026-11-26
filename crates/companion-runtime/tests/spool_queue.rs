//! `HookSpool` 的队列语义与「脚本 / exe」两种写入格式的单测。
//!
//! v0.4.3 起 hook 由 shell 脚本写入「来源指令行 + payload」，而队列里可能还有
//! v0.4.2 exe 写的单行 JSON（自带 `agent_source`）。两种格式必须共存、都不得
//! 丢事件、都不得重复投递。用例从 `hook_spool.rs` 里搬出来，避免该文件超过
//! `AGENTS.md` 第 4 节的 500 行阈值（公开 API 未变，只是测试位置变化）。

use companion_runtime::hook_frame::SOURCE_DIRECTIVE;
use companion_runtime::hook_script;
use companion_runtime::hook_spool::{append, handle_host_hook, spool_path, HookSpool};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    home
}

#[test]
fn a_non_object_payload_from_the_host_is_wrapped_instead_of_panicking() {
    // `payload[key] = …` 在非 object 上会 panic（serde_json），hook 进程以 101 退出后
    // 宿主会把这次操作判成被拦截 —— 与 #20 的「软件关掉了还在拦」同一类症状。
    let home = temp_home("hook-spool-non-object");
    let mut out = Vec::new();
    let handled =
        handle_host_hook(&home, b"[1,2,3]", "workbuddy", Some("domestic"), &mut out).unwrap();
    assert!(handled);
    assert_eq!(out, b"{}\n");
    let mut spool = HookSpool::new(&home);
    let mut events = Vec::new();
    assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 1);
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["agent_edition"], "domestic");
    assert_eq!(events[0]["payload"], json!([1, 2, 3]));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn the_hook_cli_never_panics_on_a_non_object_payload() {
    // 同一个缺陷的第二条入口：`companion-runtime hook --source codex`（codex 仍走 exe）。
    let home = temp_home("hook-cli-non-object");
    for source in ["workbuddy", "codex"] {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_companion-runtime"))
            .args(["hook", "--home"])
            .arg(&home)
            .args(["--source", source])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(b"[1,2,3]").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{source} 的 hook 不得 panic：{output:?}"
        );
        assert!(
            output.stderr.is_empty(),
            "{source} 的 hook 不得把诊断写到 stderr：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut spool = HookSpool::new(&home);
    let mut events = Vec::new();
    assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 1);
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["payload"], json!([1, 2, 3]));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn host_hook_prints_empty_object_and_never_waits_for_a_slow_daemon() {
    let home = temp_home("hook-spool-slow-daemon");
    // 端点指向一个收到请求后故意睡 3 秒的服务器；旧实现会在这里超时被拦截。
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    companion_core::atomic_json(
        &spool_path(&home).parent().unwrap().join("runtime-v1.json"),
        &json!({"protocol":1,"port":port,"token":"test"}),
    )
    .unwrap();
    let _ = std::thread::spawn(move || {
        if let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(4)) {
            std::thread::sleep(Duration::from_secs(3));
            let _ = request.respond(tiny_http::Response::empty(204));
        }
    });
    let started = Instant::now();
    let mut out = Vec::new();
    let handled = handle_host_hook(
        &home,
        br#"{"hook_event_name":"UserPromptSubmit","session_id":"s-1"}"#,
        "workbuddy",
        Some("domestic"),
        &mut out,
    )
    .unwrap();
    assert!(handled);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "hook 不得等待 IPC：耗时 {:?}",
        started.elapsed()
    );
    assert_eq!(out, b"{}\n");
    // daemon 此刻才启动：落盘事件必须被补采。
    let mut spool = HookSpool::new(&home);
    let mut events = Vec::new();
    assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 1);
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["agent_edition"], "domestic");
    assert_eq!(events[0]["session_id"], "s-1");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn codex_hook_is_not_taken_over_by_the_spool() {
    let home = temp_home("hook-spool-codex");
    let mut out = Vec::new();
    assert!(!handle_host_hook(&home, b"{}", "codex", None, &mut out).unwrap());
    assert!(out.is_empty());
    assert!(!spool_path(&home).exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn script_written_frames_and_legacy_lines_are_consumed_side_by_side() {
    let home = temp_home("hook-spool-script-frames");
    // 脚本写入的形态：来源指令行 + 原样 payload（此处故意用多行 payload）。
    hook_script::ensure(&home, "workbuddy", "domestic").unwrap();
    std::fs::create_dir_all(spool_path(&home).parent().unwrap()).unwrap();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(spool_path(&home))
        .unwrap();
    file.write_all(
        format!(
            "{} workbuddy domestic\n{{\n  \"session_id\": \"s-script\",\n  \"hook_event_name\": \"Stop\"\n}}\n",
            SOURCE_DIRECTIVE
        )
        .as_bytes(),
    )
    .unwrap();
    // v0.4.2 遗留的 exe 形态：单行 JSON，自带来源，必须继续被消费。
    file.write_all(b"{\"agent_source\":\"workbuddy\",\"session_id\":\"s-legacy\"}\n")
        .unwrap();
    drop(file);
    let mut spool = HookSpool::new(&home);
    let mut events = Vec::new();
    assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 2);
    assert_eq!(events[0]["session_id"], "s-script");
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["agent_edition"], "domestic");
    assert_eq!(events[1]["session_id"], "s-legacy");
    assert_eq!(spool.drain(|_| panic!("不得重复投递")).unwrap(), 0);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn a_half_written_frame_is_not_delivered_and_does_not_hide_the_next_event() {
    let home = temp_home("hook-spool-half-frame");
    std::fs::create_dir_all(spool_path(&home).parent().unwrap()).unwrap();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(spool_path(&home))
        .unwrap();
    // 指令行写完了、payload 只写了一半（进程被强杀 / 断电）。
    file.write_all(
        format!(
            "{} workbuddy domestic\n{{\"session_id\":\"trunc\"\n",
            SOURCE_DIRECTIVE
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    let mut spool = HookSpool::new(&home);
    assert_eq!(spool.drain(|_| {}).unwrap(), 0);
    let mut file = OpenOptions::new()
        .append(true)
        .open(spool_path(&home))
        .unwrap();
    file.write_all(
        format!(
            "{} workbuddy international\n{{\"session_id\":\"fresh\"}}\n",
            SOURCE_DIRECTIVE
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    let mut events = Vec::new();
    assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 1);
    assert_eq!(events[0]["session_id"], "fresh");
    assert_eq!(events[0]["agent_edition"], "international");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn checkpoint_prevents_duplicate_delivery_and_partial_lines_are_safe() {
    let home = temp_home("hook-spool-checkpoint");
    append(&home, &json!({"id":1})).unwrap();
    append(&home, &json!({"id":2})).unwrap();
    let mut spool = HookSpool::new(&home);
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        2
    );
    assert_eq!(ids, vec![1, 2]);
    drop(spool);
    append(&home, &json!({"id":3})).unwrap();
    let mut restarted = HookSpool::new(&home);
    let mut ids = Vec::new();
    assert_eq!(
        restarted
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(ids, vec![3]);
    // 手动制造半行：下一批完整事件仍应被消费。
    let mut file = OpenOptions::new()
        .append(true)
        .open(spool_path(&home))
        .unwrap();
    file.write_all(br#"{"broken":"#).unwrap();
    drop(file);
    append(&home, &json!({"id":4})).unwrap();
    let mut ids = Vec::new();
    assert_eq!(
        restarted
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(ids, vec![4]);
    // 再次消费不得重复投递。
    assert_eq!(restarted.drain(|_| panic!("不得重复投递")).unwrap(), 0);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn truncated_spool_resets_the_offset() {
    let home = temp_home("hook-spool-truncated");
    append(&home, &json!({"id":1})).unwrap();
    let mut spool = HookSpool::new(&home);
    assert_eq!(spool.drain(|_| {}).unwrap(), 1);
    // 截断后重新写入「同样长度」的新事件：旧实现只看 len == offset 会漏掉它。
    std::fs::write(spool_path(&home), b"").unwrap();
    append(&home, &json!({"id":2})).unwrap();
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(ids, vec![2]);
    // 即便守护进程重启，指纹也必须让消费位置回到新文件的起点。
    append(&home, &json!({"id":3})).unwrap();
    let mut restarted = HookSpool::new(&home);
    let mut ids = Vec::new();
    assert_eq!(
        restarted
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(ids, vec![3]);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn concurrent_appends_keep_one_event_per_line() {
    let home = temp_home("hook-spool-concurrent");
    let mut handles = Vec::new();
    for id in 0..8u64 {
        let home = home.clone();
        handles.push(std::thread::spawn(move || {
            append(&home, &json!({"id":id})).unwrap();
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let mut spool = HookSpool::new(&home);
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        8
    );
    ids.sort_unstable();
    assert_eq!(ids, (0..8).collect::<Vec<_>>());
    std::fs::remove_dir_all(&home).unwrap();
}
