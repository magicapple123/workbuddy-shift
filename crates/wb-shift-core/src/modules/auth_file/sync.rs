//! 写入同步：账号写入认证文件、logout 标记与默认值填充。

use super::read::*;
use super::*;

/// 把账号写入官方认证文件（原子写 + 写后校验）。对照 server.py `write_account_to_auth_file`。
pub fn write_account_to_auth_file(acc: &Value, variant: WbVariant) -> Result<(), String> {
    write_account_to_auth_file_at(acc, &variant.auth_file_path())
}

pub(super) fn write_account_to_auth_file_at(acc: &Value, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let existing = read_auth_file_at(path).unwrap_or_else(|| json!({}));
    eprintln!(
        "[auth] write_account: existing is_object={} allAccounts_len={}",
        existing.is_object(),
        existing
            .get("allAccounts")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    );
    let all_accounts = existing
        .get("allAccounts")
        .cloned()
        .or_else(|| existing.get("accounts").cloned())
        .filter(|v| v.is_array())
        .unwrap_or_else(|| json!([]));
    let account_obj = build_account_obj(acc);
    let auth_obj = build_auth_obj(acc);

    // 把目标账号并入 allAccounts（去重：按 uid 或 id）
    let target_uid = get_str(acc, "uid").unwrap_or_default();
    let mut all: Vec<Value> = all_accounts.as_array().cloned().unwrap_or_default();
    all.retain(|a| {
        let primary = a
            .get("uid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                a.get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("");
        primary != target_uid
    });
    all.push(account_obj.clone());
    eprintln!("[auth] write_account: merged allAccounts len={}", all.len());

    let session = json!({
        "account": &account_obj,
        "auth": &auth_obj,
        "accounts": &all,
        "allAccounts": &all,
    });
    let content = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
    if let Err(e) = atomic_write(path, &content) {
        eprintln!("[auth] atomic_write FAILED: {e}");
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(
                "无权限写入认证文件：请打开 系统设置→隐私与安全性→App 管理，允许本 App 控制 WorkBuddy 的数据（或为其开启『完全磁盘访问』后重试）"
                    .to_string(),
            );
        }
        return Err(e.to_string());
    }

    // 写后校验：按值比较（token 可能是明文字符串，也可能是 WorkBuddy 5.6 加密信封对象）
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let written_token = written
        .get("auth")
        .and_then(|a| a.get("accessToken"))
        .cloned()
        .unwrap_or(Value::Null);
    let expect_token = auth_obj.get("accessToken").cloned().unwrap_or(Value::Null);
    if written_token != expect_token {
        return Err("认证文件写后校验失败，未写入目标账号".to_string());
    }

    // WorkBuddy 优先读取退出标记：即使认证文件已有凭据，标记仍存在时也视为未登录。
    // 必须在写入及校验成功后清理；失败时保留退出状态，并阻止调用方报告切换成功。
    let marker = logout_marker_path(path);
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "清理 WorkBuddy 退出标记失败（{}）：{error}",
                marker.display()
            ));
        }
    }
    Ok(())
}

/// 官方退出标记是在完整认证文件名后追加后缀，不能替换 `.info` 扩展名。
pub(super) fn logout_marker_path(auth_path: &Path) -> PathBuf {
    let mut marker = auth_path.as_os_str().to_os_string();
    marker.push(".logged-out");
    PathBuf::from(marker)
}

pub(super) fn setdefault(map: &mut Map<String, Value>, key: &str, value: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), value);
    }
}
