//! 单快照 / 受保护项延迟计量 / 分类口径的测试（第三轮第 6、8 条）。
//!
//! 口径断言：`已列分类之和 == 根总量`（每个根都有一行 `*.other` 补集）。

use super::super::{invalidate_scan_cache, scan_at, sessions_payload, trash_payload};
use super::fixture::{lock_snapshot, Sandbox};
use crate::modules::variant::WbVariant;
use serde_json::Value;

/// 某个分组的分类字节数之和。
fn group_bytes(scan: &Value, group: &str) -> u64 {
    scan["categories"]
        .as_array()
        .expect("分类数组")
        .iter()
        .filter(|category| category["group"] == group)
        .map(|category| category["bytes"].as_u64().unwrap_or(0))
        .sum()
}

fn category_bytes(scan: &Value, id: &str) -> u64 {
    scan["categories"]
        .as_array()
        .expect("分类数组")
        .iter()
        .find(|category| category["id"] == id)
        .map(|category| category["bytes"].as_u64().unwrap_or(0))
        .unwrap_or(0)
}

/// 某个分类的文件数（与 [`category_bytes`] 同口径，用于三处对账）。
fn category_files(scan: &Value, id: &str) -> u64 {
    scan["categories"]
        .as_array()
        .expect("分类数组")
        .iter()
        .find(|category| category["id"] == id)
        .map(|category| category["files"].as_u64().unwrap_or(0))
        .unwrap_or(0)
}

#[test]
fn one_snapshot_serves_scan_sessions_and_trash() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("snapshot-shared");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");
    invalidate_scan_cache();

    let first = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    assert_eq!(first["cached"], false, "第一次必须真扫");
    assert!(
        first["timings"]["totalMs"].as_u64().is_some(),
        "必须返回阶段耗时，便于验收改前 / 改后"
    );

    // 会话资产与回收站复用同一份快照：同一 TTL 内会话树只走一遍。
    //
    // 受保护项在后台线程补算（不同平台的受保护目录集合不同），补算未完成时 `ensure`
    // 会重建一次快照而不是复用。这里等它收敛，避免把「平台差异」当成「没复用」。
    let mut sessions = sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    for _ in 0..100 {
        if sessions["cached"] == true {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        sessions = sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    }
    assert_eq!(sessions["cached"], true, "会话资产必须复用快照");
    assert_eq!(
        sessions["sessions"].as_array().map(Vec::len),
        Some(1),
        "快照里的会话必须带过来"
    );

    let trash = trash_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    assert_eq!(trash["cached"], true, "回收站体积必须复用快照");

    // 清理后作废：下一次必须重算。
    invalidate_scan_cache();
    assert_eq!(
        scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false)["cached"],
        false
    );
    invalidate_scan_cache();
}

#[test]
fn categories_plus_other_equal_root_total() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("snapshot-caliber");
    sandbox.make_db(&[("s1", None)]);
    sandbox.write_session_assets("s1");
    std::fs::create_dir_all(sandbox.data_root.join("logs")).expect("建日志目录");
    std::fs::write(sandbox.data_root.join("logs/app.log"), b"0123456789").expect("写日志");
    std::fs::create_dir_all(sandbox.roots.store.join("trash/20260101-000000"))
        .expect("建回收站条目");
    std::fs::write(
        sandbox
            .roots
            .store
            .join("trash/20260101-000000/manifest.json"),
        b"{}",
    )
    .expect("写清单");
    invalidate_scan_cache();

    let scan = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    let store_bytes = scan["roots"][0]["bytes"].as_u64().unwrap_or(0);
    let client_bytes = scan["roots"][1]["bytes"].as_u64().unwrap_or(0);

    assert_eq!(
        group_bytes(&scan, "tool"),
        store_bytes,
        "工具数据：分类之和（含「其它」）必须等于根总量"
    );
    assert_eq!(
        group_bytes(&scan, "client"),
        client_bytes,
        "客户端：分类之和（含「其它」）必须等于根总量"
    );
    assert_eq!(
        category_bytes(&scan, "client.cn.logs"),
        10,
        "日志分类必须等于真实字节数"
    );

    // 「其它 / 未分类」不可清理，且不重复计入已列分类。
    let other = scan["categories"]
        .as_array()
        .expect("分类数组")
        .iter()
        .find(|category| category["id"] == "client.cn.other")
        .expect("每个客户端根都必须有「其它」行");
    assert_eq!(other["deletable"], false);
    assert!(other["protected"].is_string(), "「其它」必须标明只读原因");
    invalidate_scan_cache();
}

