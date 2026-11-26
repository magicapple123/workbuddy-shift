//! QA 第 2 轮归档边界补测（fresh eyes）：验证 BUG-1 修复（收紧归档条件为
//! `len > MAX_SPOOL_BYTES && offset >= len`）没有引入新的丢事件 / 假归档问题。
//!
//! 覆盖三条边界：
//! 1. 积压超限且 offset < len：不得假归档，drain 必须消费全部积压事件；
//! 2. 全部消费完（offset == len）且超限：必须归档到 .done、offset 归零、新事件续写不丢；
//! 3. 归档后 daemon 再启动：新 spool 路径正确、checkpoint 指纹不误判（不重不漏）。

use companion_runtime::hook_spool::{append, spool_path, HookSpool};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_SPOOL_BYTES: u64 = 16 * 1024 * 1024;
const DONE_FILE_NAME: &str = "companion-hook-spool.done";
const CHECKPOINT_FILE_NAME: &str = "companion-hook-spool.offset";

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("qa-archive-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).unwrap();
    home
}

fn state_dir(home: &Path) -> PathBuf {
    home.join(".workbuddy-shift-companion")
}

fn done_path(home: &Path) -> PathBuf {
    state_dir(home).join(DONE_FILE_NAME)
}

fn checkpoint_path(home: &Path) -> PathBuf {
    state_dir(home).join(CHECKPOINT_FILE_NAME)
}

fn read_checkpoint(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(checkpoint_path(home)).unwrap()).unwrap()
}

/// 写入超过 16MiB 阈值的完整事件行（用较长 pad 控制行数，加快解析）。
/// 返回 (事件条数, 文件字节数)。
fn write_oversized_spool(home: &Path) -> (usize, u64) {
    let file = spool_path(home);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut handle = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .unwrap();
    let target = MAX_SPOOL_BYTES + 4096;
    let mut written = 0u64;
    let mut count = 0usize;
    while written < target {
        let line = format!("{}\n", json!({"id": count, "pad": "x".repeat(4000)}));
        handle.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
        count += 1;
    }
    drop(handle);
    (count, written)
}

/// 边界 1：积压超阈值且 offset < len（daemon 长期未运行后启动）。
/// 期望：本轮不归档、不触碰原文件，drain 能消费全部积压事件，不构成假归档。
#[test]
fn backlog_over_threshold_is_drained_not_falsely_archived() {
    let home = temp_home("backlog");
    let (count, bytes) = write_oversized_spool(&home);
    assert!(bytes > MAX_SPOOL_BYTES, "前置条件：文件必须超限");

    // offset=0 < len：HookSpool::new 不得归档。
    let mut spool = HookSpool::new(&home);
    assert!(!done_path(&home).exists(), "积压未消费时不得假归档到 .done");
    assert!(
        spool_path(&home).exists(),
        "积压未消费时 spool 文件必须保留"
    );
    assert_eq!(fs::metadata(spool_path(&home)).unwrap().len(), bytes);

    // drain 必须消费全部积压事件，一条不丢、一条不重。
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        count,
        "全部积压事件都应被消费"
    );
    assert_eq!(
        ids,
        (0..count as u64).collect::<Vec<_>>(),
        "积压事件必须不丢不重"
    );
    assert!(!done_path(&home).exists(), "drain 消费过程不应产生归档文件");
    assert_eq!(
        read_checkpoint(&home)["offset"].as_u64().unwrap(),
        bytes,
        "消费完 offset 应等于原文件长度"
    );
    fs::remove_dir_all(&home).unwrap();
}

