//! 自动化配置：限流开关、自动轮转配置与日志、通知配额。

use super::*;
use crate::modules::locks;

pub const ROTATE_LOG_MAX_RECORDS: usize = 200;

// ---------------------------------------------------------------------------
// 限额监听配置（hook 通路 + IDE 日志扫描的总开关）
// ---------------------------------------------------------------------------

/// 默认限额监听配置：默认开启（与改造前「账号页自动显示限额」的行为一致）。
///
/// `hookOptOut` = 用户点过「卸载 hook」→ 不再自动接入；默认 `false`（默认接入）。
/// `scanIdeLogs` = 是否扫描两个 CodeBuddy IDE 的日志；默认 `true`（IDE 的 429 不触发事件，
/// 日志是它唯一的数据源）。关闭只影响 IDE 两源，CLI / WorkBuddy 的 hook 通路与兜底扫描不变。
/// `crossClientInfer` = 是否允许「同账号其它客户端的限额」在 VS Code 无自有记录时兜底显示
/// （第四轮 13-B）；默认 `true`，关闭后账号卡不再出现「其它客户端限流至 …」。
pub fn default_rate_limit_config() -> Value {
    json!({ "enabled": true, "hookOptOut": false, "scanIdeLogs": true, "crossClientInfer": true })
}

/// 读取指定的限额监听配置文件（缺失/损坏时合并默认值）。
///
/// 与 `load_rate_limit_config` 分离只为注入路径：单测不得触碰真实 `~/.workbuddy-shift`。
pub fn load_rate_limit_config_at(path: &Path) -> Value {
    let mut cfg = default_rate_limit_config();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
            for key in ["enabled", "hookOptOut", "scanIdeLogs", "crossClientInfer"] {
                if let Some(value) = map.get(key).and_then(Value::as_bool) {
                    cfg[key] = json!(value);
                }
            }
        }
    }
    cfg
}

/// 读取限额监听配置（缺失/损坏时合并默认值）。
pub fn load_rate_limit_config() -> Value {
    load_rate_limit_config_at(&rate_limit_config_file())
}

/// 保存限额监听配置到指定路径（只保留已知字段）。
pub fn save_rate_limit_config_at(path: &Path, cfg: &Value) -> std::io::Result<()> {
    let mut merged = default_rate_limit_config();
    for key in ["enabled", "hookOptOut", "scanIdeLogs", "crossClientInfer"] {
        if let Some(value) = cfg.get(key).and_then(Value::as_bool) {
            merged[key] = json!(value);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&merged).unwrap_or_default();
    atomic_write(path, &content)
}

/// 只改写 `hookOptOut`（保留 `enabled` 等既有字段），返回写入后的完整配置。
///
/// 用户点「卸载 hook」置 `true`、「接入 hook」置 `false`；两处都不改动别的开关状态。
pub fn set_rate_limit_hook_opt_out_at(path: &Path, opt_out: bool) -> std::io::Result<Value> {
    let mut cfg = load_rate_limit_config_at(path);
    cfg["hookOptOut"] = json!(opt_out);
    save_rate_limit_config_at(path, &cfg)?;
    Ok(load_rate_limit_config_at(path))
}

// ---------------------------------------------------------------------------
// 自动轮换配置 / 日志（CodeBuddy CLI 账号轮换）
// ---------------------------------------------------------------------------

/// 默认自动轮换配置。
pub fn default_auto_rotate_config() -> Value {
    json!({
        "enabled": false,
        "check_interval_minutes": 5,
        "cooldown_minutes": 120,
        "min_gap_hours": 24,
        "min_urgency_hours": 72,
        "active_guard_minutes": 30,
        "min_remaining_credits": 0,
    })
}

/// 读取自动轮换配置（缺失/损坏时合并默认值）。
pub fn load_auto_rotate_config() -> Value {
    let mut cfg = default_auto_rotate_config();
    let f = auto_rotate_config_file();
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
                for (k, v) in map {
                    cfg[k] = v;
                }
            }
        }
    }
    cfg
}

