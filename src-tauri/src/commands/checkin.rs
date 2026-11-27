//! 签到、旅行、CLI 自动轮换等自动化配置与执行。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::{account, checkin, rotate, travel};

/// GET /api/checkin/status —— 查询单账号签到状态（档位取账号自身）。
#[tauri::command]
pub async fn get_checkin_status(account_id: String) -> Result<Value, String> {
    let acc = account::find_account(&account_id).ok_or("账号不存在")?;
    let mut status = checkin::get_checkin_status_for_display(&acc).await;
    // 结果行带档位，前端按当前档位过滤时无需再查账号。
    status["variant"] = json!(account::variant_of(&acc).as_str());
    Ok(status)
}

/// POST /api/checkin —— 单账号立即签到。
#[tauri::command]
pub async fn checkin(account_id: String) -> Result<Value, String> {
    let acc = account::find_account(&account_id).ok_or("账号不存在")?;
    Ok(checkin::checkin_account(&acc).await)
}

/// POST /api/checkin/all —— 全部账号批量签到（每个账号按自身档位）。
/// `variant` 缺省为 `None`（全部档位，保持原行为）；显式传入时只处理该档位。
/// 关闭自动签到的账号逐账号返回 skipped 原因（设置页与托盘同样遵守）。
/// `respectWindow` 只由账号页「刷新并签到」传 `true`：窗口生效且当前不在时间段内
/// 时整轮跳过（逐账号 `skipped` / `outside_checkin_window`）；缺省 `false` =
/// 设置页 / 托盘 / 单账号入口的立即签到语义。
#[tauri::command]
pub async fn checkin_all(variant: Option<String>, respect_window: Option<bool>) -> Value {
    let variant = variant.as_deref().map(|raw| WbVariant::parse(Some(raw)));
    checkin::run_checkin_all(variant, respect_window.unwrap_or(false)).await
}

/// GET /api/checkin/config —— 自动签到配置。
///
/// 回包额外带 `defaults`（出厂默认值，纯增量字段）：设置页的「恢复默认」按它落盘，
/// 前端不再硬编码任何默认值。
#[tauri::command]
pub fn get_auto_checkin_config() -> Value {
    crate::modules::config::load_checkin_config_with_defaults()
}

/// POST /api/checkin/config —— 保存自动签到配置。
#[tauri::command]
pub fn save_auto_checkin_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_checkin_config(&config).map_err(|e| e.to_string())?;
    Ok(crate::modules::config::load_checkin_config())
}

/// GET /api/checkin/logs —— 签到日志（每行带 `variant`，便于前端按档位过滤）。
#[tauri::command]
pub fn get_checkin_logs() -> Value {
    json!({ "logs": checkin::load_checkin_logs_with_variant() })
}

// ---------------------------------------------------------------------------
// 派猫猫旅行
// ---------------------------------------------------------------------------

/// GET /api/travel/status —— 查询单账号今日旅行状态标签。
///
/// 档位取账号自身；不支持成长中心的档位（国际版）由 core 返回
/// `{"status":"skipped","reason":"unsupported_variant"}` 形态，宿主原样透传，
/// 不把它吞成「未旅行」。
#[tauri::command]
pub async fn get_travel_status(account_id: String) -> Result<Value, String> {
    account::find_account(&account_id).ok_or("账号不存在")?;
    travel::reconcile_due_travel(Some(account_id.as_str())).await;
    Ok(travel::travel_display(&account_id))
}

/// GET /api/travel/config —— 自动旅行配置。
#[tauri::command]
pub fn get_auto_travel_config() -> Value {
    crate::modules::config::load_travel_config_with_defaults()
}

/// POST /api/travel/config —— 保存自动旅行配置。开启时立刻跑一轮派发/领取。
#[tauri::command]
pub fn save_auto_travel_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_travel_config(&config).map_err(|e| e.to_string())?;
    let saved = crate::modules::config::load_travel_config();
    if saved.get("enabled").and_then(Value::as_bool) == Some(true) {
        tauri::async_runtime::spawn(async {
            let _ = travel::run_travel_cycle().await;
            let _ = travel::run_travel_claim_cycle().await;
        });
    }
    Ok(saved)
}

// ---------------------------------------------------------------------------
// 自动轮换（CodeBuddy CLI）
// ---------------------------------------------------------------------------

/// GET /api/rotate/config —— 自动轮换配置。
#[tauri::command]
pub fn get_auto_rotate_config() -> Value {
    crate::modules::config::load_auto_rotate_config_with_defaults()
}

/// POST /api/rotate/config —— 保存自动轮换配置。
#[tauri::command]
pub fn save_auto_rotate_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_auto_rotate_config(&config).map_err(|e| e.to_string())?;
    Ok(crate::modules::config::load_auto_rotate_config())
}

/// GET /api/rotate/status —— 轮换状态（配置 + 上次检查/切换）。
#[tauri::command]
pub fn rotate_status() -> Value {
    rotate::rotate_status()
}

/// POST /api/rotate/run —— 手动触发一次轮换检查。
///
/// 返回体里的 `notify`（若因存活门控被推迟且未超当日预算）由宿主投递系统通知；
/// 无头 server 只返回该字段，不投递。
#[tauri::command]
pub async fn run_rotate(app: tauri::AppHandle) -> Value {
    let result = rotate::run_rotate_cycle().await;
    crate::deliver_rotate_notify(&app, &result);
    result
}

/// GET /api/rotate/logs —— 最近轮换日志。
#[tauri::command]
pub fn get_rotate_logs() -> Value {
    json!({ "logs": rotate::rotate_logs() })
}

// ---------------------------------------------------------------------------
// 设置页：数据目录 / 设置包导出导入
// ---------------------------------------------------------------------------

/// GET /api/data-dir —— 工具数据目录（设置页「关于」显示路径，桌面端可一键定位）。
///
/// 只回路径字符串：这是用户自己的数据目录，不含凭据；不做任何写操作。
#[tauri::command]
pub fn get_data_dir() -> Value {
    json!({ "path": crate::modules::config::store_dir().to_string_lossy() })
}

/// GET /api/settings/export —— 导出设置包（checkin / travel / rotate / rateLimit 四份）。
///
/// 不导出更新（GitHub）配置：那份可能带代理口令，明文落盘会泄漏（见 core 的 `bundle.rs`）。
#[tauri::command]
pub fn export_settings_bundle() -> Value {
    crate::modules::config::export_settings_bundle()
}

/// POST /api/settings/import —— 白名单校验后导入设置包。
///
/// 写入前备份现有配置；任一键非法整体拒绝，不半写（校验与回滚都在 core）。
#[tauri::command]
pub fn import_settings_bundle(bundle: Value) -> Result<Value, String> {
    crate::modules::config::import_settings_bundle(&bundle)
}
