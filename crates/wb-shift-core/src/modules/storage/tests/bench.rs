//! 存储扫描的**手动基准**（默认 `#[ignore]`，不进 CI 门禁）。
//!
//! 跑法：
//! ```text
//! cargo test -p wb-shift-core storage_bench -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 目的：给出改造前 / 改造后的**真实毫秒数**与**走盘文件数 / 整树遍历次数**，用于第三轮
//! 第 6 条的验收对照（干跑数据不得写进交付说明）。夹具是 `std::env::temp_dir()` 下自建的
//! 合成目录树，**不读写真实账号 / 客户端数据**（AGENTS §8）。
//!
//! 口径说明：两次测量都包含同一屏真实请求会做的全部工作（总览 + 会话资产 + 回收站，
//! 含 `client_running` 的进程探测），否则会把「进程探测」的固定开销算成扫描收益。
//! 每个口径跑 3 次取**最小值**：本机可能同时有别的构建在跑，平均值噪声太大。

use super::super::protected::{self, SizeCache, SizeEntry};
use super::super::roots::StorageRoots;
use super::super::{
    client_running, invalidate_scan_cache, list_entries, scan_at, scan_sessions, sessions_payload,
    trash_payload,
};
use crate::modules::variant::WbVariant;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 账号 UUID 目录数 × 每目录文件数 = 主体文件量（要构造出 2 万级文件）。
const ACCOUNTS: usize = 46;
const FILES_PER_ACCOUNT: usize = 400;
/// 受保护大目录（未命中缓存时整棵跳过，命中后按缓存给数字）。
const PROTECTED_DIRS: [(&str, usize); 3] = [("binaries", 400), ("plugins", 300), ("blobs", 200)];
/// 留在大目录快扫里的可清理目录。
const KEPT_DIRS: [(&str, usize); 2] = [("logs", 200), ("traces", 200)];
const SESSIONS: usize = 60;

/// 改造前每屏的**整树遍历次数**：工具根 1 + 客户端根 1 + 回收站 2 + 会话树 2。
const OLD_WALKS: usize = 6;
/// 改造后每屏的整树遍历次数：工具根 1（回收站体积来自同一趟）+ 客户端根 1 + 会话树 1。
const NEW_WALKS: usize = 3;

fn write_files(dir: &Path, count: usize, bytes: usize) {
    std::fs::create_dir_all(dir).expect("建目录");
    for index in 0..count {
        std::fs::write(dir.join(format!("f{index:05}.bin")), vec![b'x'; bytes]).expect("写文件");
    }
}

fn account_name(index: usize) -> String {
    format!("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e{index:04}")
}

