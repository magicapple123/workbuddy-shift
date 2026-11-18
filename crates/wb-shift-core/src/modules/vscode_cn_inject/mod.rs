//! Electron/Chromium Safe Storage 注入（参数化：CodeBuddy 桌面 IDE 与 VS Code 扩展共用）。
//!
//! 把账号会话 JSON 加密写入 `state.vscdb` 的 ItemTable，键形如
//! `secret://{"extensionId":"<ext>","key":"<key>"}`。
//!
//! 各目标之间的差异点（数据目录 / secret key / macOS Keychain 服务名 /
//! Linux 密钥环应用名）全部收敛在 [`VscodeSafeStorageTarget`] 描述符，
//! CodeBuddy CN IDE（`codebuddy_cn_ide`）、CodeBuddy 国际版 IDE（`codebuddy_ide`）
//! 与 VS Code CodeBuddy 扩展（`vscode_ext`）复用同一套加解密与读写流程。
//!
//! 桌面 IDE 的「国内 CN / 国际版」档位用 [`CodeBuddyIdeFlavor`] 表达，
//! 并映射到对应的 [`VscodeSafeStorageTarget`] 常量描述符。
//!
//! 平台加密模型对齐 Chromium/Electron Safe Storage：
//! - macOS: Keychain「`` `<app>` `` Safe Storage」→ PBKDF2-SHA1(1003) → AES-128-CBC `v10`
//! - Windows: Local State `os_crypt.encrypted_key` + DPAPI → AES-256-GCM `v10`
//! - Linux: Secret Service（`org.freedesktop.secrets`）密钥 → AES-128-CBC `v11`，
//!   无密钥环时退回 peanuts 固定密钥 `v10`

use std::path::{Path, PathBuf};

#[cfg(not(target_os = "windows"))]
use aes::Aes128;
#[cfg(target_os = "windows")]
use aes_gcm::aead::generic_array::GenericArray;
#[cfg(target_os = "windows")]
use aes_gcm::aead::{Aead, AeadCore, OsRng};
#[cfg(target_os = "windows")]
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
#[cfg(target_os = "windows")]
use base64::{engine::general_purpose, Engine as _};
#[cfg(not(target_os = "windows"))]
use cbc::cipher::block_padding::Pkcs7;
#[cfg(not(target_os = "windows"))]
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
#[cfg(not(target_os = "windows"))]
use pbkdf2::pbkdf2_hmac;
use rusqlite::{Connection, OpenFlags};
#[cfg(not(target_os = "windows"))]
use sha1::Sha1;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{LocalFree, HLOCAL};
#[cfg(target_os = "windows")]
use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

#[cfg(not(target_os = "windows"))]
type Aes128CbcEnc = cbc::Encryptor<Aes128>;
#[cfg(not(target_os = "windows"))]
type Aes128CbcDec = cbc::Decryptor<Aes128>;

const V10_PREFIX: &[u8] = b"v10";
const V11_PREFIX: &[u8] = b"v11";
#[cfg(not(target_os = "windows"))]
const CBC_IV: [u8; 16] = [b' '; 16];
#[cfg(not(target_os = "windows"))]
const SALT: &[u8] = b"saltysalt";

/// CodeBuddy 系扩展共用的 marketplace 扩展 id（secret key 中的 `extensionId`）。
pub const SECRET_EXTENSION_ID: &str = "tencent-cloud.coding-copilot";
/// CodeBuddy CN IDE 的 secret key（secret key 中的 `key`）。
pub const SECRET_KEY: &str = "planning-genie.new.accessTokencn";
/// CodeBuddy 国际版 IDE 的 secret key（secret key 中的 `key`）。
pub const INTL_SECRET_KEY: &str = "planning-genie.new.accessToken";

/// 一个「基于 Electron/Chromium Safe Storage」的应用目标描述符。
///
/// 把各目标之间的差异点参数化后，[`read_secret_for`] / [`inject_secret_for`] 等
/// 流程可对任意目标复用。新增目标只需提供一个常量描述符。
pub struct VscodeSafeStorageTarget {
    /// 数据根目录解析函数；返回 `None` 表示当前平台无法定位。
    pub data_dir_resolver: fn() -> Option<PathBuf>,
    /// 目标展示名（用于错误文案）。
    pub display_name: &'static str,
    /// secret key 中的 `extensionId` 字段。
    pub secret_item_prefix_extension_id: &'static str,
    /// secret key 中的 `key` 字段。
    pub secret_key: &'static str,
    /// macOS Keychain 通用密码服务名。
    pub macos_keychain_service: &'static str,
    /// Linux 密钥环应用名候选（按顺序尝试）：Secret Service 按 `application`
    /// 属性检索，`secret-tool` 兜底路径按同一属性查询。
    pub linux_secret_tool_app_names: &'static [&'static str],
}

/// CodeBuddy CN IDE（桌面客户端）目标描述符。
pub const CODEBUDDY_CN_TARGET: VscodeSafeStorageTarget = VscodeSafeStorageTarget {
    data_dir_resolver: codebuddy_cn_data_dir,
    display_name: "CodeBuddy CN",
    secret_item_prefix_extension_id: SECRET_EXTENSION_ID,
    secret_key: SECRET_KEY,
    macos_keychain_service: "CodeBuddy CN Safe Storage",
    linux_secret_tool_app_names: &[
        "CodeBuddy CN",
        "codebuddy cn",
        "codebuddy-cn",
        "codebuddycn",
    ],
};

