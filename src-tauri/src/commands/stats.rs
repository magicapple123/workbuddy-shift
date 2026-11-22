//! 积分、Token 统计与 token 刷新。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::Value;
use wb_shift_core::modules::{account, credit_usage, credits, refresh, token_stats};

/// POST /api/credits —— 查询单账号积分资源及到期时间。
#[tauri::command]
pub async fn get_credit_expiry(account_id: String) -> Result<Value, String> {
    let acc = account::find_account(&account_id).ok_or("账号不存在")?;
    Ok(credits::get_credit_expiry(&acc).await)
}

/// GET /api/credits/stats —— 本地快照与官方请求用量统计。
/// `refresh = true` 时才重新请求官方用量；默认读缓存。
/// `range`：`30d`（默认）/ `90d` / `all`（走本地账本）。
#[tauri::command]
pub async fn get_credit_statistics(refresh: Option<bool>, range: Option<String>) -> Value {
    credit_usage::get_statistics(refresh.unwrap_or(false), range.as_deref()).await
}

/// POST /api/refresh-official-usage —— 只采集官方用量（单飞 + 内存/磁盘缓存）。
#[tauri::command]
pub async fn refresh_official_usage() -> Result<Value, String> {
    let accounts = account::load_accounts();
    Ok(wb_shift_core::modules::official_usage::refresh_official_usage(&accounts).await)
}

#[tauri::command]
pub async fn get_token_statistics(days: Option<i64>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || token_stats::get_statistics(days))
        .await
        .map_err(|error| format!("扫描 Token 统计失败: {error}"))
}

/// POST /api/refresh-token —— 单账号刷新 token。
///
/// 响应投影由 core 的 `refresh::refresh_result_meta` 统一提供（与 HTTP 端逐字段同形）：
/// 抢不到凭据写入锁时额外带 `refreshSkipped` / `refreshSkippedReason`（issue #202）。
#[tauri::command]
pub async fn refresh_account_token(account_id: String) -> Result<Value, String> {
    let acc = account::find_account(&account_id).ok_or("账号不存在")?;
    let fresh = refresh::refresh_account_token(acc).await;
    Ok(refresh::refresh_result_meta(&fresh))
}

// ---------------------------------------------------------------------------
// 阶段 4：自动更新
// ---------------------------------------------------------------------------