/// 保存自动轮换配置（只保留已知字段）。
pub fn save_auto_rotate_config(cfg: &Value) -> std::io::Result<()> {
    let mut merged = default_auto_rotate_config();
    let allowed: Vec<&str> = vec![
        "enabled",
        "check_interval_minutes",
        "cooldown_minutes",
        "min_gap_hours",
        "min_urgency_hours",
        "active_guard_minutes",
        "min_remaining_credits",
    ];
    for k in allowed {
        if let Some(v) = cfg.get(k) {
            merged[k] = v.clone();
        }
    }
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(&merged).unwrap_or_default();
    atomic_write(&auto_rotate_config_file(), &content)
}

/// 读取自动轮换日志。
pub fn load_rotate_logs() -> Vec<Value> {
    let f = auto_rotate_logs_file();
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) {
                return arr;
            }
        }
    }
    vec![]
}

/// 保存自动轮换日志（保留最近 N 条，保持插入顺序）。
pub fn save_rotate_logs(logs: &[Value]) -> std::io::Result<()> {
    let mut kept: Vec<Value> = logs.to_vec();
    if kept.len() > ROTATE_LOG_MAX_RECORDS {
        kept.drain(..kept.len() - ROTATE_LOG_MAX_RECORDS);
    }
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(&kept).unwrap_or_default();
    atomic_write(&auto_rotate_logs_file(), &content)
}

/// 追加一条自动轮换日志。
pub fn add_rotate_log(entry: &Value) {
    let mut logs = load_rotate_logs();
    logs.push(entry.clone());
    let _ = save_rotate_logs(&logs);
}

// ---------------------------------------------------------------------------
// 轮换推迟提示预算（`~/.workbuddy-shift/auto_rotate_notify.json`）
// ---------------------------------------------------------------------------

/// 提示预算文件名（`store_dir()/auto_rotate_notify.json`）。
pub(crate) const ROTATE_NOTIFY_FILE_NAME: &str = "auto_rotate_notify.json";

/// 同一自然日内最多提示几次；超出只写轮换日志，不再打扰用户。
pub const ROTATE_NOTIFY_DAILY_LIMIT: u32 = 5;

pub(crate) static ROTATE_NOTIFY_LOCK: Mutex<()> = Mutex::new(());

pub fn auto_rotate_notify_file() -> PathBuf {
    store_dir().join(ROTATE_NOTIFY_FILE_NAME)
}

/// 本地日期（`YYYY-MM-DD`）：提示预算的跨日重置口径（与签到日志同一套本地时间）。
pub(crate) fn local_date(at_ms: i64) -> String {
    Local
        .timestamp_millis_opt(at_ms)
        .single()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 读当日已用次数：文件缺失 / 损坏 / 日期不是今天（跨日）一律按 0 计。
pub(crate) fn rotate_notify_count_at(path: &Path, today: &str) -> u32 {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if value.get("date").and_then(Value::as_str) != Some(today) {
        return 0;
    }
    value
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}

/// 领取一次「轮换推迟提示」配额：`true` = 可以投递（并且已经计数）。
///
/// 同自然日上限 [`ROTATE_NOTIFY_DAILY_LIMIT`]，跨日按本地日期清零；读取失败/损坏视为 0，
/// 不阻塞轮换。预算文件写不进去时不投递——宁可少一条通知，也不要每轮都弹。
pub fn try_consume_rotate_notify(at_ms: i64) -> bool {
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续读写配额文件。
    let _guard = locks::recover(&ROTATE_NOTIFY_LOCK);
    let path = auto_rotate_notify_file();
    try_consume_rotate_notify_at(&path, &local_date(at_ms))
}

pub(crate) fn try_consume_rotate_notify_at(path: &Path, today: &str) -> bool {
    if today.is_empty() {
        return false;
    }
    let used = rotate_notify_count_at(path, today);
    if used >= ROTATE_NOTIFY_DAILY_LIMIT {
        return false;
    }
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    let content = serde_json::to_string_pretty(&json!({
        "date": today,
        "count": used + 1,
    }))
    .unwrap_or_default();
    atomic_write(path, &content).is_ok()
}