fn build_tree(base: &Path) -> (StorageRoots, PathBuf) {
    let store = base.join(".workbuddy-shift");
    let client = base.join(".workbuddy");
    std::fs::create_dir_all(&store).expect("建工具根");
    std::fs::create_dir_all(&client).expect("建客户端根");

    // 账号 UUID 目录：改造前要全量遍历，改造后延迟计量。
    for account in 0..ACCOUNTS {
        write_files(&client.join(account_name(account)), FILES_PER_ACCOUNT, 64);
    }
    // 受保护大目录。
    for (name, count) in PROTECTED_DIRS {
        write_files(&client.join(name), count, 512);
    }
    // 留在快扫里的目录（可清理）。
    for (name, count) in KEPT_DIRS {
        write_files(&client.join(name), count, 128);
    }

    // 会话资产：60 个会话 × 6 个子项。
    let conn = Connection::open(client.join("workbuddy.db")).expect("建库");
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY, cwd TEXT NOT NULL, user_id TEXT NOT NULL,
            title TEXT, custom_title TEXT, status TEXT NOT NULL DEFAULT 'Completed',
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            deleted_at INTEGER, is_playground INTEGER NOT NULL DEFAULT 0
        );",
    )
    .expect("建表");
    std::fs::create_dir_all(client.join("projects/ws")).expect("建 projects");
    for index in 0..SESSIONS {
        let id = format!("session-{index:03}");
        conn.execute(
            "INSERT INTO sessions (id, cwd, user_id, title, custom_title, created_at, updated_at, is_playground)
             VALUES (?1, 'D:/proj', 'u1', ?2, NULL, 1, 2, 0)",
            rusqlite::params![id, format!("标题 {index}")],
        )
        .expect("写行");
        std::fs::write(
            client.join(format!("projects/ws/{id}.jsonl")),
            vec![b'y'; 2048],
        )
        .expect("写正文");
        std::fs::create_dir_all(client.join(format!("projects/ws/{id}"))).expect("建附属目录");
        std::fs::write(
            client.join(format!("projects/ws/{id}/tool-result.json")),
            b"{}",
        )
        .expect("写附属");
        write_files(&client.join(format!("file-history/{id}")), 3, 64);
        std::fs::create_dir_all(client.join("artifact-index")).expect("建索引目录");
        std::fs::write(
            client.join(format!("artifact-index/{id}.json")),
            b"{\"uri\":\"D:/proj/a.ts\"}",
        )
        .expect("写索引");
        std::fs::create_dir_all(client.join("changes-index")).expect("建变更索引");
        std::fs::write(client.join(format!("changes-index/{id}.json")), b"{}").expect("写变更索引");
        write_files(&client.join(format!("changes-detail/{id}")), 2, 64);
    }

    // 工具数据：回收站 + 基线 + 关联主表。
    write_files(&store.join("trash/20260101-000000"), 40, 32);
    std::fs::write(store.join("session_links.json"), b"{\"groups\":[]}").expect("写关联主表");
    write_files(&store.join("session-links/baselines"), 20, 64);

    let roots = StorageRoots::new(store, client.clone(), base.join(".workbuddy-ai"));
    (roots, client)
}

/// 夹具的真实文件总数（不含 `workbuddy.db`）：用于核对「确实是 2 万级」。
fn fixture_files() -> u64 {
    let accounts = (ACCOUNTS * FILES_PER_ACCOUNT) as u64;
    let protected: u64 = PROTECTED_DIRS.iter().map(|(_, count)| *count as u64).sum();
    let kept: u64 = KEPT_DIRS.iter().map(|(_, count)| *count as u64).sum();
    // 每个会话：正文 1 + 附属 1 + file-history 3 + artifact-index 1 + changes-index 1 + changes-detail 2
    let sessions = (SESSIONS * 9) as u64;
    accounts + protected + kept + sessions + 40 + 1 + 20
}

/// 改造前的一屏开销：总览（全量 `usage_index` × 根 + 会话扫描 + 回收站走盘）+
/// 会话资产（**再扫一次**会话树）+ 回收站（manifest + 再走一次盘）。
///
/// 返回 `(毫秒, 走盘文件数)`。
fn old_screen_cost(roots: &StorageRoots, data_root: &Path) -> (u128, u64) {
    let started = Instant::now();
    let store = super::super::roots::usage_index(&roots.store, 2);
    let client = super::super::roots::usage_index(&roots.client_cn, 2);
    let _ = super::super::roots::usage_of(&roots.trash_root());
    let bundles_first = scan_sessions(&roots.store, data_root, WbVariant::Cn);
    let _ = client_running(WbVariant::Cn);
    let _ = super::super::roots::usage_of(&roots.trash_root());
    let bundles_second = super::super::scan_sessions(&roots.store, data_root, WbVariant::Cn);
    let _ = list_entries(roots);
    let _ = client_running(WbVariant::Cn);
    let walked = store.get("").map(|usage| usage.files).unwrap_or(0)
        + client.get("").map(|usage| usage.files).unwrap_or(0)
        + bundles_first
            .iter()
            .map(|bundle| bundle.total().files)
            .sum::<u64>()
        + bundles_second
            .iter()
            .map(|bundle| bundle.total().files)
            .sum::<u64>();
    (started.elapsed().as_millis(), walked)
}

