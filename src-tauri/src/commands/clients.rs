//! 各客户端状态检测与账号切换（CodeBuddy CLI / CodeBuddy IDE / VS Code / JetBrains）。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::{
    account, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, codebuddy_ide_session,
    codebuddy_ide_session_sync, config, jetbrains, session, vscode_ext, vscode_session,
    vscode_session_sync,
};

/// 当前是否跑在被 `WB_SHIFT_HOME` 重定向的沙箱里。
///
/// dev 构建的数据目录由环境变量重定向到沙箱，窗口里的账号 / 会话 / 备份全部来自
/// 沙箱副本，与真实环境无关；前端据此在侧边栏标题旁显示「沙箱」角标，避免和正式版混淆。
///
/// 薄封装：只取路径与环境变量，判定口径全部在 core（`config::is_sandboxed_home`，
/// 那里有可执行的单测）。纯计算：不读文件、不加锁——首屏就会调用，任何 IO 都会
/// 拖慢第一次渲染。
#[tauri::command]
pub fn dev_environment() -> Value {
    let home = config::home_dir();
    let real_home = config::real_home_dir();
    let candidate = std::env::var(config::HOME_ENV_VAR).ok();
    json!({
        "sandboxed": config::is_sandboxed_home(candidate.as_deref(), &real_home),
        "home": home.to_string_lossy().to_string(),
        "realHome": real_home.to_string_lossy().to_string(),
    })
}

/// GET /api/codebuddy-cli/status —— CodeBuddy CLI helper 轮换状态（不含 token）。
///
/// async + spawn_blocking：状态检测可能执行 ps / helper 定位等子进程，
/// 避免在账号页挂载刷新时阻塞主线程造成页面卡顿。
#[tauri::command]
pub async fn get_codebuddy_cli_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cli::status)
        .await
        .map_err(|error| format!("查询 CodeBuddy CLI 状态失败: {error}"))
}

/// POST /api/codebuddy-cli/install-helper —— 显式安装/升级 CLI helper。
#[tauri::command]
pub async fn install_codebuddy_cli_helper(app: tauri::AppHandle) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(codebuddy_cli::install_helper)
        .await
        .map_err(|e| e.to_string())??;
    // 写入 settings.json 与随后的状态重算之间存在短暂窗口，让前端立即重读，
    // 避免把「刚写完、正在校验」误显示为脱节。
    crate::notify_codebuddy_cli_updated(&app);
    Ok(result)
}

/// POST /api/codebuddy-cli/switch —— 只切换 CodeBuddy CLI，不重启 WorkBuddy。
///
/// 任何切换都会**先关闭正在运行的 CodeBuddy CLI 再写状态**：key 是进程级快照，
/// 不关进程就会出现"新默认账号 + 旧进程仍持旧 key"的窗口。当前 CLI 会话因此会中断。
///
/// `close_running_cli` 入参**已废弃**（保留接受但忽略），仅为不破坏既有调用方。
///
/// async + spawn_blocking：切换会用登录 shell 定位 node 并执行 apiKeyHelper
/// 校验账号（子进程无超时），同步 command 会阻塞主线程造成 UI 卡顿。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_codebuddy_cli_account(
    app: tauri::AppHandle,
    account_id: String,
    close_running_cli: Option<bool>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let _ = close_running_cli;
    let result = tauri::async_runtime::spawn_blocking(move || {
        codebuddy_cli::switch_active_account(&account_id)
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::notify_codebuddy_cli_updated(&app);
    Ok(result)
}

/// GET /api/codebuddy-cn-ide/status —— CodeBuddy IDE 安装/运行/当前账号。
///
/// async + spawn_blocking：状态检测会跑 ps / mdfind 等子进程（mdfind 可能
/// 耗时数秒），账号页每次挂载都会刷新，若在主线程执行会造成页面卡顿。
#[tauri::command]
pub async fn get_codebuddy_cn_ide_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cn_ide::status)
        .await
        .map_err(|error| format!("查询 CodeBuddy IDE 状态失败: {error}"))
}

