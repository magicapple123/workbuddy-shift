//! 签到配置与日志：默认值/合并、读写、规范化/压缩与日志追加。

use super::*;
use crate::modules::locks;

// ---------------------------------------------------------------------------
// 签到配置 / 日志（对照 server.py load/save_checkin_config / load/save/add_checkin_log）
// ---------------------------------------------------------------------------

/// 默认签到配置。旧时间窗口字段仅为配置文件兼容保留，调度不再读取。
///
/// `enabled` 默认关闭：全新安装需用户在设置页显式开启；已有用户在
/// [`load_checkin_config`] 中沿用历史默认（开启），升级不改其状态。
/// `checkin_start` / `checkin_end` 为空串 = 不限制签到时间段（与改动前行为一致）。
pub fn default_checkin_config() -> Value {
    checkin_config_with_enabled(false)
}

/// 历史默认签到配置（`enabled: true`）：已有用户没有显式值时沿用。
pub(crate) fn legacy_default_checkin_config() -> Value {
    checkin_config_with_enabled(true)
}

pub(crate) fn checkin_config_with_enabled(enabled: bool) -> Value {
    json!({
        "enabled": enabled,
        "excluded_account_ids": [],
        "checkin_start": "",
        "checkin_end": "",
        "start_hour": 6,
        "end_hour": 12,
        "keepalive_days": 0,
        "lazy_refresh_hours": 24,
    })
}

/// 解析 `"HH:MM"` 本地时钟（允许 1–2 位时/分，如 `"9:5"`）。
///
/// 空串、多余字符、越界（`"24:00"` / `"23:60"`）一律返回 `None`。
pub fn parse_clock(raw: &str) -> Option<(u32, u32)> {
    let (hour, minute) = raw.split_once(':')?;
    if !(1..=2).contains(&hour.len()) || !(1..=2).contains(&minute.len()) {
        return None;
    }
    if !hour.bytes().all(|b| b.is_ascii_digit()) || !minute.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

/// 把 `input` 中的已知字段覆盖到 `merged`；缺失 / 非法字段保持 `merged` 原值。
pub(crate) fn apply_checkin_config(merged: &mut Value, input: &Value) {
    let Some(map) = input.as_object() else {
        return;
    };
    if let Some(enabled) = map.get("enabled").and_then(Value::as_bool) {
        merged["enabled"] = json!(enabled);
    }
    // 仅用稳定账号 id 排除自动签到；忽略无效项并去重，旧配置默认全部参与。
    if let Some(ids) = map.get("excluded_account_ids").and_then(Value::as_array) {
        let mut seen = HashSet::new();
        let ids: Vec<&str> = ids
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| !id.trim().is_empty() && seen.insert(*id))
            .collect();
        merged["excluded_account_ids"] = json!(ids);
    }
    for key in [
        "start_hour",
        "end_hour",
        "keepalive_days",
        "lazy_refresh_hours",
    ] {
        if let Some(value) = map.get(key).and_then(Value::as_i64) {
            merged[key] = json!(value);
        }
    }
    // 时间段保存归一化：合法值零填充后落盘，非法/非字符串归一为空串（= 不限制）。
    for key in ["checkin_start", "checkin_end"] {
        let normalized = map
            .get(key)
            .and_then(Value::as_str)
            .and_then(parse_clock)
            .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
            .unwrap_or_default();
        merged[key] = json!(normalized);
    }
}

/// 以新默认值为基线合并（保存路径）。
pub(crate) fn merge_checkin_config(input: &Value) -> Value {
    let mut merged = default_checkin_config();
    apply_checkin_config(&mut merged, input);
    merged
}

/// 读取签到配置（缺失/损坏时合并默认值）。
///
/// 配置只在用户显式保存时落盘，「无配置文件」无法区分新老安装：按使用痕迹（配置文件
/// 或签到日志）判定已有用户，没有显式值时沿用历史默认（开启）；全新安装默认关闭。
pub fn load_checkin_config() -> Value {
    load_checkin_config_at(&checkin_config_file(), &checkin_logs_file())
}

/// 设置页回包：配置 + 出厂默认值（`defaults`，纯增量字段）。
///
/// 「已落盘还是全新安装」的判定仍由 [`load_checkin_config`] 决定；`defaults` 只描述
/// 「恢复默认会写到什么」，因此永远取新安装口径的 [`default_checkin_config`]。
pub fn load_checkin_config_with_defaults() -> Value {
    with_defaults(load_checkin_config(), default_checkin_config())
}

pub(crate) fn load_checkin_config_at(config_path: &Path, logs_path: &Path) -> Value {
    let input = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    resolve_checkin_config(input.as_ref(), config_path.exists() || logs_path.exists())
}

pub(crate) fn resolve_checkin_config(input: Option<&Value>, existing_install: bool) -> Value {
    let mut merged = if existing_install {
        legacy_default_checkin_config()
    } else {
        default_checkin_config()
    };
    if let Some(input) = input {
        apply_checkin_config(&mut merged, input);
    }
    merged
}

