//! 会话列举：按 store/uid 枚举工作区会话与历史判定。

#[allow(unused_imports)]
use super::copy::*;
#[allow(unused_imports)]
use super::index_merge::*;
#[allow(unused_imports)]
use super::paths::*;
#[allow(unused_imports)]
use super::remap::*;
use super::*;

// ---------------------------------------------------------------------------
// 会话枚举
// ---------------------------------------------------------------------------

/// 列出某账号可复制的会话（按工作区 hash 分桶）。
///
/// 返回 `{ sourceUid, sessions:[{id, workspaceHash, title, updatedAt, type, hasHistory}], skipped, dataRoot }`。
/// `skipped` 为无法解析（损坏）的工作区索引数量；`dataRoot` 为解析到的扩展数据根目录
/// （找不到时为 `null`，调用方据此区分「未找到数据目录」与「该账号无会话」）。
pub fn list_vscode_sessions(uid: &str) -> Value {
    match ext_data_root() {
        Some(root) => list_sessions_in(&root, uid),
        None => json!({
            "sourceUid": uid,
            "sessions": [],
            "skipped": 0,
            "dataRoot": Value::Null,
        }),
    }
}

/// [`list_vscode_sessions`] 的可测实现：显式传入数据根目录（VS Code 插件数据仓）。
pub fn list_sessions_in(root: &Path, uid: &str) -> Value {
    list_sessions_in_store(VSCODE_STORE, root, uid)
}

/// 列出某数据仓下某账号可复制的会话（按工作区 hash 分桶）。
///
/// 返回 `{ sourceUid, sessions:[{id, workspaceHash, title, updatedAt, type, hasHistory}], skipped, dataRoot }`。
/// `skipped` 为无法解析（损坏）的工作区索引数量；`dataRoot` 为解析到的扩展数据根目录
/// （找不到时为 `null`，调用方据此区分「未找到数据目录」与「该账号无会话」）。
///
/// **多项目**（JetBrains）：遍历起点是该账号下**每个项目**的 `history/`，因此每行额外带
/// `projectKey`（客户端段之后那一级目录名）与 `projectPath`（项目路径，解码不出来为空串）。
/// `workspaceHash` 保持原义（= 该项目的会话桶目录名，本机安装级常量），前端字段不用改。
/// 同一会话可能同时出现在「无项目」桶与「项目」桶里，靠 `projectKey` 区分。
pub fn list_sessions_in_store(spec: SessionStoreSpec, root: &Path, uid: &str) -> Value {
    let mut sessions: Vec<Value> = Vec::new();
    let mut skipped = 0usize;
    let data_root = root.to_string_lossy().to_string();

    if !is_safe_uid(uid) {
        return json!({
            "sourceUid": uid,
            "sessions": sessions,
            "skipped": skipped,
            "dataRoot": data_root,
        });
    }

    for scope in history_scopes_in(spec, root, uid) {
        let history = &scope.history_root;
        // 目录不存在 = 该账号在这个项目下没有会话；读不到（权限 / 并发删除）同样跳过。
        if !history.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(history) else {
            continue;
        };
        for entry in entries.flatten() {
            let ws_dir = entry.path();
            if !ws_dir.is_dir() {
                continue;
            }
            let workspace_hash = entry.file_name().to_string_lossy().to_string();
            let Some(index) = read_json(&ws_dir.join("index.json")) else {
                skipped += 1;
                continue;
            };
            let Some(conversations) = index.get("conversations").and_then(Value::as_array) else {
                skipped += 1;
                continue;
            };
            for conversation in conversations {
                let Some(id) = conversation
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let title = conversation
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("(无标题)")
                    .to_string();
                let last = time_to_ms(conversation.get("lastMessageAt"));
                let updated_at = if last > 0 {
                    last
                } else {
                    time_to_ms(conversation.get("createdAt"))
                };
                let conv_type = conversation
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let has_history = conversation_has_history(&ws_dir, id);
                let mut row = json!({
                    "id": id,
                    "workspaceHash": workspace_hash,
                    "title": title,
                    "updatedAt": updated_at,
                    "type": conv_type,
                    "hasHistory": has_history,
                });
                // 只有「客户端段之后是项目 key」的数据仓才带项目字段：
                // VS Code / CodeBuddy IDE 的载荷保持逐字不变（那一级恒为账号 uid）。
                if spec.scope == ScopeSegment::ProjectKey {
                    row["projectKey"] = json!(scope.project_key);
                    row["projectPath"] = json!(decode_project_key(&scope.project_key));
                }
                sessions.push(row);
            }
        }
    }

    sessions.sort_by(|left, right| {
        let left_at = left.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
        let right_at = right.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
        right_at.cmp(&left_at)
    });

    json!({
        "sourceUid": uid,
        "sessions": sessions,
        "skipped": skipped,
        "dataRoot": data_root,
    })
}

/// 会话是否含正文（`index.json` 有 messages，或磁盘 `messages/` 下有文件）。
pub(super) fn conversation_has_history(ws_dir: &Path, conv_id: &str) -> bool {
    let conv_dir = ws_dir.join(conv_id);
    if let Some(index) = read_json(&conv_dir.join("index.json")) {
        if index
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| !messages.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
    }
    match std::fs::read_dir(conv_dir.join("messages")) {
        Ok(entries) => entries.flatten().any(|entry| entry.path().is_file()),
        Err(_) => false,
    }
}
