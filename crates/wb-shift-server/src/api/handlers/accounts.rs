//! accounts 域的 HTTP handler。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::*;

pub(crate) async fn api_status(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    let auth = auth_file::read_auth_file(variant);
    let current = auth.as_ref().map(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        json!({
            "uid": account::display_value(&acct, "uid"),
            "nickname": account::display_value(&acct, "nickname"),
            "email": account::display_value(&acct, "email"),
        })
    });
    json_ok(json!({
        "running": cached_workbuddy_running(variant),
        "authFile": variant.auth_file_path().to_string_lossy(),
        "current": current,
        "appPath": auth_file::workbuddy_app_path(variant).to_string_lossy(),
        "version": update::APP_VERSION,
        "variant": variant.as_str(),
    }))
}

/// GET /api/accounts —— 返回全部档位的账号，`current` 取请求档位的登录态。
pub(crate) async fn api_accounts(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    json_ok(json!({
        "accounts": account::load_accounts()
            .iter()
            .map(account::account_meta)
            .collect::<Vec<_>>(),
        "current": auth_file::read_auth_file(variant)
            .and_then(|a| a.get("account").and_then(|x| x.get("uid")).and_then(|x| x.as_str()).map(String::from)),
        "variant": variant.as_str(),
    }))
}

pub(crate) async fn api_delete(
    body: Result<Json<DeleteAccountRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match account::delete_account(&request.account_id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/update-account-display —— 更新账号本地展示字段（备注 / 显示选择）。
///
/// body：`{ accountId, patch }`，patch 可含 `note`（字符串或 null）与 `displayField`。
pub(crate) async fn api_update_account_display(
    body: Result<Json<UpdateAccountDisplayRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match account::update_account_display(&request.account_id, &request.patch.to_value()) {
        Ok(meta) => json_ok(json!({ "ok": true, "account": meta })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/import-local —— 导入本机当前账号（body 可选 `variant`，缺省国内版）。
///
/// body 允许缺失，保持改造前的调用方式可用。
pub(crate) async fn api_import_local(body: Option<Json<VariantRequest>>) -> Response {
    let variant = WbVariant::parse(
        body.as_ref()
            .and_then(|Json(request)| request.variant.as_deref()),
    );
    match account::import_local(variant) {
        Ok(acc) => json_ok(json!({ "ok": true, "account": acc })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

pub(crate) async fn api_export_accounts(
    body: Result<Json<ExportAccountsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::export_accounts(&request.account_ids) {
        Ok(records) => json_ok(json!({ "ok": true, "accounts": records })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_export_accounts_to_path(
    body: Result<Json<ExportAccountsToPathRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::export_accounts_to_path(&request.account_ids, &request.path) {
        Ok(path) => json_ok(json!({ "ok": true, "path": path })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_preview_import(
    body: Result<Json<ImportFileRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::preview_accounts(&request.file_text) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_import(
    body: Result<Json<ImportAccountsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    match export_import::import_accounts(&request.file_text, &request.indexes) {
        Ok(result) => json_ok(json!({
            "ok": true,
            "imported": result.imported,
            "skipped": result.skipped,
            "overwritten": result.overwritten,
        })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/accounts/export-encrypted —— 用口令加密导出**全部**账号到指定路径。
///
/// 口令策略、信封格式与原子写都在 core（`export_accounts_encrypted_at`），
/// 这里只转发参数与整形响应；错误文案与桌面端逐字一致。
/// 明文 `path` 会原样回给调用方（前端用它展示"已导出到…"），口令一概不回传。
///
/// 两处顺序/线程安排是有意的：
/// - **口令先问 core**（`validate_password`）再读账号库：无口令 / 短口令的无效请求
///   不该先把整份含 token 的账号库读进内存才被打回。判断仍只有 core 一处，
///   宿主不复制 8 位下限；
/// - **PBKDF2 600000 次 + 文件读写 + 账号库读取全部同步**，所以整段进
///   `spawn_blocking`（与 `commands::export_accounts_encrypted`、`api_vscode_ext_switch`
///   同一形态），否则会占住 tokio worker，同一进程的其它请求一起排队。
pub(crate) async fn api_export_accounts_encrypted(
    body: Result<Json<ExportAccountsEncryptedRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if let Err(error) = export_import::validate_password(&request.password) {
        return json_err(error, StatusCode::BAD_REQUEST);
    }
    let path = request.path;
    let password = request.password;
    let result = tokio::task::spawn_blocking(move || {
        let accounts = account::load_accounts();
        let count = accounts.len();
        export_import::export_accounts_encrypted_at(&path, &password, &accounts, config::now_ms())
            .map(|()| json!({ "ok": true, "path": path, "count": count }))
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/accounts/import-encrypted —— 用口令解密备份文件并按既有合并语义导入。
///
/// 口令同样先问 core 再看文件/账号库（7 位口令 + 不存在的路径必须报"至少 8 位"，
/// 而不是"文件不存在"）；解密 + 合并 + 写回全在 `spawn_blocking` 里跑。
pub(crate) async fn api_import_accounts_encrypted(
    body: Result<Json<ImportAccountsEncryptedRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if let Err(error) = export_import::validate_password(&request.password) {
        return json_err(error, StatusCode::BAD_REQUEST);
    }
    let path = request.path;
    let password = request.password;
    let result = tokio::task::spawn_blocking(move || {
        export_import::import_accounts_encrypted_at(&path, &password)
    })
    .await;
    match result {
        Ok(Ok(imported)) => json_ok(json!({
            "ok": true,
            "imported": imported.imported,
            "skipped": imported.skipped,
            "overwritten": imported.overwritten,
        })),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// ---------------------------------------------------------------------------
// OAuth 登录
// ---------------------------------------------------------------------------

/// POST /api/oauth/start —— 发起扫码登录（body 可选 `variant`，缺省国内版）。
pub(crate) async fn api_oauth_start(body: Option<Json<VariantRequest>>) -> Response {
    let variant = WbVariant::parse(
        body.as_ref()
            .and_then(|Json(request)| request.variant.as_deref()),
    );
    match oauth::oauth_start(variant).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/oauth/status —— 轮询采集结果（档位取发起时记录，无需传参）。
pub(crate) async fn api_oauth_status(
    body: Result<Json<OAuthStatusRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    json_ok(oauth::oauth_poll(&request.login_id).await)
}

// ---------------------------------------------------------------------------
// 切换
// ---------------------------------------------------------------------------

/// POST /api/switch —— 切换账号（备份 → 关进程 → 恢复/复制/同步会话 → 写认证 → 重启）。
///
/// 运行标记由**后台任务**持有：请求 future 被取消（浏览器断开）不会清标记，
/// 后台切换仍在写账号，第二次切换必须继续被 409 拒绝。详见 `state::SwitchState`。
pub(crate) async fn api_switch(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SwitchRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if request.account_id.trim().is_empty() {
        return json_err("缺少 accountId".to_string(), StatusCode::BAD_REQUEST);
    }
    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(request.sync_selections.as_ref()) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };

    if !state.switch.try_begin() {
        return json_err("已有切换任务进行中".to_string(), StatusCode::CONFLICT);
    }
    state.switch.set_progress("开始切换账号…");

    let progress_state = Arc::clone(&state.switch);
    let progress: switch::ProgressFn = Box::new(move |msg| progress_state.set_progress(msg));
    let task_state = Arc::clone(&state.switch);
    let result = tokio::task::spawn_blocking(move || {
        // 守卫必须在任务内部：只有切换真正结束（或 panic 展开）才清运行标记。
        let _run_guard = task_state.run_guard();
        switch::switch_account(
            Some(&progress),
            &request.account_id,
            request.restart,
            request.share_sessions,
            &request.copy_session_ids,
            &sync_selections,
        )
    })
    .await;

    match result {
        Ok(Ok(v)) => json_ok(v),
        Ok(Err(e)) => json_err(e, StatusCode::BAD_REQUEST),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(crate) async fn api_switch_progress(State(state): State<Arc<AppState>>) -> Response {
    json_ok(json!({
        "running": state.switch.is_running(),
        "progress": state.switch.progress(),
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    /// 向 router 发一个 POST（回环 Host、无 Origin＝与 webui 的同源请求一致）。
    ///
    /// 直接打 router 而不是调 handler：这样 DTO 反序列化、错误信封、路由注册三件事
    /// 都在同一条链路上被验证，缺一环这条用例就会红。
    async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:57910")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("构造测试请求");
        let response = crate::api::router(crate::api::shared_state())
            .oneshot(request)
            .await
            .expect("Router 作为 Service 应始终返回响应");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("读取响应体");
        (status, serde_json::from_slice(&bytes).expect("响应是 JSON"))
    }

    /// 加密导出的 DTO 字段是**必填**的：缺 `password` 在边界 400。
    ///
    /// 这条用例钉住的是「不写 `default`」这个选择：若 DTO 带 `default`，缺字段会变成
    /// 空串并继续走到 core，最后报「密码至少 8 位」——请求形状错误被伪装成用户输入错误。
    /// 边界拒绝也意味着这条路径**不会读账号库**。
    #[tokio::test]
    async fn export_encrypted_requires_password_at_the_boundary() {
        let (status, value) = post_json(
            "/api/accounts/export-encrypted",
            json!({ "path": "C:/tmp/encrypted.json" }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            value["ok"],
            json!(false),
            "错误信封必须是 {{ok:false,error}}"
        );
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("请求参数无效"),
            "应复用既有边界错误文案：{error}"
        );
        assert!(error.contains("password"), "缺字段名要能看出来：{error}");
        assert!(
            !error.contains("C:/tmp/encrypted.json"),
            "边界错误不得回显请求内容：{error}"
        );
    }

    /// `path` 与 `password` 一样必填，两条路由四组缺字段都要在边界 400。
    ///
    /// 缺字段若被降级成空串，请求形状错误就会伪装成用户输入错误：空 `password` 会得到
    /// 「密码至少 8 位」，空 `path` 会得到 core 的「必须使用绝对路径 / 缺少导入文件路径」，
    /// 排查时分不清是调用方写错了字段名还是用户填错了内容。边界拒绝还保证这条路径
    /// **不会读账号库**（反序列化先失败）。
    #[tokio::test]
    async fn encrypted_routes_require_both_fields_at_the_boundary() {
        let cases = [
            (
                "/api/accounts/export-encrypted",
                json!({ "password": "12345678" }),
                "path",
            ),
            (
                "/api/accounts/import-encrypted",
                json!({ "password": "12345678" }),
                "path",
            ),
            (
                "/api/accounts/import-encrypted",
                json!({ "path": "C:/tmp/encrypted.json" }),
                "password",
            ),
        ];

        for (uri, body, missing) in cases {
            let (status, value) = post_json(uri, body).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{uri} 缺 {missing} 必须是 400"
            );
            assert_eq!(value["ok"], json!(false), "{uri} 错误信封形状不符");
            let error = value["error"].as_str().unwrap_or_default();
            assert!(
                error.contains("请求参数无效"),
                "{uri} 应复用既有边界文案：{error}"
            );
            assert!(
                error.contains(missing),
                "{uri} 的边界错误要能看出缺哪个字段：{error}"
            );
            assert!(
                !error.contains("12345678"),
                "{uri} 边界错误不得回显请求里的口令：{error}"
            );
        }
    }

    /// 未知字段同样 400（`deny_unknown_fields`）：不给「多塞字段还能读」留模糊边界。
    #[tokio::test]
    async fn export_encrypted_rejects_unknown_fields() {
        let (status, value) = post_json(
            "/api/accounts/export-encrypted",
            json!({ "path": "C:/tmp/encrypted.json", "password": "12345678", "extra": 1 }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let error = value["error"].as_str().unwrap_or_default();
        assert!(error.contains("请求参数无效"), "未知字段必须被拒：{error}");
    }

    /// 口令策略经 HTTP 与桌面端同源：7 位口令直接被拒，且**在打开文件之前**。
    ///
    /// core 的 `import_accounts_encrypted_at` 第一步就是 `validate_password`，所以这里
    /// 既不会读磁盘也不会碰账号库——用例不依赖任何真实账号或临时文件。
    #[tokio::test]
    async fn import_encrypted_rejects_short_password_with_readable_error() {
        let (status, value) = post_json(
            "/api/accounts/import-encrypted",
            json!({ "path": "C:/does-not-exist/encrypted.json", "password": "1234567" }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(value["ok"], json!(false));
        let error = value["error"].as_str().unwrap_or_default();
        assert!(error.contains("至少 8 位"), "文案应明确 8 位下限：{error}");
        assert!(!error.contains("1234567"), "错误文案不得回显口令：{error}");
        assert!(
            !error.contains("不存在"),
            "口令不足时不该先去看文件：{error}"
        );
    }

    /// 校验顺序：**口令先于文件访问**（回归 P2）。
    ///
    /// 路径指向一个不存在的文件，口令只有 7 位。若先访问文件，用户会拿到
    /// 「导入文件不存在」——把一个"口令没填对"的错误指向了文件，排查方向直接跑偏。
    /// 断言必须报 8 位下限、且文案里不出现「不存在」，才算顺序正确。
    #[tokio::test]
    async fn import_encrypted_validates_password_before_touching_the_file() {
        let missing = "C:/definitely-not-here/wb-shift-encrypted-backup.json";
        let (status, value) = post_json(
            "/api/accounts/import-encrypted",
            json!({ "path": missing, "password": "1234567" }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("至少 8 位"),
            "口令不足必须优先报出，而不是文件问题：{error}"
        );
        assert!(
            !error.contains("不存在"),
            "7 位口令不该走到打开文件那一步：{error}"
        );
        assert!(
            !error.contains(missing) && !error.contains("wb-shift-encrypted-backup"),
            "错误文案不得回显路径：{error}"
        );
    }
}
