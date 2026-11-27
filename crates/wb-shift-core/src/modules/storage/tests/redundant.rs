//! 冗余副本的清理行为测试：同账号多份才清理，跨账号的合法副本必须留着。

use super::super::clean::clean;
use super::super::sessions::scan_sessions;
use super::fixture::{lock_snapshot, Sandbox};
use crate::modules::variant::WbVariant;
use rusqlite::Connection;
use serde_json::json;

/// 一份「同组同账号两份 + 另一个账号一份」的现场：三行都有资产，组 g1 三个成员。
fn duplicate_group(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.make_db_rows(&[("a1", "u1", 100), ("a2", "u1", 200), ("b1", "u2", 50)]);
    for id in ["a1", "a2", "b1"] {
        sandbox.write_session_assets(id);
    }
    sandbox.write_links(json!([Sandbox::group_json(
        "g1",
        &[("u1", "a1"), ("u1", "a2"), ("u2", "b1")]
    )]));
    sandbox
}

#[test]
fn redundant_counts_only_same_uid_copies() {
    let _guard = lock_snapshot();
    let sandbox = duplicate_group("redundant-scope");
    let bundles = scan_sessions(&sandbox.roots.store, &sandbox.data_root, WbVariant::Cn);
    let find = |id: &str| {
        bundles
            .iter()
            .find(|bundle| bundle.id == id)
            .expect("找到会话")
    };

    assert!(find("a1").redundant, "同账号两份才算冗余");
    assert_eq!(find("a1").live_copies, 2);
    assert!(find("a2").redundant);
    assert!(
        !find("b1").redundant,
        "跨账号各一份是合法副本，不能被标成冗余"
    );
    assert_eq!(find("b1").live_copies, 1);
    assert_eq!(find("b1").group_id.as_deref(), Some("g1"));

    // 列表按钮上的数字（可清理条数）= 一键清理实际会删的条数 = 1（桶内 2 份留 1 份）。
    let payload = super::super::sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    assert_eq!(payload["redundantCleanable"], 1, "{payload}");
}

/// 软删成员不参与副本计数：`1 live + 1 软删` 既不是冗余，也没有可清理项。
///
/// 这条钉住 QA 回归里实测到的不一致：关联表口径会数出 2 份（成员记录都在），于是列表
/// 显示「1 条可清理」，可 `redundantAll` 只认存活行 → 点下去报「没有检测到冗余副本」。
#[test]
fn redundant_ignores_soft_deleted_members() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("redundant-soft-deleted");
    sandbox.make_db_rows(&[("a1", "u1", 100), ("a2", "u1", 200)]);
    for id in ["a1", "a2"] {
        sandbox.write_session_assets(id);
    }
    sandbox.write_links(json!([Sandbox::group_json(
        "g1",
        &[("u1", "a1"), ("u1", "a2")]
    )]));
    // a2 被软删：关联表里仍有两个成员，但本机只剩一条存活行。
    let conn = Connection::open(sandbox.data_root.join("workbuddy.db")).expect("打开库");
    conn.execute(
        "UPDATE sessions SET deleted_at = 1700000000000 WHERE id = 'a2'",
        [],
    )
    .expect("软删");
    drop(conn);

    let bundles = scan_sessions(&sandbox.roots.store, &sandbox.data_root, WbVariant::Cn);
    let find = |id: &str| {
        bundles
            .iter()
            .find(|bundle| bundle.id == id)
            .expect("找到会话")
    };
    assert_eq!(find("a2").row_status, "softDeleted");
    assert!(
        !find("a1").redundant,
        "软删的那份不算副本：{find_a1:?}",
        find_a1 = find("a1")
    );
    assert_eq!(find("a1").live_copies, 1, "存活行只有自己这一条");
    assert!(!find("a2").redundant);

    // 列表上的「可清理条数」必须与一键清理的结果一致：都是 0。
    let payload = super::super::sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    assert_eq!(payload["redundantCleanable"], 0, "{payload}");
    let error = sandbox
        .session_clean(json!([{ "kind": "redundantAll" }]), true)
        .expect_err("没有可清理项");
    assert!(error.contains("没有检测到冗余副本"), "{error}");

    // 两份都存活时才是冗余：口径切换后 `redundantAll` 删的也正是「多出来的那一份」。
    let conn = Connection::open(sandbox.data_root.join("workbuddy.db")).expect("打开库");
    conn.execute("UPDATE sessions SET deleted_at = NULL WHERE id = 'a2'", [])
        .expect("恢复");
    drop(conn);
    super::super::invalidate_scan_cache();
    let payload = super::super::sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    assert_eq!(payload["redundantCleanable"], 1, "{payload}");
    let plan = sandbox
        .session_clean(json!([{ "kind": "redundantAll" }]), true)
        .expect("dry-run");
    assert_eq!(plan["rows"].as_array().expect("rows").len(), 1, "{plan}");
}