/// CodeBuddy 国际版 IDE（桌面客户端）目标描述符。
pub const CODEBUDDY_INTL_TARGET: VscodeSafeStorageTarget = VscodeSafeStorageTarget {
    data_dir_resolver: codebuddy_intl_data_dir,
    display_name: "CodeBuddy",
    secret_item_prefix_extension_id: SECRET_EXTENSION_ID,
    secret_key: INTL_SECRET_KEY,
    macos_keychain_service: "CodeBuddy Safe Storage",
    linux_secret_tool_app_names: &["CodeBuddy", "codebuddy"],
};

/// CodeBuddy 桌面 IDE 档位：国内 CN 与国际版共用加密，密钥/目录不同。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeBuddyIdeFlavor {
    Cn,
    Intl,
}

impl CodeBuddyIdeFlavor {
    pub fn product_label(self) -> &'static str {
        match self {
            Self::Cn => "CodeBuddy CN",
            Self::Intl => "CodeBuddy",
        }
    }

    pub fn data_dir_name(self) -> &'static str {
        self.product_label()
    }

    pub fn secret_key(self) -> &'static str {
        match self {
            Self::Cn => SECRET_KEY,
            Self::Intl => INTL_SECRET_KEY,
        }
    }

    pub fn keychain_service(self) -> &'static str {
        match self {
            Self::Cn => "CodeBuddy CN Safe Storage",
            Self::Intl => "CodeBuddy Safe Storage",
        }
    }

    pub fn linux_secret_apps(self) -> &'static [&'static str] {
        match self {
            Self::Cn => &[
                "CodeBuddy CN",
                "codebuddy cn",
                "codebuddy-cn",
                "codebuddycn",
            ],
            Self::Intl => &["CodeBuddy", "codebuddy"],
        }
    }

    /// 映射到对应的静态目标描述符。
    pub fn target(self) -> &'static VscodeSafeStorageTarget {
        match self {
            Self::Cn => &CODEBUDDY_CN_TARGET,
            Self::Intl => &CODEBUDDY_INTL_TARGET,
        }
    }
}

/// ItemTable 完整 key（默认目标 = CodeBuddy CN，保持既有行为）。
pub fn secret_storage_item_key() -> String {
    secret_storage_item_key_for(&CODEBUDDY_CN_TARGET)
}

/// 构造指定目标的 ItemTable 完整 key。
pub fn secret_storage_item_key_for(target: &VscodeSafeStorageTarget) -> String {
    format!(
        r#"secret://{{"extensionId":"{}","key":"{}"}}"#,
        target.secret_item_prefix_extension_id, target.secret_key
    )
}

/// CodeBuddy 桌面 IDE 数据目录（按档位区分 CN / 国际版）。
pub fn codebuddy_ide_data_dir(flavor: CodeBuddyIdeFlavor) -> Option<PathBuf> {
    let name = flavor.data_dir_name();
    #[cfg(target_os = "macos")]
    {
        Some(
            crate::modules::config::home_dir()
                .join("Library/Application Support")
                .join(name),
        )
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join(name))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs::config_dir().map(|d| d.join(name))
    }
}

/// CodeBuddy CN 数据目录（保持既有行为）。
pub fn codebuddy_cn_data_dir() -> Option<PathBuf> {
    codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Cn)
}

/// CodeBuddy 国际版数据目录。
pub fn codebuddy_intl_data_dir() -> Option<PathBuf> {
    codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Intl)
}

/// CodeBuddy CN state.vscdb 路径（保持既有行为）。
pub fn codebuddy_cn_state_db_path() -> Option<PathBuf> {
    state_db_path_for(&CODEBUDDY_CN_TARGET)
}

/// 指定目标的 state.vscdb 路径。
pub fn state_db_path_for(target: &VscodeSafeStorageTarget) -> Option<PathBuf> {
    (target.data_dir_resolver)().map(|d| d.join("User").join("globalStorage").join("state.vscdb"))
}

/// 解析目标 state.vscdb 路径（保持既有行为）。
pub fn resolve_state_db_path(user_data_dir: Option<&Path>) -> Result<PathBuf, String> {
    resolve_state_db_path_for(&CODEBUDDY_CN_TARGET, user_data_dir)
}

/// 解析指定目标的 state.vscdb 路径；优先返回已存在的候选，否则准备首选路径。
pub fn resolve_state_db_path_for(
    target: &VscodeSafeStorageTarget,
    user_data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let root = match user_data_dir {
        Some(p) => p.to_path_buf(),
        None => (target.data_dir_resolver)()
            .ok_or_else(|| format!("无法定位 {} 数据目录", target.display_name))?,
    };
    let candidates = [
        root.join("User").join("globalStorage").join("state.vscdb"),
        root.join("globalStorage").join("state.vscdb"),
        root.join("state.vscdb"),
    ];
    if let Some(path) = candidates.iter().find(|p| p.exists()) {
        return Ok(path.clone());
    }
    let preferred = candidates[0].clone();
    if let Some(parent) = preferred.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 globalStorage 失败: {e}"))?;
    }
    Ok(preferred)
}

mod inject;
mod safestorage;

pub use inject::{
    has_secret_row_for, inject_codebuddy_cn_secret, inject_codebuddy_ide_secret, inject_secret_for,
    read_codebuddy_cn_secret, read_codebuddy_ide_secret, read_secret_for,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::inject::*;
    #[allow(unused_imports)]
    use super::safestorage::*;
    include!("tests/inject.rs");
}
