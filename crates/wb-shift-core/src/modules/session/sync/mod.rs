//! 会话同步的只读预览与写入计划（design §5 / §6）。
//!
//! 预览只读双方共同参与的关联组，核对身份、成员、基线与正文后下发一次性预览凭据；
//! 计划阶段重新加载全部输入并逐项核对凭据与内容版本，任何变化都跳过该项而不是猜测。
//! 本模块不做任何写入：备份与阶段化执行在 `exec`，恢复在 `recovery`。

//
// 子模块职责：`preview` 只读预览、`plan` 写入计划、`wait` 归档 / 结束态判定、
// `liveness` 代表成员口径与会话行存活判定；
// 本模块不做任何写入：备份与阶段化执行在 `exec`，恢复在 `recovery`。

use super::copy::*;
use super::*;

mod liveness;
mod plan;
mod preview;
mod wait;

// 组内存活成员判定：管理页列表过滤与会话组批量清理（`session_groups`）共用同一口径。
pub(crate) use liveness::group_any_alive;
pub(in crate::modules::session) use plan::*;
pub(in crate::modules::session) use preview::{
    live_preview_binding, member_row_owned_by, session_row_info,
};
pub use preview::{session_links_preview, session_links_preview_cross};
// 单测注入入口：生产路径走公开预览入口，这里只为 `session::tests` 转出。
#[allow(unused_imports)]
pub(in crate::modules::session) use preview::{
    session_links_preview_at, session_links_preview_cross_at,
};
pub(in crate::modules::session) use wait::*;

/// 会话同步能力不足时的错误文案。
pub const SESSION_SYNC_UNSUPPORTED: &str = "该档位暂不支持会话同步";

/// 预览凭据过期/失配时的原因码（design §5.2：不继承用户旧选择）。
pub const REASON_PREVIEW_STALE: &str = "previewStale";

/// 预览项与预览绑定里的归档动作取值。
pub(super) const ARCHIVE_ACTION_STATUS_ONLY: &str = "statusOnly";

/// 一条 `syncSelections` 入参（取代只传 `syncLinkIds`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSelection {
    pub group_id: String,
    pub preview_token: String,
    pub mode: SyncMode,
}

impl SyncSelection {
    /// 解析单条入参；缺字段或未知模式直接拒绝（不静默跳过、不回落默认值）。
    pub fn parse(value: &Value) -> Result<Self, String> {
        let group_id = value
            .get("groupId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "同步选择项缺少 groupId".to_string())?;
        let preview_token = value
            .get("previewToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| format!("同步选择项缺少 previewToken（组 {group_id}）"))?;
        let mode = value
            .get("mode")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("同步选择项缺少 mode（组 {group_id}）"))?;
        Ok(Self {
            group_id: group_id.to_string(),
            preview_token: preview_token.to_string(),
            mode: SyncMode::parse(mode)?,
        })
    }
}

/// 解析 `syncSelections`；缺省或 null 视为未勾选同步。
pub fn parse_sync_selections(value: Option<&Value>) -> Result<Vec<SyncSelection>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = value.as_array() else {
        return Err("syncSelections 必须是数组".to_string());
    };
    items.iter().map(SyncSelection::parse).collect()
}

/// 成员当前正文状态；`projects/` 下找不到正文一律按 Missing（不当作空正文）。
pub(crate) fn member_content_state(paths: &SessionPaths, session_id: &str) -> ContentState {
    match find_project_jsonl(paths, session_id) {
        Some(path) => session_link::read_content_snapshot(&path, session_id),
        None => ContentState::Missing,
    }
}