#[test]
fn overview_category_sessions_and_trash_share_one_caliber() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("snapshot-caliber-three");
    sandbox.make_db(&[("s1", None), ("s2", None)]);
    sandbox.write_session_assets("s1");
    sandbox.write_session_assets("s2");
    // 回收站：一个条目、两个文件（体积可由手算核对）。
    let entry = sandbox.roots.store.join("trash/20260101-000000");
    std::fs::create_dir_all(entry.join("files/app")).expect("建回收站条目");
    std::fs::write(entry.join("manifest.json"), b"{}").expect("写清单");
    std::fs::write(entry.join("files/app/a.log"), b"0123456789").expect("写回收站文件");
    std::fs::write(entry.join("files/app/b.log"), b"0123456789").expect("写回收站文件");
    invalidate_scan_cache();

    let scan = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    let sessions = sessions_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);
    let trash = trash_payload(&sandbox.roots, WbVariant::Cn, &sandbox.data_root);

    // 1) 回收站：总览卡 == 分类行 == 回收站接口（三者同一份快照、同一趟遍历）。
    //    体积 = 清单 2 B + 两个 10 B 文件 = 22 B。
    assert_eq!(
        scan["trash"]["bytes"], 22,
        "回收站卡必须等于真实体积：{scan}"
    );
    assert_eq!(scan["trash"]["files"], 3, "清单 + 两个文件");
    assert_eq!(scan["trash"]["entries"], 1, "一个条目目录");
    assert_eq!(
        scan["trash"]["bytes"],
        category_bytes(&scan, "tool.trash"),
        "总览回收站卡与分类行必须同数字"
    );
    assert_eq!(
        scan["trash"]["files"],
        category_files(&scan, "tool.trash"),
        "回收站文件数也必须同口径"
    );
    assert_eq!(
        scan["trash"]["bytes"], trash["bytes"],
        "回收站接口与总览卡必须同数字"
    );
    assert_eq!(scan["trash"]["files"], trash["files"]);
    assert_eq!(
        trash["entries"].as_array().map(Vec::len),
        Some(1),
        "接口必须真的返回这一个条目"
    );

    // 2) 会话资产：总览卡 == 会话接口逐条之和（同一个 `bundle.total()`）。
    let bundle_bytes: u64 = sessions["sessions"]
        .as_array()
        .expect("会话数组")
        .iter()
        .map(|bundle| bundle["bytes"].as_u64().unwrap_or(0))
        .sum();
    let bundle_files: u64 = sessions["sessions"]
        .as_array()
        .expect("会话数组")
        .iter()
        .map(|bundle| bundle["files"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        scan["sessions"]["bytes"], bundle_bytes,
        "会话卡必须等于逐条之和"
    );
    assert_eq!(scan["sessions"]["files"], bundle_files);

    // 3) 总量 == 工具数据 + 客户端数据（会话资产与回收站是子集，不可相加）。
    let store_bytes = scan["roots"][0]["bytes"].as_u64().unwrap_or(0);
    let client_bytes = scan["roots"][1]["bytes"].as_u64().unwrap_or(0);
    assert_eq!(scan["totals"]["bytes"], store_bytes + client_bytes);
    assert!(
        scan["sessions"]["bytes"].as_u64().unwrap_or(0) <= client_bytes,
        "会话资产是客户端数据的子集，不得大于它"
    );
    assert!(
        scan["trash"]["bytes"].as_u64().unwrap_or(0) <= store_bytes,
        "回收站是工具数据的子集，不得大于它"
    );

    // 4) 分类之和 == 根总量（含「其它 / 未分类」补集）。
    assert_eq!(group_bytes(&scan, "tool"), store_bytes);
    assert_eq!(group_bytes(&scan, "client"), client_bytes);
    invalidate_scan_cache();
}

#[test]
fn protected_directories_are_deferred_then_served_from_cache() {
    let _guard = lock_snapshot();
    let sandbox = Sandbox::new("snapshot-protected");
    let binaries = sandbox.data_root.join("binaries");
    std::fs::create_dir_all(&binaries).expect("建运行时目录");
    std::fs::write(binaries.join("app.bin"), b"1234567890").expect("写运行时");
    // 账号目录是最典型的「大但不可清理」项：必须被延迟计量。
    let account = sandbox
        .data_root
        .join("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0");
    std::fs::create_dir_all(&account).expect("建账号目录");
    std::fs::write(account.join("data.bin"), b"01234").expect("写账号数据");
    invalidate_scan_cache();

    // 1) 缓存缺失：受保护项 pending，数字先不给（不能伪装成 0 字节的完整总量）。
    let cold = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    assert_eq!(cold["protectedPending"], true, "首次必须标注计算中");
    assert_eq!(category_bytes(&cold, "client.cn.binaries"), 0);

    // 2) 等后台补算把缓存写完：不同平台的受保护目录集合不同（多一个目录就会多一个
    //    pending 键），手写固定键会在 Linux/macOS 上永远凑不齐。这里只等真实线程收敛，
    //    下一次扫描必须重建一次，把完整数字与 pending=false 一起交回页面。
    let mut warm = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    for _ in 0..100 {
        if warm["protectedPending"] == false {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        warm = scan_at(&sandbox.roots, WbVariant::Cn, &sandbox.data_root, false);
    }
    assert_eq!(warm["protectedPending"], false, "缓存命中后不再 pending");
    assert_eq!(
        category_bytes(&warm, "client.cn.binaries"),
        10,
        "受保护项必须从缓存拿到真实数字"
    );
    // 账号目录不单列分类，但已经计入根总量 → 落在「其它」里。
    let client_bytes = warm["roots"][1]["bytes"].as_u64().unwrap_or(0);
    assert!(
        client_bytes >= 15,
        "根总量必须包含缓存里的受保护项：{client_bytes}"
    );
    assert_eq!(group_bytes(&warm, "client"), client_bytes);
    invalidate_scan_cache();
}
