//! 会话列举：按 flavor/uid 枚举 IDE 会话。

use super::paths::*;
use super::*;

/// 列出某账号可复制的 IDE 会话（按工作区 hash 分桶）。
///
/// 未找到数据根时 `dataRoot` 为 `null`，调用方据此区分「未找到数据目录」与「该账号无会话」。
pub fn list_codebuddy_ide_sessions(uid: &str) -> Value {
    match ide_data_root() {
        Some(root) => vscode_session::list_sessions_in_store(CODEBUDDY_IDE_STORE, &root, uid),
        None => empty_session_list(Some(uid), None),
    }
}

/// 列出**当前国内版 IDE 登录账号**可复制的会话（宿主列表入口）。
pub fn list_current_codebuddy_ide_sessions() -> Value {
    list_current_ide_sessions(IdeFlavor::Cn)
}

/// 列出**当前国际版 IDE 登录账号**可复制的会话（宿主列表入口）。
pub fn list_current_intl_ide_sessions() -> Value {
    list_current_ide_sessions(IdeFlavor::Intl)
}

/// 列出当前 IDE 登录账号可复制的会话。
///
/// 未登录 / uid 非法时 `sourceUid` 为 `null`；数据根不存在时 `dataRoot` 为 `null`。
/// 两个字段独立出现，前端据此区分「请先登录」与「未找到数据目录」。
pub(super) fn list_current_ide_sessions(flavor: IdeFlavor) -> Value {
    match flavor.active_uid() {
        Some(uid) => list_codebuddy_ide_sessions(&uid),
        None => empty_session_list(None, ide_data_root().as_deref()),
    }
}