/// POST /api/codebuddy-cn-ide/switch —— 注入凭证并可选重启 CodeBuddy CN IDE。
///
/// `copySessions` 非空时，切换前先把勾选的会话复制到目标账号（默认沿用会话 id，
/// 目标已有同 id 时才重随机）并登记关联；`syncSelections` 非空时，再把关联会话的
/// 新增内容同步过去（只同步不复制同样可用）。两者都为空时行为与纯切换逐字一致。
///
/// async + spawn_blocking：切换会关闭并重启 CodeBuddy CN，可能阻塞数十秒，
/// 与 WorkBuddy 切换同理，若在同步 command（主线程）执行会卡死整个 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_codebuddy_cn_ide_account(
    account_id: String,
    restart: Option<bool>,
    copy_sessions: Option<Vec<vscode_session::CopyItem>>,
    sync_selections: Option<Value>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let restart = restart.unwrap_or(true);
    let items = copy_sessions.unwrap_or_default();
    // 入参形状由 core 校验（缺 groupId / previewToken / mode 一律拒绝）；这里只做透传。
    let sync_selections = session::parse_sync_selections(sync_selections.as_ref())?;
    tauri::async_runtime::spawn_blocking(move || {
        if items.is_empty() && sync_selections.is_empty() {
            codebuddy_cn_ide::switch_account(&account_id, restart)
        } else {
            codebuddy_ide_session::switch_codebuddy_cn_ide_with_copy(
                &account_id,
                restart,
                &items,
                &sync_selections,
            )
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// GET /api/codebuddy-cn-ide/sessions —— 列出当前 IDE 账号可复制的会话。
///
/// async + spawn_blocking：会扫描 IDE 会话目录（可能较大）并读取本机登录 secret，
/// 避免阻塞主线程。未登录/未安装时返回空列表而非报错（供前端渲染空态）。
#[tauri::command]
pub async fn list_codebuddy_ide_sessions() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_ide_session::list_current_codebuddy_ide_sessions)
        .await
        .map_err(|error| format!("列出 CodeBuddy IDE 会话失败: {error}"))
}

/// POST /api/codebuddy-cn-ide/session-links —— 预览「当前 IDE 账号 → 目标账号」可同步的关联会话。
///
/// 只读：每组的 `defaultChecked` 与 `availableModes` 是前端勾选权限的唯一来源。
/// async + spawn_blocking：会扫描会话目录并读取正文，避免阻塞 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn codebuddy_ide_session_links_preview(
    target_account_id: String,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        codebuddy_ide_session_sync::links_preview(&target)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/codebuddy-cn-ide/detect —— 读取本机 CN IDE 当前登录并尝试匹配账号库。
///
/// async + spawn_blocking：会通过 Keychain/secret 读取子进程，避免阻塞主线程。
#[tauri::command]
pub async fn detect_codebuddy_cn_ide_account() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cn_ide::detect_current_account)
        .await
        .map_err(|e| e.to_string())?
}

/// GET /api/vscode-ext/status —— VS Code CodeBuddy 扩展安装/运行/当前账号。
///
/// async + spawn_blocking：状态检测会跑 tasklist/ps 等子进程，账号页每次挂载都会
/// 刷新，若在主线程执行会造成页面卡顿。
#[tauri::command]
pub async fn get_vscode_ext_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(vscode_ext::status)
        .await
        .map_err(|error| format!("查询 VS Code CodeBuddy 插件状态失败: {error}"))
}

/// GET /api/vscode-ext/sessions —— 列出当前 VS Code 扩展账号可复制的会话。
///
/// async + spawn_blocking：会扫描扩展数据目录（可能较大）并读取本地状态文件，
/// 避免阻塞主线程。未登录/未安装时返回空列表而非报错（供前端渲染空态）。
#[tauri::command]
pub async fn list_vscode_sessions() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| match vscode_ext::active_ext_uid() {
        Some(uid) => vscode_session::list_vscode_sessions(&uid),
        None => json!({ "sourceUid": Value::Null, "sessions": [], "skipped": 0 }),
    })
    .await
    .map_err(|error| format!("列出 VS Code CodeBuddy 插件会话失败: {error}"))
}