#[test]
fn redundant_all_deletes_duplicates_and_keeps_newest() {
    let _guard = lock_snapshot();
    let sandbox = duplicate_group("redundant-all");
    let body = |id: &str| sandbox.data_root.join(format!("projects/ws/{id}.jsonl"));

    let plan = sandbox
        .session_clean(json!([{ "kind": "redundantAll" }]), true)
        .expect("dry-run 应成功");
    let doomed: Vec<&str> = plan["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["sessionId"].as_str().expect("sessionId"))
        .collect();
    assert_eq!(doomed, vec!["a1"], "只删同账号较旧的那份：{plan}");
    let note = plan["note"].as_str().expect("note");
    assert!(note.contains("1 条"), "{note}");
    assert!(note.contains("跨账号"), "必须写明跨账号副本不删：{note}");
    assert!(body("a1").exists(), "dry-run 不得移动文件");

    let result = sandbox
        .session_clean(json!([{ "kind": "redundantAll" }]), false)
        .expect("清理成功");
    assert_eq!(result["deletedRows"], 1, "{result}");
    assert!(result["moved"].as_u64().unwrap_or(0) > 0, "{result}");
    assert!(!body("a1").exists(), "重复副本应已进回收站");
    assert!(body("a2").exists(), "每组保留最新一条");
    assert!(body("b1").exists(), "跨账号的合法副本不得删除");
    // 结果里的说明是完成时态（与确认框里的「将删除」区分开）。
    let done = result["note"].as_str().expect("note");
    assert!(done.starts_with("已删除 1 条"), "{done}");
}

#[test]
fn redundant_group_keeps_newest_or_the_explicit_one() {
    let _guard = lock_snapshot();
    let sandbox = duplicate_group("redundant-group");
    let doomed_of = |target: serde_json::Value| -> Result<Vec<String>, String> {
        let plan = sandbox.session_clean(json!([target]), true)?;
        Ok(plan["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["sessionId"].as_str().expect("sessionId").to_string())
            .collect())
    };
    // 不给保留项：保留最新（a2），删旧的（a1）——不能整组删光。
    assert_eq!(
        doomed_of(json!({ "kind": "redundant", "groupId": "g1" })).expect("dry-run"),
        vec!["a1".to_string()]
    );
    // 显式指定保留旧的一份：删新的那一份。
    assert_eq!(
        doomed_of(json!({ "kind": "redundant", "groupId": "g1", "keepSessionId": "a1" }))
            .expect("dry-run"),
        vec!["a2".to_string()]
    );
    // 指定了不属于该组的会话：拒绝，不做任何猜测。
    let error = doomed_of(json!({ "kind": "redundant", "groupId": "g1", "keepSessionId": "zzz" }))
        .expect_err("必须拒绝");
    assert!(error.contains("不属于该关联组"), "{error}");
}

#[test]
fn redundant_refuses_group_without_same_uid_duplicates() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("redundant-legit-cross");
    sandbox.make_db_rows(&[("a1", "u1", 100), ("b1", "u2", 50)]);
    for id in ["a1", "b1"] {
        sandbox.write_session_assets(id);
    }
    sandbox.write_links(json!([Sandbox::group_json(
        "g1",
        &[("u1", "a1"), ("u2", "b1")]
    )]));

    let error = sandbox
        .session_clean(json!([{ "kind": "redundant", "groupId": "g1" }]), false)
        .expect_err("跨账号的组没有冗余副本");
    assert!(error.contains("没有可清理的冗余副本"), "{error}");
    assert!(sandbox.data_root.join("projects/ws/b1.jsonl").exists());
}

#[test]
fn redundant_all_rejects_when_nothing_to_clean() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("redundant-empty");
    sandbox.make_db_rows(&[("a1", "u1", 100)]);
    sandbox.write_session_assets("a1");

    let error = sandbox
        .session_clean(json!([{ "kind": "redundantAll" }]), false)
        .expect_err("没有冗余副本");
    assert!(error.contains("没有检测到冗余副本"), "{error}");
}

#[test]
fn redundant_is_refused_while_client_runs() {
    let _guard = lock_snapshot();
    let sandbox = duplicate_group("redundant-running");
    for target in [
        json!([{ "kind": "redundantAll" }]),
        json!([{ "kind": "redundant", "groupId": "g1" }]),
    ] {
        let error =
            clean(&sandbox.ctx_running(), false, &target).expect_err("客户端运行中必须拒绝");
        assert!(error.contains("请先退出 WorkBuddy 客户端"), "{error}");
    }
    assert!(sandbox.data_root.join("projects/ws/a1.jsonl").exists());
}
