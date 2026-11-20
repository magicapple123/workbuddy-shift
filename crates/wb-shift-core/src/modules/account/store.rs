//! 账号存储：账号文件读写、按 id 查找/删除与 upsert。

use super::display::*;
use super::identity::*;
use super::*;

pub(super) fn load_accounts_from_path(path: &Path) -> Vec<Value> {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(Value::Array(accounts)) = serde_json::from_str::<Value>(&text) {
            return accounts;
        }
    }
    vec![]
}

pub(super) fn save_accounts_to_path(path: &Path, accounts: &[Value]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(accounts).unwrap_or_default();
    atomic_write(path, &content)
}

pub(super) fn find_account_in(accounts: &[Value], account_id: &str) -> Option<Value> {
    accounts
        .iter()
        .find(|account| {
            account.get("id").and_then(Value::as_str) == Some(account_id)
                || account.get("uid").and_then(Value::as_str) == Some(account_id)
        })
        .cloned()
}

pub(super) fn delete_account_from_path(path: &Path, account_id: &str) -> Result<(), String> {
    let mut accounts = load_accounts_from_path(path);
    let before = accounts.len();
    accounts.retain(|account| account.get("id").and_then(Value::as_str) != Some(account_id));
    if accounts.len() == before {
        return Err("账号不存在".to_string());
    }
    save_accounts_to_path(path, &accounts).map_err(|error| error.to_string())
}

/// 读取账号库；文件缺失或损坏返回空列表。
pub fn load_accounts() -> Vec<Value> {
    load_accounts_from_path(&accounts_file())
}

/// 指定工具存储根下的账号库路径（会话操作等注入路径的场景用）。
pub fn accounts_file_in(store_root: &Path) -> std::path::PathBuf {
    store_root.join("accounts.json")
}

/// 读取指定账号库文件；文件缺失或损坏返回空列表。
pub fn load_accounts_at(path: &Path) -> Vec<Value> {
    load_accounts_from_path(path)
}

/// 写回账号库（原子写），保持原 JSON 数组结构。
pub fn save_accounts(accounts: &[Value]) -> std::io::Result<()> {
    save_accounts_to_path(&accounts_file(), accounts)
}

/// 按 id 或 uid 查找账号。
pub fn find_account(account_id: &str) -> Option<Value> {
    find_account_in(&load_accounts(), account_id)
}

/// 按 id 覆盖；id 缺失或未命中时按 uid 回退覆盖，仍未命中则补 id 追加。
pub fn upsert_account(updated: &Value) -> std::io::Result<()> {
    let mut accounts = load_accounts();
    upsert_account_in(&mut accounts, updated);
    save_accounts(&accounts)
}

/// 覆盖写入的内存实现：按 id 覆盖；id 缺失或未命中时回退按 uid 收敛到已有
/// 记录；仍无归属才追加，且追加前补 id。
///
/// 无 id 但有 uid 的历史记录按 uid 覆盖，并在原记录上补 id；新追加记录也补 id。
/// 两种身份都缺失的旧记录无法在此处安全关联到原行，导入入口会先生成 id。
pub(super) fn upsert_account_in(accounts: &mut Vec<Value>, updated: &Value) {
    let id = get_str(updated, "id");
    let uid = get_str(updated, "uid");
    let id_matched_index = id.as_deref().and_then(|id| {
        accounts
            .iter()
            .position(|a| get_str(a, "id").as_deref() == Some(id))
    });
    let uid_matched_index = id_matched_index
        .is_none()
        .then(|| {
            uid.as_deref().and_then(|uid| {
                accounts
                    .iter()
                    .position(|a| get_str(a, "uid").as_deref() == Some(uid))
            })
        })
        .flatten();
    let matched_index = id_matched_index.or(uid_matched_index);

    if let Some(index) = matched_index {
        let mut next = updated.clone();
        inherit_existing_variant(&accounts[index], &mut next);
        // 本地展示字段（备注 / 显示选择 / 标记）不从官方数据来：updated 缺失时保留已有值。
        // 刷新 Token 走的就是这条路径，白名单见 `LOCAL_ACCOUNT_FIELDS`（漏一个字段
        // 就会让它在刷新后被整条记录覆盖掉，R5-04 的 `mark` 即为此）。
        inherit_local_fields(&accounts[index], &mut next);
        // uid 回退时保留本地稳定 id，即使刷新对象携带了另一 id；历史脏数据
        // 的空 id 不继承，优先保留刷新对象的有效 id，否则生成新 id。
        if uid_matched_index.is_some() {
            if let Some(existing_id) = get_str(&accounts[index], "id") {
                next["id"] = Value::String(existing_id);
            }
        }
        if get_str(&next, "id").is_none() {
            next["id"] = Value::String(uuid::Uuid::new_v4().to_string());
        }
        accounts[index] = next;
        return;
    }

    let mut appended = updated.clone();
    if get_str(&appended, "id").is_none() {
        appended["id"] = Value::String(uuid::Uuid::new_v4().to_string());
    }
    accounts.push(appended);
}

/// 删除账号（按 id）。
pub fn delete_account(account_id: &str) -> Result<(), String> {
    delete_account_from_path(&accounts_file(), account_id)
}

/// 导入本机当前账号（从该档位的登录态文件读取）。
pub fn import_local(variant: WbVariant) -> Result<Value, String> {
    let acc = crate::modules::auth_file::import_from_auth_file(variant)
        .ok_or("未读取到本地 WorkBuddy 登录信息")?;
    let saved = save_collected_account(acc).map_err(|e| e.to_string())?;
    Ok(account_meta(&saved))
}