/// 保存签到配置（只保留已知字段）。
pub fn save_checkin_config(cfg: &Value) -> std::io::Result<()> {
    let merged = merge_checkin_config(cfg);
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(&merged).unwrap_or_default();
    atomic_write(&checkin_config_file(), &content)
}

/// 读取签到日志。
pub fn load_checkin_logs() -> Vec<Value> {
    let f = checkin_logs_file();
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) {
                return arr;
            }
        }
    }
    vec![]
}

pub(crate) fn save_checkin_logs_unlocked(logs: &[Value]) -> std::io::Result<()> {
    let kept = normalize_checkin_logs(logs, now_ms());
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(&kept).unwrap_or_default();
    atomic_write(&checkin_logs_file(), &content)
}

pub(crate) fn checkin_log_local_date(ts_ms: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

pub(crate) fn legacy_checkin_identity(entry: &Value) -> Option<String> {
    if let Some(account_id) = entry
        .get("accountId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(format!("account:{account_id}"));
    }

    // Old log rows predate accountId and only carried the display identity in
    // `email`. Keep this fallback namespaced so it can never merge with a
    // stable local account ID that happens to have the same text.
    entry
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|email| format!("legacy:{email}"))
}

/// Apply the persisted check-in log contract without changing the source file.
///
/// `success` and `error` entries retain their full multiplicity. Only repeated
/// legacy `already` rows are reduced to the latest timestamp for one account
/// and local calendar date.
pub(crate) fn normalize_checkin_logs(logs: &[Value], at_ms: i64) -> Vec<Value> {
    let cutoff = at_ms.saturating_sub(CHECKIN_LOG_KEEP_DAYS * 24 * 3600 * 1000);
    let retained: Vec<(usize, i64, &Value)> = logs
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let ts = norm_ts(entry.get("ts"))?;
            (ts >= cutoff).then_some((index, ts, entry))
        })
        .collect();

    let mut dedupable_already_indices = HashSet::new();
    let mut latest_already = HashMap::<(String, String), (i64, usize)>::new();
    for (index, ts, entry) in &retained {
        if entry.get("result").and_then(Value::as_str) != Some("already") {
            continue;
        }
        let Some(identity) = legacy_checkin_identity(entry) else {
            continue;
        };
        let Some(date) = checkin_log_local_date(*ts) else {
            continue;
        };
        dedupable_already_indices.insert(*index);
        let candidate = (*ts, *index);
        latest_already
            .entry((identity, date))
            .and_modify(|current| {
                if candidate >= *current {
                    *current = candidate;
                }
            })
            .or_insert(candidate);
    }

    let winning_already_indices: HashSet<usize> = latest_already
        .into_values()
        .map(|(_, index)| index)
        .collect();
    let mut normalized: Vec<Value> = retained
        .into_iter()
        .filter(|(index, _, entry)| {
            entry.get("result").and_then(Value::as_str) != Some("already")
                || !dedupable_already_indices.contains(index)
                || winning_already_indices.contains(index)
        })
        .map(|(_, _, entry)| entry.clone())
        .collect();

    if normalized.len() > CHECKIN_LOG_MAX_RECORDS {
        normalized.drain(..normalized.len() - CHECKIN_LOG_MAX_RECORDS);
    }
    normalized
}

pub(crate) fn compact_checkin_logs_at(path: &Path, at_ms: i64) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path)?;
    let Ok(Value::Array(logs)) = serde_json::from_str::<Value>(&text) else {
        // Preserve unreadable user data rather than replacing it with an empty
        // file. Normal log loading keeps its existing tolerant behavior.
        return Ok(false);
    };
    let normalized = normalize_checkin_logs(&logs, at_ms);
    if normalized == logs {
        return Ok(false);
    }
    let content = serde_json::to_string_pretty(&normalized).unwrap_or_default();
    atomic_write(path, &content)?;
    Ok(true)
}

/// Compact legacy persisted check-in logs once during host startup.
///
/// Returns `true` only when the file was rewritten. Loading logs remains a
/// read-only operation; both hosts invoke this explicit migration before their
/// first automatic verification cycle.
pub fn compact_checkin_logs() -> std::io::Result<bool> {
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续写入。
    let _guard = locks::recover(&CHECKIN_LOG_WRITE_LOCK);
    compact_checkin_logs_at(&checkin_logs_file(), now_ms())
}

/// 追加一条签到日志。
pub fn add_checkin_log(entry: &Value) {
    // Account-scoped check-in coordination permits unrelated accounts to run
    // concurrently. Serialize the file read-modify-write so neither entry is lost.
    let _guard = locks::recover(&CHECKIN_LOG_WRITE_LOCK);
    let mut logs = load_checkin_logs();
    logs.push(entry.clone());
    let _ = save_checkin_logs_unlocked(&logs);
}
