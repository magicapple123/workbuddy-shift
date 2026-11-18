//! update 域的 HTTP handler。

use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::Value;
use wb_shift_core::modules::*;

pub(crate) async fn api_update_check(RawQuery(query): RawQuery) -> Response {
    // 与桌面端 check_update 参数同义：force 绕过缓存、proxy 为可选代理。
    let proxy = query_param(query.as_deref(), "proxy").filter(|value| !value.is_empty());
    let force = query_flag_enabled(query.as_deref(), "force");
    json_ok(update::update_check(proxy.as_deref(), force).await)
}

pub(crate) async fn api_update_config() -> Response {
    json_ok(update::load_github_config())
}

pub(crate) async fn api_save_update_config(Json(body): Json<Value>) -> Response {
    // 与桌面端 save_github_config 同形：成功时直接返回保存后的配置。
    let submitted = submitted_config(&body);
    match update::save_github_config(submitted) {
        Ok(()) => json_ok(update::load_github_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}
