//! VS Code 内 CodeBuddy 扩展（`tencent-cloud.coding-copilot`）账号切换。
//!
//! 复用 WorkBuddy 账号库中的 CN token（www.codebuddy.cn），写入 VS Code 用户数据目录
//! （`%APPDATA%\Code` / `~/Library/Application Support/Code` / `$XDG_CONFIG_HOME/Code`）
//! 下 `User/globalStorage/state.vscdb` 的 Safe Storage secret：
//! `secret://{"extensionId":"tencent-cloud.coding-copilot","key":"Tencent-Cloud.coding-copilot.new.accessToken"}`。
//!
//! 与 CodeBuddy CN IDE（`codebuddy_cn_ide`）共用同一套 Safe Storage 加解密流程，
//! 仅目标描述符不同。写入前 VS Code 必须完全退出（主进程把 ItemTable 全量缓存在内存，
//! 运行中写入读不到也留不住）：`restart = true` 时由本模块负责
//! 「优雅退出 → 等待进程退出 → 注入 → 重新打开」，超时只报错、**绝不强杀**。
//! 扩展从未登录（`state.vscdb` 无会话 secret 行）时按新会话载荷写入，不再要求先登录一次。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;
use std::time::Duration;
#[cfg(not(target_os = "windows"))]
use std::time::Instant;

use crate::modules::account::{self, get_str};
use crate::modules::auth_file::build_account_obj;
use crate::modules::codebuddy_cn_ide::{match_account_for_token, parse_token_from_secret};
use crate::modules::config::{atomic_write, now_ms, store_dir};
use crate::modules::process;
use crate::modules::variant::codebuddy_domain_for;
use crate::modules::vscode_cn_inject::{
    has_secret_row_for, inject_secret_for, read_secret_for, state_db_path_for,
    VscodeSafeStorageTarget,
};

const STATE_FILE: &str = "vscode_ext.json";
/// VS Code 扩展的 marketplace id（= secret key 中的 `extensionId`）。
///
/// 也是插件宿主 `exthost/` 下本扩展日志目录名的判定依据（`limits::scan` 用它把
/// `ms-python.python`、`github.copilot-chat` 等无关扩展的日志排除在候选集之外）。
pub(crate) const EXTENSION_ID: &str = "tencent-cloud.coding-copilot";
/// 扩展写入 globalState 的 id 前缀（= 载荷顶层 `id`）。
const PAYLOAD_ID: &str = "Tencent-Cloud.coding-copilot";

/// VS Code CodeBuddy 扩展目标描述符。
///
/// 目录解析是**唯一扩展点**：目前仅支持官方 `Code`，暂不处理 Insiders / Cursor /
/// 便携版 `--user-data-dir`，如需扩展只需替换 `data_dir_resolver`。
const VSCODE_TARGET: VscodeSafeStorageTarget = VscodeSafeStorageTarget {
    data_dir_resolver: vscode_data_dir,
    display_name: "VS Code",
    secret_item_prefix_extension_id: EXTENSION_ID,
    secret_key: "Tencent-Cloud.coding-copilot.new.accessToken",
    macos_keychain_service: "Code Safe Storage",
    linux_secret_tool_app_names: &["Code", "code"],
};

/// 解析 VS Code 官方版用户数据目录（单点，可扩展）。
///
/// - Windows: `%APPDATA%\Code`（`dirs::data_dir()` 即 Roaming）。
/// - macOS: `~/Library/Application Support/Code`。
/// - Linux: `$XDG_CONFIG_HOME/Code`（缺省 `~/.config/Code`）。
fn vscode_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::modules::config::home_dir().join("Library/Application Support/Code"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join("Code"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs::config_dir().map(|d| d.join("Code"))
    }
}

/// VS Code state.vscdb 路径。
pub fn vscode_ext_state_db_path() -> Option<PathBuf> {
    state_db_path_for(&VSCODE_TARGET)
}

fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE)
}

fn load_state() -> Value {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_state(state: &Value) -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    atomic_write(&path, &content).map_err(|e| e.to_string())
}

fn set_active_account_id(account_id: &str) -> Result<(), String> {
    let mut state = load_state();
    if let Some(obj) = state.as_object_mut() {
        obj.insert("activeAccountId".to_string(), json!(account_id));
        obj.insert("updatedAt".to_string(), json!(now_ms()));
    }
    save_state(&state)
}