/// 边界 2：全部消费完（offset == len）且超阈值时重启。
/// 期望：归档到 .done 正常、原 spool 路径让位、offset 归零、新事件续写不丢。
#[test]
fn fully_consumed_over_threshold_archives_and_new_events_survive() {
    let home = temp_home("full-archive");
    let (count, bytes) = write_oversized_spool(&home);

    // 第一轮：daemon 消费全部积压，offset 追上 len。此时 offset==len 是在 drain 后达成的，
    // 但 new() 时 offset=0 < len，所以本轮同样不应归档。
    {
        let mut spool = HookSpool::new(&home);
        assert_eq!(spool.drain(|_| {}).unwrap(), count);
        assert!(!done_path(&home).exists());
        assert_eq!(read_checkpoint(&home)["offset"].as_u64().unwrap(), bytes);
    }

    // 第二轮重启：offset==len 且超限 → 归档、offset 归零。
    {
        let mut spool = HookSpool::new(&home);
        assert!(done_path(&home).exists(), "已消费完且超限时应归档到 .done");
        assert_eq!(
            fs::metadata(done_path(&home)).unwrap().len(),
            bytes,
            "归档文件应保留原始内容"
        );
        assert!(
            !spool_path(&home).exists(),
            "归档后原路径应不存在，等待新事件重建"
        );
        assert_eq!(
            read_checkpoint(&home)["offset"].as_u64().unwrap(),
            0,
            "归档后 offset 必须归零"
        );
        assert_eq!(
            read_checkpoint(&home)["tail"].as_array().unwrap().len(),
            0,
            "归档后指纹必须清空"
        );

        // 归档后新事件写入新 spool 文件，不得丢失。
        append(&home, &json!({"id": "after-archive"})).unwrap();
        let mut got = Vec::new();
        assert_eq!(
            spool
                .drain(|v| got.push(v["id"].as_str().unwrap().to_string()))
                .unwrap(),
            1
        );
        assert_eq!(got, vec!["after-archive".to_string()]);
    }
    fs::remove_dir_all(&home).unwrap();
}

/// 边界 3：归档后 daemon 再启动。
/// 期望：新 spool 文件路径正确、checkpoint 指纹不误判（重启后只消费新事件，不重不漏），
/// 且已归档的 .done 文件不被后续写入改动。
#[test]
fn restart_after_archive_uses_fresh_spool_and_valid_fingerprint() {
    let home = temp_home("restart-after-archive");
    let (count, _bytes) = write_oversized_spool(&home);

    // 消费完并重启触发归档。
    {
        let mut spool = HookSpool::new(&home);
        assert_eq!(spool.drain(|_| {}).unwrap(), count);
    }
    {
        let _spool = HookSpool::new(&home);
        assert!(done_path(&home).exists(), "重启应完成归档");
        assert_eq!(read_checkpoint(&home)["offset"].as_u64().unwrap(), 0);
    }

    // 归档后 daemon 再次启动：offset 保持 0，新 spool 路径正确。
    let mut spool = HookSpool::new(&home);
    assert_eq!(
        read_checkpoint(&home)["offset"].as_u64().unwrap(),
        0,
        "归档后重启 offset 应保持 0，不得误判指纹"
    );
    let done_before = fs::metadata(done_path(&home)).unwrap().len();

    append(&home, &json!({"id": 1})).unwrap();
    append(&home, &json!({"id": 2})).unwrap();
    append(&home, &json!({"id": 3})).unwrap();
    assert!(
        spool_path(&home).metadata().unwrap().len() > 0,
        "新事件应写入 spool 路径（而非 .done）"
    );
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        3
    );
    assert_eq!(ids, vec![1, 2, 3]);

    // 再次重启，指纹应继续有效：只消费新增事件，不重复也不跳过。
    assert!(
        !read_checkpoint(&home)["tail"]
            .as_array()
            .unwrap()
            .is_empty(),
        "消费后应记录指纹"
    );
    drop(spool);
    let mut spool = HookSpool::new(&home);
    append(&home, &json!({"id": 4})).unwrap();
    let mut ids = Vec::new();
    assert_eq!(
        spool
            .drain(|v| ids.push(v["id"].as_u64().unwrap()))
            .unwrap(),
        1,
        "指纹有效时重启后只消费新事件"
    );
    assert_eq!(ids, vec![4]);
    assert_eq!(
        fs::metadata(done_path(&home)).unwrap().len(),
        done_before,
        "归档文件不应被后续写入改动"
    );
    fs::remove_dir_all(&home).unwrap();
}
