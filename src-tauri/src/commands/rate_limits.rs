//! 模型限额查询与 hook 安装/卸载。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::Value;
use wb_shift_core::modules::{limits, rate_limit_hook};

/// GET /api/rate-limits —— 模型限额台账（全部账号当前受限的模型与官方恢复时刻）。
///
/// 不接收档位参数：扫描本身就是全局的（两档位各扫一遍）。扫描读取本机日志文件，
/// 放 blocking 线程避免阻塞主线程；无受限模型时返回空数组，不是错误。
#[tauri::command]
pub async fn get_rate_limits() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(limits::get_rate_limits)
        .await
        .map_err(|error| format!("扫描模型限额失败: {error}"))
}

/// GET /api/rate-limits/hook-status —— hook 安装状态（脚本 + 三处客户端配置逐项结果）。
///
/// 读三个 `settings.json` 与一个脚本文件，放 blocking 线程避免占用主线程。
#[tauri::command]
pub async fn get_rate_limit_hook_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(limits::rate_limit_hook_status)
        .await
        .map_err(|error| format!("查询限额 hook 状态失败: {error}"))
}

/// POST /api/rate-limits/install-hook —— 安装 hook（幂等，写前备份；同时清除「卸载过」标记）。
#[tauri::command]
pub async fn install_rate_limit_hook() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let result = rate_limit_hook::install_hook();
        // 扫描范围随安装结果变化（只对未注册的来源扫日志），缓存必须作废。
        limits::invalidate_scan_cache();
        result.map(|_| limits::rate_limit_hook_status())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// POST /api/rate-limits/uninstall-hook —— 卸载 hook（移除注册条目，尽量逐字节还原）。
///
/// 卸载即用户拒绝自动接入：`install_hook` / `hookOptOut` 的置位在 core 里与安装逻辑同处，
/// 两个宿主（桌面端 / webui）共用同一语义。
#[tauri::command]
pub async fn uninstall_rate_limit_hook() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let result = rate_limit_hook::uninstall_hook();
        limits::invalidate_scan_cache();
        result.map(|_| limits::rate_limit_hook_status())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// GET /api/rate-limits/config —— 限额监听开关。
///
/// 回包额外带 `defaults`（出厂默认值，纯增量字段）：设置页「恢复默认」按它落盘。
#[tauri::command]
pub fn get_rate_limit_config() -> Value {
    crate::modules::config::load_rate_limit_config_with_defaults()
}

/// POST /api/rate-limits/config —— 保存限额监听开关。
///
/// 走 `limits::save_rate_limit_config`：`scanIdeLogs` 变化时作废扫描缓存（下一次按当前
/// 来源范围重算），否则关掉 IDE 扫描后下一次还会按旧缓存把两个 IDE 扫一遍。
#[tauri::command]
pub fn save_rate_limit_config(config: Value) -> Result<Value, String> {
    limits::save_rate_limit_config(&config).map_err(|e| e.to_string())?;
    Ok(crate::modules::config::load_rate_limit_config())
}
