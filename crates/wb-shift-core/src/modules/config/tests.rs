use super::configs::auto_rotate_notify_file;
use super::configs::compact_checkin_logs_at;
use super::configs::default_checkin_config;
use super::configs::legacy_default_checkin_config;
use super::configs::load_checkin_config_at;
use super::configs::load_rate_limit_config_at;
use super::configs::load_travel_config_at;
use super::configs::merge_checkin_config;
use super::configs::normalize_checkin_logs;
use super::configs::parse_clock;
use super::configs::resolve_checkin_config;
use super::configs::resolve_travel_config;
use super::configs::save_rate_limit_config_at;
use super::configs::set_rate_limit_hook_opt_out_at;
use super::configs::try_consume_rotate_notify_at;
use super::configs::ROTATE_NOTIFY_DAILY_LIMIT;
use super::configs::ROTATE_NOTIFY_FILE_NAME;
use super::http::http_client_builder;
use super::http::is_route_missing;
use super::http::normalize_error_body;
use super::http::DEFAULT_HTTP_USER_AGENT;
use super::paths::dev_home_dir_at;
use super::paths::is_sandboxed_home;
use super::paths::rate_limit_config_file;
use super::paths::workbuddy_exe_cache_file;
use super::paths::DEV_SANDBOX_DIR_NAME;
use super::store::codebuddy_cn_app_cache_file;
use super::store::codebuddy_ide_app_cache_file;
use super::store::parse_codebuddy_cn_app_cache_json;
use super::store::parse_workbuddy_exe_cache_json;
use super::store::parse_workbuddy_exe_cache_json_for;
use super::store::upsert_workbuddy_exe_cache_json;
use super::CHECKIN_LOG_KEEP_DAYS;
use super::CHECKIN_LOG_MAX_RECORDS;
use crate::modules::variant::WbVariant;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 回归 issue #94：网关 401 返回的整页 HTML 要归一化为可读信息，
/// 不能把 `<html>…` 原样塞进通知与界面卡片。
#[test]
fn normalize_error_body_extracts_html_title() {
    let html = "<html>\n<head><title>401 Authorization Required</title></head>\n\
                <body>\n<center><h1>401 Authorization Required</h1></center>\n\
                <hr><center>openresty</center>\n</body>\n</html>\n";
    assert_eq!(
        normalize_error_body(html),
        "服务端返回 HTML 错误页：401 Authorization Required"
    );

    assert_eq!(
        normalize_error_body("<!DOCTYPE html><html><body>boom</body></html>"),
        "服务端返回 HTML 错误页（无标题）"
    );

    // 非 HTML 错误体保持原有截断行为。
    let plain = "plain gateway error";
    assert_eq!(normalize_error_body(plain), plain);
    let long = "x".repeat(600);
    assert_eq!(normalize_error_body(&long).chars().count(), 500);
}

fn local_timestamp_ms(year: i32, month: u32, day: u32, hour: u32) -> i64 {
    Local
        .with_ymd_and_hms(year, month, day, hour, 0, 0)
        .single()
        .expect("test timestamp must be unambiguous")
        .timestamp_millis()
}

#[test]
fn auto_checkin_defaults_disabled_and_preserves_legacy_fields() {
    let cfg = default_checkin_config();
    assert_eq!(cfg.get("enabled").and_then(Value::as_bool), Some(false));
    assert_eq!(cfg["excluded_account_ids"], json!([]));
    assert_eq!(cfg.get("start_hour").and_then(Value::as_i64), Some(6));
    assert_eq!(cfg.get("end_hour").and_then(Value::as_i64), Some(12));

    // 历史默认仍保留开启，供已有用户在读取路径上沿用。
    let legacy = legacy_default_checkin_config();
    assert_eq!(legacy.get("enabled").and_then(Value::as_bool), Some(true));
    assert_eq!(
        legacy.get("lazy_refresh_hours").and_then(Value::as_i64),
        Some(24)
    );
}

#[test]
fn auto_checkin_exclusions_survive_config_roundtrip_and_global_toggle() {
    let cfg = merge_checkin_config(&json!({
        "enabled": true,
        "excluded_account_ids": ["account-b", "account-a", "account-b", "", "  ", null, 42],
        "checkin_start": "9:5",
        "checkin_end": "12:00",
        "keepalive_days": 7
    }));
    assert_eq!(
        cfg["excluded_account_ids"],
        json!(["account-b", "account-a"])
    );
    let serialized = serde_json::to_string(&cfg).unwrap();
    let mut reloaded: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(merge_checkin_config(&reloaded), cfg);
    reloaded["enabled"] = json!(false);
    let disabled = merge_checkin_config(&reloaded);
    assert_eq!(
        disabled["excluded_account_ids"],
        cfg["excluded_account_ids"]
    );
    assert_eq!(disabled["checkin_start"], "09:05");
    assert_eq!(disabled["keepalive_days"], 7);
}

