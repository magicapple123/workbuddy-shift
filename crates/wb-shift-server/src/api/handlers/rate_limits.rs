//! rate-limits 域的 HTTP handler。

use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::Value;
use wb_shift_core::modules::*;

/// GET /api/rate-limits —— 模型限额台账（全部账号当前受限的模型与官方恢复时刻）。
///
/// 扫描本机日志文件，放 blocking 线程避免占用运行时线程；无受限模型时返回空数组。
pub(crate) async fn api_rate_limits() -> Response {
    match tokio::task::spawn_blocking(limits::get_rate_limits).await {
        Ok(payload) => json_ok(payload),
        Err(error) => json_err(
            format!("扫描模型限额失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// GET /api/rate-limits/hook-status —— hook 安装状态（脚本 + 三处客户端配置逐项结果）。
pub(crate) async fn api_rate_limit_hook_status() -> Response {
    match tokio::task::spawn_blocking(limits::rate_limit_hook_status).await {
        Ok(status) => json_ok(status),
        Err(error) => json_err(
            format!("查询限额 hook 状态失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// POST /api/rate-limits/install-hook —— 安装 hook（幂等，写前备份；同时清除「卸载过」标记）。
pub(crate) async fn api_install_rate_limit_hook() -> Response {
    match tokio::task::spawn_blocking(|| {
        let result = rate_limit_hook::install_hook();
        // 扫描范围随安装结果变化（只对未注册的来源扫日志），缓存必须作废。
        limits::invalidate_scan_cache();
        result.map(|_| limits::rate_limit_hook_status())
    })
    .await
    {
        Ok(Ok(status)) => json_ok(status),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/rate-limits/uninstall-hook —— 卸载 hook（移除注册条目，尽量逐字节还原）。
///
/// 卸载即用户拒绝自动接入（`hookOptOut`），与安装逻辑同处 core，两个宿主共用同一语义。
pub(crate) async fn api_uninstall_rate_limit_hook() -> Response {
    match tokio::task::spawn_blocking(|| {
        let result = rate_limit_hook::uninstall_hook();
        limits::invalidate_scan_cache();
        result.map(|_| limits::rate_limit_hook_status())
    })
    .await
    {
        Ok(Ok(status)) => json_ok(status),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 回包额外带 `defaults`（出厂默认值，纯增量字段）：设置页「恢复默认」按它落盘。
pub(crate) async fn api_rate_limit_config() -> Response {
    json_ok(config::load_rate_limit_config_with_defaults())
}

/// POST /api/rate-limits/config —— 保存限额监听配置。
///
/// 与桌面端同语义：`scanIdeLogs` 变化时作废扫描缓存，下一次按当前来源范围重算。
pub(crate) async fn api_save_rate_limit_config(Json(body): Json<Value>) -> Response {
    let submitted = submitted_config(&body);
    match limits::save_rate_limit_config(submitted) {
        Ok(()) => json_ok(config::load_rate_limit_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}
