//! notifications 域的 HTTP handler。

use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::*;

/// GET /api/notifications —— 最近的应用内提示（新的在前，最多 100 条）。
pub(crate) async fn api_notifications() -> Response {
    match notifications::list() {
        Ok(items) => json_ok(json!({ "items": items })),
        Err(error) => json_err(error, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/notifications/record —— 记录一条提示（前端 toast 同步写一份）。
pub(crate) async fn api_record_notification(Json(body): Json<Value>) -> Response {
    let level = body.get("level").and_then(|v| v.as_str()).unwrap_or("info");
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let description = body.get("description").and_then(|v| v.as_str());
    match notifications::record(level, title, description) {
        Ok(()) => json_ok(json!({ "recorded": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/notifications/clear —— 清空通知存档。
pub(crate) async fn api_clear_notifications() -> Response {
    match notifications::clear() {
        Ok(()) => json_ok(json!({ "cleared": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}
