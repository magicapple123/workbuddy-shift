//! WorkBuddy「自动化」定时任务的切号提示与同步开关。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析与错误字符串化，业务逻辑在 wb-shift-core。
//! 这两个命令是 **desktop-only**：WebUI 的切号入口不展示提示块（见
//! `scripts/check-command-routes.mjs` 的 desktop-only 清单）。

use serde_json::{json, Value};
use wb_shift_core::modules::automation_sync;
use wb_shift_core::modules::session::SessionPaths;

/// 当前登录账号启用中的定时任务摘要（只读）：`{ count, names, enabled }`。
///
/// 入参只要 `uid`：任务属于**源账号**（当前登录），而前端手上的档位是目标账号的，
/// 由 core 按 uid 自动在两个档位库里查（见 `automation_sync::summary_any`）。
#[tauri::command(rename_all = "camelCase")]
pub fn automation_tasks_summary(uid: String) -> Result<Value, String> {
    if uid.trim().is_empty() {
        return Err("缺少 uid".to_string());
    }
    Ok(automation_sync::summary_any(&uid))
}

/// 同步开关状态（默认关闭）。
#[tauri::command]
pub fn automation_sync_enabled() -> Value {
    let paths = SessionPaths::for_variant(wb_shift_core::modules::variant::WbVariant::Cn);
    json!({ "enabled": automation_sync::is_enabled(&paths) })
}

/// 写同步开关。`enabled = false` 时只剩切号弹窗里的提示，库零写入。
#[tauri::command(rename_all = "camelCase")]
pub fn automation_sync_set_enabled(enabled: bool) -> Result<Value, String> {
    let paths = SessionPaths::for_variant(wb_shift_core::modules::variant::WbVariant::Cn);
    automation_sync::set_enabled(&paths, enabled)?;
    Ok(json!({ "enabled": enabled }))
}
