//! 回收站行为测试：移动、恢复、拒绝覆盖、彻底删除。

use super::move_::move_unit;
use super::*;

fn sandbox(name: &str) -> StorageRoots {
    let base = std::env::temp_dir().join(format!("wb-storage-trash-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let store = base.join(".workbuddy-shift");
    std::fs::create_dir_all(store.join("trash")).expect("建回收站");
    std::fs::create_dir_all(base.join(".workbuddy")).expect("建客户端根");
    StorageRoots::new(store, base.join(".workbuddy"), base.join(".workbuddy-ai"))
}

#[test]
fn move_then_restore_returns_file_to_original_path() {
    let roots = sandbox("roundtrip");
    let file = roots.client_cn.join("logs/app.log");
    std::fs::create_dir_all(file.parent().expect("父目录")).expect("建目录");
    std::fs::write(&file, b"hello").expect("写文件");

    let id = new_entry_id(&roots);
    let item = move_unit(
        &roots,
        &id,
        "cn",
        &roots.client_cn,
        &file,
        "client.cn.logs",
        None,
    )
    .expect("移入回收站");
    assert!(!file.exists(), "源文件应已移走");
    let entry = TrashEntry {
        id: id.clone(),
        created_at: now_ms(),
        items: vec![item],
        rows: Vec::new(),
        db_backups: Vec::new(),
    };
    write_manifest(&roots, &entry).expect("写清单");

    let result = restore_entry(&roots, &id).expect("恢复");
    assert_eq!(result["restoredFiles"], 1);
    assert!(file.exists(), "文件必须回到原路径");
    assert_eq!(std::fs::read(&file).expect("读回"), b"hello");
    let _ = std::fs::remove_dir_all(roots.store.parent().expect("父目录"));
}

#[test]
fn restore_refuses_to_overwrite_existing_target() {
    let roots = sandbox("conflict");
    let file = roots.client_cn.join("logs/app.log");
    std::fs::create_dir_all(file.parent().expect("父目录")).expect("建目录");
    std::fs::write(&file, b"hello").expect("写文件");
    let id = new_entry_id(&roots);
    let item = move_unit(
        &roots,
        &id,
        "cn",
        &roots.client_cn,
        &file,
        "client.cn.logs",
        None,
    )
    .expect("移入回收站");
    write_manifest(
        &roots,
        &TrashEntry {
            id: id.clone(),
            created_at: now_ms(),
            items: vec![item],
            rows: Vec::new(),
            db_backups: Vec::new(),
        },
    )
    .expect("写清单");
    std::fs::write(&file, b"new").expect("重建同名文件");

    let result = restore_entry(&roots, &id).expect("恢复");
    assert_eq!(result["restoredFiles"], 0);
    assert_eq!(result["failures"].as_array().map(|v| v.len()), Some(1));
    assert_eq!(std::fs::read(&file).expect("读回"), b"new", "不得覆盖");
    let _ = std::fs::remove_dir_all(roots.store.parent().expect("父目录"));
}

#[test]
fn purge_removes_entry_directory() {
    let roots = sandbox("purge");
    let file = roots.client_cn.join("logs/app.log");
    std::fs::create_dir_all(file.parent().expect("父目录")).expect("建目录");
    std::fs::write(&file, b"x").expect("写文件");
    let id = new_entry_id(&roots);
    let item = move_unit(
        &roots,
        &id,
        "cn",
        &roots.client_cn,
        &file,
        "client.cn.logs",
        None,
    )
    .expect("移入回收站");
    write_manifest(
        &roots,
        &TrashEntry {
            id: id.clone(),
            created_at: now_ms(),
            items: vec![item],
            rows: Vec::new(),
            db_backups: Vec::new(),
        },
    )
    .expect("写清单");
    purge(&roots, Some(&id)).expect("彻底删除");
    assert!(!roots.trash_root().join(&id).exists());
    let _ = std::fs::remove_dir_all(roots.store.parent().expect("父目录"));
}
