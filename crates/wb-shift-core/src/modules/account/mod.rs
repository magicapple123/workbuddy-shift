//! 账号存储：读取/写入 `~/.workbuddy-shift/accounts.json`，与 Python 版共享数据目录。
//!
//! 对照 server.py `load_accounts` / `save_accounts` / `find_account` /
//! `account_display_name` / `account_meta`。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

use crate::modules::config::{accounts_file, atomic_write, now_ms};
use crate::modules::variant::WbVariant;

/// 判断字段是否为 WorkBuddy 5.6 加密信封对象（`{$wbEncrypted, envelope}`）。
///
/// 导出导入预览与采集共用同一判定，不要在调用方复制实现。
pub fn is_envelope(v: &Value, key: &str) -> bool {
    matches!(v.get(key), Some(Value::Object(map)) if map.contains_key("$wbEncrypted"))
}

/// 是否持有未过期的明文 access_token（OAuth 扫码所得形态）。
/// 无 expiresAt 时视为有效（保守：不因缺字段丢弃明文凭据）。
fn has_unexpired_plain_token(acc: &Value) -> bool {
    let Some(Value::String(s)) = acc.get("access_token") else {
        return false;
    };
    if s.trim().is_empty() {
        return false;
    }
    match acc.get("expiresAt").and_then(|v| v.as_i64()) {
        Some(exp) => exp > now_ms(),
        None => true,
    }
}

// 手动添加账号（token 方式）已随 UI 入口「手动添加」一并下线；
// `identity_email` 中的 "手动添加" 占位过滤保留，用于兼容历史手动添加的旧账号。

mod display;
mod identity;
mod store;

pub use display::{
    account_display_name, account_meta, build_auth_headers, display_value, envelope_token_error,
    get_str, mark_meta, update_account_display, update_account_display_in, MARK_COLORS,
    MARK_TEXT_MAX_CHARS,
};
pub use identity::{
    enterprise_id_of, is_enterprise_account, save_collected_account, secret_value,
    upsert_collected_account, variant_of,
};
pub use store::{
    accounts_file_in, delete_account, find_account, import_local, load_accounts, load_accounts_at,
    save_accounts, upsert_account,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::display::*;
    use super::store::*;
    include!("tests/store.rs");
    include!("tests/display.rs");
}
