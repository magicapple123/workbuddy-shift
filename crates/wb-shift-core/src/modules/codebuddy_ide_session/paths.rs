//! IDE 数据根与复制源：数据根解析、空列表与复制来源定位。

use super::*;

/// IDE 会话树的数据根（`…/CodeBuddyExtension/Data`）；目录不存在时 `None`。
pub fn ide_data_root() -> Option<PathBuf> {
    vscode_session::store_data_root(CODEBUDDY_IDE_STORE)
}

/// 空列表的固定形状：始终带 `sourceUid` / `sessions` / `skipped` / `dataRoot`。
pub(super) fn empty_session_list(uid: Option<&str>, data_root: Option<&std::path::Path>) -> Value {
    json!({
        "sourceUid": uid,
        "sessions": [],
        "skipped": 0,
        "dataRoot": data_root.map(|path| path.to_string_lossy().into_owned()),
    })
}

/// 解析复制源端：数据根 + 当前 IDE 登录账号 uid。
///
/// 两个都会在关闭 IDE **之前**校验，避免「注定失败却已关掉用户的 IDE」。
pub(crate) fn copy_source(flavor: IdeFlavor) -> Result<(PathBuf, String), String> {
    let root = ide_data_root().ok_or_else(|| {
        "未找到 CodeBuddy IDE 数据目录，无法复制会话。请先打开 CodeBuddy IDE 并登录一次。"
            .to_string()
    })?;
    let uid = flavor.active_uid().ok_or_else(|| {
        "未检测到 CodeBuddy IDE 当前登录账号，无法定位源会话。请先在 CodeBuddy IDE 中登录后重试。"
            .to_string()
    })?;
    Ok((root, uid))
}
