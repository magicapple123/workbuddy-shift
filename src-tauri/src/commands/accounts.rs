//! 账号库、OAuth、导入导出与权限引导（GET/POST /api/accounts 等）。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use super::AppStatus;
use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::{account, auth_file, config, export_import, oauth, process, update};

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

// ---------------------------------------------------------------------------
// 加密导出 / 导入（换机迁移，issue #186）
//
// 口令策略（≥8 位）、信封格式、体积上限与文件读写都在 core
// （`export_import::export_accounts_encrypted_at` / `import_accounts_encrypted_at`），
// 两个宿主只做参数转发与结果整形，保证桌面端与 WebUI 的错误文案逐字一致。
// 口令只作为参数向下传：这里不记日志、不回显，错误文案里也不会出现它。
// ---------------------------------------------------------------------------

/// POST /api/accounts/export-encrypted —— 用口令加密导出**全部**账号到用户选择的路径。
///
/// PBKDF2 是 600000 次迭代（本机实测 0.1–0.3 s），同步 command 会在 Tauri 主线程上跑，
/// 期间窗口消息循环被占住；与 `get_status` 同样丢进 blocking 线程，避免导出时界面卡顿。
#[tauri::command]
pub async fn export_accounts_encrypted(path: String, password: String) -> Result<Value, String> {
    match tauri::async_runtime::spawn_blocking(move || {
        let accounts = account::load_accounts();
        let count = accounts.len();
        export_import::export_accounts_encrypted_at(&path, &password, &accounts, config::now_ms())?;
        Ok(json!({ "ok": true, "path": path, "count": count }))
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("导出加密备份失败: {error}")),
    }
}

/// POST /api/accounts/import-encrypted —— 用口令解密备份文件并按既有合并语义导入。
///
/// 合并语义完全复用「导入备份」那条路径（按 uid 去重覆盖、保留本地展示字段、缺 id 补齐），
/// 加密只改变"文件怎么读出来"，不改变"记录怎么合进去"。
#[tauri::command]
pub async fn import_accounts_encrypted(path: String, password: String) -> Result<Value, String> {
    match tauri::async_runtime::spawn_blocking(move || {
        export_import::import_accounts_encrypted_at(&path, &password)
    })
    .await
    {
        Ok(Ok(result)) => Ok(json!({
            "ok": true,
            "imported": result.imported,
            "skipped": result.skipped,
            "overwritten": result.overwritten,
        })),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(format!("导入加密备份失败: {error}")),
    }
}

/// 打开系统设置授权面板。仅 macOS 提供「完全磁盘访问 / App 管理」；
/// 传 `target="app_management"` 尝试「App 管理」（macOS 15+，部分版本不支持深链），
/// 缺省「完全磁盘访问」（该 anchor 各版本均有效）。
///
/// 使用 macOS 13+ 深链接格式（`com.apple.settings.PrivacySecurity.extension?Privacy_*`）。
///
/// 其它平台**返回明确错误**，不假装成功：Windows / Linux 没有对应的隐私授权开关，
/// 写认证文件只受文件读写权限限制。此前这里无 `#[cfg]` 分支且丢弃了 spawn 结果，
/// 于是 Windows 上点按钮完全没有反馈（第三轮第 11 条）。
#[tauri::command]
pub fn open_permission_settings(target: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let t = target.unwrap_or_else(|| "all_files".to_string());
        let url = match t.as_str() {
            "app_management" => {
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AppManagement"
            }
            _ => {
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"
            }
        };
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开系统设置失败：{error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Err("当前系统没有「完全磁盘访问 / App 管理」设置项：Windows 与 Linux 上写认证文件只受文件读写权限限制，请改用「在资源管理器中显示认证文件」检查该文件是否被客户端占用或只读".to_string())
    }
}

/// 写入失败时的授权引导文案（按平台给，别让 Windows 用户去找「完全磁盘访问」）。
#[cfg(target_os = "macos")]
const PERMISSION_HINT: &str = "请在 系统设置→隐私与安全性 中授权：优先「App 管理」开启 WorkBuddy Shift，若没有则去「完全磁盘访问」把 WorkBuddy Shift 拖进去；授权后需重启 App 生效";
/// Windows 没有隐私授权开关：写不进去只可能是占用、只读 / ACL 或安全软件拦截。
#[cfg(target_os = "windows")]
const PERMISSION_HINT: &str = "写认证文件失败通常是：WorkBuddy 客户端正在占用该文件、文件只读或被 ACL 拒绝（属性→安全），或杀毒 / 受控文件夹访问拦截了写入。关闭客户端后重试，或把 WorkBuddy Shift 加入受控文件夹访问白名单";
#[cfg(all(unix, not(target_os = "macos")))]
const PERMISSION_HINT: &str = "写认证文件失败通常是文件所有者或权限位不对：确认该文件属于当前用户且可写（ls -l 后按需 chmod u+w）";

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
            "hint": PERMISSION_HINT,
        }),
    }
}

/// 在系统文件管理器中选中并定位本 App 的可执行文件（macOS 拖拽授权时用）。
///
/// 三个平台各有自己的入口，此前只实现了 macOS，且丢弃了 spawn 结果：
/// Windows 上 `open -R` 根本不存在，按钮毫无反应（第三轮第 11 条）。
/// 命令名是稳定契约（前端 `reveal_app_in_finder` 仍按此调用），没改。
#[tauri::command]
pub fn reveal_app_in_finder() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位应用路径失败：{e}"))?;
    spawn_reveal_file(&exe)
}

/// 与 `wb_shift_core::modules::storage::reveal` 的同名三步保持一致
/// （Windows `explorer /select,` / macOS `open -R` / Linux `xdg-open <父目录>`）。
/// 那份实现在 core，且被 storage 白名单校验包裹，这里只需要“选中这个文件”，
/// 因此按同一套命令重写一份；要改命令格式时两处一起改。
fn spawn_reveal_file(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // explorer 对多数失败仍返回 0，只能要求进程启动成功。
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开资源管理器失败：{error}"))
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开访达失败：{error}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开文件管理器失败：{error}"))
    }
}
