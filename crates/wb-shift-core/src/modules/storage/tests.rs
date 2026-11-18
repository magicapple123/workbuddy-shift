//! 存储管理的行为测试：全部在临时沙箱里造数，不读写真实账号 / 客户端数据。

use super::catalog::{self, CLIPBOARD_RETENTION_DAYS};
use super::clean::{clean, CleanContext};
use super::roots::StorageRoots;
use super::trash;
use crate::modules::variant::WbVariant;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Sandbox {
    base: PathBuf,
    roots: StorageRoots,
    data_root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
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

    fn ctx(&self) -> CleanContext {
        CleanContext {
            roots: self.roots.clone(),
            variant: WbVariant::Cn,
            data_root: self.data_root.clone(),
            client_running: false,
        }
    }

    fn make_db(&self, ids: &[(&str, Option<i64>)]) {
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

    fn write_session_assets(&self, id: &str) {
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

    fn session_clean(&self, targets: Value, dry_run: bool) -> Result<Value, String> {
        clean(&self.ctx(), dry_run, &targets)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

#[test]
fn dry_run_writes_nothing_and_creates_no_trash() {
    let sandbox = Sandbox::new("dry-run");
    let log = sandbox.data_root.join("logs/app.log");
    std::fs::create_dir_all(log.parent().expect("父目录")).expect("建日志目录");
    std::fs::write(&log, b"log").expect("写日志");
    let before = mtime(&log);

    let plan = sandbox
        .session_clean(
            json!([{ "kind": "category", "id": "client.cn.logs" }]),
            true,
        )
        .expect("dry-run 应成功");
    assert_eq!(plan["dryRun"], true);
    assert_eq!(plan["items"].as_array().map(|items| items.len()), Some(1));
    assert!(log.exists(), "dry-run 不得移动文件");
    assert_eq!(mtime(&log), before, "dry-run 不得改变 mtime");
    assert!(
        !sandbox.roots.trash_root().exists(),
        "dry-run 不得创建回收站"
    );
}

#[test]
fn category_clean_goes_to_trash_and_restores() {
    let sandbox = Sandbox::new("category-roundtrip");
    let log = sandbox.data_root.join("logs/app.log");
    std::fs::create_dir_all(log.parent().expect("父目录")).expect("建日志目录");
    std::fs::write(&log, b"log").expect("写日志");

    let result = sandbox
        .session_clean(
            json!([{ "kind": "category", "id": "client.cn.logs" }]),
            false,
        )
        .expect("清理成功");
    assert_eq!(result["moved"], 1);
    assert!(!log.exists(), "源目录应已移走");
    let id = result["trashId"].as_str().expect("回收站 id").to_string();

    let restored = trash::restore_entry(&sandbox.roots, &id).expect("恢复成功");
    assert_eq!(restored["restoredFiles"], 1);
    assert!(log.exists(), "目录必须回到原路径");
}

#[test]
fn single_entry_removal_marks_partial_and_keeps_row() {
    let sandbox = Sandbox::new("single-entry");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");

    let result = sandbox
        .session_clean(
            json!([{ "kind": "session", "id": "s1", "entries": ["file-history"] }]),
            false,
        )
        .expect("删单文件成功");
    assert_eq!(result["deletedRows"], 0);

    let payload = super::sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    let bundle = payload["sessions"]
        .as_array()
        .expect("会话数组")
        .iter()
        .find(|item| item["id"] == "s1")
        .expect("找到会话");
    assert_eq!(bundle["presence"], "partial");
    assert_eq!(bundle["rowStatus"], "live");
}

#[test]
fn body_removal_marks_body_missing_without_deleting_row() {
    let sandbox = Sandbox::new("body-missing");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");

    sandbox
        .session_clean(
            json!([{ "kind": "session", "id": "s1", "entries": ["body"] }]),
            false,
        )
        .expect("删正文成功");

    let payload = super::sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    let bundle = payload["sessions"]
        .as_array()
        .expect("会话数组")
        .iter()
        .find(|item| item["id"] == "s1")
        .expect("找到会话");
    assert_eq!(bundle["presence"], "bodyMissing");

    let conn = Connection::open(sandbox.data_root.join("workbuddy.db")).expect("开库");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id='s1'", [], |row| {
            row.get(0)
        })
        .expect("查行");
    assert_eq!(rows, 1, "删正文不得删行");
}

#[test]
fn live_session_delete_backs_up_db_and_restores_row_and_files() {
    let sandbox = Sandbox::new("live-roundtrip");
    sandbox.make_db(&[("s1", None), ("s2", None)]);
    sandbox.write_session_assets("s1");

    let result = sandbox
        .session_clean(
            json!([{ "kind": "session", "id": "s1", "deleteRow": true }]),
            false,
        )
        .expect("删会话成功");
    assert_eq!(result["deletedRows"], 1);
    let warnings = result["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|item| item.as_str().unwrap_or_default().contains("云端")),
        "必须提示云端可能拉回：{warnings:?}"
    );

    let conn = Connection::open(sandbox.data_root.join("workbuddy.db")).expect("开库");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id='s1'", [], |row| {
            row.get(0)
        })
        .expect("查行");
    assert_eq!(rows, 0, "行应已删除");

    let id = result["trashId"].as_str().expect("回收站 id").to_string();
    let backup = sandbox.roots.storage_backup_root().join(&id).join("cn.db");
    assert!(backup.is_file(), "删行前必须有整库备份");

    let restored = trash::restore_entry(&sandbox.roots, &id).expect("恢复成功");
    assert_eq!(restored["restoredRows"], 1);
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id='s1'", [], |row| {
            row.get(0)
        })
        .expect("查行");
    assert_eq!(rows, 1, "行必须恢复");
    assert!(
        sandbox.data_root.join("projects/ws/s1.jsonl").exists(),
        "正文必须恢复"
    );
}

