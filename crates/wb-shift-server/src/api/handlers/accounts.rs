//! accounts 域的 HTTP handler。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::*;

pub(crate) async fn api_status(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    let auth = auth_file::read_auth_file(variant);
    let current = auth.as_ref().map(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        json!({
            "uid": account::display_value(&acct, "uid"),
            "nickname": account::display_value(&acct, "nickname"),
            "email": account::display_value(&acct, "email"),
        })
    });
    json_ok(json!({
        "running": cached_workbuddy_running(variant),
        "authFile": variant.auth_file_path().to_string_lossy(),
        "current": current,
        "appPath": auth_file::workbuddy_app_path(variant).to_string_lossy(),
        "version": update::APP_VERSION,
        "variant": variant.as_str(),
    }))
}

/// GET /api/accounts —— 返回全部档位的账号，`current` 取请求档位的登录态。
pub(crate) async fn api_accounts(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    json_ok(json!({
        "accounts": account::load_accounts()
            .iter()
            .map(account::account_meta)
            .collect::<Vec<_>>(),
        "current": auth_file::read_auth_file(variant)
            .and_then(|a| a.get("account").and_then(|x| x.get("uid")).and_then(|x| x.as_str()).map(String::from)),
        "variant": variant.as_str(),
    }))
}

pub(crate) async fn api_delete(
    body: Result<Json<DeleteAccountRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match account::delete_account(&request.account_id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/update-account-display —— 更新账号本地展示字段（备注 / 显示选择）。
///
/// body：`{ accountId, patch }`，patch 可含 `note`（字符串或 null）与 `displayField`。
pub(crate) async fn api_update_account_display(
    body: Result<Json<UpdateAccountDisplayRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match account::update_account_display(&request.account_id, &request.patch.to_value()) {
        Ok(meta) => json_ok(json!({ "ok": true, "account": meta })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/import-local —— 导入本机当前账号（body 可选 `variant`，缺省国内版）。
///
/// body 允许缺失，保持改造前的调用方式可用。
pub(crate) async fn api_import_local(body: Option<Json<VariantRequest>>) -> Response {
    let variant = WbVariant::parse(
        body.as_ref()
            .and_then(|Json(request)| request.variant.as_deref()),
    );
    match account::import_local(variant) {
        Ok(acc) => json_ok(json!({ "ok": true, "account": acc })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

pub(crate) async fn api_export_accounts(
    body: Result<Json<ExportAccountsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::export_accounts(&request.account_ids) {
        Ok(records) => json_ok(json!({ "ok": true, "accounts": records })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_export_accounts_to_path(
    body: Result<Json<ExportAccountsToPathRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::export_accounts_to_path(&request.account_ids, &request.path) {
        Ok(path) => json_ok(json!({ "ok": true, "path": path })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_preview_import(
    body: Result<Json<ImportFileRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::preview_accounts(&request.file_text) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_import(
    body: Result<Json<ImportAccountsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::import_accounts(&request.file_text, &request.indexes) {
        Ok(result) => json_ok(json!({
            "ok": true,
            "imported": result.imported,
            "skipped": result.skipped,
            "overwritten": result.overwritten,
        })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// OAuth 登录
// ---------------------------------------------------------------------------

/// POST /api/oauth/start —— 发起扫码登录（body 可选 `variant`，缺省国内版）。
pub(crate) async fn api_oauth_start(body: Option<Json<VariantRequest>>) -> Response {
    let variant = WbVariant::parse(
        body.as_ref()
            .and_then(|Json(request)| request.variant.as_deref()),
    );
    match oauth::oauth_start(variant).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/oauth/status —— 轮询采集结果（档位取发起时记录，无需传参）。
pub(crate) async fn api_oauth_status(
    body: Result<Json<OAuthStatusRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    json_ok(oauth::oauth_poll(&request.login_id).await)
}

// ---------------------------------------------------------------------------
// 切换
// ---------------------------------------------------------------------------

/// POST /api/switch —— 切换账号（备份 → 关进程 → 恢复/复制/同步会话 → 写认证 → 重启）。
///
/// 运行标记由**后台任务**持有：请求 future 被取消（浏览器断开）不会清标记，
/// 后台切换仍在写账号，第二次切换必须继续被 409 拒绝。详见 `state::SwitchState`。
pub(crate) async fn api_switch(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SwitchRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if request.account_id.trim().is_empty() {
        return json_err("缺少 accountId".to_string(), StatusCode::BAD_REQUEST);
    }
    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(request.sync_selections.as_ref()) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };

    if !state.switch.try_begin() {
        return json_err("已有切换任务进行中".to_string(), StatusCode::CONFLICT);
    }
    state.switch.set_progress("开始切换账号…");

    let progress_state = Arc::clone(&state.switch);
    let progress: switch::ProgressFn = Box::new(move |msg| progress_state.set_progress(msg));
    let task_state = Arc::clone(&state.switch);
    let result = tokio::task::spawn_blocking(move || {
        // 守卫必须在任务内部：只有切换真正结束（或 panic 展开）才清运行标记。
        let _run_guard = task_state.run_guard();
        switch::switch_account(
            Some(&progress),
            &request.account_id,
            request.restart,
            request.share_sessions,
            &request.copy_session_ids,
            &sync_selections,
        )
    })
    .await;

    match result {
        Ok(Ok(v)) => json_ok(v),
        Ok(Err(e)) => json_err(e, StatusCode::BAD_REQUEST),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_switch_progress(State(state): State<Arc<AppState>>) -> Response {
    json_ok(json!({
        "running": state.switch.is_running(),
        "progress": state.switch.progress(),
    }))
}
