//! 旅行（成长中心）配置与缓存：默认值、读写。

use super::*;

// ---------------------------------------------------------------------------
// 派猫猫旅行配置 / 缓存
// ---------------------------------------------------------------------------

/// 默认自动旅行配置（全新安装默认关闭）。
pub fn default_travel_config() -> Value {
    travel_config_with_enabled(false)
}

/// 历史默认自动旅行配置（`enabled: true`）：已有用户没有显式值时沿用。
pub(crate) fn legacy_default_travel_config() -> Value {
    travel_config_with_enabled(true)
}

pub(crate) fn travel_config_with_enabled(enabled: bool) -> Value {
    json!({ "enabled": enabled })
}

/// 读取自动旅行配置（缺失/损坏时合并默认值）。
///
/// 与签到同理：配置只在显式保存时落盘，按使用痕迹（配置文件或旅行缓存）判定已有用户，
/// 没有显式值时沿用历史默认（开启）；全新安装默认关闭。
pub fn load_travel_config() -> Value {
    load_travel_config_at(&travel_config_file(), &travel_cache_file())
}

/// 设置页回包：配置 + 出厂默认值（`defaults`，纯增量字段）。
pub fn load_travel_config_with_defaults() -> Value {
    with_defaults(load_travel_config(), default_travel_config())
}

pub(crate) fn load_travel_config_at(config_path: &Path, cache_path: &Path) -> Value {
    let input = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    resolve_travel_config(input.as_ref(), config_path.exists() || cache_path.exists())
}

pub(crate) fn resolve_travel_config(input: Option<&Value>, existing_install: bool) -> Value {
    let mut cfg = if existing_install {
        legacy_default_travel_config()
    } else {
        default_travel_config()
    };
    if let Some(enabled) = input
        .and_then(Value::as_object)
        .and_then(|map| map.get("enabled"))
        .and_then(Value::as_bool)
    {
        cfg["enabled"] = json!(enabled);
    }
    cfg
}

/// 保存自动旅行配置（只保留已知字段）。
pub fn save_travel_config(cfg: &Value) -> std::io::Result<()> {
    let mut merged = default_travel_config();
    if let Some(enabled) = cfg.get("enabled").and_then(Value::as_bool) {
        merged["enabled"] = json!(enabled);
    }
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(&merged).unwrap_or_default();
    atomic_write(&travel_config_file(), &content)
}

/// 读取旅行缓存（`{ date, completed, results: { accountId: {...} } }`）。
pub fn load_travel_cache() -> Value {
    let f = travel_cache_file();
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
                return Value::Object(map);
            }
        }
    }
    json!({})
}

/// 保存旅行缓存。
pub fn save_travel_cache(cache: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir())?;
    let content = serde_json::to_string_pretty(cache).unwrap_or_default();
    atomic_write(&travel_cache_file(), &content)
}
