//! 会话复制：复制入口与按目标 uid 的复制实现。

use super::*;

/// 把勾选的会话复制到目标账号（默认沿用源会话 id，目标已有同 id 时才重随机）。返回复制报告。
///
/// 源端（数据根 + 当前登录 uid）由 [`copy_source`] 在关闭 IDE **之前**解析后传入；
/// 备份根固定为 `<工具存储根>/backups/<backup_kind>/<utc_iso>/`（供人工回滚）。
pub(crate) fn copy_codebuddy_ide_sessions(
    root: &std::path::Path,
    source_uid: &str,
    target_uid: &str,
    items: &[CopyItem],
) -> Result<Value, String> {
    let backup_root = backup_dir()
        .join(CODEBUDDY_IDE_STORE.backup_kind)
        .join(utc_iso());
    copy_codebuddy_ide_sessions_in(root, &backup_root, source_uid, target_uid, items)
}

/// 复制内核入口（源端与备份根已解析）：固定数据仓与复制策略，交给共用内核执行。
pub(crate) fn copy_codebuddy_ide_sessions_in(
    root: &std::path::Path,
    backup_root: &std::path::Path,
    source_uid: &str,
    target_uid: &str,
    items: &[CopyItem],
) -> Result<Value, String> {
    vscode_session::copy_sessions_in_with(
        CODEBUDDY_IDE_STORE,
        IDE_COPY,
        root,
        backup_root,
        source_uid,
        target_uid,
        items,
    )
}
