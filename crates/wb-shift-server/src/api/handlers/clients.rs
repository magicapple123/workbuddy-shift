//! clients 域的 HTTP handler。

use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::*;

pub(crate) async fn api_codebuddy_cli_status() -> Response {
    json_ok(codebuddy_cli::status())
}

pub(crate) async fn api_codebuddy_cli_install_helper() -> Response {
    match codebuddy_cli::install_helper() {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_codebuddy_cli_switch(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    // 入参 `closeRunningCli` 已废弃：后端一律先关闭正在运行的 CLI 再写状态，忽略该值。
    // 无头模式不投递系统通知，切号结果（含关闭数量）照常返回给调用方。
    match codebuddy_cli::switch_active_account(id) {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_codebuddy_cn_ide_status() -> Response {
    json_ok(codebuddy_cn_ide::status())
}

pub(crate) async fn api_codebuddy_cn_ide_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // 可选：切换前把勾选会话复制到目标账号（与 /api/vscode-ext/switch 同形）。
    // 任一条目非法即整包拒绝（与 Tauri 侧 `Option<Vec<CopyItem>>` 的 serde 整包报错同形），
    // 避免「部分成功 + 静默丢弃」让用户误以为全部复制成功。
    let copy_items: Vec<vscode_session::CopyItem> = match body
        .get("copySessions")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .map(|item| serde_json::from_value::<vscode_session::CopyItem>(item.clone()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
    {
        Ok(items) => items.unwrap_or_default(),
        Err(error) => {
            return json_err(
                format!("copySessions 条目非法：{error}"),
                StatusCode::BAD_REQUEST,
            )
        }
    };
    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(body.get("syncSelections")) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    let result = if copy_items.is_empty() && sync_selections.is_empty() {
        codebuddy_cn_ide::switch_account(account_id, restart)
    } else {
        codebuddy_ide_session::switch_codebuddy_cn_ide_with_copy(
            account_id,
            restart,
            &copy_items,
            &sync_selections,
        )
    };
    match result {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// GET /api/codebuddy-cn-ide/sessions —— 当前 IDE 账号可复制的会话（未登录返回空列表）。
pub(crate) async fn api_codebuddy_cn_ide_sessions() -> Response {
    let result =
        tokio::task::spawn_blocking(codebuddy_ide_session::list_current_codebuddy_ide_sessions)
            .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/codebuddy-cn-ide/session-links —— 预览当前 IDE 账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `codebuddy_ide_session_links_preview` 同形：直接返回 core 的只读预览。
pub(crate) async fn api_codebuddy_cn_ide_session_links_preview(
    Json(body): Json<Value>,
) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let result = tokio::task::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        codebuddy_ide_session_sync::links_preview(&target)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_codebuddy_cn_ide_detect() -> Response {
    match codebuddy_cn_ide::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_vscode_ext_status() -> Response {
    json_ok(vscode_ext::status())
}

/// GET /api/vscode-ext/sessions —— 当前 VS Code 扩展账号可复制的会话（未登录返回空列表）。
pub(crate) async fn api_vscode_ext_sessions() -> Response {
    let result = tokio::task::spawn_blocking(|| match vscode_ext::active_ext_uid() {
        Some(uid) => vscode_session::list_vscode_sessions(&uid),
        None => json!({ "sourceUid": null, "sessions": [], "skipped": 0 }),
    })
    .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_vscode_ext_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // 默认重启（= 自动关闭并重开）：VS Code 运行时由后端先优雅退出再写入。
    // 显式传 restart=false 时退回「请先完全退出 VS Code」的手动模式。
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // 可选：切换前把勾选会话复制到目标账号（与 /api/vscode-ext/* 命名风格一致）。
    // 任一条目非法即整包拒绝（与 Tauri 侧 `Option<Vec<CopyItem>>` 的 serde 整包报错同形），
    // 避免「部分成功 + 静默丢弃」让用户误以为全部复制成功。
    let copy_items: Vec<vscode_session::CopyItem> = match body
        .get("copySessions")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .map(|item| serde_json::from_value::<vscode_session::CopyItem>(item.clone()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
    {
        Ok(items) => items.unwrap_or_default(),
        Err(error) => {
            return json_err(
                format!("copySessions 条目非法：{error}"),
                StatusCode::BAD_REQUEST,
            )
        }
    };

    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(body.get("syncSelections")) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };

    let result = if copy_items.is_empty() && sync_selections.is_empty() {
        vscode_ext::switch_account(account_id, restart)
    } else {
        vscode_session::switch_vscode_ext_with_copy(
            account_id,
            restart,
            &copy_items,
            &sync_selections,
        )
    };
    match result {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/vscode-ext/session-links —— 预览当前插件账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `vscode_session_links_preview` 同形：直接返回 core 的只读预览
/// （`supported` / `storeStatus` / `groups`），每组的 `defaultChecked` 与 `availableModes`
/// 是前端的勾选权限来源。
pub(crate) async fn api_vscode_ext_session_links_preview(Json(body): Json<Value>) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let result = tokio::task::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        vscode_session_sync::links_preview(&target)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_codebuddy_ide_status() -> Response {
    json_ok(codebuddy_ide::status())
}

/// POST /api/codebuddy-ide/switch —— 注入凭证到 CodeBuddy IDE（国际版），可选复制 / 同步会话。
///
/// `copySessions` / `syncSelections` 与国内版（`/api/codebuddy-cn-ide/switch`）同形：
/// 任一条目非法即整包拒绝，两者都为空时行为与纯切换逐字一致。
pub(crate) async fn api_codebuddy_ide_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let copy_items: Vec<vscode_session::CopyItem> = match body
        .get("copySessions")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .map(|item| serde_json::from_value::<vscode_session::CopyItem>(item.clone()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
    {
        Ok(items) => items.unwrap_or_default(),
        Err(error) => {
            return json_err(
                format!("copySessions 条目非法：{error}"),
                StatusCode::BAD_REQUEST,
            )
        }
    };
    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(body.get("syncSelections")) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    let result = if copy_items.is_empty() && sync_selections.is_empty() {
        codebuddy_ide::switch_account(account_id, restart)
    } else {
        codebuddy_ide_session::switch_codebuddy_intl_ide_with_copy(
            account_id,
            restart,
            &copy_items,
            &sync_selections,
        )
    };
    match result {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// GET /api/codebuddy-ide/sessions —— 当前国际版 IDE 账号可复制的会话（未登录返回空列表）。
pub(crate) async fn api_codebuddy_intl_ide_sessions() -> Response {
    let result =
        tokio::task::spawn_blocking(codebuddy_ide_session::list_current_intl_ide_sessions).await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/codebuddy-ide/session-links —— 预览当前国际版 IDE 账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `codebuddy_intl_ide_session_links_preview` 同形：直接返回 core 的只读预览。
pub(crate) async fn api_codebuddy_intl_ide_session_links_preview(
    Json(body): Json<Value>,
) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let result = tokio::task::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        codebuddy_ide_session_sync::links_preview_intl(&target)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_vscode_ext_detect() -> Response {
    match vscode_ext::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_codebuddy_ide_detect() -> Response {
    match codebuddy_ide::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_jetbrains_status() -> Response {
    json_ok(jetbrains::status())
}

pub(crate) async fn api_jetbrains_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // 可选：目标配置目录名列表（如 ["PyCharm2026.2"]）。缺省 / 空数组 = 全部装了插件的 IDE。
    let config_dirs: Option<Vec<String>> = body
        .get("configDirs")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|list| !list.is_empty());
    match jetbrains::switch_account(account_id, restart, config_dirs.as_deref()) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_jetbrains_detect() -> Response {
    match jetbrains::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}
