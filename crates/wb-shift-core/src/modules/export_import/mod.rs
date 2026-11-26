//! 账号导出/导入：解析导入文件、按 uid 去重合并、计数。
//!
//! 纯逻辑（`parse_accounts_json` / `merge_import_records` / `select_export_records`）
//! 不依赖文件系统，便于无 UI 环境单测；`export_accounts` / `import_accounts`
//! 负责读写账号库（`~/.workbuddy-shift/accounts.json`）。
//!
//! `crypto` 是换机迁移用的加密信封（口令 + AES-256-GCM）：只做字节进/字节出，
//! 明文仍是上面这套导出 payload，解密后照样走 `merge_import_records` 的合并语义。
//! `encrypted` 把信封接到文件上（`export_accounts_encrypted_at` /
//! `import_accounts_encrypted_at`），并持有两个宿主共用的口令策略（至少 8 位）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::modules::account;

/// 导入结果计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportResult {
    /// 成功合入账号库的数量（含覆盖与新增）。
    pub imported: usize,
    /// 未导入的数量（缺凭据 / 索引越界）。
    pub skipped: usize,
    /// 其中覆盖了同 uid 本地账号的数量。
    pub overwritten: usize,
}

mod crypto;
mod encrypted;
mod export;
mod import;

pub use crypto::{decrypt_export, encrypt_export};
pub use encrypted::{
    export_accounts_encrypted_at, import_accounts_encrypted_at, validate_password, MIN_PASSWORD_LEN,
};
pub use export::{
    export_accounts, export_accounts_to_path, select_export_records, write_records_to_file,
};
pub use import::{import_accounts, merge_import_records, parse_accounts_json, preview_accounts};

#[cfg(test)]
mod tests {
    use super::*;

    use super::encrypted::merge_encrypted_bytes;
    use super::export::*;
    // issue #232：第三方 `enc:v1:` 密文判定（预览与合并共用同一份）。
    use super::import::is_foreign_ciphertext;
    include!("tests/import.rs");
    include!("tests/crypto.rs");
    include!("tests/encrypted.rs");
}
