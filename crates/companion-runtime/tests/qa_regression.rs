//! QA 独立回归验证（fresh eyes）：Issue #20 hook 超时修复的边界路径。
//! 这些测试由 QA 独立构造，不复用工程师写的断言，用于验证 spool 队列的
//! 不丢不重、磁盘失败兜底、并发读写与归档路径。

use companion_runtime::hook_spool::{
    append, handle_host_hook, spool_path, HookSpool, SPOOL_FILE_NAME,
};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("qa-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).unwrap();
    home
}

/// 场景：spool 文件被外部删除并重建（如用户手动清理 / 杀毒软件隔离）。
/// 期望：offset 感知到文件缩短并从头消费，既不跳过新事件，也不重复旧事件。
#[test]
fn qa_spool_deleted_and_recreated_does_not_skip_events() {
    let home = temp_home("spool-recreate");
    append(&home, &json!({"id": 1})).unwrap();
    let mut spool = HookSpool::new(&home);
    let mut first = Vec::new();
    assert_eq!(
        spool
            .drain(|v| first.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(first, vec![1]);

    fs::remove_file(spool_path(&home)).unwrap();
    append(&home, &json!({"id": 2})).unwrap();
    let mut second = Vec::new();
    assert_eq!(
        spool
            .drain(|v| second.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1
    );
    assert_eq!(second, vec![2], "重建后的事件必须被消费");

    let mut restarted = HookSpool::new(&home);
    assert_eq!(
        restarted.drain(|_| panic!("不得重复投递")).unwrap(),
        0,
        "重启后不得重复投递"
    );
    fs::remove_dir_all(&home).unwrap();
}

/// 场景：消费回调 panic（模拟 ingest 内部异常）。
/// 期望：offset 不推进，重启后事件仍在，实现 at-least-once 而非丢事件。
#[test]
fn qa_consumer_panic_keeps_event_for_redelivery() {
    let home = temp_home("consumer-panic");
    append(&home, &json!({"id": 1})).unwrap();
    {
        let mut spool = HookSpool::new(&home);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = spool.drain(|_| panic!("qa boom"));
        }));
        assert!(result.is_err(), "回调 panic 应被 catch 到");
    }
    let mut restarted = HookSpool::new(&home);
    let mut ids = Vec::new();
    assert_eq!(
        restarted
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1,
        "panic 后事件必须重投递，不能丢"
    );
    assert_eq!(ids, vec![1]);
    fs::remove_dir_all(&home).unwrap();
}

/// 场景：daemon 关闭期间积压了超过 16MiB 的事件，随后 daemon 启动。
/// 期望（设计承诺）：这些未消费事件应被补采，而不是静默丢弃。
/// 若失败，说明 HookSpool::new() 在 drain 之前就 rename 归档，丢掉了未消费事件。
#[test]
fn qa_oversized_spool_should_not_drop_unconsumed_events() {
    let home = temp_home("oversized");
    let spool_file = spool_path(&home);
    fs::create_dir_all(spool_file.parent().unwrap()).unwrap();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spool_file)
        .unwrap();
    let line = format!("{}\n", json!({"id": 1, "pad": "x".repeat(200)}));
    let target = 16 * 1024 * 1024 + 4096;
    let mut written = 0usize;
    while written < target {
        file.write_all(line.as_bytes()).unwrap();
        written += line.len();
    }
    drop(file);

    let mut spool = HookSpool::new(&home);
    let mut count = 0usize;
    let _ = spool.drain(|_| count += 1);
    assert!(
        count > 0,
        "超过 {SPOOL_FILE_NAME} 大小阈值时，未消费事件不应被静默丢弃"
    );
    fs::remove_dir_all(&home).unwrap();
}

/// 场景：磁盘写入失败（home 落在普通文件下，create_dir_all 必然失败）。
/// 期望：hook 仍先输出 `{}`、返回 Ok(true) 接管，绝不拦截宿主。
#[test]
fn qa_host_hook_prints_object_even_when_disk_write_fails() {
    let home = temp_home("disk-fail");
    let blocker = home.join("not-a-dir");
    fs::write(&blocker, b"x").unwrap();
    let failed_home = blocker.join("child");
    let mut out = Vec::new();
    let handled = handle_host_hook(
        &failed_home,
        br#"{"hook_event_name":"Stop"}"#,
        "workbuddy",
        Some("domestic"),
        &mut out,
    )
    .unwrap();
    assert!(handled, "必须接管，不能再走同步 RPC");
    assert_eq!(out, b"{}\n", "磁盘失败也必须已输出 `{{}}`");
    fs::remove_dir_all(&home).unwrap();
}

/// 场景：daemon 每 100ms drain 的同时，多个 hook 进程持续 append。
/// 期望：并发读写下事件不丢不重（append 独占锁 vs drain 只整行读取）。
#[test]
fn qa_concurrent_drain_and_append_never_loses_events() {
    let home = temp_home("concurrent-drain");
    let writer_home = home.clone();
    let writer = std::thread::spawn(move || {
        for id in 0..100u64 {
            append(&writer_home, &json!({"id": id})).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let mut spool = HookSpool::new(&home);
    let mut ids = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while ids.len() < 100 && Instant::now() < deadline {
        let _ = spool.drain(|v| ids.push(v["id"].as_u64().unwrap()));
        std::thread::sleep(Duration::from_millis(5));
    }
    writer.join().unwrap();
    let _ = spool.drain(|v| ids.push(v["id"].as_u64().unwrap()));
    ids.sort_unstable();
    assert_eq!(ids, (0..100).collect::<Vec<_>>(), "并发读写不得丢事件");
    fs::remove_dir_all(&home).unwrap();
}
