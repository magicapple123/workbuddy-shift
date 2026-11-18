//! 存储管理：扫描 / 会话资产包 / 清理 / 回收站 / 定位 / 重校验。
//!
//! 只做参数解析与错误字符串化；业务规则（白名单、回收站、备份、行快照）在
//! `wb_shift_core::modules::storage`。破坏性接口默认 `dryRun: true`。

use serde_json::Value;
use wb_shift_core::modules::storage::{self, CleanContext, StorageRoots};
use wb_shift_core::modules::variant::WbVariant;

/// GET /api/storage/scan —— 总览与分类明细（60 秒缓存）。
#[tauri::command]
pub async fn storage_scan(variant: Option<String>) -> Result<Value, String> {
    let variant = WbVariant::parse(variant.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::scan(&roots, variant)
    })
    .await
    .map_err(|error| error.to_string())
}

/// POST /api/storage/sessions —— 全部会话资产包（含逐子项）。
#[tauri::command]
pub async fn storage_sessions(variant: Option<String>) -> Result<Value, String> {
    let variant = WbVariant::parse(variant.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::sessions_payload(&roots, variant, &variant.data_root())
    })
    .await
    .map_err(|error| error.to_string())
}

/// POST /api/storage/clean —— 清理（`dryRun` 缺省 true）。
#[tauri::command(rename_all = "camelCase")]
pub async fn storage_clean(
    variant: Option<String>,
    dry_run: Option<bool>,
    targets: Value,
) -> Result<Value, String> {
    let variant = WbVariant::parse(variant.as_deref());
    let dry_run = dry_run.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = CleanContext::for_variant(StorageRoots::resolve(), variant);
        storage::clean(&ctx, dry_run, &targets)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// GET /api/storage/trash —— 回收站条目。
#[tauri::command]
pub async fn storage_trash() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || storage::trash_payload(&StorageRoots::resolve()))
        .await
        .map_err(|error| error.to_string())
}

/// POST /api/storage/trash/restore —— 恢复一个回收站条目（文件 + DB 行）。
#[tauri::command]
pub async fn storage_trash_restore(id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        storage::restore_entry(&StorageRoots::resolve(), &id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/storage/trash/purge —— 彻底删除（`id` 为空表示清空）。
#[tauri::command]
pub async fn storage_trash_purge(id: Option<String>) -> Result<Value, String> {
    let id = id.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let id = (!id.trim().is_empty()).then_some(id.as_str());
        storage::purge(&StorageRoots::resolve(), id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/storage/reveal —— 在系统文件管理器中定位。
#[tauri::command]
pub async fn storage_reveal(path: String, mode: Option<String>) -> Result<Value, String> {
    let mode = mode.unwrap_or_else(|| "select".to_string());
    tauri::async_runtime::spawn_blocking(move || {
        storage::reveal(&StorageRoots::resolve(), &path, &mode)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/storage/revalidate —— 外部删除后重校验（只接受 `kind:id`）。
#[tauri::command(rename_all = "camelCase")]
pub async fn storage_revalidate(variant: Option<String>, targets: Value) -> Result<Value, String> {
    let variant = WbVariant::parse(variant.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::revalidate(&roots, variant, &variant.data_root(), &targets)
    })
    .await
    .map_err(|error| error.to_string())?
}