#[test]
fn auto_checkin_legacy_or_invalid_exclusions_default_to_empty() {
    assert_eq!(
        merge_checkin_config(&json!({}))["excluded_account_ids"],
        json!([])
    );
    for invalid in [
        json!(null),
        json!(true),
        json!("account-a"),
        json!({"id": "account-a"}),
    ] {
        assert_eq!(
            merge_checkin_config(&json!({"excluded_account_ids": invalid}))["excluded_account_ids"],
            json!([])
        );
    }
}

#[test]
fn auto_checkin_exclusions_survive_usage_trace_defaults() {
    let input = json!({"excluded_account_ids": ["account-a", "account-a", null]});
    for existing_install in [false, true] {
        let resolved = resolve_checkin_config(Some(&input), existing_install);
        assert_eq!(resolved["enabled"], json!(existing_install));
        assert_eq!(resolved["excluded_account_ids"], json!(["account-a"]));
        // 保存已解析配置后，新老安装均保留开关状态和账号排除列表。
        assert_eq!(merge_checkin_config(&resolved), resolved);
    }
}

#[test]
fn checkin_default_follows_usage_trace() {
    // 全新安装（无配置文件、无签到日志）：默认关闭。
    let fresh = resolve_checkin_config(None, false);
    assert_eq!(fresh.get("enabled").and_then(Value::as_bool), Some(false));

    // 已有用户（签到日志即使用痕迹）：沿用历史默认开启，升级不改状态。
    let existing = resolve_checkin_config(None, true);
    assert_eq!(existing.get("enabled").and_then(Value::as_bool), Some(true));
    // 痕迹只影响 enabled，其余字段仍与默认一致。
    assert_eq!(
        existing.get("lazy_refresh_hours").and_then(Value::as_i64),
        Some(24)
    );
    assert_eq!(existing.get("checkin_start"), Some(&json!("")));

    // 显式值优先于痕迹。
    for existing_install in [false, true] {
        let off = resolve_checkin_config(Some(&json!({"enabled": false})), existing_install);
        assert_eq!(off.get("enabled").and_then(Value::as_bool), Some(false));
        let on = resolve_checkin_config(Some(&json!({"enabled": true})), existing_install);
        assert_eq!(on.get("enabled").and_then(Value::as_bool), Some(true));
    }
}

