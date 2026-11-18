//! safeStorage 加解密：前缀/密钥派生、CBC/GCM/DPAPI 与 secret payload 编解码。

#[allow(unused_imports)]
use super::inject::*;
use super::*;

/// 只挑已存在的 state.vscdb 候选路径；找不到返回 `None`，**不创建任何目录或文件**。
///
/// 只读探测（读 secret、查行存在性）必须走这里，不要复用
/// [`resolve_state_db_path_for`]——它有 `create_dir_all` 副作用，会让
/// `installed`（依赖数据目录存在性）形成自我维持的误报（issue #91）。
pub(super) fn find_existing_state_db_path(root: &Path) -> Option<PathBuf> {
    [
        root.join("User").join("globalStorage").join("state.vscdb"),
        root.join("globalStorage").join("state.vscdb"),
        root.join("state.vscdb"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

pub(super) fn data_root_from_db(db_path: &Path) -> Result<&Path, String> {
    db_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .ok_or_else(|| format!("无法从 db 路径推断数据目录: {}", db_path.display()))
}

pub(super) fn decode_buffer_data(buffer: &serde_json::Value) -> Result<Vec<u8>, String> {
    let data_arr = buffer["data"]
        .as_array()
        .ok_or_else(|| "Secret data is not in Buffer format".to_string())?;
    let mut encrypted_bytes = Vec::with_capacity(data_arr.len());
    for (idx, v) in data_arr.iter().enumerate() {
        let n = v
            .as_u64()
            .ok_or_else(|| format!("Secret data element at index {idx} is not an integer"))?;
        if n > 255 {
            return Err(format!(
                "Secret data element at index {idx} is out of range ({n} > 255)"
            ));
        }
        encrypted_bytes.push(n as u8);
    }
    Ok(encrypted_bytes)
}

pub(super) fn encode_secret_buffer(encrypted: Vec<u8>) -> Result<String, String> {
    let buffer_json = serde_json::json!({
        "type": "Buffer",
        "data": encrypted
    });
    serde_json::to_string(&buffer_json).map_err(|e| format!("Failed to serialize Buffer: {e}"))
}

pub(super) fn detect_prefix(encrypted: &[u8]) -> Option<&'static str> {
    if encrypted.starts_with(V10_PREFIX) {
        Some("v10")
    } else if encrypted.starts_with(V11_PREFIX) {
        Some("v11")
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn pbkdf2_sha1_key(password: &str, iterations: u32) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), SALT, iterations, &mut key);
    key
}

#[cfg(not(target_os = "windows"))]
pub(super) fn decrypt_cbc_prefixed(
    encrypted: &[u8],
    expected_prefix: &[u8],
    key: &[u8; 16],
) -> Result<Vec<u8>, String> {
    if !encrypted.starts_with(expected_prefix) {
        return Err(format!(
            "Unexpected ciphertext prefix: {:?}",
            &encrypted[..encrypted.len().min(3)]
        ));
    }
    let raw = &encrypted[expected_prefix.len()..];
    let cipher = Aes128CbcDec::new_from_slices(key, &CBC_IV)
        .map_err(|e| format!("Failed to init AES-CBC decryptor: {e}"))?;
    let mut buf = raw.to_vec();
    let plain = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| format!("AES-CBC decryption failed: {e}"))?
        .to_vec();
    Ok(plain)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn encrypt_cbc_prefixed(
    prefix: &[u8],
    key: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = Aes128CbcEnc::new_from_slices(key, &CBC_IV)
        .map_err(|e| format!("Failed to init AES-CBC encryptor: {e}"))?;
    let mut buf = plaintext.to_vec();
    let msg_len = buf.len();
    let pad_len = 16 - (msg_len % 16);
    buf.resize(msg_len + pad_len, 0);
    let ciphertext = cipher
        .encrypt_padded_mut::<Pkcs7>(&mut buf, msg_len)
        .map_err(|e| format!("AES-CBC encryption failed: {e}"))?
        .to_vec();
    let mut result = Vec::with_capacity(prefix.len() + ciphertext.len());
    result.extend_from_slice(prefix);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// 运行命令并取 trim 后的 stdout；带超时兜底。
///
/// macOS 上 `security find-generic-password` 可能因 Keychain 授权弹窗而长时间
/// 挂起（甚至无限期等待用户决定），因此不能使用无超时的阻塞式 `.output()`；
/// 子进程输出也需并发读取（复用 process 模块实现），避免写满管道死锁。
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn run_command_get_trimmed(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Option<String> {
    let output = crate::modules::process::run_cmd_timeout(program, args, timeout_secs)?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(target_os = "macos")]
pub(super) fn get_macos_safe_storage_password(service: &str) -> Result<String, String> {
    // 只查询一次：解密只依赖 password 本身、与 account 属性无关，
    // 单次查询最多触发一次钥匙串授权弹窗（多候选循环会逐次弹窗）。
    run_command_get_trimmed(
        "security",
        &["find-generic-password", "-w", "-s", service],
        10,
    )
    .ok_or_else(|| {
        format!("无法从 Keychain 读取「{service}」密码。请先手动打开对应应用并登录一次。")
    })
}

#[cfg(target_os = "linux")]
pub(super) const LINUX_V10_KEY: [u8; 16] = [
    0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27, 0x27, 0x78,
];
#[cfg(target_os = "linux")]
pub(super) const LINUX_EMPTY_KEY: [u8; 16] = [
    0xd0, 0xd0, 0xec, 0x9c, 0x7d, 0x77, 0xd4, 0x3a, 0xc5, 0x41, 0x87, 0xfa, 0x48, 0x18, 0xd1, 0x7f,
];

#[cfg(target_os = "linux")]
pub(super) fn get_linux_v11_key(app_names: &[&str]) -> Option<[u8; 16]> {
    // 优先原生 D-Bus（Secret Service）：`secret-tool` 属于 libsecret-tools，多数发行版
    // 默认不安装，而 Electron 早把密码写进了 gnome-keyring，只差一个读得到的客户端。
    if let Some(password) = crate::modules::linux_keyring::find_password(app_names) {
        return Some(pbkdf2_sha1_key(&password, 1));
    }
    // 兜底：极少数只装了 libsecret-tools 的环境，按老路子再试一次。
    for app in app_names {
        if let Some(password) =
            run_command_get_trimmed("secret-tool", &["lookup", "application", app], 10)
        {
            return Some(pbkdf2_sha1_key(&password, 1));
        }
    }
    None
}

/// v11 密钥缺失时的提示：直接说清「谁去开、怎么开」，不要只说一句加载失败。
///
/// 文案刻意不出现 "Safe Storage" / "Keychain"：cn-ide 的注入失败提示会按这两个词
/// 追加 macOS 钥匙串（Keychain）指引，那是平台错位的建议。
#[cfg(target_os = "linux")]
pub(super) fn linux_v11_key_error(target: &VscodeSafeStorageTarget) -> String {
    format!(
        "无法从系统密钥环读取「{}」的登录凭证密钥（v11）。请确认 gnome-keyring / KWallet 已启动、登录密钥环已解锁，并先用 {} 手动登录一次。",
        target.display_name, target.display_name
    )
}

#[cfg(target_os = "windows")]
pub(super) fn get_local_state_path(data_root: &Path) -> Result<PathBuf, String> {
    let path = data_root.join("Local State");
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("未找到 Local State: {}", path.display()))
    }
}

#[cfg(target_os = "windows")]
pub(super) fn dpapi_decrypt(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let data_in = CRYPT_INTEGER_BLOB {
            cbData: encrypted.len() as u32,
            pbData: encrypted.as_ptr() as *mut u8,
        };
        let mut data_out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        CryptUnprotectData(&data_in, None, None, None, None, 0, &mut data_out)
            .map_err(|e| format!("DPAPI CryptUnprotectData failed: {e}"))?;
        if data_out.pbData.is_null() || data_out.cbData == 0 {
            return Err("DPAPI returned empty data".to_string());
        }
        let slice = std::slice::from_raw_parts(data_out.pbData, data_out.cbData as usize);
        let result = slice.to_vec();
        let _ = LocalFree(HLOCAL(data_out.pbData as _));
        Ok(result)
    }
}

