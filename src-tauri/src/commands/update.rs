//! 更新源配置、检查更新与下载/重启。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::Value;
use wb_shift_core::modules::update;

/// GET /api/update/config —— 更新源配置（owner/repo/token）。
#[tauri::command]
pub fn get_github_config() -> Value {
    update::load_github_config()
}

/// POST /api/update/config —— 保存更新源配置。
#[tauri::command]
pub fn save_github_config(config: Value) -> Result<Value, String> {
    update::save_github_config(&config).map_err(|e| e.to_string())?;
    Ok(update::load_github_config())
}

/// GET /api/update/check —— 检查 GitHub Releases 是否有新版本。
/// force=true 时绕过缓存强制刷新（设置页手动检查）。
///
/// 内部走统一更新状态机（`update_service::check`）：检查结果同时写入快照并 emit
/// `update-state`，托盘菜单与前端弹窗因此始终显示同一阶段；返回值结构与改造前一致。
#[tauri::command]
pub async fn check_update(
    app: tauri::AppHandle,
    proxy: Option<String>,
    force: Option<bool>,
) -> Value {
    crate::update_service::check(&app, proxy.as_deref(), force.unwrap_or(false)).await
}

/// GET /api/update/state —— 当前更新状态快照（前端首屏初始化 + 事件丢失兜底）。
#[tauri::command]
pub fn update_state() -> crate::update_service::UpdateSnapshot {
    crate::update_service::snapshot()
}

/// POST /api/update/download —— 启动更新包下载。
///
/// `proxy` 为设置页输入框当前值（可能尚未保存）：检查与下载用同一个代理源，
/// 避免「填了代理但没点保存 → 检查通过、下载失败」（第三轮第 1 条）。
/// 异步：立即返回，进度与阶段经 `update-state` 事件推送、托盘 tooltip 实时显示。
#[tauri::command]
pub fn update_download(app: tauri::AppHandle, proxy: Option<String>) -> Result<(), String> {
    crate::update_service::start_download(&app, proxy)
}

/// POST /api/update/restart —— 安装已下载的更新包并重启应用。
///
/// 安装时机在用户点「重启以完成升级」时：Windows 安装器要求应用退出才能完成安装，
/// macOS 安装可能触发 `/Applications` 写授权，都不适合在下载完成时静默执行。
#[tauri::command]
pub async fn update_restart(app: tauri::AppHandle) -> Result<(), String> {
    crate::update_service::restart(&app).await
}