#[test]
fn load_checkin_config_uses_usage_trace_files() {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-checkin-config-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("auto_checkin_config.json");
    let logs_path = dir.join("auto_checkin_logs.json");

    // 两者皆无 → 全新安装，默认关闭。
    assert_eq!(
        load_checkin_config_at(&config_path, &logs_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );

    // 只有签到日志（使用痕迹）→ 已有用户，保持开启。
    std::fs::write(&logs_path, "[]").unwrap();
    assert_eq!(
        load_checkin_config_at(&config_path, &logs_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );

    // 显式保存的 false 覆盖痕迹。
    std::fs::write(&config_path, "{\"enabled\": false}").unwrap();
    assert_eq!(
        load_checkin_config_at(&config_path, &logs_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );

    // 配置文件损坏但存在 → 仍按已有用户处理（不因损坏而改状态）。
    std::fs::remove_file(&logs_path).unwrap();
    std::fs::write(&config_path, "not-json").unwrap();
    assert_eq!(
        load_checkin_config_at(&config_path, &logs_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn travel_default_follows_usage_trace() {
    // 全新安装默认关闭，已有用户（旅行缓存痕迹）沿用开启。
    assert_eq!(
        resolve_travel_config(None, false)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        resolve_travel_config(None, true)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );

    // 显式值优先；非法值回落基线。
    assert_eq!(
        resolve_travel_config(Some(&json!({"enabled": false})), true)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        resolve_travel_config(Some(&json!({"enabled": true})), false)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        resolve_travel_config(Some(&json!({"enabled": "no"})), false)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn load_travel_config_uses_usage_trace_files() {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-travel-config-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("auto_travel_config.json");
    let cache_path = dir.join("travel_cache.json");

    // 两者皆无 → 全新安装，默认关闭。
    assert_eq!(
        load_travel_config_at(&config_path, &cache_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );

    // 只有旅行缓存（使用痕迹）→ 已有用户，保持开启。
    std::fs::write(&cache_path, "{}").unwrap();
    assert_eq!(
        load_travel_config_at(&config_path, &cache_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );

    // 显式保存的 false 覆盖痕迹。
    std::fs::write(&config_path, "{\"enabled\": false}").unwrap();
    assert_eq!(
        load_travel_config_at(&config_path, &cache_path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(false)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_checkin_explicit_value_wins_and_invalid_value_uses_default() {
    // 保存路径（以新默认值为基线）：显式值原样保留，缺失/非法才回落默认。
    let enabled = merge_checkin_config(&json!({"enabled": true, "keepalive_days": 7}));
    assert_eq!(enabled.get("enabled").and_then(Value::as_bool), Some(true));
    assert_eq!(
        enabled.get("keepalive_days").and_then(Value::as_i64),
        Some(7)
    );

    let disabled = merge_checkin_config(&json!({"enabled": false, "keepalive_days": 7}));
    assert_eq!(
        disabled.get("enabled").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        disabled.get("keepalive_days").and_then(Value::as_i64),
        Some(7)
    );

    let corrupt = merge_checkin_config(&json!({"enabled": "no", "lazy_refresh_hours": null}));
    assert_eq!(corrupt.get("enabled").and_then(Value::as_bool), Some(false));
    assert_eq!(
        corrupt.get("lazy_refresh_hours").and_then(Value::as_i64),
        Some(24)
    );
}

#[test]
fn parse_clock_accepts_short_forms_and_rejects_malformed_values() {
    assert_eq!(parse_clock("22:00"), Some((22, 0)));
    assert_eq!(parse_clock("9:5"), Some((9, 5)));
    assert_eq!(parse_clock("00:00"), Some((0, 0)));
    assert_eq!(parse_clock("23:59"), Some((23, 59)));

    for raw in [
        "24:00",     // 小时越界
        "23:60",     // 分钟越界
        "22",        // 缺分钟
        "",          // 空串
        "abc",       // 非数字
        "22:00:00",  // 多余字符
        "22:",       // 分钟为空
        ":00",       // 小时为空
        "-1:00",     // 符号
        "+1:00",     // 符号
        " 22:00",    // 前导空格
        "22:00 ",    // 尾随空格
        "０１:００", // 全角数字
    ] {
        assert_eq!(parse_clock(raw), None, "必须拒绝 {raw:?}");
    }
}

#[test]
fn checkin_window_defaults_to_unset_and_normalizes_on_save() {
    let defaults = default_checkin_config();
    assert_eq!(defaults.get("checkin_start"), Some(&json!("")));
    assert_eq!(defaults.get("checkin_end"), Some(&json!("")));

    // 合法值零填充后落盘。
    let merged = merge_checkin_config(&json!({
        "checkin_start": "9:5",
        "checkin_end": "23:30"
    }));
    assert_eq!(merged.get("checkin_start"), Some(&json!("09:05")));
    assert_eq!(merged.get("checkin_end"), Some(&json!("23:30")));
}

#[test]
fn checkin_window_invalid_values_normalize_to_empty_and_keep_other_fields() {
    // 缺失 → 空串。
    let missing = merge_checkin_config(&json!({}));
    assert_eq!(missing.get("checkin_start"), Some(&json!("")));
    assert_eq!(missing.get("checkin_end"), Some(&json!("")));

    // 非字符串 / 越界 / 格式错误 → 空串，不落盘未知值。
    for bad in [json!(1234), json!(null), json!(true), json!("25:00")] {
        let merged = merge_checkin_config(&json!({"checkin_start": bad}));
        assert_eq!(
            merged.get("checkin_start"),
            Some(&json!("")),
            "非法取值必须归一为空串: {bad:?}"
        );
    }

    // 其它字段与旧时间窗口字段不受影响。
    let merged = merge_checkin_config(&json!({
        "checkin_start": "25:00",
        "checkin_end": 1234,
        "enabled": false,
        "keepalive_days": 7,
        "start_hour": 3,
        "end_hour": 9
    }));
    assert_eq!(merged.get("enabled").and_then(Value::as_bool), Some(false));
    assert_eq!(
        merged.get("keepalive_days").and_then(Value::as_i64),
        Some(7)
    );
    assert_eq!(merged.get("start_hour").and_then(Value::as_i64), Some(3));
    assert_eq!(merged.get("end_hour").and_then(Value::as_i64), Some(9));
    assert_eq!(
        merged.get("lazy_refresh_hours").and_then(Value::as_i64),
        Some(24)
    );
}

#[test]
fn checkin_log_normalization_keeps_latest_already_per_identity_and_local_date() {
    let day = local_timestamp_ms(2026, 8, 20, 12);
    let logs = vec![
        json!({"accountId": "a", "email": "same", "result": "already", "ts": day + 1, "marker": "a-old"}),
        json!({"accountId": "b", "email": "same", "result": "already", "ts": day + 2, "marker": "b"}),
        json!({"accountId": "a", "email": "same", "result": "already", "ts": day + 3, "marker": "a-new"}),
        json!({"email": "legacy@example.com", "result": "already", "ts": day + 4, "marker": "legacy-old"}),
        json!({"email": "legacy@example.com", "result": "already", "ts": day + 5, "marker": "legacy-new"}),
        json!({"result": "already", "ts": day + 6, "marker": "no-identity"}),
    ];

    let normalized = normalize_checkin_logs(&logs, day + 10);
    let markers: Vec<&str> = normalized
        .iter()
        .filter_map(|entry| entry.get("marker").and_then(Value::as_str))
        .collect();

    assert_eq!(markers, vec!["b", "a-new", "legacy-new", "no-identity"]);
}

#[test]
fn checkin_log_identity_namespaces_stable_ids_and_legacy_email_fallbacks() {
    let day = local_timestamp_ms(2026, 8, 20, 12);
    let logs = vec![
        json!({"accountId": "same@example.com", "email": "display", "result": "already", "ts": day + 1, "marker": "stable-old"}),
        json!({"email": "same@example.com", "result": "already", "ts": day + 2, "marker": "legacy-old"}),
        json!({"accountId": "same@example.com", "email": "display", "result": "already", "ts": day + 3, "marker": "stable-new"}),
        json!({"email": "same@example.com", "result": "already", "ts": day + 4, "marker": "legacy-new"}),
    ];

    let normalized = normalize_checkin_logs(&logs, day + 10);
    let markers: Vec<&str> = normalized
        .iter()
        .filter_map(|entry| entry.get("marker").and_then(Value::as_str))
        .collect();

    assert_eq!(markers, vec!["stable-new", "legacy-new"]);
}

#[test]
fn checkin_log_normalization_keeps_already_for_separate_dates() {
    let first_day = local_timestamp_ms(2026, 8, 19, 12);
    let second_day = local_timestamp_ms(2026, 8, 20, 12);
    let logs = vec![
        json!({"accountId": "a", "result": "already", "ts": first_day}),
        json!({"accountId": "a", "result": "already", "ts": second_day}),
    ];

    assert_eq!(normalize_checkin_logs(&logs, second_day).len(), 2);
}

#[test]
fn checkin_log_normalization_preserves_success_and_error_multiplicity() {
    let day = local_timestamp_ms(2026, 8, 20, 12);
    let logs = vec![
        json!({"accountId": "a", "result": "success", "ts": day + 1}),
        json!({"accountId": "a", "result": "success", "ts": day + 2}),
        json!({"accountId": "a", "result": "error", "ts": day + 3}),
        json!({"accountId": "a", "result": "error", "ts": day + 4}),
    ];

    assert_eq!(normalize_checkin_logs(&logs, day + 10), logs);
}

#[test]
fn checkin_log_normalization_applies_retention_and_record_cap() {
    let now = local_timestamp_ms(2026, 8, 20, 12);
    let cutoff = now - CHECKIN_LOG_KEEP_DAYS * 24 * 3600 * 1000;
    let mut logs = vec![json!({
        "accountId": "old",
        "result": "success",
        "ts": cutoff - 1,
        "marker": -1,
    })];
    logs.extend((0..505).map(|marker| {
        json!({
            "accountId": "a",
            "result": "success",
            "ts": now,
            "marker": marker,
        })
    }));

    let normalized = normalize_checkin_logs(&logs, now);
    assert_eq!(normalized.len(), CHECKIN_LOG_MAX_RECORDS);
    assert_eq!(normalized[0]["marker"], 5);
    assert_eq!(normalized.last().unwrap()["marker"], 504);
}

#[test]
fn checkin_log_normalization_deduplicates_before_taking_final_500() {
    let now = local_timestamp_ms(2026, 8, 20, 12);
    let mut logs = vec![
        json!({"accountId": "duplicate", "result": "already", "ts": now - 2, "marker": "duplicate-old"}),
        json!({"accountId": "duplicate", "result": "already", "ts": now - 1, "marker": "duplicate-new"}),
    ];
    logs.extend((0..500).map(|marker| {
        json!({
            "accountId": "a",
            "result": "success",
            "ts": now,
            "marker": marker,
        })
    }));

    let normalized = normalize_checkin_logs(&logs, now);
    assert_eq!(normalized.len(), CHECKIN_LOG_MAX_RECORDS);
    assert_eq!(normalized[0]["marker"], 0);
    assert_eq!(normalized.last().unwrap()["marker"], 499);
    assert!(normalized
        .iter()
        .all(|entry| entry["marker"] != "duplicate-old"));
}

#[test]
fn checkin_log_normalization_is_idempotent() {
    let day = local_timestamp_ms(2026, 8, 20, 12);
    let logs = vec![
        json!({"accountId": "a", "result": "already", "ts": day + 1}),
        json!({"accountId": "a", "result": "already", "ts": day + 2}),
        json!({"accountId": "a", "result": "success", "ts": day + 3}),
    ];

    let once = normalize_checkin_logs(&logs, day + 10);
    assert_eq!(normalize_checkin_logs(&once, day + 10), once);
}

#[test]
fn persisted_checkin_log_compaction_writes_only_when_changed() {
    let day = local_timestamp_ms(2026, 8, 20, 12);
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-checkin-log-compaction-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("logs.json");
    let logs = json!([
        {"accountId": "a", "result": "already", "ts": day + 1},
        {"accountId": "a", "result": "already", "ts": day + 2}
    ]);
    std::fs::write(&path, serde_json::to_string_pretty(&logs).unwrap()).unwrap();

    assert!(compact_checkin_logs_at(&path, day + 10).unwrap());
    let after_first = std::fs::read_to_string(&path).unwrap();
    assert!(!compact_checkin_logs_at(&path, day + 10).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after_first);

    let missing = dir.join("missing.json");
    assert!(!compact_checkin_logs_at(&missing, day + 10).unwrap());
    assert!(!missing.exists());

    let corrupt = dir.join("corrupt.json");
    std::fs::write(&corrupt, "not-json").unwrap();
    assert!(!compact_checkin_logs_at(&corrupt, day + 10).unwrap());
    assert_eq!(std::fs::read_to_string(&corrupt).unwrap(), "not-json");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn parse_workbuddy_exe_cache_json_reads_exe() {
    let path = parse_workbuddy_exe_cache_json(
        r#"{ "exe": "D:\\Users\\Zhou\\AppData\\Local\\Programs\\WorkBuddy\\WorkBuddy.exe" }"#,
    )
    .expect("valid cache");
    assert_eq!(
        path.to_string_lossy(),
        r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe"
    );
}

#[test]
fn parse_workbuddy_exe_cache_json_ignores_corrupt_and_empty() {
    assert!(parse_workbuddy_exe_cache_json("not-json").is_none());
    assert!(parse_workbuddy_exe_cache_json(r#"{ "exe": "  " }"#).is_none());
    assert!(parse_workbuddy_exe_cache_json("{}").is_none());
}

#[test]
fn workbuddy_exe_cache_reads_new_format_per_variant() {
    let text = r#"{
  "cn": "C:\\Programs\\WorkBuddy\\WorkBuddy.exe",
  "ai": "C:\\Programs\\WorkBuddyAI\\WorkBuddyAI.exe"
}"#;
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(text, WbVariant::Cn)
            .unwrap()
            .to_string_lossy(),
        r"C:\Programs\WorkBuddy\WorkBuddy.exe"
    );
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(text, WbVariant::Ai)
            .unwrap()
            .to_string_lossy(),
        r"C:\Programs\WorkBuddyAI\WorkBuddyAI.exe"
    );
}

#[test]
fn workbuddy_exe_cache_reads_legacy_single_key_as_cn_only() {
    let text = r#"{ "exe": "C:\\Programs\\WorkBuddy\\WorkBuddy.exe" }"#;
    assert!(parse_workbuddy_exe_cache_json_for(text, WbVariant::Cn).is_some());
    assert!(parse_workbuddy_exe_cache_json_for(text, WbVariant::Ai).is_none());
    assert!(parse_workbuddy_exe_cache_json_for(r#"{ "exe": "  " }"#, WbVariant::Cn).is_none());
    assert!(parse_workbuddy_exe_cache_json_for("not-json", WbVariant::Cn).is_none());
}

#[test]
fn workbuddy_exe_cache_write_upgrades_legacy_and_keeps_both_keys() {
    // 旧格式写入国际版 → 升级为新格式，且国内版旧值迁到 cn 键
    let migrated = upsert_workbuddy_exe_cache_json(
        r#"{ "exe": "/Applications/WorkBuddy.app" }"#,
        WbVariant::Ai,
        Path::new("/Applications/WorkBuddy AI.app"),
    );
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(&migrated, WbVariant::Cn)
            .unwrap()
            .to_string_lossy(),
        "/Applications/WorkBuddy.app"
    );
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(&migrated, WbVariant::Ai)
            .unwrap()
            .to_string_lossy(),
        "/Applications/WorkBuddy AI.app"
    );
    assert!(!migrated.contains("\"exe\""), "写回新格式: {migrated}");

    // 再写国内版：两档位互不覆盖
    let both = upsert_workbuddy_exe_cache_json(
        &migrated,
        WbVariant::Cn,
        Path::new("/Applications/CodeBuddy.app"),
    );
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(&both, WbVariant::Cn)
            .unwrap()
            .to_string_lossy(),
        "/Applications/CodeBuddy.app"
    );
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(&both, WbVariant::Ai)
            .unwrap()
            .to_string_lossy(),
        "/Applications/WorkBuddy AI.app"
    );

    // 损坏内容不从零继承，直接重建
    let recovered = upsert_workbuddy_exe_cache_json("not-json", WbVariant::Ai, Path::new("/x/a"));
    assert_eq!(
        parse_workbuddy_exe_cache_json_for(&recovered, WbVariant::Ai)
            .unwrap()
            .to_string_lossy(),
        "/x/a"
    );
}

#[test]
fn parse_codebuddy_cn_app_cache_json_reads_exe() {
    let path = parse_codebuddy_cn_app_cache_json(r#"{ "exe": "/Applications/CodeBuddy CN.app" }"#)
        .expect("valid cache");
    assert_eq!(path.to_string_lossy(), "/Applications/CodeBuddy CN.app");
}

#[test]
fn parse_codebuddy_cn_app_cache_json_ignores_corrupt_and_empty() {
    assert!(parse_codebuddy_cn_app_cache_json("not-json").is_none());
    assert!(parse_codebuddy_cn_app_cache_json(r#"{ "exe": "  " }"#).is_none());
    assert!(parse_codebuddy_cn_app_cache_json("{}").is_none());
}

#[test]
fn codebuddy_cn_app_cache_file_is_not_workbuddy_exe_cache() {
    assert_ne!(codebuddy_cn_app_cache_file(), workbuddy_exe_cache_file());
    assert!(codebuddy_cn_app_cache_file()
        .file_name()
        .is_some_and(|n| n == "codebuddy_cn_app.json"));
    assert_ne!(
        codebuddy_ide_app_cache_file(),
        codebuddy_cn_app_cache_file()
    );
    assert!(codebuddy_ide_app_cache_file()
        .file_name()
        .is_some_and(|n| n == "codebuddy_ide_app.json"));
}

#[test]
fn default_http_user_agent_matches_official_chrome_desktop() {
    assert_eq!(
        DEFAULT_HTTP_USER_AGENT,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36"
    );
    let _ = http_client_builder();
}

#[test]
fn route_missing_is_404_only() {
    assert!(is_route_missing(
        &json!({"code": 404, "message": "not found"})
    ));
    assert!(is_route_missing(&json!({"code": "404"})));
    assert!(is_route_missing(&json!({"data": {"code": 404}})));
    // 非 404 一律不得当作路径问题回落。
    assert!(!is_route_missing(
        &json!({"code": 401, "message": "unauthorized"})
    ));
    assert!(!is_route_missing(&json!({"code": 403})));
    assert!(!is_route_missing(
        &json!({"code": 10085, "msg": "请求不合法"})
    ));
    assert!(!is_route_missing(
        &json!({"code": -1, "message": "error sending request"})
    ));
    assert!(!is_route_missing(&json!({"code": 0, "data": {}})));
    assert!(!is_route_missing(&Value::Null));
}

/// 限额监听配置：默认开启、显式 false 生效、损坏/缺字段回默认，且只写已知字段。
#[test]
fn rate_limit_config_defaults_to_enabled_and_keeps_only_known_fields() {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-rate-limit-config-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rate_limit_config.json");

    // 文件缺失 → 默认开启、未卸载过、IDE 日志扫描开启。
    let defaults = load_rate_limit_config_at(&path);
    assert_eq!(defaults.get("enabled").and_then(Value::as_bool), Some(true));
    assert_eq!(
        defaults.get("hookOptOut").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        defaults.get("scanIdeLogs").and_then(Value::as_bool),
        Some(true)
    );

    // 显式关闭 → 生效。
    save_rate_limit_config_at(
        &path,
        &json!({"enabled": false, "scanIdeLogs": false, "extra": 1}),
    )
    .unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.get("enabled").and_then(Value::as_bool), Some(false));
    assert_eq!(
        saved.get("scanIdeLogs").and_then(Value::as_bool),
        Some(false),
        "scanIdeLogs 显式 false 必须落盘"
    );
    assert!(saved.get("extra").is_none(), "只保留已知字段: {saved}");
    assert_eq!(
        saved.as_object().unwrap().len(),
        4,
        "只有 enabled + hookOptOut + scanIdeLogs + crossClientInfer"
    );
    assert_eq!(
        load_rate_limit_config_at(&path)
            .get("scanIdeLogs")
            .and_then(Value::as_bool),
        Some(false),
        "读回仍是显式 false（不被默认值冲掉）"
    );

    // 显式 true 与显式 false 都如实往返（默认值不覆盖显式值）。
    save_rate_limit_config_at(&path, &json!({"scanIdeLogs": true})).unwrap();
    let round_trip = load_rate_limit_config_at(&path);
    assert_eq!(
        round_trip.get("scanIdeLogs").and_then(Value::as_bool),
        Some(true)
    );

    // 损坏内容 / 类型不符 → 回默认，不报错。
    std::fs::write(&path, "not-json").unwrap();
    assert_eq!(
        load_rate_limit_config_at(&path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        load_rate_limit_config_at(&path)
            .get("scanIdeLogs")
            .and_then(Value::as_bool),
        Some(true)
    );
    std::fs::write(&path, json!({"enabled": "no"}).to_string()).unwrap();
    assert_eq!(
        load_rate_limit_config_at(&path)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(rate_limit_config_file().ends_with("rate_limit_config.json"));
    std::fs::remove_dir_all(&dir).ok();
}

/// `hookOptOut` 与 `enabled` 并列：只保留已知字段，且单字段改写不动另一个开关。
#[test]
fn rate_limit_hook_opt_out_survives_known_field_merge() {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-rate-limit-opt-out-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rate_limit_config.json");

    // 只保留已知字段：多余键不落盘。
    save_rate_limit_config_at(
        &path,
        &json!({"enabled": false, "hookOptOut": true, "scanIdeLogs": false, "unknown": "x"}),
    )
    .unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.get("hookOptOut").and_then(Value::as_bool), Some(true));
    assert_eq!(saved.get("enabled").and_then(Value::as_bool), Some(false));
    assert!(saved.get("unknown").is_none(), "只保留已知字段: {saved}");

    // 单字段改写：置 true / 置 false 都不动 `enabled` 与 `scanIdeLogs`。
    let after_opt_out = set_rate_limit_hook_opt_out_at(&path, true).unwrap();
    assert_eq!(
        after_opt_out.get("hookOptOut").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        after_opt_out.get("enabled").and_then(Value::as_bool),
        Some(false),
        "改写 hookOptOut 不得重置限额监听开关"
    );
    assert_eq!(
        after_opt_out.get("scanIdeLogs").and_then(Value::as_bool),
        Some(false),
        "改写 hookOptOut 不得重置 IDE 日志扫描开关"
    );
    let cleared = set_rate_limit_hook_opt_out_at(&path, false).unwrap();
    assert_eq!(
        cleared.get("hookOptOut").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(cleared.get("enabled").and_then(Value::as_bool), Some(false));
    assert_eq!(
        cleared.get("scanIdeLogs").and_then(Value::as_bool),
        Some(false)
    );
    // 配置缺失时也能写入（首次卸载 / 首次接入）。
    let fresh = dir.join("fresh.json");
    assert_eq!(
        set_rate_limit_hook_opt_out_at(&fresh, true)
            .unwrap()
            .get("hookOptOut")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        load_rate_limit_config_at(&fresh)
            .get("enabled")
            .and_then(Value::as_bool),
        Some(true)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 轮换推迟提示预算：同日第 1..5 次放行、第 6 次拒绝；跨日重置；损坏回退 0。
#[test]
fn rotate_notify_budget_caps_per_local_day_and_resets_on_a_new_day() {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-rotate-notify-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(ROTATE_NOTIFY_FILE_NAME);
    let today = "2026-09-18";

    // 文件不存在 → 从 0 开始，前 5 次都放行。
    for expected_count in 1..=ROTATE_NOTIFY_DAILY_LIMIT {
        assert!(
            try_consume_rotate_notify_at(&path, today),
            "第 {expected_count} 次应放行"
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["count"], json!(expected_count));
        assert_eq!(saved["date"], json!(today));
    }
    // 第 6 次：拒绝，且预算文件不再被改写（仍停在 5）。
    assert!(!try_consume_rotate_notify_at(&path, today));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["count"], json!(ROTATE_NOTIFY_DAILY_LIMIT));

    // 跨日：日期变化即清零，重新放行。
    assert!(try_consume_rotate_notify_at(&path, "2026-09-19"));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["date"], json!("2026-09-19"));
    assert_eq!(saved["count"], json!(1));

    // 损坏 / 字段缺失 / 类型不符 → 按 0 计（不阻塞轮换）。
    for broken in [
        "not-json",
        "{}",
        r#"{"date":"2026-09-20","count":"many"}"#,
        "[]",
    ] {
        std::fs::write(&path, broken).unwrap();
        assert!(
            try_consume_rotate_notify_at(&path, today),
            "损坏内容应按 0 计: {broken}"
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["count"], json!(1), "损坏后从 1 重新起算: {broken}");
        assert_eq!(saved["date"], json!(today));
    }

    // 空日期视为不可用（不写坏文件）。
    std::fs::remove_file(&path).unwrap();
    assert!(!try_consume_rotate_notify_at(&path, ""));
    assert!(!path.exists());
    assert!(auto_rotate_notify_file().ends_with(ROTATE_NOTIFY_FILE_NAME));
    std::fs::remove_dir_all(&dir).ok();
}

// -----------------------------------------------------------------------
// 沙箱判定（WB_SHIFT_HOME 重定向 → 界面角标）
// -----------------------------------------------------------------------

/// 「真实家目录」取一个**确实存在**的目录：`canonicalize` 生效才能验证
/// 大小写 / 尾斜杠归一，也不依赖跑测试的机器上家目录长什么样。
fn sandbox_real_home() -> PathBuf {
    std::env::temp_dir()
}

fn sandbox_as_str(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[test]
fn sandbox_flag_is_false_without_env() {
    assert!(!is_sandboxed_home(None, &sandbox_real_home()));
}

#[test]
fn sandbox_flag_is_false_for_blank_env() {
    assert!(!is_sandboxed_home(Some(""), &sandbox_real_home()));
    assert!(!is_sandboxed_home(Some("   "), &sandbox_real_home()));
}

#[test]
fn sandbox_flag_is_false_when_env_equals_real_home() {
    let home = sandbox_real_home();
    assert!(!is_sandboxed_home(Some(&sandbox_as_str(&home)), &home));
}

/// 尾分隔符差异仍是同一个目录：`D:\wb-shift-dev\` == `D:\wb-shift-dev`。
#[test]
fn sandbox_flag_is_false_with_trailing_separator() {
    let home = sandbox_real_home();
    let with_sep = format!("{}{}", sandbox_as_str(&home), std::path::MAIN_SEPARATOR);
    assert!(!is_sandboxed_home(Some(&with_sep), &home));
}

/// 大小写差异只在大小写不敏感的机器上有意义（Windows / 默认 macOS 卷）。
#[cfg(windows)]
#[test]
fn sandbox_flag_ignores_case_difference() {
    let home = sandbox_real_home();
    let upper = sandbox_as_str(&home).to_uppercase();
    assert!(!is_sandboxed_home(Some(&upper), &home));
}

/// 家目录下的子目录同样算「重定向」：沙箱常常就落在家目录里。
#[test]
fn sandbox_flag_is_true_for_real_home_subdirectory() {
    let home = sandbox_real_home();
    let sandbox = home.join("wb-shift-dev-sandbox-probe");
    assert!(is_sandboxed_home(Some(&sandbox_as_str(&sandbox)), &home));
}

#[test]
fn sandbox_flag_is_true_for_unrelated_path() {
    let home = sandbox_real_home();
    assert!(is_sandboxed_home(Some(r"D:\wb-shift-dev"), &home));
}

// -----------------------------------------------------------------------
// 开发构建默认沙箱家目录（`tauri dev` 兜底，见 dev_home_dir_at）
// -----------------------------------------------------------------------

/// 没有 `WB_DEV_SANDBOX` → 真实家目录下的隐藏目录。
#[test]
fn dev_home_dir_at_defaults_to_hidden_dir_under_real_home() {
    let home = sandbox_real_home();
    assert_eq!(
        dev_home_dir_at(None, &home),
        home.join(DEV_SANDBOX_DIR_NAME)
    );
}

/// 空串环境变量等同于没设：不能把沙箱解析成空路径 / 当前工作目录。
#[test]
fn dev_home_dir_at_treats_empty_candidate_as_unset() {
    let home = sandbox_real_home();
    assert_eq!(
        dev_home_dir_at(Some(""), &home),
        home.join(DEV_SANDBOX_DIR_NAME)
    );
}

/// 全空白同理（`set WB_DEV_SANDBOX=   ` 这种手滑写法）。
#[test]
fn dev_home_dir_at_treats_blank_candidate_as_unset() {
    let home = sandbox_real_home();
    assert_eq!(
        dev_home_dir_at(Some("  "), &home),
        home.join(DEV_SANDBOX_DIR_NAME)
    );
    assert_eq!(
        dev_home_dir_at(Some("\t\n "), &home),
        home.join(DEV_SANDBOX_DIR_NAME)
    );
}

/// 绝对路径原样生效（启动器的 D 盘沙箱必须不被改写）。
///
/// 盘符路径只有 Windows 认作绝对路径，其它平台用各自的前缀验证同一规则。
#[test]
fn dev_home_dir_at_keeps_absolute_candidate() {
    let home = sandbox_real_home();
    #[cfg(windows)]
    {
        // 正斜杠与反斜杠两种写法都要落在同一个盘符目录上。
        for raw in [r"D:\wb-shift-dev", "D:/wb-shift-dev"] {
            let resolved = dev_home_dir_at(Some(raw), &home);
            assert!(resolved.is_absolute(), "必须是绝对路径: {raw}");
            assert_eq!(
                resolved,
                Path::new("D:/wb-shift-dev"),
                "绝对路径不得再挂到家目录下: {raw}"
            );
        }
    }
    #[cfg(not(windows))]
    {
        let resolved = dev_home_dir_at(Some("/d/wb-shift-dev"), &home);
        assert!(resolved.is_absolute());
        assert_eq!(resolved, Path::new("/d/wb-shift-dev"));
    }
}

/// 相对路径基于真实家目录解析，结果一定是绝对路径（工作目录不可信）。
#[test]
fn dev_home_dir_at_resolves_relative_candidate_against_real_home() {
    let home = sandbox_real_home();
    let resolved = dev_home_dir_at(Some("relative/dir"), &home);
    assert_eq!(resolved, home.join("relative/dir"));
    assert!(resolved.is_absolute());
    assert!(resolved.starts_with(&home));
}
