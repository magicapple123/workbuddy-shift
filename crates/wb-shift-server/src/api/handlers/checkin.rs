//! checkin 域的 HTTP handler。

use axum::extract::{Query, RawQuery};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::*;

/// GET /api/checkin/status —— 传 accountId 时只查询该账号；缺省保留旧批量响应。
/// 两种形式都遵守单账号自动签到开关，避免展示状态时触发已关闭账号的请求。
pub(crate) async fn api_checkin_status(Query(query): Query<HashMap<String, String>>) -> Response {
    if let Some(id) = query.get("accountId") {
        let Some(acc) = account::find_account(id) else {
            return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
        };
        let status = checkin::get_checkin_status_for_display(&acc).await;
        return json_ok(checkin_status_item(&acc, status));
    }
    let list = account::load_accounts();
    let mut items = Vec::new();
    for acc in &list {
        let status = checkin::get_checkin_status_for_display(acc).await;
        items.push(checkin_status_item(acc, status));
    }
    json_ok(json!({ "accounts": items }))
}

pub(crate) fn checkin_status_item(account: &Value, mut status: Value) -> Value {
    status["accountId"] = account.get("id").cloned().unwrap_or(Value::Null);
    status["email"] = json!(account::account_display_name(account));
    status["variant"] = json!(account::variant_of(account).as_str());
    status
}

pub(crate) async fn api_checkin(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(checkin::checkin_account(&acc).await)
}

pub(crate) async fn api_checkin_all(body: Option<Json<Value>>) -> Response {
    // 缺省（无 body / 无 variant）= 全部档位，保持与桌面端 set 前的行为一致。
    let variant = body
        .as_ref()
        .and_then(|Json(value)| value.get("variant"))
        .and_then(|value| value.as_str())
        .map(|raw| WbVariant::parse(Some(raw)));
    // `respectWindow`（camelCase，与 `accountId` 先例一致）只由账号页「刷新并签到」
    // 传 true：窗口生效且当前不在时间段内时整轮跳过；缺省 false 保持设置页 /
    // 托盘 / 旧客户端的立即签到语义。
    let respect_window = body
        .as_ref()
        .and_then(|Json(value)| value.get("respectWindow"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    json_ok(checkin::run_checkin_all(variant, respect_window).await)
}

/// 回包额外带 `defaults`（出厂默认值，纯增量字段）：设置页「恢复默认」按它落盘。
pub(crate) async fn api_checkin_config() -> Response {
    json_ok(config::load_checkin_config_with_defaults())
}

pub(crate) async fn api_save_checkin_config(Json(body): Json<Value>) -> Response {
    let submitted = submitted_config(&body);
    match config::save_checkin_config(submitted) {
        Ok(()) => json_ok(config::load_checkin_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

/// GET /api/checkin/logs —— 签到日志（每行带 `variant`，便于前端按档位过滤）。
pub(crate) async fn api_checkin_logs() -> Response {
    json_ok(json!({ "logs": checkin::load_checkin_logs_with_variant() }))
}

pub(crate) async fn api_travel_status(RawQuery(query): RawQuery) -> Response {
    // 与桌面端 get_travel_status 同形：传 accountId 时只返回该账号的状态。
    // 不带参数时保留旧的批量响应，供旧客户端/脚本过渡使用。
    if let Some(id) = query_param(query.as_deref(), "accountId").filter(|value| !value.is_empty()) {
        if account::find_account(&id).is_none() {
            return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
        }
        travel::reconcile_due_travel(Some(&id)).await;
        return json_ok(travel::travel_display(&id));
    }
    travel::reconcile_due_travel(None).await;
    let items = account::load_accounts()
        .iter()
        .map(|acc| {
            let id = acc.get("id").and_then(Value::as_str).unwrap_or("");
            let mut value = travel::travel_display(id);
            value["accountId"] = acc.get("id").cloned().unwrap_or(Value::Null);
            value["email"] = json!(account::account_display_name(acc));
            value
        })
        .collect::<Vec<_>>();
    json_ok(json!({ "accounts": items }))
}

pub(crate) async fn api_travel_config() -> Response {
    json_ok(config::load_travel_config_with_defaults())
}

pub(crate) async fn api_save_travel_config(Json(body): Json<Value>) -> Response {
    let submitted = submitted_config(&body);
    match config::save_travel_config(submitted) {
        Ok(()) => {
            let saved = config::load_travel_config();
            if saved.get("enabled").and_then(Value::as_bool) == Some(true) {
                tokio::spawn(async {
                    let _ = travel::run_travel_cycle().await;
                    let _ = travel::run_travel_claim_cycle().await;
                });
            }
            json_ok(saved)
        }
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 设置页：数据目录 / 设置包导出导入
// ---------------------------------------------------------------------------

/// GET /api/data-dir —— 工具数据目录（设置页「关于」显示路径）。
///
/// 只回路径字符串：这是用户自己的数据目录，不含凭据，也不做任何写操作。
pub(crate) async fn api_data_dir() -> Response {
    json_ok(json!({ "path": config::store_dir().to_string_lossy() }))
}

/// GET /api/settings/export —— 导出设置包（checkin / travel / rotate / rateLimit 四份）。
///
/// 不导出更新（GitHub）配置：那份可能带代理口令，明文落盘会泄漏（见 core 的 `bundle.rs`）。
pub(crate) async fn api_export_settings_bundle() -> Response {
    json_ok(config::export_settings_bundle())
}

/// POST /api/settings/import —— 白名单校验后导入设置包；写入前备份，任一键非法整体拒绝。
pub(crate) async fn api_import_settings_bundle(
    Json(body): Json<ImportSettingsBundleRequest>,
) -> Response {
    match config::import_settings_bundle(&body.bundle) {
        Ok(report) => json_ok(report),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}