/// 改造后首屏**真正走盘**的文件数。
///
/// 受保护大目录（账号 UUID 目录 / 运行时 / 插件 / 图片附件）在缓存命中时只读一次 JSON、
/// 不走盘，所以必须把缓存里的文件数从「客户端根总量」里扣掉再计；否则会把「读 JSON」
/// 也算成扫描收益，数字虚高。
fn new_walked_files(scan: &Value, sessions: &Value, cached_files: u64) -> u64 {
    let store_files = scan["roots"][0]["files"].as_u64().unwrap_or(0);
    let client_files = scan["roots"][1]["files"].as_u64().unwrap_or(0);
    let session_files = sessions["sessions"]
        .as_array()
        .map(|bundles| {
            bundles
                .iter()
                .map(|bundle| bundle["files"].as_u64().unwrap_or(0))
                .sum::<u64>()
        })
        .unwrap_or(0);
    // 会话树（`projects`）既在客户端快扫里，又被 `scan_sessions` 逐会话走一遍 —— 这是
    // 会话资产的必要代价（要按会话归集），这里如实计入。
    store_files + client_files.saturating_sub(cached_files) + session_files
}

/// 改造后「计入总量」的文件数（含从缓存读到的受保护项），用于核对没有漏算。
fn new_counted_files(scan: &Value) -> u64 {
    scan["roots"]
        .as_array()
        .map(|roots| {
            roots
                .iter()
                .map(|root| root["files"].as_u64().unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// 受保护项缓存夹具：模拟「后台补算已完成」的稳态。
fn protected_cache() -> SizeCache {
    let mut cache = SizeCache {
        at: protected::now_secs(),
        variant: "cn".to_string(),
        sizes: BTreeMap::new(),
    };
    for account in 0..ACCOUNTS {
        cache.sizes.insert(
            protected::key("cn", &account_name(account)),
            SizeEntry {
                bytes: (FILES_PER_ACCOUNT * 64) as u64,
                files: FILES_PER_ACCOUNT as u64,
            },
        );
    }
    for (name, count) in PROTECTED_DIRS {
        cache.sizes.insert(
            protected::key("cn", name),
            SizeEntry {
                bytes: (count * 512) as u64,
                files: count as u64,
            },
        );
    }
    cache
}

/// 扫描结果里某个分类的字节数（`tool.trash` 与总览卡对账用）。
fn category_bytes(scan: &Value, id: &str) -> u64 {
    scan["categories"]
        .as_array()
        .expect("分类数组")
        .iter()
        .find(|category| category["id"] == id)
        .map(|category| category["bytes"].as_u64().unwrap_or(0))
        .unwrap_or(0)
}

#[test]
#[ignore = "手动基准：需要合成 2 万级文件的目录树，跑法见文件头注释"]
fn storage_bench_before_after() {
    let _guard = super::fixture::lock_snapshot();
    let base = std::env::temp_dir().join(format!("wb-storage-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (roots, data_root) = build_tree(&base);
    let cache = protected_cache();

    // 每个口径跑 3 次取最小值：本机可能同时有别的构建在跑，平均值噪声太大。
    let mut old_best = (u128::MAX, 0_u64);
    let mut new_best = u128::MAX;
    let mut new_walked_best = 0_u64;
    let mut new_counted = 0_u64;
    let mut warm_best = u128::MAX;
    let mut phase = Value::Null;
    // 缓存里已经算好的受保护项文件数：命中缓存时这部分**不走盘**。
    let cached_files: u64 = cache.sizes.values().map(|entry| entry.files).sum();

    for _ in 0..3 {
        let (old_ms, walked) = old_screen_cost(&roots, &data_root);
        if old_ms < old_best.0 {
            old_best = (old_ms, walked);
        }

        protected::save(&roots.store, &cache).expect("写缓存");
        // 只作废「快照」，不作废受保护项体积缓存：`scan_at(refresh = true)` 会把
        // `storage_size_cache.json` 的 `at` 归零，受保护项变成 pending、总量退化成部分和，
        // 一屏的数字就不再是稳态（这里要量的是「后台补算已完成」的稳态）。
        invalidate_scan_cache();
        let clock = Instant::now();
        let scan = scan_at(&roots, WbVariant::Cn, &data_root, false);
        assert_eq!(
            scan["protectedPending"], false,
            "基准必须跑在受保护项已补齐的稳态上，否则总量是部分和"
        );
        let sessions = sessions_payload(&roots, WbVariant::Cn, &data_root);
        let trash = trash_payload(&roots, WbVariant::Cn, &data_root);
        let new_ms = clock.elapsed().as_millis();
        if new_ms < new_best {
            new_best = new_ms;
            new_walked_best = new_walked_files(&scan, &sessions, cached_files);
            new_counted = new_counted_files(&scan);
            phase = scan["timings"].clone();
        }
        // 回收站三个面上的数字必须一致（第 8 条口径）：总览卡 / 分类行 / 回收站接口。
        assert_eq!(
            scan["trash"]["bytes"],
            category_bytes(&scan, "tool.trash"),
            "总览回收站卡与分类行必须是同一个数字"
        );
        assert_eq!(
            scan["trash"]["bytes"], trash["bytes"],
            "回收站接口与总览卡必须是同一个数字"
        );

        let warm_clock = Instant::now();
        let warm = scan_at(&roots, WbVariant::Cn, &data_root, false);
        assert_eq!(warm["cached"], true, "同一 TTL 内二次进入必须命中快照");
        let cached_ms = warm_clock.elapsed().as_millis();
        if cached_ms < warm_best {
            warm_best = cached_ms;
        }
    }

    let (old_ms, old_walked) = old_best;
    let build_ms = phase["totalMs"].as_u64().unwrap_or(0);
    // 进程探测单独计时：它不属于扫描成本，必须从「扫描提速」的结论里说清楚。
    let probe_clock = Instant::now();
    let _ = client_running(WbVariant::Cn);
    let probe_ms = probe_clock.elapsed().as_millis();

    println!("=== storage bench ===");
    println!(
        "合成树：{} 个文件（{ACCOUNTS} 个账号目录 × {FILES_PER_ACCOUNT} + 受保护/可清理目录 + {SESSIONS} 个会话）",
        fixture_files()
    );
    println!("改造前一屏（总览 + 会话资产 + 回收站，会话树走 2 遍）:");
    println!("  耗时 {old_ms} ms · 整树遍历 {OLD_WALKS} 次 · 走盘 {old_walked} 个文件");
    println!("改造后一屏（同一份快照）:");
    println!("  耗时 {new_best} ms · 整树遍历 {NEW_WALKS} 次 · 走盘 {new_walked_best} 个文件");
    println!("  计入总量的文件 {new_counted} 个（差额 = 受保护项从缓存读到的 {cached_files} 个，不走盘）");
    println!("  快照阶段耗时：{phase}（构建本身 {build_ms} ms）");
    println!("改造后二次进入（纯缓存，只做 JSON 装配 + 进程探测）：{warm_best} ms");
    println!(
        "  （`client_running` 进程探测本身约 {probe_ms} ms，两次测量都已计入，不是本次收益）"
    );
    println!(
        "相对改造前：{:.0}% 更快（按一屏耗时）；走盘文件数 {:.0}% 更少",
        100.0 * (1.0 - (new_best as f64) / (old_ms.max(1) as f64)),
        100.0 * (1.0 - (new_walked_best as f64) / (old_walked.max(1) as f64))
    );

    assert!(
        old_ms > 0 && new_best > 0,
        "基准必须真的跑出数字：old={old_ms} new={new_best}"
    );
    assert!(
        new_walked_best < old_walked,
        "改造后走盘文件数必须更少：old={old_walked} new={new_walked_best}"
    );
    assert!(
        new_counted >= fixture_files(),
        "计入总量的文件数不得少于夹具真实文件数：counted={new_counted} files={}",
        fixture_files()
    );
    invalidate_scan_cache();
    let _ = std::fs::remove_dir_all(&base);
}
