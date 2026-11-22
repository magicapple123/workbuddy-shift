//! 换机迁移的加密导出信封：口令派生密钥 + AES-256-GCM，零网络、零托管。
//!
//! 文件是 JSON 信封（便于人工确认"这是不是本工具导出的文件"），但正文 `data` 全是密文：
//! 明文永远是**现有导出 payload 的 JSON 字节**（`export.rs` 的 `select_export_records`），
//! 本模块不另造账号结构，也不做合并——解密后仍交给 `import.rs` 的 `merge_*`。
//!
//! 安全约束（实现时逐条核对）：
//! - 口令与派生密钥**绝不进日志、错误文案或 `Debug`**：错误只用固定文案 + 结构位置；
//! - `format` / `version` / `kdf.algo` / `cipher.algo` 必须精确匹配，不做"尽力解析"；
//! - 迭代次数从信封读，但设上限，避免恶意文件把 CPU 打满（见
//!   [`MAX_KDF_ITERATIONS`]）；
//! - 任何长度异常都在 `Nonce::from_slice` 之前拦住——那里长度不对会 panic；
//! - 只接受认证通过后的明文：GCM 校验失败一律返回错误，绝不把密文当明文继续。
//!
//! 不新增依赖：`aes-gcm` / `pbkdf2` / `sha2` / `base64` 都已在 `Cargo.toml` 中，
//! 随机数用 `aes-gcm` 依赖图里现成的 `OsRng`（不额外引入 `getrandom`）。

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::modules::export_file::MAX_EXPORT_BYTES;

/// 信封 `format` 字段的固定取值：只认本工具的导出文件。
pub const ENVELOPE_FORMAT: &str = "wb-shift-export";

/// 信封 `version` 字段。改信封结构（字段增减、算法更换）必须递增，
/// 旧版本读取端靠它明确拒绝而不是猜。
pub const ENVELOPE_VERSION: u32 = 1;

/// 密钥派生算法标识（HMAC-SHA256）。
pub const KDF_ALGO: &str = "pbkdf2-sha256";

/// 加密算法标识（AES-256-GCM，认证标签随密文，由 aes-gcm 默认行为提供）。
pub const CIPHER_ALGO: &str = "aes-256-gcm";

/// 默认迭代次数：600000 次。
///
/// 取值依据：OWASP《Password Storage Cheat Sheet》对 PBKDF2-HMAC-SHA256 的推荐下限
/// 就是 600000 次（2023 版），本机实测约 0.1–0.3 s，用户可接受。写进信封是为了
/// 将来提高强度时**旧文件仍可解**：读取端一律按文件里的次数派生，不按常量。
pub const PBKDF2_ITERATIONS: u32 = 600_000;

/// 读取端接受的迭代次数上限。超过直接拒绝：迭代次数是攻击者可控的输入，
/// 不设上限等于允许用一个 40 亿次的文件把导入流程卡死（CPU 打满）。
pub const MAX_KDF_ITERATIONS: u32 = 5_000_000;

/// 明文体积上限：与 `export_file::MAX_EXPORT_BYTES`（16 MiB）同一档，
/// 账号导出远小于此，超限说明调用方拿错了数据。
pub const MAX_PLAINTEXT_BYTES: usize = MAX_EXPORT_BYTES;

/// 信封文件体积上限：16 MiB 明文 base64 后约 21.4 MiB，留出信封字段与回旋余量取 24 MiB。
/// 解析前先按字节数拒绝，避免把超大文件读进 `serde_json`。
pub const MAX_ENCRYPTED_BYTES: usize = 24 * 1024 * 1024;

/// 盐长度（字节），每次导出随机生成。
const SALT_LEN: usize = 16;
/// GCM nonce 长度（字节），每次导出随机生成。AES-256-GCM 只接受 12 字节。
const NONCE_LEN: usize = 12;
/// PBKDF2 输出密钥长度（AES-256 用 32 字节）。
const KEY_LEN: usize = 32;
/// GCM 认证标签长度（字节）：密文短于此必然不合法（无标签可校验）。
const GCM_TAG_LEN: usize = 16;

/// 信封根对象。字段全为非机密值：`salt` / `nonce` 按设计就是公开参数，
/// `data` 是密文。故意不 derive `Debug`，避免将来有人顺手把整个信封打进日志。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Envelope {
    /// 固定 `wb-shift-export`。
    format: String,
    /// 信封版本，当前只接受 [`ENVELOPE_VERSION`]。
    version: u32,
    /// 密钥派生参数。
    kdf: KdfParams,
    /// 加密参数。
    cipher: CipherParams,
    /// 导出时间（Unix 毫秒），仅用于展示与排查。
    created_at: i64,
    /// base64（标准字母表，带填充）的 AES-256-GCM 密文，含认证标签。
    data: String,
}

/// `kdf` 子对象。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KdfParams {
    /// 固定 `pbkdf2-sha256`。
    algo: String,
    /// 迭代次数，读取端按它派生，上限见 [`MAX_KDF_ITERATIONS`]。
    iterations: u32,
    /// base64 的 16 字节盐。
    salt: String,
}

/// `cipher` 子对象。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CipherParams {
    /// 固定 `aes-256-gcm`。
    algo: String,
    /// base64 的 12 字节 nonce。
    nonce: String,
}

