//! 数据库写入面：删行前的整库在线备份、删行、清理关联成员。单独成文件只为控制规模。

use super::*;

/// 整库在线备份到 `backups/storage/<stamp>/<variant>.db`，并复制 `-wal` / `-shm`。
pub(crate) fn backup_db(ctx: &CleanContext, stamp: &str) -> Result<String, String> {
    let source = ctx.data_root.join("workbuddy.db");
    if !source.is_file() {
        return Err("找不到 workbuddy.db，拒绝在无备份的情况下删行".to_string());
    }
    let dir = ctx.roots.storage_backup_root().join(stamp);
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建备份目录失败：{error}"))?;
    let target = dir.join(format!("{}.db", ctx.variant.as_str()));
    let src =
        rusqlite::Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("打开数据库失败：{error}"))?;
    let mut dst =
        rusqlite::Connection::open(&target).map_err(|error| format!("创建备份失败：{error}"))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|error| format!("初始化在线备份失败：{error}"))?;
        backup
            .run_to_completion(5, std::time::Duration::from_millis(50), None)
            .map_err(|error| format!("在线备份失败：{error}"))?;
    }
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", source.to_string_lossy()));
        if side.is_file() {
            let _ = std::fs::copy(&side, dir.join(format!("{}{suffix}", ctx.variant.as_str())));
        }
    }
    Ok(target.to_string_lossy().to_string())
}

pub(crate) fn delete_row(data_root: &Path, session_id: &str) -> Result<(), String> {
    let db = data_root.join("workbuddy.db");
    let conn =
        rusqlite::Connection::open(&db).map_err(|error| format!("打开数据库失败：{error}"))?;
    let changed = conn
        .execute("DELETE FROM sessions WHERE id = ?1", [session_id])
        .map_err(|error| format!("删除会话行失败：{error}"))?;
    if changed == 0 {
        return Err("会话行已不存在（可能被客户端清理）".to_string());
    }
    Ok(())
}

/// 从关联主表里移除已删除的成员；主表不可读时跳过（行已删，残留成员由维护入口清理）。
pub(crate) fn remove_group_members(
    store_root: &Path,
    variant: WbVariant,
    session_ids: &[String],
) -> Result<(), String> {
    let file = store_root.join("session_links.json");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(());
    };
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return Ok(());
    };
    let Some(groups) = value.get_mut("groups").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut touched = false;
    for group in groups.iter_mut() {
        if group.get("variant").and_then(Value::as_str) != Some(variant.as_str()) {
            continue;
        }
        let Some(members) = group.get_mut("members").and_then(Value::as_array_mut) else {
            continue;
        };
        let before = members.len();
        members.retain(|member| {
            let id = member
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            !session_ids.iter().any(|candidate| candidate == id)
        });
        if members.len() != before {
            touched = true;
        }
    }
    if !touched {
        return Ok(());
    }
    let payload = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    crate::modules::config::atomic_write(&file, &payload).map_err(|error| error.to_string())
}