#[test]
fn protected_categories_and_running_client_are_rejected_before_writing() {
    let sandbox = Sandbox::new("reject");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");

    let error = sandbox
        .session_clean(
            json!([{ "kind": "category", "id": "client.cn.binaries" }]),
            false,
        )
        .expect_err("受保护分类必须拒绝");
    assert!(error.contains("运行时"), "{error}");

    let mut ctx = sandbox.ctx();
    ctx.client_running = true;
    let error = clean(
        &ctx,
        false,
        &json!([{ "kind": "session", "id": "s1", "deleteRow": true }]),
    )
    .expect_err("客户端运行中必须拒绝");
    assert!(error.contains("退出"), "{error}");
    assert!(
        sandbox.data_root.join("projects/ws/s1.jsonl").exists(),
        "被拒绝时不得写入"
    );
    assert!(
        !sandbox.roots.trash_root().exists(),
        "被拒绝时不得创建回收站"
    );
}

#[test]
fn clipboard_category_only_selects_files_older_than_retention() {
    let sandbox = Sandbox::new("clipboard");
    let dir = sandbox.data_root.join("clipboard-images");
    std::fs::create_dir_all(&dir).expect("建目录");
    let old = dir.join("old.png");
    let fresh = dir.join("fresh.png");
    std::fs::write(&old, b"old").expect("写旧文件");
    std::fs::write(&fresh, b"fresh").expect("写新文件");

    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs((CLIPBOARD_RETENTION_DAYS + 5) * 24 * 3600);
    set_mtime(&old, cutoff);

    let catalog = catalog::catalog(&sandbox.roots);
    let category =
        catalog::find(&catalog.categories, "client.cn.clipboard-images").expect("剪贴板分类");
    assert_eq!(category.units.len(), 1, "只应选中 30 天前的文件");
    assert!(category.units[0].path.ends_with("old.png"));
}

fn set_mtime(path: &Path, time: std::time::SystemTime) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("打开文件");
    file.set_modified(time).expect("设置 mtime");
}

#[test]
fn whitelist_rejects_outside_paths() {
    let sandbox = Sandbox::new("whitelist");
    let outside = sandbox.base.join("outside.txt");
    std::fs::write(&outside, b"x").expect("写外部文件");
    assert!(
        sandbox.roots.root_containing(&outside).is_none(),
        "白名单外路径不得解析到根"
    );
    let target = sandbox.data_root.join("logs/app.log");
    assert!(
        sandbox.roots.root_containing(&target).is_some(),
        "白名单内路径必须解析到根"
    );
    let error = super::reveal(&sandbox.roots, &outside.to_string_lossy(), "select")
        .expect_err("白名单外必须拒绝");
    assert!(error.contains("白名单"), "{error}");
}

