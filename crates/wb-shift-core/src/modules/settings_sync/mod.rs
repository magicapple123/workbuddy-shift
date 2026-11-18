//! WorkBuddy 设置跨账号同步（T5）。
//!
//! 只同步 `storage/user-<uid>-personal/{global,scoped}/preferences.json`（界面 / 通用设置）；
//! `expert.json`、`skill-picker.json`、全局 `~/.workbuddy/settings.json` 都不碰。
//!
//! 口径（AUDIT §3 T5）：
//! - 以切出的源账号为准**逐键覆盖**，目标独有键保留；
//! - denylist 跳过设备本地状态（扩展刷新游标、增长提示、加载提示已读）；
//! - `appearance.themeSelection` 复制后把 `uid` 改写为目标 uid、`revision` 抬到
//!   `max(now, 源 revision + 1)`，否则客户端会认为「不是我的主题」而回落；
//! - 只同步目标已存在的 scope 目录；
//! - 目标账号目录不存在（从未在本机登录）→ 跳过并说明，不创建目录；
//! - cn / intl 数据根隔离，跨档直接拒绝；
//! - 覆盖前把目标文件备份到 `backups/settings/<uid>/<timestamp>/`，备份失败即零写入。

mod apply;
mod merge;
#[cfg(test)]
mod tests;

pub use apply::{apply, sync_on_switch};
pub use merge::merge_preferences;

use crate::modules::account;
use crate::modules::config::{atomic_write, now_ms, store_dir};
use crate::modules::variant::WbVariant;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// 不同步的键：都是设备本地状态，跟着账号走会让新账号误以为「已看过 / 已刷新」。
pub const DENYLIST: [&str; 3] = [
    "extensionListRefresh",
    "officeWindowGrowthNudge",
    "loadingTipsDismissed",
];

/// 主题选择键（扁平键名，含点号；不是嵌套对象）。
pub const THEME_SELECTION_KEY: &str = "appearance.themeSelection";

const CONFIG_FILE: &str = "settings_sync_config.json";
const STATE_FILE: &str = "settings_sync_state.json";

/// 一次同步的上下文：账号库、工具存储根与两个客户端数据根（单测可注入）。
#[derive(Clone, Debug)]
pub struct SettingsSyncContext {
    pub accounts: Vec<Value>,
    pub store_root: PathBuf,
    pub cn_root: PathBuf,
    pub ai_root: PathBuf,
}

impl SettingsSyncContext {
    pub fn production() -> Self {
        Self {
            accounts: account::load_accounts(),
            store_root: store_dir(),
            cn_root: WbVariant::Cn.data_root(),
            ai_root: WbVariant::Ai.data_root(),
        }
    }

    fn data_root(&self, variant: WbVariant) -> &Path {
        match variant {
            WbVariant::Cn => &self.cn_root,
            WbVariant::Ai => &self.ai_root,
        }
    }

    fn account_dir(&self, account: &Value) -> PathBuf {
        let uid = account_uid(account);
        let variant = account::variant_of(account);
        self.data_root(variant)
            .join("storage")
            .join(format!("user-{uid}-personal"))
    }
}

fn account_uid(account: &Value) -> String {
    account
        .get("uid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn account_label(account: &Value) -> String {
    account::account_display_name(account)
}

/// 同步配置（开关）；文件不存在时用默认值，不写盘。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncConfig {
    pub enabled: bool,
    pub auto_on_switch: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        // 默认开启：切号后设置保持一致是产品默认行为；用户可在设置页关掉。
        Self {
            enabled: true,
            auto_on_switch: true,
        }
    }
}

