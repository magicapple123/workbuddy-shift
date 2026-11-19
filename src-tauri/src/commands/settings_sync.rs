//! WorkBuddy 设置跨账号同步：状态 / 配置 / 预览 / 执行 / 备份列举 / 备份还原。
//!
//! 只做参数解析；合并口径、备份与零写入保证在
//! `wb_shift_core::modules::settings_sync`。

use serde_json::Value;
use wb_shift_core::modules::settings_sync::{self, SettingsSyncContext, SyncConfig};

/// GET /api/settings-sync —— 配置 + 各账号主题快照 + 最近一次结果。
#[tauri::command]
pub async fn get_settings_sync_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        settings_sync::status(&SettingsSyncContext::production())
    })
    .await
    .map_err(|error| error.to_string())
}

/// POST /api/settings-sync/config —— 保存开关（缺省字段保持原值）。
#[tauri::command(rename_all = "camelCase")]
pub async fn save_settings_sync_config(
    enabled: Option<bool>,
    auto_on_switch: Option<bool>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = wb_shift_core::modules::config::store_dir();
        let current = settings_sync::load_config(&store);
        let next = SyncConfig {
            enabled: enabled.unwrap_or(current.enabled),
            auto_on_switch: auto_on_switch.unwrap_or(current.auto_on_switch),
        };
        let saved = settings_sync::save_config(&store, next)?;
        serde_json::to_value(serde_json::json!({
            "enabled": saved.enabled,
            "autoOnSwitch": saved.auto_on_switch,
        }))
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/settings-sync/preview —— 只读预览（源 → 同档位其余账号）。
#[tauri::command(rename_all = "camelCase")]
pub async fn preview_settings_sync(source_account_id: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        settings_sync::preview(
            &SettingsSyncContext::production(),
            source_account_id.as_deref(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/settings-sync/apply —— 执行同步（`dryRun` 缺省 true）。
#[tauri::command(rename_all = "camelCase")]
pub async fn sync_settings(
    source_account_id: Option<String>,
    dry_run: Option<bool>,
) -> Result<Value, String> {
    let dry_run = dry_run.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        settings_sync::apply(
            &SettingsSyncContext::production(),
            source_account_id.as_deref(),
            dry_run,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// GET /api/settings-sync/backups —— 列举某账号的设置备份（时间戳倒序）。
#[tauri::command(rename_all = "camelCase")]
pub async fn list_settings_sync_backups(uid: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        settings_sync::list_backups(&SettingsSyncContext::production(), &uid)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/settings-sync/restore —— 还原某次备份（还原前自动再备份当前文件）。
#[tauri::command(rename_all = "camelCase")]
pub async fn restore_settings_sync_backup(uid: String, stamp: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        settings_sync::restore_backup(&SettingsSyncContext::production(), &uid, &stamp)
    })
    .await
    .map_err(|error| error.to_string())?
}
