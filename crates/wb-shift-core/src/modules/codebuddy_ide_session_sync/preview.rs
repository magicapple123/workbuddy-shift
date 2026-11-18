//! 链接预览：CN/国际版链接预览入口与 flavor 过滤。

use super::*;

/// 预览「当前国内版 IDE 账号 → 目标账号」可同步的关联会话（只读，不写任何会话正文）。
pub fn links_preview(target_acc: &Value) -> Result<Value, String> {
    links_preview_for(IdeFlavor::Cn, target_acc)
}

/// 预览「当前国际版 IDE 账号 → 目标账号」可同步的关联会话（语义与国内版一致）。
pub fn links_preview_intl(target_acc: &Value) -> Result<Value, String> {
    links_preview_for(IdeFlavor::Intl, target_acc)
}

pub(super) fn links_preview_for(flavor: IdeFlavor, target_acc: &Value) -> Result<Value, String> {
    let root = codebuddy_ide_session::ide_data_root()
        .ok_or_else(|| "未找到 CodeBuddy IDE 数据目录，无法同步会话".to_string())?;
    let source_uid = active_source_uid(flavor)?;
    // 共用 `codebuddy_ide_session_links.json`：按组的 variant 过滤，国内版组不进国际版预览。
    vscode_session_sync::links_preview_in_for_variant(
        CODEBUDDY_IDE_STORE,
        &root,
        &SessionPaths::for_codebuddy_ide(),
        &source_uid,
        target_acc,
        link_variant(flavor),
    )
}
