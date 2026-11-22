//! 加密导出/导入的**文件桥接层**：口令策略 + 文件读写 + 复用既有 payload 与合并语义。
//!
//! `crypto.rs` 只做字节进/字节出的信封加解密（纯函数，不碰文件系统）；本模块把信封
//! 接到用户手上的文件上：
//!
//! - **导出**：账号记录 → 既有导出 payload（与 `export.rs` 同形的 JSON 数组）→
//!   `encrypt_export` → 原子写（临时文件 + rename，失败不留半截文件）；
//! - **导入**：读文件 → `decrypt_export` → 反序列化 → `merge_import_records`
//!   （合并语义、`uid` 去重、本地字段保留、缺 id 补齐全部沿用导入备份那条路径，
//!   本模块**不另造一套**）；
//! - **不落盘的部分**（[`merge_encrypted_bytes`]）单独暴露：错口令、坏文件必须能
//!   在"还没写账号库"的时候失败，测试也只能靠它证明这一点。
//!
//! ## 口令策略：至少 8 位
//!
//! 校验放在 core 而不是两个宿主里：桌面端与 WebUI 必须给同一句错误文案，否则
//! 同一个口令在两边一边能过一边不能过。8 位不是"强度足够"的承诺，而是堵住
//! "空口令 / 一位数"这种等于把凭据放明文的用法；真正的强度由 PBKDF2 600000 次
//! 迭代承担（见 `crypto.rs` 的 `PBKDF2_ITERATIONS`）。
//!
//! 口令只作为参数向下传：**不进日志、不进错误文案**。错误里只出现"至少 8 位"与
//! 当前长度（长度不是秘密），绝不回显口令本身。
//!
//! ## 体积上限：两个数不是同一个
//!
//! 明文 16 MiB 沿用 `export_file::MAX_EXPORT_BYTES`（由 `crypto` 侧的
//! `MAX_PLAINTEXT_BYTES` 引用）；加密后的信封上限是 24 MiB
//! （`crypto::MAX_ENCRYPTED_BYTES`，base64 膨胀 4/3 后留余量）。读文件时必须用**后者**
//! 先按字节数拒绝，不能把 24 MiB 的信封当成 16 MiB 的明文上限去判。

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::crypto::{decrypt_export, encrypt_export, MAX_ENCRYPTED_BYTES};
use super::export::validate_export_path;
use super::import::{merge_import_records, parse_accounts_json};
use super::ImportResult;
use crate::modules::account;
use crate::modules::config::atomic_write_bytes;

/// 加密导出/导入的最短口令长度（按**字符**计，不按字节：中文口令两位也算两位）。
///
/// 前端 `src/lib/encrypted-backup.ts` 的同名常量必须与它一致；改这里要同步改那边，
/// 否则会出现"界面上能点、后端拒绝"或反过来的裂缝。
pub const MIN_PASSWORD_LEN: usize = 8;

/// 口令策略校验：不足 [`MIN_PASSWORD_LEN`] 位直接拒绝。
///
/// 错误文案明确写出"至少 8 位"与当前长度，便于用户当场改；不含口令本身。
pub fn validate_password(password: &str) -> Result<(), String> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_LEN {
        return Err(format!(
            "密码至少 {MIN_PASSWORD_LEN} 位（当前 {length} 位）"
        ));
    }
    Ok(())
}

