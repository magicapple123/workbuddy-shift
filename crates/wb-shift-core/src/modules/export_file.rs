//! 文本导出落盘：只允许 `.csv`、体积上限 16 MiB、原子写。
//!
//! 桌面端先弹保存对话框拿到路径，再调这里写入；WebUI 用浏览器 Blob 下载，不经过本模块。
//! 校验放在 core，两个宿主共用同一套错误文案（`{ok:false,error}` 由宿主包装）。

use crate::modules::config::{atomic_write, now_ms};
use serde_json::{json, Value};
use std::path::Path;

/// 导出的体积上限：16 MiB（约 30 万行明细，远高于实际需要）。
pub const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;

/// 允许的扩展名（小写比较）。
pub const ALLOWED_EXTENSIONS: [&str; 1] = ["csv"];

/// 保存文本到指定路径。
///
/// 拒绝：空路径、非 `.csv`、内容超限、父目录不存在（不自动创建——用户选的路径必须已存在，
/// 避免在任意位置造目录）。写入用原子替换，失败不留半截文件。
pub fn save_text_file(path: &str, content: &str) -> Result<Value, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("缺少保存路径".to_string());
    }
    let target = Path::new(trimmed);
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("只允许保存 .csv 文件（收到 .{extension}）"));
    }
    let bytes = content.len();
    if bytes > MAX_EXPORT_BYTES {
        return Err(format!(
            "内容超过 {} MiB 上限（{} 字节）",
            MAX_EXPORT_BYTES / 1024 / 1024,
            bytes
        ));
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(format!("目标目录不存在：{}", parent.display()));
        }
    }
    atomic_write(target, content).map_err(|error| format!("写入失败：{error}"))?;
    Ok(json!({
        "ok": true,
        "path": target.to_string_lossy(),
        "bytes": bytes,
        "savedAt": now_ms(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wb-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建目录");
        dir.join(name)
    }

    #[test]
    fn writes_csv_atomically() {
        let path = temp_path("t2-ok.csv");
        let result =
            save_text_file(&path.to_string_lossy(), "\u{feff}a,b\r\n1,2\r\n").expect("保存成功");
        assert_eq!(result["ok"], json!(true));
        let text = std::fs::read_to_string(&path).expect("读回");
        assert!(text.starts_with('\u{feff}'), "必须保留 BOM");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_non_csv_and_oversize_and_missing_parent() {
        let path = temp_path("t2-bad.txt");
        let error = save_text_file(&path.to_string_lossy(), "x").expect_err("扩展名必须拒绝");
        assert!(error.contains(".csv"), "{error}");

        let csv = temp_path("t2-big.csv");
        let huge = "x".repeat(MAX_EXPORT_BYTES + 1);
        let error = save_text_file(&csv.to_string_lossy(), &huge).expect_err("超限必须拒绝");
        assert!(error.contains("上限"), "{error}");

        let missing = std::env::temp_dir()
            .join(format!("wb-export-missing-{}", std::process::id()))
            .join("a.csv");
        let error = save_text_file(&missing.to_string_lossy(), "x").expect_err("父目录不存在");
        assert!(error.contains("目录不存在"), "{error}");
    }
}