#[test]
fn revalidate_reports_missing_body_after_external_delete() {
    let sandbox = Sandbox::new("revalidate");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");

    let before = super::revalidate(
        &sandbox.roots,
        WbVariant::Cn,
        &sandbox.data_root,
        &json!(["session:s1"]),
    )
    .expect("校验成功");
    assert_eq!(before["items"][0]["presence"], "present");

    std::fs::remove_file(sandbox.data_root.join("projects/ws/s1.jsonl")).expect("模拟外部删除");
    let after = super::revalidate(
        &sandbox.roots,
        WbVariant::Cn,
        &sandbox.data_root,
        &json!(["session:s1"]),
    )
    .expect("校验成功");
    assert_eq!(after["items"][0]["presence"], "bodyMissing");
}

/// 会话资产包分类口径（从 `sessions.rs` 拆出，避免单文件超过 500 行）。
mod session_assets {
    use super::super::sessions::scan_sessions_at;
    use crate::modules::variant::WbVariant;
    use serde_json::{json, Map, Value};
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wb-storage-sessions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建夹具目录");
        dir
    }

    fn row(id: &str, deleted: Option<i64>) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("id".into(), json!(id));
        map.insert("user_id".into(), json!("u1"));
        map.insert("cwd".into(), json!("D:/workbuddy-shift"));
        map.insert("title".into(), json!(format!("标题 {id}")));
        map.insert("updated_at".into(), json!(100));
        map.insert("is_playground".into(), json!(0));
        map.insert(
            "deleted_at".into(),
            deleted.map(|v| json!(v)).unwrap_or(Value::Null),
        );
        map
    }

    #[test]
    fn presence_reflects_missing_body_and_sidecar() {
        let root = temp_root("presence");
        std::fs::create_dir_all(root.join("projects/ws")).expect("建目录");
        std::fs::write(root.join("projects/ws/s1.jsonl"), b"{}").expect("写正文");
        std::fs::create_dir_all(root.join("projects/ws/s1/subagents")).expect("建附属目录");
        std::fs::create_dir_all(root.join("artifact-index")).expect("建索引目录");
        std::fs::write(root.join("artifact-index/s1.json"), b"{}").expect("写索引");

        let bundles = scan_sessions_at(WbVariant::Cn, &root, &root, vec![row("s1", None)]);
        let bundle = bundles.iter().find(|b| b.id == "s1").expect("找到会话");
        assert_eq!(bundle.row_status, "live");
        assert_eq!(
            bundle.presence, "partial",
            "缺 file-history / changes 应为 partial"
        );
        assert!(bundle.has_body());

        let bundles = scan_sessions_at(WbVariant::Cn, &root, &root, vec![row("s2", None)]);
        let bundle = bundles.iter().find(|b| b.id == "s2").expect("找到会话");
        assert_eq!(bundle.presence, "bodyMissing");
        assert_eq!(bundle.row_status, "live");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn soft_deleted_and_orphan_are_distinguished() {
        let root = temp_root("orphan");
        std::fs::create_dir_all(root.join("projects/ws")).expect("建目录");
        std::fs::write(root.join("projects/ws/dead.jsonl"), b"{}").expect("写正文");
        std::fs::write(root.join("projects/ws/orphan.jsonl"), b"{}").expect("写正文");

        let bundles = scan_sessions_at(WbVariant::Cn, &root, &root, vec![row("dead", Some(1234))]);
        let dead = bundles.iter().find(|b| b.id == "dead").expect("软删除行");
        assert_eq!(dead.row_status, "softDeleted");
        let orphan = bundles.iter().find(|b| b.id == "orphan").expect("孤儿");
        assert_eq!(orphan.row_status, "none");
        assert_eq!(orphan.presence, "orphan");
        let _ = std::fs::remove_dir_all(&root);
    }
}
