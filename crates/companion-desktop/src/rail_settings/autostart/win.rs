//! Windows 登录自启：Run 值读写与拼写规范化。

use super::*;

/// Quoted absolute program path for the Run value. SetupAPI caps the whole
/// command line at 260 characters.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn run_value_data(program: &Path) -> Result<String, String> {
    let data = format!("\"{}\"", plain_path(&program.to_string_lossy()));
    if data.chars().count() > 260 {
        return Err("开机自启路径超过 260 字符，无法写入注册表".into());
    }
    Ok(data)
}

/// `std::fs::canonicalize` spells Windows paths in the verbatim `\\?\` form.
/// Explorer, Task Manager and security software all expect the ordinary
/// spelling in a Run value, so the prefix is stripped before writing and before
/// comparing. `\\?\UNC\server\share` becomes `\\server\share`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn plain_path(value: &str) -> String {
    match value.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => value.strip_prefix(r"\\?\").unwrap_or(value).to_owned(),
    }
}

/// `REG_SZ` data is UTF-16LE, and its terminator counts towards `cbData`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn reg_sz_bytes(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Canonical form of a stored Run value, so a respelled path (short names, for
/// instance) compares equal to the freshly resolved target. An unresolvable
/// path is compared as-is. The verbatim `\\?\` prefix is removed here too, or
/// every read would look different from the plain value `run_value_data` writes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn canonical_run_value(value: &str) -> String {
    match std::fs::canonicalize(value.trim().trim_matches('"')) {
        Ok(path) => format!("\"{}\"", plain_path(&path.to_string_lossy())),
        Err(_) => value.to_owned(),
    }
}

/// `\\?\` is the same file after canonicalization, but Explorer does not want
/// that spelling in a Run value. Quoted short names are not rejected: those
/// already compare equal and must not be rewritten on every read.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn stored_run_spelling_rejected(value: &str) -> bool {
    value.trim().trim_matches('"').starts_with(r"\\?\")
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn run_value_needs_rewrite(current: &str, expected: &str) -> bool {
    stored_run_spelling_rejected(current) || canonical_run_value(current) != expected
}

/// Windows login item: one value under the current user's `Run` key. The
/// predefined root handle is used directly — `RegSetKeyValueW` creates a
/// missing subkey and `RegDeleteKeyValueW` deletes a missing value — so no key
/// is opened (or closed) and no `Win32_Security` feature is needed.
#[cfg(target_os = "windows")]
pub(crate) mod win_login {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    pub(crate) const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    /// Well above the 260-character command line the Run key allows.
    pub(crate) const MAX_BYTES: usize = 4096;

    pub(crate) fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn write(name: &str, data: &str) -> Result<(), String> {
        let subkey = wide(RUN_KEY);
        let name = wide(name);
        let bytes = super::reg_sz_bytes(data);
        // SAFETY: the buffers outlive the call and predefined keys are never closed.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                bytes.as_ptr().cast::<c_void>(),
                (bytes.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "无法写入开机自启项（注册表错误 {status}），可能被安全软件或系统策略阻止"
            ));
        }
        Ok(())
    }

    pub fn read(name: &str) -> Result<Option<String>, String> {
        let subkey = wide(RUN_KEY);
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = MAX_BYTES as u32;
        let mut data = vec![0u16; MAX_BYTES / 2];
        // SAFETY: the buffers outlive the call; `RRF_RT_REG_SZ` restricts the
        // data to a string and makes Windows count the terminator.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut kind,
                data.as_mut_ptr().cast::<c_void>(),
                &mut bytes,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("无法读取开机自启项（注册表错误 {status}）"));
        }
        data.truncate(bytes as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        Ok(Some(String::from_utf16_lossy(&data)))
    }

    pub fn remove(name: &str) -> Result<(), String> {
        let subkey = wide(RUN_KEY);
        let name = wide(name);
        let status =
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(format!("无法删除开机自启项（注册表错误 {status}）"));
        }
        Ok(())
    }
}