pub fn load_config(store_root: &Path) -> SyncConfig {
    let path = store_root.join(CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(path) else {
        return SyncConfig::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return SyncConfig::default();
    };
    SyncConfig {
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        auto_on_switch: value
            .get("autoOnSwitch")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

pub fn save_config(store_root: &Path, config: SyncConfig) -> Result<SyncConfig, String> {
    let path = store_root.join(CONFIG_FILE);
    let payload = json!({
        "enabled": config.enabled,
        "autoOnSwitch": config.auto_on_switch,
        "updatedAt": now_ms(),
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?;
    atomic_write(&path, &text).map_err(|error| format!("保存同步配置失败：{error}"))?;
    Ok(config)
}

/// 状态：配置 + 每个账号的主题快照 + 最近一次同步结果。
pub fn status(ctx: &SettingsSyncContext) -> Value {
    let config = load_config(&ctx.store_root);
    let accounts: Vec<Value> = ctx
        .accounts
        .iter()
        .map(|account| {
            let uid = account_uid(account);
            let dir = ctx.account_dir(account);
            let prefs = read_prefs(&dir.join("global").join("preferences.json"));
            json!({
                "accountId": account.get("id").cloned().unwrap_or(Value::Null),
                "uid": uid,
                "label": account_label(account),
                "variant": account::variant_of(account).as_str(),
                "hasDirectory": dir.is_dir(),
                "directory": dir.to_string_lossy(),
                "theme": prefs.as_ref().and_then(|p| p.get("appearanceTheme")).cloned().unwrap_or(Value::Null),
                "themeResource": prefs.as_ref()
                    .and_then(|p| p.get(THEME_SELECTION_KEY))
                    .and_then(|selection| selection.get("resourceKey"))
                    .cloned()
                    .unwrap_or(Value::Null),
                "scopes": scope_names(&dir),
            })
        })
        .collect();
    json!({
        "config": { "enabled": config.enabled, "autoOnSwitch": config.auto_on_switch },
        "accounts": accounts,
        "lastResult": read_state(&ctx.store_root),
        "denylist": DENYLIST,
    })
}

fn scope_names(dir: &Path) -> Vec<String> {
    let scoped = dir.join("scoped");
    let Ok(entries) = std::fs::read_dir(&scoped) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

fn read_prefs(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn read_state(store_root: &Path) -> Value {
    std::fs::read_to_string(store_root.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

fn write_state(store_root: &Path, value: &Value) {
    if let Ok(text) = serde_json::to_string_pretty(value) {
        let _ = atomic_write(&store_root.join(STATE_FILE), &text);
    }
}

/// 目标账号：显式 source 为 `None` 时取当前登录账号；目标为同档位其余账号。
fn resolve_accounts<'a>(
    ctx: &'a SettingsSyncContext,
    source: Option<&str>,
) -> Result<(Value, Vec<&'a Value>, WbVariant), String> {
    let source_account = match source {
        Some(id) => ctx
            .accounts
            .iter()
            .find(|account| {
                account.get("id").and_then(Value::as_str) == Some(id) || account_uid(account) == id
            })
            .cloned()
            .ok_or_else(|| format!("源账号不存在：{id}"))?,
        None => {
            let variant = WbVariant::Cn;
            let uid = crate::modules::auth_file::read_auth_file(variant)
                .and_then(|auth| {
                    auth.get("account")
                        .and_then(|account| account.get("uid"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .ok_or_else(|| "未找到当前登录账号，请显式指定源账号".to_string())?;
            ctx.accounts
                .iter()
                .find(|account| account_uid(account) == uid)
                .cloned()
                .ok_or_else(|| format!("当前登录账号不在账号库中：{uid}"))?
        }
    };
    let variant = account::variant_of(&source_account);
    let source_uid = account_uid(&source_account);
    if source_uid.is_empty() {
        return Err("源账号缺少 uid".to_string());
    }
    let targets: Vec<&Value> = ctx
        .accounts
        .iter()
        .filter(|account| account_uid(account) != source_uid)
        .filter(|account| account::variant_of(account) == variant)
        .collect();
    Ok((source_account, targets, variant))
}

/// 预览：只读，返回每个目标账号会改哪些键、跳过哪些键。
pub fn preview(ctx: &SettingsSyncContext, source: Option<&str>) -> Result<Value, String> {
    let (source_account, targets, variant) = resolve_accounts(ctx, source)?;
    let source_uid = account_uid(&source_account);
    let source_dir = ctx.account_dir(&source_account);
    let source_global = read_prefs(&source_dir.join("global").join("preferences.json"))
        .ok_or_else(|| {
            format!("源账号（{source_uid}）没有 global/preferences.json，无法作为同步来源")
        })?;
    let now = now_ms();

    let mut plans = Vec::new();
    for target in targets {
        let target_uid = account_uid(target);
        let target_dir = ctx.account_dir(target);
        if !target_dir.is_dir() {
            plans.push(json!({
                "uid": target_uid,
                "label": account_label(target),
                "skipped": true,
                "reason": "目标账号从未在本机登录（目录不存在）",
                "changes": [],
            }));
            continue;
        }
        let target_global =
            read_prefs(&target_dir.join("global").join("preferences.json")).unwrap_or_default();
        let outcome = merge_preferences(&source_global, &target_global, &target_uid, now);
        let scopes = plan_scopes(&source_dir, &target_dir, &target_uid, now);
        plans.push(json!({
            "uid": target_uid,
            "label": account_label(target),
            "skipped": false,
            "changes": outcome.changed,
            "skippedKeys": outcome.skipped,
            "scopes": scopes,
        }));
    }

    Ok(json!({
        "variant": variant.as_str(),
        "source": { "uid": source_uid, "label": account_label(&source_account), "directory": source_dir.to_string_lossy() },
        "targets": plans,
        "dryRun": true,
    }))
}

fn plan_scopes(source_dir: &Path, target_dir: &Path, target_uid: &str, now: i64) -> Value {
    let target_scopes = scope_names(target_dir);
    let mut synced = Vec::new();
    let mut skipped = Vec::new();
    for scope in target_scopes {
        let source_file = source_dir
            .join("scoped")
            .join(&scope)
            .join("preferences.json");
        let target_file = target_dir
            .join("scoped")
            .join(&scope)
            .join("preferences.json");
        let Some(source_prefs) = read_prefs(&source_file) else {
            skipped.push(json!({ "scope": scope, "reason": "源账号没有该 scope" }));
            continue;
        };
        let target_prefs = read_prefs(&target_file).unwrap_or_default();
        let outcome = merge_preferences(&source_prefs, &target_prefs, target_uid, now);
        if outcome.changed.is_empty() {
            continue;
        }
        synced.push(json!({ "scope": scope, "changes": outcome.changed }));
    }
    json!({ "synced": synced, "skipped": skipped })
}

fn timestamp_id(ms: i64) -> String {
    let secs = ms / 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

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
