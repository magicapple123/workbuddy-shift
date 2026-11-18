//! 文件移动：同卷 `rename`、跨卷「复制 → 校验 → 删除源」，以及时间戳目录名。
//!
//! 单独成文件只为控制单文件规模；对 `trash` 模块外不暴露。

use super::{entry_dir, now_ms, TrashItem};
use crate::modules::storage::roots::{
    ensure_no_symlink_components, ensure_within, usage_of, StorageRoots,
};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(super) fn mtime_ms(path: &Path) -> i64 {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 生成不冲突的条目 id：`yyyyMMdd-HHmmss`，同秒内追加序号。
pub fn new_entry_id(roots: &StorageRoots) -> String {
    let secs = now_ms() / 1000;
    let base = utc_stamp(secs);
    let trash = roots.trash_root();
    if !trash.join(&base).exists() {
        return base;
    }
    for index in 2..100 {
        let candidate = format!("{base}-{index}");
        if !trash.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", now_ms() % 1000)
}

fn utc_stamp(secs: i64) -> String {
    // 只用于目录名：用 UTC 分解，避免依赖 chrono。
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// Howard Hinnant 的 days → (y, m, d) 算法（公历，无时区）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 把一个单元移进回收站。
///
/// 调用方必须已完成 dry-run 与确认；本函数不做业务判断，只保证路径安全与失败可见。
pub(crate) fn move_unit(
    roots: &StorageRoots,
    entry_id: &str,
    root_id: &str,
    root: &Path,
    path: &Path,
    category: &str,
    session_id: Option<&str>,
) -> Result<TrashItem, String> {
    ensure_within(root, path)?;
    if path == root {
        return Err("拒绝移动白名单根本身".to_string());
    }
    ensure_no_symlink_components(root, path)?;
    let rel = path
        .strip_prefix(root)
        .map_err(|_| "路径不在白名单根内".to_string())?;
    let dir = entry_dir(roots, entry_id)?;
    let stored = dir.join("files").join(root_id).join(rel);
    if let Some(parent) = stored.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建回收站目录失败：{error}"))?;
    }
    let usage = usage_of(path);
    let meta = std::fs::symlink_metadata(path).map_err(|error| format!("读取目标失败：{error}"))?;
    let kind = if meta.is_dir() { "dir" } else { "file" }.to_string();
    let mtime = mtime_ms(path);

    if std::fs::rename(path, &stored).is_err() {
        // 跨卷：复制 → 校验 → 删除源；任一步失败都保留源，回收站里的半成品由 purge 清理。
        copy_recursive(path, &stored)?;
        let copied = usage_of(&stored);
        if copied.bytes != usage.bytes {
            return Err(format!(
                "跨卷复制校验失败（{} != {} 字节），源文件已保留",
                copied.bytes, usage.bytes
            ));
        }
        remove_source(path)?;
    }

    Ok(TrashItem {
        root_id: root_id.to_string(),
        original_path: path.to_string_lossy().to_string(),
        stored_path: stored.to_string_lossy().to_string(),
        usage,
        mtime,
        category: category.to_string(),
        session_id: session_id.map(str::to_string),
        kind,
        result: "moved".to_string(),
    })
}

pub(crate) fn copy_recursive(source: &Path, target: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if meta.is_file() {
        std::fs::copy(source, target).map_err(|error| format!("复制失败：{error}"))?;
        return Ok(());
    }
    std::fs::create_dir_all(target).map_err(|error| format!("创建目录失败：{error}"))?;
    let entries = std::fs::read_dir(source).map_err(|error| format!("读目录失败：{error}"))?;
    for entry in entries.flatten() {
        copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

pub(super) fn remove_source(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path).map_err(|error| format!("删除源目录失败：{error}"))
    } else {
        std::fs::remove_file(path).map_err(|error| format!("删除源文件失败：{error}"))
    }
}
