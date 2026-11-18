//! storage 域的 HTTP handler。
//!
//! 与 `src-tauri/src/commands/storage.rs` 一一对应；破坏性接口默认 dry-run，
//! 业务规则在 `wb_shift_core::modules::storage`。

use axum::extract::rejection::JsonRejection;
use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use wb_shift_core::modules::storage::{self, CleanContext, StorageRoots};
use wb_shift_core::modules::variant::WbVariant;

/// GET /api/storage/scan —— 总览与分类明细（60 秒缓存）。
pub(crate) async fn api_storage_scan(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    let result = tokio::task::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::scan(&roots, variant)
    })
    .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/sessions —— 全部会话资产包（含逐子项）。
pub(crate) async fn api_storage_sessions(
    payload: Result<Json<StorageSessionsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let variant = WbVariant::parse(request.variant.as_deref());
    let result = tokio::task::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::sessions_payload(&roots, variant, &variant.data_root())
    })
    .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/clean —— 清理（`dryRun` 缺省 true）。
pub(crate) async fn api_storage_clean(
    payload: Result<Json<StorageCleanRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let variant = WbVariant::parse(request.variant.as_deref());
    let dry_run = request.dry_run.unwrap_or(true);
    let targets = request.targets;
    let result = tokio::task::spawn_blocking(move || {
        let ctx = CleanContext::for_variant(StorageRoots::resolve(), variant);
        storage::clean(&ctx, dry_run, &targets)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// GET /api/storage/trash —— 回收站条目。
pub(crate) async fn api_storage_trash() -> Response {
    let result =
        tokio::task::spawn_blocking(|| storage::trash_payload(&StorageRoots::resolve())).await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/trash/restore —— 恢复一个回收站条目（文件 + DB 行）。
pub(crate) async fn api_storage_trash_restore(
    payload: Result<Json<StorageTrashRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        storage::restore_entry(&StorageRoots::resolve(), &request.id)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/trash/purge —— 彻底删除（`id` 为空表示清空）。
pub(crate) async fn api_storage_trash_purge(
    payload: Result<Json<StorageTrashPurgeRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        let id = request.id.filter(|id| !id.trim().is_empty());
        storage::purge(&StorageRoots::resolve(), id.as_deref())
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/reveal —— 在系统文件管理器中定位。
pub(crate) async fn api_storage_reveal(
    payload: Result<Json<StorageRevealRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let mode = request.mode.unwrap_or_else(|| "select".to_string());
    let result = tokio::task::spawn_blocking(move || {
        storage::reveal(&StorageRoots::resolve(), &request.path, &mode)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/storage/revalidate —— 外部删除后重校验（只接受 `kind:id`）。
pub(crate) async fn api_storage_revalidate(
    payload: Result<Json<StorageRevalidateRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let variant = WbVariant::parse(request.variant.as_deref());
    let targets = request.targets;
    let result = tokio::task::spawn_blocking(move || {
        let roots = StorageRoots::resolve();
        storage::revalidate(&roots, variant, &variant.data_root(), &targets)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}
