//! 存储管理测试夹具：一切都在临时沙箱里造数，不读写真实账号 / 客户端数据。
//!
//! 单独一个文件是为了让 `tests.rs` 与 `tests/snapshot.rs` 共用同一份夹具，
//! 同时把每个测试文件控制在 ~300 行（`scripts/check-structure.mjs` 的长文件门槛）。

use super::super::clean::{clean as run_clean, CleanContext};
use super::super::roots::StorageRoots;
use crate::modules::variant::WbVariant;
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// 快照是**进程级**缓存，而 cargo 默认并行跑用例：一个用例的 `clean`（内部会
/// `invalidate_scan_cache`）会把另一个用例刚建好的快照清掉，导致「复用快照」的断言
/// 随机失败。凡是要读写快照的用例都先拿这把测试锁，让它们串行执行。
static SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());

/// 取测试锁；中毒时取回内部值继续（用例之间没有共享数据，不需要 `panic`）。
pub(super) fn lock_snapshot() -> MutexGuard<'static, ()> {
    SNAPSHOT_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct Sandbox {
    pub(super) base: PathBuf,
    pub(super) roots: StorageRoots,
    pub(super) data_root: PathBuf,
}

impl Sandbox {
    pub(super) fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("wb-storage-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = base.join(".workbuddy-shift");
        let client = base.join(".workbuddy");
        std::fs::create_dir_all(&store).expect("建工具根");
        std::fs::create_dir_all(client.join("projects/ws")).expect("建客户端根");
        let roots = StorageRoots::new(store, client.clone(), base.join(".workbuddy-ai"));
        Self {
            base,
            roots,
            data_root: client,
        }
    }

    pub(super) fn ctx(&self) -> CleanContext {
        CleanContext {
            roots: self.roots.clone(),
            variant: WbVariant::Cn,
            data_root: self.data_root.clone(),
            client_running: false,
        }
    }

    pub(super) fn make_db(&self, ids: &[(&str, Option<i64>)]) {
        let conn = Connection::open(self.data_root.join("workbuddy.db")).expect("建库");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL, user_id TEXT NOT NULL,
                title TEXT, custom_title TEXT, status TEXT NOT NULL DEFAULT 'Completed',
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                deleted_at INTEGER, is_playground INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("建表");
        for (id, deleted) in ids {
            conn.execute(
                "INSERT INTO sessions (id, cwd, user_id, title, created_at, updated_at, deleted_at, is_playground)
                 VALUES (?1, 'D:/proj', 'u1', ?2, 1, 2, ?3, 0)",
                rusqlite::params![id, format!("标题 {id}"), deleted],
            )
            .expect("写行");
        }
    }

    pub(super) fn write_session_assets(&self, id: &str) {
        let body = self.data_root.join(format!("projects/ws/{id}.jsonl"));
        std::fs::write(&body, b"{\"type\":\"user\"}\n").expect("写正文");
        std::fs::create_dir_all(self.data_root.join(format!("projects/ws/{id}/subagents")))
            .expect("建附属目录");
        std::fs::write(
            self.data_root
                .join(format!("projects/ws/{id}/subagents/a.jsonl")),
            b"{}",
        )
        .expect("写子代理");
        std::fs::create_dir_all(self.data_root.join(format!("file-history/{id}")))
            .expect("建文件历史");
        std::fs::write(
            self.data_root.join(format!("file-history/{id}/v1")),
            b"history",
        )
        .expect("写历史");
        std::fs::create_dir_all(self.data_root.join("artifact-index")).expect("建索引目录");
        std::fs::write(
            self.data_root.join(format!("artifact-index/{id}.json")),
            b"{\"uri\":\"D:/proj/a.ts\"}",
        )
        .expect("写索引");
        std::fs::create_dir_all(self.data_root.join("changes-index")).expect("建变更索引目录");
        std::fs::write(
            self.data_root.join(format!("changes-index/{id}.json")),
            b"{}",
        )
        .expect("写变更索引");
        std::fs::create_dir_all(self.data_root.join(format!("changes-detail/{id}")))
            .expect("建变更明细目录");
        std::fs::write(
            self.data_root.join(format!("changes-detail/{id}/0.json")),
            b"{}",
        )
        .expect("写变更明细");
    }

    pub(super) fn session_clean(&self, targets: Value, dry_run: bool) -> Result<Value, String> {
        run_clean(&self.ctx(), dry_run, &targets)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
