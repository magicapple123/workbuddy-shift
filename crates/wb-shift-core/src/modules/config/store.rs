use super::paths::store_dir;
use super::paths::workbuddy_exe_cache_file;
use crate::modules::variant::WbVariant;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// 并发运行标志（替代 Python threading.Lock，Send 安全可跨 await）
// ---------------------------------------------------------------------------

/// RAII 运行标志：进入临界区置 true，Drop 时复位。
pub struct RunFlagGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> RunFlagGuard<'a> {
    /// 尝试获取标志；已被占用返回 None。
    pub fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        if flag.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self { flag })
        }
    }
}

impl Drop for RunFlagGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// 时间
// ---------------------------------------------------------------------------

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn utc_iso() -> String {
    // 对照 Python utc_iso：%Y-%m-%dT%H-%M-%S + "Z"
    format!("{}Z", chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S"))
}

// ---------------------------------------------------------------------------
// 文件
// ---------------------------------------------------------------------------

/// 原子写文件（临时文件 + rename），对照 Python atomic_write。
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{file_name}.tmp-{}", uuid::Uuid::new_v4().simple()));
    if let Err(e) = std::fs::write(&tmp, content) {
        eprintln!("[atomic] write tmp FAILED: {e}");
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("[atomic] rename FAILED: {e}");
        // rename 失败时清理临时文件，避免在目标目录残留 `<name>.tmp-*`。
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 时间戳归一化
// ---------------------------------------------------------------------------

/// 把秒/毫秒/字符串时间戳统一为毫秒；无效返回 None。对照 server.py `_norm_ts`。
pub fn norm_ts(v: Option<&Value>) -> Option<i64> {
    let mut ts: i64 = match v {
        Some(Value::String(s)) => s.trim().parse::<f64>().ok()? as i64,
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))?,
        _ => return None,
    };
    if ts < 10_000_000_000 {
        ts *= 1000; // 秒 → 毫秒
    }
    Some(ts)
}

/// 旧格式（单 `exe` 字段）解析，CodeBuddy CN 应用缓存沿用该格式。
pub(super) fn parse_workbuddy_exe_cache_json(text: &str) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(text).ok()?;
    let exe = v.get("exe")?.as_str()?.trim();
    if exe.is_empty() {
        None
    } else {
        Some(PathBuf::from(exe))
    }
}

/// 按档位读缓存：新格式按档位分键，旧格式单键仅国内版认。
pub(super) fn parse_workbuddy_exe_cache_json_for(
    text: &str,
    variant: WbVariant,
) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(text).ok()?;
    let keyed = v.get(variant.exe_cache_key());
    let legacy = if variant == WbVariant::Cn {
        v.get("exe")
    } else {
        None
    };
    let exe = keyed.or(legacy)?.as_str()?.trim();
    if exe.is_empty() {
        None
    } else {
        Some(PathBuf::from(exe))
    }
}

/// 读取上次成功解析到的 WorkBuddy 应用路径（按档位分键）；损坏或空文件视为无缓存。
pub fn load_workbuddy_exe_cache(variant: WbVariant) -> Option<PathBuf> {
    let f = workbuddy_exe_cache_file();
    if !f.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&f).ok()?;
    parse_workbuddy_exe_cache_json_for(&text, variant)
}

/// 把某档位的路径并入缓存内容（旧格式单键按国内版迁移，写回新格式）。
pub(super) fn upsert_workbuddy_exe_cache_json(
    text: &str,
    variant: WbVariant,
    exe: &Path,
) -> String {
    let mut root = serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = root.as_object_mut() {
        if let Some(Value::String(legacy)) = obj.remove("exe") {
            if !legacy.trim().is_empty() {
                obj.entry(WbVariant::Cn.exe_cache_key().to_string())
                    .or_insert(Value::String(legacy));
            }
        }
        obj.insert(
            variant.exe_cache_key().to_string(),
            json!(exe.to_string_lossy()),
        );
    }
    serde_json::to_string_pretty(&root).unwrap_or_default()
}

/// 记住已存在的 WorkBuddy 应用路径（按档位分键；旧格式单键在写回时升级）。
pub fn save_workbuddy_exe_cache(variant: WbVariant, exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let file = workbuddy_exe_cache_file();
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    atomic_write(&file, &upsert_workbuddy_exe_cache_json(&text, variant, exe))
}

/// 清除某档位的缓存项；无其它档位残留则删除文件。
pub fn clear_workbuddy_exe_cache(variant: WbVariant) {
    let file = workbuddy_exe_cache_file();
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let Ok(mut root) = serde_json::from_str::<Value>(&text) else {
        let _ = std::fs::remove_file(&file);
        return;
    };
    let Some(obj) = root.as_object_mut() else {
        let _ = std::fs::remove_file(&file);
        return;
    };
    obj.remove(variant.exe_cache_key());
    if variant == WbVariant::Cn {
        obj.remove("exe");
    }
    if obj.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let content = serde_json::to_string_pretty(&root).unwrap_or_default();
    let _ = atomic_write(&file, &content);
}

pub fn codebuddy_cn_app_cache_file() -> PathBuf {
    store_dir().join("codebuddy_cn_app.json")
}

pub(super) fn parse_codebuddy_cn_app_cache_json(text: &str) -> Option<PathBuf> {
    parse_workbuddy_exe_cache_json(text)
}

/// 读取上次成功解析到的 CodeBuddy CN 应用路径；损坏或空文件视为无缓存。
pub fn load_codebuddy_cn_app_cache() -> Option<PathBuf> {
    let f = codebuddy_cn_app_cache_file();
    if !f.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&f).ok()?;
    parse_codebuddy_cn_app_cache_json(&text)
}

pub fn save_codebuddy_cn_app_cache(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let content =
        serde_json::to_string_pretty(&json!({ "exe": path.to_string_lossy() })).unwrap_or_default();
    atomic_write(&codebuddy_cn_app_cache_file(), &content)
}

pub fn clear_codebuddy_cn_app_cache() {
    let _ = std::fs::remove_file(codebuddy_cn_app_cache_file());
}

pub fn codebuddy_ide_app_cache_file() -> PathBuf {
    store_dir().join("codebuddy_ide_app.json")
}

fn parse_codebuddy_ide_app_cache_json(text: &str) -> Option<PathBuf> {
    parse_workbuddy_exe_cache_json(text)
}

pub fn load_codebuddy_ide_app_cache() -> Option<PathBuf> {
    let f = codebuddy_ide_app_cache_file();
    if !f.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&f).ok()?;
    parse_codebuddy_ide_app_cache_json(&text)
}

pub fn save_codebuddy_ide_app_cache(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let content =
        serde_json::to_string_pretty(&json!({ "exe": path.to_string_lossy() })).unwrap_or_default();
    atomic_write(&codebuddy_ide_app_cache_file(), &content)
}

pub fn clear_codebuddy_ide_app_cache() {
    let _ = std::fs::remove_file(codebuddy_ide_app_cache_file());
}

/// 报告里某一类条目的数量（缺字段按 0）。
pub(crate) fn count_items(report: &Value, key: &str) -> usize {
    report
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}
