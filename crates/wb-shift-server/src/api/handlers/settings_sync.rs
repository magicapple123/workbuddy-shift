//! settings-sync 域的 HTTP handler：状态 / 配置 / 预览 / 执行 / 备份列举 / 备份还原。

use axum::extract::rejection::JsonRejection;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use std::collections::HashMap;

use crate::api::*;
use wb_shift_core::modules::settings_sync::{self, SettingsSyncContext, SyncConfig};

/// GET /api/settings-sync —— 配置 + 各账号主题快照 + 最近一次结果。
pub(crate) async fn api_settings_sync_status() -> Response {
    let result =
        tokio::task::spawn_blocking(|| settings_sync::status(&SettingsSyncContext::production()))
            .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/settings-sync/config —— 保存开关（缺省字段保持原值）。
pub(crate) async fn api_save_settings_sync_config(
    payload: Result<Json<SettingsSyncConfigRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        let store = wb_shift_core::modules::config::store_dir();
        let current = settings_sync::load_config(&store);
        let next = SyncConfig {
            enabled: request.enabled.unwrap_or(current.enabled),
            auto_on_switch: request.auto_on_switch.unwrap_or(current.auto_on_switch),
        };
        let saved = settings_sync::save_config(&store, next)?;
        Ok::<_, String>(serde_json::json!({
            "enabled": saved.enabled,
            "autoOnSwitch": saved.auto_on_switch,
        }))
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/settings-sync/preview —— 只读预览（源 → 同档位其余账号）。
pub(crate) async fn api_preview_settings_sync(
    payload: Result<Json<SettingsSyncPreviewRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        settings_sync::preview(
            &SettingsSyncContext::production(),
            request.source_account_id.as_deref(),
        )
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/settings-sync/apply —— 执行同步（`dryRun` 缺省 true）。
pub(crate) async fn api_sync_settings(
    payload: Result<Json<SettingsSyncApplyRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let dry_run = request.dry_run.unwrap_or(true);
    let result = tokio::task::spawn_blocking(move || {
        settings_sync::apply(
            &SettingsSyncContext::production(),
            request.source_account_id.as_deref(),
            dry_run,
        )
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// GET /api/settings-sync/backups?uid=... —— 列举某账号的设置备份（时间戳倒序）。
pub(crate) async fn api_list_settings_sync_backups(
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(uid) = query.get("uid").cloned() else {
        return json_err("缺少 uid".to_string(), StatusCode::BAD_REQUEST);
    };
    let result = tokio::task::spawn_blocking(move || {
        settings_sync::list_backups(&SettingsSyncContext::production(), &uid)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/settings-sync/restore —— 还原某次备份（还原前自动再备份当前文件）。
pub(crate) async fn api_restore_settings_sync_backup(
    payload: Result<Json<SettingsSyncRestoreRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        settings_sync::restore_backup(
            &SettingsSyncContext::production(),
            &request.uid,
            &request.stamp,
        )
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}