#[cfg(target_os = "windows")]
pub(super) fn get_windows_encryption_key(data_root: &Path) -> Result<Vec<u8>, String> {
    let local_state = get_local_state_path(data_root)?;
    let text =
        std::fs::read_to_string(&local_state).map_err(|e| format!("读取 Local State 失败: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 Local State 失败: {e}"))?;
    let encrypted_key_b64 = json["os_crypt"]["encrypted_key"]
        .as_str()
        .ok_or_else(|| "Local State 缺少 os_crypt.encrypted_key".to_string())?;
    let encrypted_key_bytes = general_purpose::STANDARD
        .decode(encrypted_key_b64)
        .map_err(|e| format!("Base64 decode failed for encrypted_key: {e}"))?;
    if encrypted_key_bytes.len() < 6 {
        return Err("encrypted_key data too short".to_string());
    }
    let prefix = String::from_utf8_lossy(&encrypted_key_bytes[..5]);
    if prefix != "DPAPI" {
        return Err(format!("encrypted_key prefix is not DPAPI, got: {prefix}"));
    }
    dpapi_decrypt(&encrypted_key_bytes[5..])
}

#[cfg(target_os = "windows")]
pub(super) fn decrypt_windows_gcm_v10(key: &[u8], encrypted: &[u8]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 31 {
        return Err("ciphertext too short for AES-GCM".to_string());
    }
    if &encrypted[..3] != V10_PREFIX {
        return Err(format!(
            "Unexpected ciphertext prefix: {:?}",
            &encrypted[..3]
        ));
    }
    let nonce_bytes = &encrypted[3..15];
    let ciphertext = &encrypted[15..];
    let cipher = Aes256Gcm::new(GenericArray::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("AES-GCM decryption failed: {e}"))
}

