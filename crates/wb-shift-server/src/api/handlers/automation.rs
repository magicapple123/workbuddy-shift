//! automation（CodeBuddy CLI 自动轮换）域的 HTTP handler。

use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::*;

pub(crate) async fn api_rotate_config() -> Response {
    json_ok(config::load_auto_rotate_config())
}

pub(crate) async fn api_save_rotate_config(Json(body): Json<Value>) -> Response {
    // 桌面端 Tauri 按参数名注入 config；WebUI 的 call() 会包成 {config}。
    let submitted = submitted_config(&body);
    match config::save_auto_rotate_config(submitted) {
        Ok(()) => json_ok(config::load_auto_rotate_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_rotate_status() -> Response {
    json_ok(rotate::rotate_status())
}

pub(crate) async fn api_rotate_run() -> Response {
    json_ok(rotate::run_rotate_cycle().await)
}

pub(crate) async fn api_rotate_logs() -> Response {
    json_ok(json!({ "logs": rotate::rotate_logs() }))
}
