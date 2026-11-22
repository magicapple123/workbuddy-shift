//! stats 域的 HTTP handler。

use axum::extract::rejection::JsonRejection;
use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::Value;
use wb_shift_core::modules::*;

pub(crate) async fn api_credits(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(credits::get_credit_expiry(&acc).await)
}

pub(crate) async fn api_credit_statistics(RawQuery(query): RawQuery) -> Response {
    let raw = query.as_deref();
    let refresh = query_flag_enabled(raw, "refresh");
    let range = query_param(raw, "range");
    json_ok(credit_usage::get_statistics(refresh, range.as_deref()).await)
}

/// POST /api/refresh-official-usage —— 只采集官方用量（单飞 + 内存/磁盘缓存）。
pub(crate) async fn api_refresh_official_usage(
    payload: Result<Json<RefreshOfficialUsageRequest>, JsonRejection>,
) -> Response {
    if let Err(rejection) = parse_body(payload) {
        return invalid_body(rejection);
    }
    let accounts = account::load_accounts();
    let value = wb_shift_core::modules::official_usage::refresh_official_usage(&accounts).await;
    json_ok(value)
}

/// POST /api/save-text-file —— 把 CSV 文本写到用户选定的路径（只允许 .csv、≤16 MiB）。
pub(crate) async fn api_save_text_file(
    payload: Result<Json<SaveTextFileRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(payload) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let result = tokio::task::spawn_blocking(move || {
        wb_shift_core::modules::export_file::save_text_file(&request.path, &request.content)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_token_statistics(RawQuery(query): RawQuery) -> Response {
    let days = query.as_deref().and_then(|value| {
        value
            .split('&')
            .find_map(|part| part.strip_prefix("days=")?.parse::<i64>().ok())
    });
    match tokio::task::spawn_blocking(move || token_stats::get_statistics(days)).await {
        Ok(statistics) => json_ok(statistics),
        Err(error) => json_err(
            format!("扫描 Token 统计失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// POST /api/refresh-token —— 刷新单账号 token。
///
/// 只返回 `refresh::refresh_result_meta`（= `account_meta` + 跳过标记，与桌面端
/// `refresh_account_token` 命令逐字段同形）：core 的刷新函数返回的是完整账号记录
/// （含 access_token / refresh_token），直接回给浏览器会越过「账号库元数据不含 token」
/// 的既有边界，两个宿主也会因此出现响应形态不一致。
///
/// issue #202：抢不到凭据写入锁时本次没有刷新，`refresh_result_meta` 会补
/// `refreshSkipped` / `refreshSkippedReason` 两个字段，前端据此提示「稍后重试」，
/// 而不是把未刷新的旧 meta 当成成功。
pub(crate) async fn api_refresh_token(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let fresh = refresh::refresh_account_token(acc).await;
    json_ok(refresh::refresh_result_meta(&fresh))
}

// ---------------------------------------------------------------------------
// 自动轮换（CodeBuddy CLI）
// ---------------------------------------------------------------------------
