//! credential（R6-06 按需解密开关）域的 HTTP handler。
//!
//! 与桌面端 `commands::get_credential_config` / `save_credential_config` 同源：两边都只
//! 拆参数、调 core 的同一个函数，不存在「WebUI 与桌面行为不同」的可能。
//!
//! 回包里**只有**开关与探测结论：没有令牌、没有密钥、没有认证文件路径。

use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::Value;
use wb_shift_core::modules::config;

/// `GET /api/credential-config` —— 只读 / 写开关 + 探测结论 + 出厂默认值。
pub(crate) async fn api_credential_config() -> Response {
    json_ok(config::load_credential_config_with_defaults())
}

/// `POST /api/credential-config` —— 只保存三个开关位，返回回读值。
///
/// 桌面端 Tauri 按参数名注入 config；WebUI 的 `call()` 会包成 `{config}`。
/// 平台探测结论不接受外部提交（core 侧白名单过滤），所以这里不需要额外校验。
pub(crate) async fn api_save_credential_config(Json(body): Json<Value>) -> Response {
    let submitted = submitted_config(&body);
    match config::save_credential_switches(submitted) {
        Ok(saved) => json_ok(saved),
        Err(error) => json_err(error.to_string(), StatusCode::BAD_REQUEST),
    }
}