fn active_account_id_from_state() -> Option<String> {
    load_state()
        .get("activeAccountId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 把底层解密错误翻译成更明确的用户文案（尤其非 `v10` 前缀的场景）。
fn describe_secret_error(err: String) -> String {
    if err.contains("Unexpected ciphertext prefix")
        || err.contains("Unsupported Linux ciphertext prefix")
    {
        format!(
            "{err}\n\n检测到 VS Code 使用了当前版本不支持的 Safe Storage 加密前缀（可能是 Chromium 127+ 的 v20 app-bound 加密）。目前仅支持 v10，请反馈该问题。"
        )
    } else {
        err
    }
}

/// 账号条目的匹配键：优先 `uid`，缺失时回退 `id`（均取非空字符串）。
fn account_entry_key(entry: &Value) -> Option<&str> {
    entry
        .get("uid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
}

/// 构造写入 VS Code 扩展 secret 的会话 JSON（merge 策略）。
///
/// 先以既有明文为底（保留 `accounts[]` / `auth` 等扩展私有字段），仅覆盖账号身份相关的
/// 顶层键与 `auth` 内的凭据键；读不到既有 secret（未登录）时退化为新建完整载荷
/// （`accounts` 单元素）。
pub fn build_ext_session_json(acc: &Value, existing: Option<&str>) -> String {
    let uid = get_str(acc, "uid").unwrap_or_default();
    let domain = codebuddy_domain_for(
        get_str(acc, "domain").unwrap_or_default().as_str(),
        account::variant_of(acc),
    );
    let refresh_token = get_str(acc, "refresh_token").unwrap_or_default();
    let access_token = get_str(acc, "access_token").unwrap_or_default();
    let token_type = get_str(acc, "token_type").unwrap_or_else(|| "Bearer".to_string());
    let expires_at = acc.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);
    let refresh_expires_at = acc.get("refreshExpiresAt").and_then(|v| v.as_i64());

    let mut root: serde_json::Map<String, Value> = existing
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    // 目标账号条目：显式 lastLogin=true，供顶层 account 与 accounts[] upsert 复用。
    let mut account_obj = build_account_obj(acc);
    if let Some(map) = account_obj.as_object_mut() {
        map.insert("lastLogin".to_string(), json!(true));
    }

    // auth 同样走 merge：以既有 auth 为底，只覆盖身份/凭据键，扩展私有键
    //（`scope` / `sessionState` / `notBeforePolicy` 等）原样保留。
    let mut auth: serde_json::Map<String, Value> = root
        .get("auth")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    auth.insert("accessToken".to_string(), json!(access_token));
    auth.insert("refreshToken".to_string(), json!(refresh_token));
    auth.insert("tokenType".to_string(), json!(token_type));
    auth.insert("domain".to_string(), json!(domain));
    auth.insert("expiresAt".to_string(), json!(expires_at));
    // 注：`expiresIn` / `refreshExpiresIn` 刻意沿用写绝对时间的既有写法，与线上已验证的
    // CN IDE 实现（`codebuddy_cn_ide::build_session_json`）保持一致，不在本次修正。
    auth.insert("expiresIn".to_string(), json!(expires_at));
    auth.insert("refreshExpiresIn".to_string(), json!(0));
    match refresh_expires_at {
        // 账号库优先
        Some(value) => {
            auth.insert("refreshExpiresAt".to_string(), json!(value));
        }
        // 账号库没有：保留既有值；连既有值也没有时维持旧的 0（不让键凭空消失）。
        None if !auth.contains_key("refreshExpiresAt") => {
            auth.insert("refreshExpiresAt".to_string(), json!(0));
        }
        None => {}
    }
    auth.insert("lastRefreshTime".to_string(), json!(now_ms()));

    root.insert("id".to_string(), json!(PAYLOAD_ID));
    root.insert("token".to_string(), json!(access_token));
    root.insert("refreshToken".to_string(), json!(refresh_token));
    root.insert("expiresAt".to_string(), json!(expires_at));
    root.insert("domain".to_string(), json!(domain));
    root.insert(
        "accessToken".to_string(),
        json!(format!("{uid}+{access_token}")),
    );
    root.insert("converted".to_string(), json!(true));
    root.insert("account".to_string(), account_obj.clone());
    root.insert("auth".to_string(), Value::Object(auth));

    // accounts[] upsert：保留其他账号条目、数组顺序稳定，并把当前账号标记为 lastLogin。
    // 命中（按 uid，缺失回退 id）原地替换，未命中追加；数组不存在或非数组则新建单元素数组。
    let target_key = account_entry_key(&account_obj).map(str::to_string);
    let has_accounts_array = root.get("accounts").map(Value::is_array).unwrap_or(false);
    if has_accounts_array {
        if let Some(entries) = root.get_mut("accounts").and_then(Value::as_array_mut) {
            for entry in entries.iter_mut() {
                if let Some(map) = entry.as_object_mut() {
                    map.insert("lastLogin".to_string(), json!(false));
                }
            }
            let matched = target_key.as_deref().and_then(|key| {
                entries
                    .iter()
                    .position(|entry| account_entry_key(entry) == Some(key))
            });
            match matched {
                Some(index) => entries[index] = account_obj,
                None => entries.push(account_obj),
            }
        }
    } else {
        root.insert("accounts".to_string(), json!([account_obj]));
    }

    Value::Object(root).to_string()
}

mod detect;
mod status;
mod switch;

pub use detect::{active_ext_uid, detect_current_account};
pub use status::{is_vscode_running, status};
#[allow(unused_imports)]
pub(crate) use switch::validate_switch_target;
pub use switch::{
    close_vscode_for_switch, relaunch_closed_editor, switch_account, switch_account_after_close,
    ClosedEditor,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::detect::*;
    #[allow(unused_imports)]
    use super::status::*;
    #[allow(unused_imports)]
    use super::switch::*;
    include!("tests/switch.rs");
    // 启动后存活探测用例（缺陷二）：测的是 `status.rs` 的 `launch_vscode_with`，
    // 与切换语义无关；单开文件也让 `tests/switch.rs` 留在长文件阈值内。
    include!("tests/status.rs");
}
