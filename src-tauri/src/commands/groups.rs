//! 关联会话组视图、同步、成员增删与编辑器重启守卫。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::Value;
use wb_shift_core::modules::session_groups;

#[tauri::command(rename_all = "camelCase")]
pub async fn list_session_groups(
    client: String,
    variant_scope: Option<String>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || session_groups::list(client, scope))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_session_group(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || session_groups::detail(client, scope, &group_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn preview_session_group_pair(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    source_member_id: String,
    target_member_id: String,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::preview_pair(
            client,
            scope,
            &group_id,
            &source_member_id,
            &target_member_id,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[allow(clippy::too_many_arguments)] // 与 core 内核同口径：参数都是本次同步的显式输入
#[tauri::command(rename_all = "camelCase")]
pub async fn sync_session_group_pair(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    source_member_id: String,
    target_member_id: String,
    preview_token: String,
    mode: String,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::sync_pair(
            client,
            scope,
            &group_id,
            &source_member_id,
            &target_member_id,
            &preview_token,
            &mode,
            restart,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn sync_session_group_unify(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    source_member_id: String,
    targets: Vec<session_groups::GroupUnifyTarget>,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::sync_unify_batch(
            client,
            scope,
            &group_id,
            &source_member_id,
            &targets,
            restart,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn sync_session_group_safe_batch(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::sync_safe_batch(client, scope, &group_id, restart)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 跨组安全批量同步：把当前客户端下所有可安全快进的副本一次同步完。
///
/// `group_ids` 由前端从列表载荷筛出（`summaryStatus == "behind"`），后端逐组重算来源与
/// 复核；分歧组不猜、整组跳过并在报告的 `skippedGroups` 里给出原因。`restart` 缺省 false：
/// 插件侧运行中需用户确认（确认框授权）后传 true，由后端关闭并在结束后重开一次宿主。
#[tauri::command(rename_all = "camelCase")]
pub async fn sync_all_session_groups_safe(
    client: String,
    variant_scope: Option<String>,
    group_ids: Vec<String>,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::sync_all_safe(client, scope, &group_ids, restart)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn add_session_group_member(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    source_member_id: String,
    target_account_id: String,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::add_member(
            client,
            scope,
            &group_id,
            &source_member_id,
            &target_account_id,
            restart,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 编辑器 / IDE：把来源账号的勾选会话复制到目标账号并登记关联（无现成组则新建关联组）。
///
/// `restart` 缺省 false：VS Code 运行中需用户确认（确认框授权）后传 true，由后端关闭并重开。
/// `variantScope` 只有 CodeBuddy IDE 需要（`cn` / `ai` 决定运行态判定与关联组的档位）：
/// 缺省时后端直接拒绝，不会替用户猜一个档位。
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_linked_sessions(
    client: String,
    variant_scope: Option<String>,
    source_account_id: String,
    target_account_id: String,
    session_ids: Vec<String>,
    restart: Option<bool>,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    let restart = restart.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::copy_linked_sessions(
            client,
            scope,
            &source_account_id,
            &target_account_id,
            &session_ids,
            restart,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 预检：是否需要关闭 VS Code 才能安全执行写操作（运行中 且 目标账号含当前登录账号）。
///
/// 供前端决定「是否先弹确认框」；最终把关仍在各写入口的实时判定。
#[tauri::command(rename_all = "camelCase")]
pub fn vscode_restart_precheck(target_account_ids: Vec<String>) -> Value {
    session_groups::vscode_restart_precheck(&target_account_ids)
}

/// 预检：是否需要关闭 JetBrains IDE 才能安全执行写操作。
///
/// 与 VS Code 版同形（前端共用一套确认框）；JetBrains 的判定与目标账号无关——只要还有
/// 装了插件且在运行的实例就得先关闭（IDEA / PyCharm 共用同一份数据目录）。
#[tauri::command(rename_all = "camelCase")]
pub fn jetbrains_restart_precheck(target_account_ids: Vec<String>) -> Value {
    session_groups::jetbrains_restart_precheck(&target_account_ids)
}

/// 取消关联：从会话组移除一个成员，只解除管理关系，不删除账号内的会话内容。
#[tauri::command(rename_all = "camelCase")]
pub async fn unlink_session_group_member(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
    member_id: String,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::remove_member(client, scope, &group_id, &member_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 删除会话组：组内所有成员一起解除关联，只解除管理关系，不删除账号内的会话内容。
#[tauri::command(rename_all = "camelCase")]
pub async fn delete_session_group(
    client: String,
    variant_scope: Option<String>,
    group_id: String,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::delete_group(client, scope, &group_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 批量清理失效关联组：组内**所有**成员指向的会话行都已不存活时才删整组。
///
/// `dry_run = true` 只统计不写盘（前端先用它取数量再弹确认框）；`dry_run = false` 会先备份
/// 关联记录再写盘。判定不可用时拒绝执行（破坏性操作不 fail-open）。
#[tauri::command(rename_all = "camelCase")]
pub async fn prune_session_groups(
    client: String,
    variant_scope: Option<String>,
    dry_run: bool,
) -> Result<Value, String> {
    let client = session_groups::SessionClient::parse(&client)?;
    let scope = session_groups::parse_variant_scope(variant_scope.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        session_groups::prune_dead_groups(client, scope, dry_run)
    })
    .await
    .map_err(|error| error.to_string())?
}

// ---------------------------------------------------------------------------
// 阶段 3：签到 + token 刷新
// ---------------------------------------------------------------------------