/// POST /api/vscode-ext/switch —— 注入凭证到 VS Code CodeBuddy 扩展。
///
/// `copySessions` 非空时，切换前先把勾选的会话复制到目标账号（新 id，加法）并登记关联；
/// `syncSelections` 非空时，再把关联会话的新增内容同步过去（只同步不复制同样可用）。
/// `restart` 缺省 true：VS Code 正在运行时由后端「优雅退出 → 写入 → 重新打开」，
/// 传 false 则退回「请先完全退出 VS Code」的手动模式。
///
/// async + spawn_blocking：读写 state.vscdb + DPAPI 解密 + 等待编辑器退出 + 会话目录
/// 复制都可能阻塞，避免卡 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_vscode_ext_account(
    account_id: String,
    restart: Option<bool>,
    copy_sessions: Option<Vec<vscode_session::CopyItem>>,
    sync_selections: Option<Value>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let restart = restart.unwrap_or(true);
    let items = copy_sessions.unwrap_or_default();
    // 入参形状由 core 校验（缺 groupId / previewToken / mode 一律拒绝）；这里只做透传。
    let sync_selections = session::parse_sync_selections(sync_selections.as_ref())?;
    tauri::async_runtime::spawn_blocking(move || {
        // 复制与同步都没勾选时才退回纯切换路径（不再由 `copySessions` 单独决定）。
        if items.is_empty() && sync_selections.is_empty() {
            vscode_ext::switch_account(&account_id, restart)
        } else {
            vscode_session::switch_vscode_ext_with_copy(
                &account_id,
                restart,
                &items,
                &sync_selections,
            )
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/vscode-ext/session-links —— 预览「当前插件账号 → 目标账号」可同步的关联会话。
///
/// 只读：返回 `supported / storeStatus / groups`，其中每组的 `defaultChecked` 与
/// `availableModes` 是前端勾选权限的唯一来源，前端不得自行扩大。
/// async + spawn_blocking：会扫描扩展数据目录并读取会话正文，避免阻塞 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn vscode_session_links_preview(target_account_id: String) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        vscode_session_sync::links_preview(&target)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_codebuddy_ide_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_ide::status)
        .await
        .map_err(|error| format!("查询 CodeBuddy IDE 状态失败: {error}"))
}

/// GET /api/jetbrains/status —— JetBrains IDE（IDEA / PyCharm）CodeBuddy 插件状态。
///
/// async + spawn_blocking：状态检测会跑 CIM / tasklist 等子进程，账号页每次挂载
/// 都会刷新，若在主线程执行会造成页面卡顿/闪窗。
#[tauri::command]
pub async fn get_jetbrains_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(jetbrains::status)
        .await
        .map_err(|error| format!("查询 JetBrains IDE 状态失败: {error}"))
}

/// POST /api/jetbrains/switch —— 注入凭证到 JetBrains IDE 的 CodeBuddy 插件。
///
/// `restart` 缺省 true：IDE 正在运行时由后端「优雅退出 → 写入 → 重新打开」，
/// 传 false 则退回「请先完全退出 IDE」的手动模式。
/// `configDirs` 可选：目标配置目录名列表（如 ["PyCharm2026.2"]），缺省 / 空数组
/// = 全部装了插件的 IDE；非空时只写所选目录、只关闭/重开这些目录的运行实例。
///
/// async + spawn_blocking：等待 IDE 退出 + 读写 secret-storage.xml 都可能阻塞。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_jetbrains_account(
    account_id: String,
    restart: Option<bool>,
    config_dirs: Option<Vec<String>>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let dirs = config_dirs.filter(|list| !list.is_empty());
    tauri::async_runtime::spawn_blocking(move || {
        jetbrains::switch_account(&account_id, restart.unwrap_or(true), dirs.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/jetbrains/detect —— 读取本机 JetBrains IDE 插件当前登录并尝试匹配账号库。
///
/// async + spawn_blocking：会读多份 secret-storage.xml，避免阻塞主线程。
#[tauri::command]
pub async fn detect_jetbrains_account() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(jetbrains::detect_current_account)
        .await
        .map_err(|e| e.to_string())?
}

/// POST /api/codebuddy-ide/switch —— 注入凭证到 CodeBuddy IDE（国际版），可选复制 / 同步会话。
///
/// `copySessions` 非空时，切换前先把勾选的会话复制到目标账号（默认沿用会话 id，
/// 目标已有同 id 时才重随机）并登记关联；`syncSelections` 非空时，再把关联会话的
/// 新增内容同步过去（只同步不复制同样可用）。两者都为空时行为与纯切换逐字一致。
///
/// async + spawn_blocking：切换会关闭并重启 CodeBuddy，可能阻塞数十秒，
/// 与 WorkBuddy 切换同理，若在同步 command（主线程）执行会卡死整个 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_codebuddy_ide_account(
    account_id: String,
    restart: Option<bool>,
    copy_sessions: Option<Vec<vscode_session::CopyItem>>,
    sync_selections: Option<Value>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let restart = restart.unwrap_or(true);
    let items = copy_sessions.unwrap_or_default();
    // 入参形状由 core 校验（缺 groupId / previewToken / mode 一律拒绝）；这里只做透传。
    let sync_selections = session::parse_sync_selections(sync_selections.as_ref())?;
    tauri::async_runtime::spawn_blocking(move || {
        if items.is_empty() && sync_selections.is_empty() {
            codebuddy_ide::switch_account(&account_id, restart)
        } else {
            codebuddy_ide_session::switch_codebuddy_intl_ide_with_copy(
                &account_id,
                restart,
                &items,
                &sync_selections,
            )
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// GET /api/codebuddy-ide/sessions —— 列出当前国际版 IDE 账号可复制的会话。
///
/// async + spawn_blocking：会扫描 IDE 会话目录（可能较大）并读取本机登录 secret，
/// 避免阻塞主线程。未登录/未安装时返回空列表而非报错（供前端渲染空态）。
#[tauri::command]
pub async fn list_codebuddy_intl_ide_sessions() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_ide_session::list_current_intl_ide_sessions)
        .await
        .map_err(|error| format!("列出 CodeBuddy IDE 会话失败: {error}"))
}

/// POST /api/codebuddy-ide/session-links —— 预览「当前国际版 IDE 账号 → 目标账号」可同步的关联会话。
///
/// 只读：每组的 `defaultChecked` 与 `availableModes` 是前端勾选权限的唯一来源。
/// async + spawn_blocking：会扫描会话目录并读取正文，避免阻塞 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn codebuddy_intl_ide_session_links_preview(
    target_account_id: String,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        codebuddy_ide_session_sync::links_preview_intl(&target)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/vscode-ext/detect —— 读取本机 VS Code 扩展当前登录并尝试匹配账号库。
///
/// async + spawn_blocking：会通过 Safe Storage 读取子进程，避免阻塞主线程。
#[tauri::command]
pub async fn detect_vscode_ext_account() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(vscode_ext::detect_current_account)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn detect_codebuddy_ide_account() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_ide::detect_current_account)
        .await
        .map_err(|e| e.to_string())?
}
