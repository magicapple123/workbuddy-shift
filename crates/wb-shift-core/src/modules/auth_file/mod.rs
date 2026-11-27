//! 官方认证文件 `workbuddy-desktop.info` 的路径与读写（四段 JSON）。
//!
//! 对照 server.py `auth_file_path` / `workbuddy_app_path` / `read_auth_file` /
//! `import_from_auth_file`。切换写入（build_account_obj / build_auth_obj /
//! write_account_to_auth_file）在阶段 2 随 switch.rs 落地。

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

use crate::modules::account::{get_str, secret_value};
use crate::modules::config::{atomic_write, backup_dir, now_ms, utc_iso};
use crate::modules::variant::WbVariant;

/// WorkBuddy 应用路径（按档位）。
pub fn workbuddy_app_path(variant: WbVariant) -> PathBuf {
    #[cfg(target_os = "macos")]
    return crate::modules::process::macos_workbuddy_app_path(variant);

    #[cfg(target_os = "windows")]
    {
        // 探测顺序：运行进程 Path → 缓存 → 注册表 → 环境变量/盘符扫描。
        // 都找不到时返回 LOCALAPPDATA 默认路径，供启动失败文案写出尝试路径。
        if let Some(exe) = crate::modules::process::windows_workbuddy_exe_path(variant) {
            return exe;
        }
        crate::modules::process::windows_default_app_path(variant)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return variant.linux_app_path();
}

mod read;
mod sync;

pub use read::{
    backup_auth_file, build_account_obj, build_auth_obj, import_from_auth_file, read_auth_file,
    read_auth_file_at,
};
// 凭据写入互斥与切号写回（issue #202）：两个入口（切号 / 令牌刷新）都从这里取锁，
// 保证「读快照 → 写回」与「刷新 → 回写」不会交叉覆盖。
pub use sync::{
    ensure_snapshot_is_current, try_acquire_credential_write_lock, write_account_to_auth_file,
    write_back_credentials, CredentialFreshness, CREDENTIAL_LOCK_BUSY_MESSAGE,
    CREDENTIAL_LOCK_FILE, CREDENTIAL_LOCK_UNAVAILABLE_PREFIX,
};
// 「这是不是当前登录账号」的口径与令牌刷新侧共用（T01），因此只放到 crate 可见，
// 不进公开 API：对外暴露账号身份判定没有调用方，也会把内部口径锁死。
pub(crate) use sync::same_account;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::read::*;
    use super::sync::*;
    include!("tests/read.rs");
    include!("tests/sync.rs");
}
