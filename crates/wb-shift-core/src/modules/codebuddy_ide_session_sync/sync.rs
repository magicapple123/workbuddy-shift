//! 显式同步：按 flavor 的 sync_selected 适配。

use super::*;

/// 执行勾选的同步项（两个 IDE 共用；国内版 / 国际版只差来源 uid 与运行态判定）。
///
/// 由切换编排调用（同步只随切换发生，没有独立的宿主入口）。
/// **前提**：调用方已确认 IDE 完全退出（写入会被运行中的客户端覆盖）；本函数自行复查一次。
pub(crate) fn sync_selected_for(
    flavor: IdeFlavor,
    target_acc: &Value,
    selections: &[SyncSelection],
) -> Result<Value, String> {
    if selections.is_empty() {
        return Ok(json!({ "synced": [], "skipped": [], "errors": [] }));
    }
    if flavor.is_running() {
        return Err(
            "检测到 CodeBuddy IDE 正在运行，请先完全退出后再同步会话，否则写入会被 CodeBuddy IDE 覆盖。"
                .to_string(),
        );
    }
    let root = codebuddy_ide_session::ide_data_root()
        .ok_or_else(|| "未找到 CodeBuddy IDE 数据目录，无法同步会话".to_string())?;
    let source_uid = active_source_uid(flavor)?;
    vscode_session_sync::sync_selected_in_for_variant(
        CODEBUDDY_IDE_STORE,
        &root,
        &SessionPaths::for_codebuddy_ide(),
        &source_uid,
        target_acc,
        selections,
        link_variant(flavor),
    )
}