/// 用口令把导出明文加密成信封字节（紧凑 JSON，无换行）。
///
/// `plain` 必须是现有导出 payload 的 JSON 字节；`created_at_ms` 由调用方提供
/// （core 不读系统时间，便于宿主与测试注入固定时间）。
///
/// 失败场景：口令为空、明文超限、系统随机源不可用、加密失败。
pub fn encrypt_export(plain: &[u8], password: &str, created_at_ms: i64) -> Result<Vec<u8>, String> {
    if password.is_empty() {
        return Err("导出密码不能为空".to_string());
    }
    if plain.len() > MAX_PLAINTEXT_BYTES {
        return Err(format!(
            "导出内容超过 {} MiB 上限（{} 字节）",
            MAX_PLAINTEXT_BYTES / 1024 / 1024,
            plain.len()
        ));
    }

    // 盐与 nonce 必须每次独立随机：复用 nonce 会让 GCM 直接失去机密性。
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = OsRng;
    rng.try_fill_bytes(&mut salt)
        .map_err(|_| "生成随机盐失败".to_string())?;
    rng.try_fill_bytes(&mut nonce_bytes)
        .map_err(|_| "生成随机 nonce 失败".to_string())?;

    let key = derive_key(password, &salt, PBKDF2_ITERATIONS);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "初始加密器失败".to_string())?;
    // 长度由上面的数组固定为 12，`Nonce::from_slice` 不会 panic。
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plain)
        .map_err(|_| "加密失败".to_string())?;

    let envelope = Envelope {
        format: ENVELOPE_FORMAT.to_string(),
        version: ENVELOPE_VERSION,
        kdf: KdfParams {
            algo: KDF_ALGO.to_string(),
            iterations: PBKDF2_ITERATIONS,
            salt: BASE64.encode(salt),
        },
        cipher: CipherParams {
            algo: CIPHER_ALGO.to_string(),
            nonce: BASE64.encode(nonce_bytes),
        },
        created_at: created_at_ms,
        data: BASE64.encode(ciphertext),
    };
    serde_json::to_vec(&envelope).map_err(|_| "序列化导出文件失败".to_string())
}

/// 用口令解开信封，返回明文（现有导出 payload 的 JSON 字节）。
///
/// 任何不认识的格式、被篡改的密文、错误口令、截断文件都返回可读错误；
/// 绝不 panic，也不会把密文当明文返回。错误文案**不含**口令、密钥或明文片段。
pub fn decrypt_export(bytes: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Err("导出文件内容为空".to_string());
    }
    if bytes.len() > MAX_ENCRYPTED_BYTES {
        return Err(format!(
            "导出文件超过 {} MiB 上限（{} 字节）",
            MAX_ENCRYPTED_BYTES / 1024 / 1024,
            bytes.len()
        ));
    }
    if password.is_empty() {
        return Err("导入密码不能为空".to_string());
    }

    // `deny_unknown_fields`：多出字段也算格式不符，避免"多塞字段还能读"的模糊边界。
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|error| format!("不是合法的导出文件（JSON 解析失败：{error}）"))?;

    if envelope.format != ENVELOPE_FORMAT {
        return Err("不是本工具导出的文件".to_string());
    }
    if envelope.version != ENVELOPE_VERSION {
        return Err(format!(
            "导出文件版本不受支持（version={}，仅支持 {ENVELOPE_VERSION}）",
            envelope.version
        ));
    }
    if envelope.kdf.algo != KDF_ALGO {
        return Err("导出文件的密钥派生算法不受支持".to_string());
    }
    if envelope.kdf.iterations == 0 {
        return Err("导出文件的迭代次数不合法".to_string());
    }
    if envelope.kdf.iterations > MAX_KDF_ITERATIONS {
        return Err(format!(
            "导出文件的迭代次数超过上限（{}）",
            MAX_KDF_ITERATIONS
        ));
    }
    if envelope.cipher.algo != CIPHER_ALGO {
        return Err("导出文件的加密算法不受支持".to_string());
    }

    let salt = decode_b64("盐值", &envelope.kdf.salt)?;
    if salt.len() != SALT_LEN {
        return Err("导出文件的盐值长度不合法".to_string());
    }
    let nonce_bytes = decode_b64("nonce", &envelope.cipher.nonce)?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err("导出文件的 nonce 长度不合法".to_string());
    }
    let ciphertext = decode_b64("密文", &envelope.data)?;
    if ciphertext.len() < GCM_TAG_LEN {
        return Err("导出文件的密文已损坏（长度不足）".to_string());
    }

    let key = derive_key(password, &salt, envelope.kdf.iterations);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "初始解密器失败".to_string())?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_slice())
        // 口令错、盐/nonce 被改、密文被改、截断，全部落到这一个分支：
        // 对外统一措辞，不区分"密码错"与"文件坏"，避免给离线爆破者额外信息。
        .map_err(|_| "解密失败：密码错误或文件已损坏".to_string())?;

    if plain.len() > MAX_PLAINTEXT_BYTES {
        return Err(format!(
            "解密结果超过 {} MiB 上限（{} 字节）",
            MAX_PLAINTEXT_BYTES / 1024 / 1024,
            plain.len()
        ));
    }
    Ok(plain)
}

/// PBKDF2-HMAC-SHA256：32 字节密钥，次数由调用方给定。
///
/// 密钥留在栈上定长数组里，用完随函数返回丢弃（不堆分配、不外传）。
/// 注意：本阶段不引入 `zeroize` crate，栈内存不做显式擦除。
fn derive_key(password: &str, salt: &[u8], iterations: u32) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut key);
    key
}

/// 解 base64；错误文案只带字段名，不回显内容（内容可能很长，也可能被塞脏字）。
fn decode_b64(field_name: &str, value: &str) -> Result<Vec<u8>, String> {
    BASE64
        .decode(value.trim())
        .map_err(|_| format!("导出文件的{field_name}不是合法的 base64"))
}
