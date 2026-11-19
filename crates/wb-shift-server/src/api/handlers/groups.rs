//! groups 域的 HTTP handler。

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::*;

/// 解析「客户端 + 档位范围」共同前缀。空 `client` 保留改造前的 `缺少 client` 文案。
fn client_scope(
    client: &str,
    variant_scope: Option<&str>,
) -> Result<(session_groups::SessionClient, Option<WbVariant>), String> {
    if client.trim().is_empty() {
        return Err("缺少 client".to_string());
    }
    let client = session_groups::SessionClient::parse(client)?;
    let scope = session_groups::parse_variant_scope(variant_scope)?;
    Ok((client, scope))
}

pub(crate) async fn api_list_session_groups(
    body: Result<Json<SessionGroupListRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::list(client, scope) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_get_session_group(
    body: Result<Json<SessionGroupRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::detail(client, scope, &request.group_id) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_preview_session_group_pair(
    body: Result<Json<SessionGroupPairPreviewRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::preview_pair(
        client,
        scope,
        &request.group_id,
        &request.source_member_id,
        &request.target_member_id,
    ) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_sync_session_group_pair(
    body: Result<Json<SessionGroupPairSyncRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::sync_pair(
        client,
        scope,
        &request.group_id,
        &request.source_member_id,
        &request.target_member_id,
        &request.preview_token,
        &request.mode,
        request.restart,
    ) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_sync_session_group_unify(
    body: Result<Json<SessionGroupUnifyRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    // 目标副本的结构与业务校验留在 core（桌面端同一份实现、同一套错误文案）。
    let targets = match serde_json::from_value::<Vec<session_groups::GroupUnifyTarget>>(
        serde_json::Value::Array(request.targets),
    ) {
        Ok(value) => value,
        Err(error) => {
            return json_err(
                format!("目标副本参数无效：{error}"),
                StatusCode::BAD_REQUEST,
            )
        }
    };
    match session_groups::sync_unify_batch(
        client,
        scope,
        &request.group_id,
        &request.source_member_id,
        &targets,
        request.restart,
    ) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_sync_session_group_safe_batch(
    body: Result<Json<SessionGroupSyncSafeRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::sync_safe_batch(client, scope, &request.group_id, request.restart) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_add_session_group_member(
    body: Result<Json<SessionGroupAddMemberRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::add_member(
        client,
        scope,
        &request.group_id,
        &request.source_member_id,
        &request.target_account_id,
        request.restart,
    ) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_copy_linked_sessions(
    body: Result<Json<CopyLinkedSessionsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, _scope) = match client_scope(&request.client, None) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::copy_linked_sessions(
        client,
        &request.source_account_id,
        &request.target_account_id,
        &request.session_ids,
        request.restart,
    ) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_vscode_restart_precheck(
    body: Result<Json<VscodeRestartPrecheckRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    json_ok(session_groups::vscode_restart_precheck(
        &request.target_account_ids,
    ))
}

/// JetBrains 侧的运行中预检（与 VS Code 版同形，供前端复用同一套确认框）。
pub(crate) async fn api_jetbrains_restart_precheck(
    body: Result<Json<JetbrainsRestartPrecheckRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    json_ok(session_groups::jetbrains_restart_precheck(
        &request.target_account_ids,
    ))
}

/// 取消关联：只解除管理关系，不触碰账号内的会话内容。
pub(crate) async fn api_unlink_session_group_member(
    body: Result<Json<SessionGroupUnlinkRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::remove_member(client, scope, &request.group_id, &request.member_id) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// 删除会话组：组内所有成员一起解除关联，只解除管理关系，不触碰账号内的会话内容。
pub(crate) async fn api_delete_session_group(
    body: Result<Json<SessionGroupRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::delete_group(client, scope, &request.group_id) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// 批量清理失效关联组：组内所有成员指向的会话行都已不存活时才删整组。
///
/// `dryRun` 只统计不写盘；真删会先备份关联记录。判定不可用时 core 返回可读错误（不 fail-open）。
pub(crate) async fn api_prune_session_groups(
    body: Result<Json<SessionGroupPruneRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let (client, scope) = match client_scope(&request.client, request.variant_scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::prune_dead_groups(client, scope, request.dry_run) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 签到 / 保活
// ---------------------------------------------------------------------------
