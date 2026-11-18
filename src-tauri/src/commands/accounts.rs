//! 账号库、OAuth、导入导出与权限引导（GET/POST /api/accounts 等）。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use super::AppStatus;
use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::{account, auth_file, export_import, oauth, process, update};

/// GET /api/status —— WorkBuddy 运行状态 + 当前账号。
///
/// `variant` 缺省国内版：不传参数时行为与改造前逐字一致（只多返回 `variant` 字段）。
#[tauri::command]
pub async fn get_status(variant: Option<String>) -> Result<AppStatus, String> {
    let variant = WbVariant::parse(variant.as_deref());
    // Windows 的运行状态检测会启动 tasklist 子进程。同步 command 默认在
    // Tauri 主线程执行，标题栏拖拽期间一旦焦点事件触发状态刷新，就会阻塞
    // 原生窗口消息循环。放入 blocking 线程，保持窗口移动与 IPC 查询解耦。
    tauri::async_runtime::spawn_blocking(move || build_app_status(variant))
        .await
        .map_err(|error| format!("查询应用状态失败: {error}"))
}

fn build_app_status(variant: WbVariant) -> AppStatus {
    let auth = auth_file::read_auth_file(variant);
    let current = auth.as_ref().map(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        json!({
            "uid": account::display_value(&acct, "uid"),
            "nickname": account::display_value(&acct, "nickname"),
            "email": account::display_value(&acct, "email"),
        })
    });
    AppStatus {
        running: process::is_workbuddy_running(variant),
        auth_file: variant.auth_file_path().to_string_lossy().to_string(),
        current,
        app_path: auth_file::workbuddy_app_path(variant)
            .to_string_lossy()
            .to_string(),
        version: update::APP_VERSION.to_string(),
        variant: variant.as_str().to_string(),
    }
}

/// GET /api/accounts —— 账号列表（account_meta，不含 token）。
///
/// 返回全部档位的账号；每行 meta 自带 `variant`，由前端按当前档位过滤。
#[tauri::command]
pub fn get_accounts() -> Value {
    let metas: Vec<Value> = account::load_accounts()
        .iter()
        .map(account::account_meta)
        .collect();
    json!({ "accounts": metas })
}

// ---------------------------------------------------------------------------
// 开发环境（沙箱角标）
// ---------------------------------------------------------------------------

/// DELETE /api/delete —— 删除账号。
#[tauri::command]
pub fn delete_account(account_id: String) -> Result<Value, String> {
    // 删除语义（含「账号不存在」错误文案）只在 core 实现一次：HTTP 宿主
    // `POST /api/delete` 调用的也是同一个 `account::delete_account`。
    account::delete_account(&account_id).map(|()| json!({ "ok": true }))
}

/// POST /api/update-account-display —— 更新账号本地展示字段（备注 / 显示选择）。
#[tauri::command]
pub fn update_account_display(account_id: String, patch: Value) -> Result<Value, String> {
    // 与 webui POST /api/update-account-display 同形：{ ok, account }。
    account::update_account_display(&account_id, &patch)
        .map(|account| json!({ "ok": true, "account": account }))
}

/// POST /api/oauth/start —— 发起 OAuth 扫码登录（`variant` 缺省国内版）。
#[tauri::command]
pub async fn oauth_start(variant: Option<String>) -> Result<Value, String> {
    oauth::oauth_start(WbVariant::parse(variant.as_deref())).await
}

/// GET /api/oauth/status —— 轮询采集结果（档位取发起时记录，无需传参）。
#[tauri::command]
pub async fn oauth_status(login_id: String) -> Value {
    oauth::oauth_poll(&login_id).await
}

/// POST /api/import-local —— 导入本机当前账号（`variant` 缺省国内版）。
#[tauri::command]
pub fn import_local(variant: Option<String>) -> Result<Value, String> {
    account::import_local(WbVariant::parse(variant.as_deref()))
        .map(|acc| json!({ "ok": true, "account": acc }))
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

/// POST /api/export-accounts —— 按账号 id 列表导出完整记录（含 token）。
#[tauri::command]
pub fn export_accounts(account_ids: Vec<String>) -> Result<Value, String> {
    export_import::export_accounts(&account_ids)
        .map(|records| json!({ "ok": true, "accounts": records }))
}

/// POST /api/export-accounts-to-path —— 把勾选账号的完整记录写入用户选择的路径（保存对话框产物）。
#[tauri::command]
pub fn export_accounts_to_path(account_ids: Vec<String>, path: String) -> Result<Value, String> {
    export_import::export_accounts_to_path(&account_ids, &path)
        .map(|path| json!({ "ok": true, "path": path }))
}

/// POST /api/import/preview —— 解析导入文件并返回脱敏预览（含文件内索引）。
#[tauri::command]
pub fn preview_import_accounts(file_text: String) -> Result<Value, String> {
    export_import::preview_accounts(&file_text)
}

/// POST /api/import —— 按选中索引把账号导入账号库，返回导入/跳过/覆盖计数。
#[tauri::command]
pub fn import_accounts(file_text: String, indexes: Vec<usize>) -> Result<Value, String> {
    let result = export_import::import_accounts(&file_text, &indexes)?;
    Ok(json!({
        "ok": true,
        "imported": result.imported,
        "skipped": result.skipped,
        "overwritten": result.overwritten,
    }))
}

/// 打开系统设置授权面板。默认「完全磁盘访问」（该 anchor 各版本均有效）；
/// 传 `target="app_management"` 尝试「App 管理」（macOS 15+，部分版本不支持深链）。
///
/// 使用 macOS 13+ 深链接格式（`com.apple.settings.PrivacySecurity.extension?Privacy_*`）。
#[tauri::command]
pub fn open_permission_settings(target: Option<String>) -> Result<(), String> {
    let t = target.unwrap_or_else(|| "all_files".to_string());
    let url = match t.as_str() {
        "app_management" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AppManagement"
        }
        _ => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"
        }
    };
    let _ = std::process::Command::new("open").arg(url).spawn();
    Ok(())
}

/// 权限自检：尝试在认证文件目录写/删探针文件，确认完全磁盘访问等授权是否生效。
///
/// 探针放在该档位的登录态文件旁边（两档位同目录，`variant` 缺省国内版）。
#[tauri::command]
pub fn check_auth_permission(variant: Option<String>) -> Value {
    let variant = WbVariant::parse(variant.as_deref());
    let path = variant.auth_file_path();
    // 由档位路径派生探针名，避免再写一份档位相关的文件名。
    let probe = path.with_extension("info.probe");
    match std::fs::write(&probe, "probe") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            json!({ "ok": true, "variant": variant.as_str(), "message": "认证目录可写，权限正常" })
        }
        Err(e) => json!({
            "ok": false,
            "variant": variant.as_str(),
            "error": e.to_string(),
            "dir": path.parent().map(|p| p.to_string_lossy().to_string()),
            "hint": "请在 系统设置→隐私与安全性 中授权：优先「App 管理」开启 WorkBuddy Shift，若没有则去「完全磁盘访问」把 WorkBuddy Shift 拖进去；授权后需重启 App 生效",
        }),
    }
}

/// 在 Finder 中显示当前 App（便于拖拽到「完全磁盘访问」授权框）。
#[tauri::command]
pub fn reveal_app_in_finder() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(&exe)
        .spawn();
    Ok(())
}