#[cfg(target_os = "windows")]
pub(super) fn encrypt_windows_gcm_v10(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(GenericArray::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("AES-GCM encryption failed: {e}"))?;
    let mut result = Vec::with_capacity(3 + nonce.len() + ciphertext.len());
    result.extend_from_slice(V10_PREFIX);
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub(super) fn decrypt_secret_payload(
    encrypted: &[u8],
    data_root: &Path,
    target: &VscodeSafeStorageTarget,
) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = target;
        let key = get_windows_encryption_key(data_root)?;
        decrypt_windows_gcm_v10(&key, encrypted)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = data_root;
        let password = get_macos_safe_storage_password(target.macos_keychain_service)?;
        let key = pbkdf2_sha1_key(&password, 1003);
        decrypt_cbc_prefixed(encrypted, V10_PREFIX, &key)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = data_root;
        match detect_prefix(encrypted) {
            Some("v11") => {
                let key = get_linux_v11_key(target.linux_secret_tool_app_names)
                    .ok_or_else(|| linux_v11_key_error(target))?;
                match decrypt_cbc_prefixed(encrypted, V11_PREFIX, &key) {
                    Ok(value) => Ok(value),
                    Err(_) => decrypt_cbc_prefixed(encrypted, V11_PREFIX, &LINUX_EMPTY_KEY),
                }
            }
            Some("v10") => match decrypt_cbc_prefixed(encrypted, V10_PREFIX, &LINUX_V10_KEY) {
                Ok(value) => Ok(value),
                Err(_) => decrypt_cbc_prefixed(encrypted, V10_PREFIX, &LINUX_EMPTY_KEY),
            },
            _ => Err(format!(
                "Unsupported Linux ciphertext prefix: {:?}",
                &encrypted[..encrypted.len().min(3)]
            )),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (encrypted, data_root, target);
        Err("Unsupported platform".to_string())
    }
}

pub(super) fn encrypt_secret_payload(
    plaintext: &[u8],
    preferred_prefix: Option<&str>,
    data_root: &Path,
    target: &VscodeSafeStorageTarget,
) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = (preferred_prefix, target);
        let key = get_windows_encryption_key(data_root)?;
        encrypt_windows_gcm_v10(&key, plaintext)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (preferred_prefix, data_root);
        let password = get_macos_safe_storage_password(target.macos_keychain_service)?;
        let key = pbkdf2_sha1_key(&password, 1003);
        encrypt_cbc_prefixed(V10_PREFIX, &key, plaintext)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = data_root;
        let target_prefix = if let Some(prefix) = preferred_prefix {
            prefix
        } else if get_linux_v11_key(target.linux_secret_tool_app_names).is_some() {
            "v11"
        } else {
            "v10"
        };
        if target_prefix == "v11" {
            let key = get_linux_v11_key(target.linux_secret_tool_app_names)
                .ok_or_else(|| linux_v11_key_error(target))?;
            return encrypt_cbc_prefixed(V11_PREFIX, &key, plaintext);
        }
        encrypt_cbc_prefixed(V10_PREFIX, &LINUX_V10_KEY, plaintext)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (plaintext, preferred_prefix, data_root, target);
        Err("Unsupported platform".to_string())
    }
}

pub(super) fn decode_secret_storage_value(
    raw_value: &str,
    data_root: &Path,
    target: &VscodeSafeStorageTarget,
) -> Result<String, String> {
    let parsed: serde_json::Value = match serde_json::from_str(raw_value) {
        Ok(value) => value,
        Err(_) => return Ok(raw_value.to_string()),
    };
    if parsed.get("data").is_some() {
        let encrypted_bytes = decode_buffer_data(&parsed)?;
        let decrypted = decrypt_secret_payload(&encrypted_bytes, data_root, target)?;
        return String::from_utf8(decrypted)
            .map_err(|e| format!("Decrypted data is not valid UTF-8: {e}"));
    }
    if let Some(value) = parsed.as_str() {
        return Ok(value.to_string());
    }
    Ok(raw_value.to_string())
}
