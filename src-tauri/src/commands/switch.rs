//! WorkBuddy 主账号切换（带实时进度事件）。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::json;
use serde_json::Value;
use tauri::Emitter;
use wb_shift_core::modules::{session, switch};

/// POST /api/switch —— 切换账号（备份 → 关进程 → 恢复/复制/同步会话 → 写认证 → 重启）。
///
/// `syncSelections` 与 HTTP 端同形（`[{groupId, previewToken, mode}]`）。
///
/// async + spawn_blocking：切换中关闭/启动 WorkBuddy 会阻塞数十秒，
/// 若在同步 command（主线程）执行会卡死整个 UI（loading 遮罩无法渲染）。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_account(
    app: tauri::AppHandle,
    account_id: String,
    restart: Option<bool>,
    share_sessions: Option<bool>,
    copy_session_ids: Option<Vec<String>>,
    sync_selections: Option<Value>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let restart = restart.unwrap_or(true);
    let share_sessions = share_sessions.unwrap_or(false);
    let copy_ids = copy_session_ids.unwrap_or_default();
    // 入参形状由 core 校验（缺 groupId / previewToken / mode 一律拒绝）；这里只做透传，
    // 不在命令层做业务判定。
    let sync_selections = session::parse_sync_selections(sync_selections.as_ref())?;
    // 切换流程会 move 走 app（进度回调），复位要用的句柄先留一份。
    #[cfg(desktop)]
    let rearm_app = app.clone();
    let progress: switch::ProgressFn = Box::new(move |message| {
        let _ = app.emit("switch-progress", json!({ "message": message }));
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
        switch::switch_account(
            Some(&progress),
            &account_id,
            restart,
            share_sessions,
            &copy_ids,
            &sync_selections,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    // 切换会关掉再重启 WorkBuddy，悬浮栏依赖的采集连接随之中断。成功之后复位一次：
    // 用户开着悬浮窗就让它重新接上，关着就什么都不做（见 `companion::rearm`）。
    #[cfg(desktop)]
    {
        let _ =
            tauri::async_runtime::spawn_blocking(move || crate::companion::rearm(&rearm_app)).await;
    }
    result
}