/// 导出：把 `accounts` 加密写入 `path`。
///
/// - `accounts` 由调用方传入（宿主传 `account::load_accounts()`），便于单测注入夹具，
///   也让"导出全部账号"这件事在 core 里保持无状态；
/// - `created_at_ms` 同样由调用方注入（core 不读系统时间，测试才能固定信封内容）；
/// - 路径校验与明文导出一致：绝对路径 + `.json` 结尾（信封本身就是 JSON）；
/// - 目标文件已存在时**整体替换**（原子写 = 临时文件 + rename）。与"导出备份"覆盖同名
///   文件的行为一致，不做追加、不做二次确认——用户刚在保存对话框里选过这个路径。
pub fn export_accounts_encrypted_at(
    path: &str,
    password: &str,
    accounts: &[Value],
    created_at_ms: i64,
) -> Result<(), String> {
    validate_password(password)?;
    validate_export_path(path)?;
    if accounts.is_empty() {
        return Err("请先选择要导出的账号".to_string());
    }

    let plain = serde_json::to_vec_pretty(accounts)
        .map_err(|error| format!("序列化导出内容失败：{error}"))?;
    let sealed = encrypt_export(&plain, password, created_at_ms)?;
    if sealed.len() > MAX_ENCRYPTED_BYTES {
        return Err(format!(
            "加密导出文件超过 {} MiB 上限（{} 字节）",
            MAX_ENCRYPTED_BYTES / 1024 / 1024,
            sealed.len()
        ));
    }

    let target = PathBuf::from(path.trim());
    // 父目录必须已存在：不替用户在任意位置造目录（与 `export_file::save_text_file` 同口径）。
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(format!("目标目录不存在：{}", parent.display()));
        }
    }
    atomic_write_bytes(&target, &sealed).map_err(|error| format!("写入加密导出文件失败：{error}"))
}

/// 解密 + 合并，**不碰文件系统**：错口令 / 坏文件在这里就失败，账号库一个字都不会改。
///
/// `text` 的解析与合并全部复用导入备份的实现（`parse_accounts_json` +
/// `merge_import_records`），索引取文件里的全部记录——加密导出导的就是整份账号集，
/// 导入时不该再让用户勾选一遍。
pub(super) fn merge_encrypted_bytes(
    bytes: &[u8],
    password: &str,
    accounts: &mut Vec<Value>,
) -> Result<ImportResult, String> {
    validate_password(password)?;
    let plain = decrypt_export(bytes, password)?;
    let text = String::from_utf8(plain)
        .map_err(|_| "解密后的内容不是合法的 UTF-8 文本（文件可能已损坏）".to_string())?;
    let records = parse_accounts_json(&text)?;
    let indexes: Vec<usize> = (0..records.len()).collect();
    merge_import_records(accounts, &text, &indexes)
}

/// 导入：读 `path` → 解密 → 按既有合并语义写回账号库，返回导入/跳过/覆盖计数。
///
/// 失败场景都返回可读错误（**不是** empty 结果）：路径为空、文件不存在、体积超限、
/// 读取失败、口令不足 8 位、文件不是本工具的信封、口令错误 / 文件被篡改、账号库写入失败。
/// 其中任何一步失败都**不会**写账号库——合并结果只在最后一步落盘。
pub fn import_accounts_encrypted_at(path: &str, password: &str) -> Result<ImportResult, String> {
    validate_password(password)?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("缺少导入文件路径".to_string());
    }
    let target = Path::new(trimmed);
    if !target.is_file() {
        return Err(format!("导入文件不存在：{}", target.display()));
    }
    let metadata =
        std::fs::metadata(target).map_err(|error| format!("读取导入文件失败：{error}"))?;
    let size = metadata.len() as usize;
    if size > MAX_ENCRYPTED_BYTES {
        return Err(format!(
            "导入文件超过 {} MiB 上限（{} 字节）",
            MAX_ENCRYPTED_BYTES / 1024 / 1024,
            size
        ));
    }

    let bytes = std::fs::read(target).map_err(|error| format!("读取导入文件失败：{error}"))?;
    let mut accounts = account::load_accounts();
    // 错口令在这里返回 Err：`?` 直接跳过下面的写回，账号库保持原样。
    let result = merge_encrypted_bytes(&bytes, password, &mut accounts)?;
    account::save_accounts(&accounts).map_err(|error| format!("保存账号库失败：{error}"))?;
    Ok(result)
}
