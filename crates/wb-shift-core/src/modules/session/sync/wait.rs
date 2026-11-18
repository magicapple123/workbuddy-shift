//! 归档状态与来源结束判定：读取 sessions.status，决定归档授权与末条 incomplete 归一。
use super::*;

/// 会话行 `status` 的读取结果（区分「缺列」与「列为 NULL」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::modules::session) enum SessionStatus {
    /// 会话表没有 `status` 列（旧库）：不支持状态读写，但正文同步照旧可用。
    Unsupported,
    /// 行不存在或已软删除：没有可读写状态的对象。
    Absent,
    /// 行存在且未删除；`None` 表示 `status` 为 NULL。
    Present(Option<String>),
}

/// 读取会话行的 `status`（行必须存在且未软删除）。
///
/// 读失败按 Err 上抛：状态是写入门禁，读不出来就不能声称「不具备归档资格」。
pub(in crate::modules::session) fn read_member_status(
    paths: &SessionPaths,
    cid: &str,
) -> Result<SessionStatus, String> {
    let conn = open_db(&paths.workbuddy_db(), true)
        .ok_or_else(|| "会话数据无法打开，无法读取会话状态".to_string())?;
    if !table_exists(&conn, "sessions") {
        return Err("会话数据缺少数据表，无法读取会话状态".to_string());
    }
    if !column_exists(&conn, "sessions", "status") {
        return Ok(SessionStatus::Unsupported);
    }
    let row: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT status, deleted_at FROM sessions WHERE id = ?1",
            [cid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("会话状态读取失败：{error}"))?;
    match row {
        // 行不存在或已软删除：与 Absent 同义。
        None | Some((_, Some(_))) => Ok(SessionStatus::Absent),
        Some((status, None)) => Ok(SessionStatus::Present(status)),
    }
}

/// 单向粘滞归档资格：来源已归档，且目标处于允许被归档的终态。
///
/// 不反向传导取消归档；缺列、NULL、未知值与活跃态一律不授权。
pub(in crate::modules::session) fn archive_qualifies(
    source: &SessionStatus,
    target: &SessionStatus,
) -> bool {
    let SessionStatus::Present(Some(source_status)) = source else {
        return false;
    };
    if source_status != SESSION_STATUS_ARCHIVED {
        return false;
    }
    let SessionStatus::Present(Some(target_status)) = target else {
        return false;
    };
    ARCHIVE_TARGET_TERMINAL_STATUSES.contains(&target_status.as_str())
}

/// 来源会话是否「已经结束」到可以把末条 `incomplete` 归一的程度。
///
/// 只有 [`SETTLED_ROW_STATUSES`]（终态与归档态）算结束——与复制里「副本行原样保留」
/// 用的是同一份集合，两处口径不分开维护。其余情况（`working` / `active` / `Pending` /
/// 未知值 / 缺 `status` 列 / 行缺失 / 读失败）**一律不归一**：同步也可能被用来镜像一个
/// 正在运行的会话，那时的 `incomplete` 是真实状态，改了就是篡改内容；读不出状态时宁可
/// 不动，也不猜。
pub(in crate::modules::session) fn source_settled_for_normalize(
    paths: &SessionPaths,
    cid: &str,
) -> bool {
    match read_member_status(paths, cid) {
        Ok(SessionStatus::Present(Some(status))) => SETTLED_ROW_STATUSES
            .iter()
            .any(|settled| status.trim().eq_ignore_ascii_case(settled)),
        Ok(SessionStatus::Present(None) | SessionStatus::Absent | SessionStatus::Unsupported)
        | Err(_) => false,
    }
}

/// 读取双方会话行状态并算出本次的归档动作；读取失败一律按「不授权」处理，
/// 不改变既有正文判定的结果（状态同步是附加能力，不是正文同步的前置条件）。
pub(in crate::modules::session) fn preview_archive_state(
    source_paths: &SessionPaths,
    source_session_id: &str,
    target_paths: &SessionPaths,
    target_session_id: &str,
) -> PreviewArchiveState {
    let source =
        read_member_status(source_paths, source_session_id).unwrap_or(SessionStatus::Absent);
    let target =
        read_member_status(target_paths, target_session_id).unwrap_or(SessionStatus::Absent);
    let source_status = match &source {
        SessionStatus::Present(status) => status.clone(),
        _ => None,
    };
    let target_status = match &target {
        SessionStatus::Present(status) => status.clone(),
        _ => None,
    };
    let action =
        archive_qualifies(&source, &target).then(|| ARCHIVE_ACTION_STATUS_ONLY.to_string());
    PreviewArchiveState {
        source_status,
        target_status,
        action,
    }
}

/// 预览时记入凭据的归档状态（双方 status 与本次授权的归档动作）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::modules::session) struct PreviewArchiveState {
    pub(in crate::modules::session) source_status: Option<String>,
    pub(in crate::modules::session) target_status: Option<String>,
    /// `Some("statusOnly")` 或 None。
    pub(in crate::modules::session) action: Option<String>,
}
