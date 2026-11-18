//! 身份与聚合：档位/企业标识、继承、identity email 与采集结果合并。

use super::display::*;
use super::store::*;
use super::*;

/// 字段值读取：接受明文字符串或 WorkBuddy 5.6 加密信封对象，其他类型返回 None。
/// 信封在本机同一 keyblob 下可由 WorkBuddy 自行解密，导入与切换写回时需原样保留。
/// 空白字符串按「没有值」处理（与 `get_str` 的空值语义一致）：否则 auth 文件里的
/// `"accessToken": ""` 会被判为已有 token，导入一条空凭据账号。
pub fn secret_value(v: &Value, key: &str) -> Option<Value> {
    match v.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(Value::String(s.clone())),
        Some(o @ Value::Object(map)) if map.contains_key("$wbEncrypted") => Some(o.clone()),
        _ => None,
    }
}

/// 账号档位：显式 `variant` 字段优先，缺失时按 `domain` 后缀兜底，缺省国内版。
pub fn variant_of(acc: &Value) -> WbVariant {
    WbVariant::from_account(acc)
}

/// 企业 ID 字段读取（兼容 camelCase `enterpriseId` 与 snake_case `enterprise_id`）。
///
/// 官方客户端以该字段是否存在分流个人版 / 企业版积分接口（issue #167）；
/// 请求头（[`build_auth_headers`]）与积分查询共用同一字段口径，不要各写一份。
pub fn enterprise_id_of(account: &Value) -> Option<String> {
    get_str(account, "enterpriseId").or_else(|| get_str(account, "enterprise_id"))
}

/// 是否企业版账号：有企业 ID 直接命中；历史备份可能只带
/// `type ∈ {ultimate, exclusive}` 而缺 ID（对齐 WorkDaddy 兜底判定），
/// 这类账号仍需走企业分流，取数前补拉企业 ID。
pub fn is_enterprise_account(account: &Value) -> bool {
    enterprise_id_of(account).is_some()
        || matches!(
            get_str(account, "type").as_deref(),
            Some("ultimate" | "exclusive")
        )
}

/// 合并采集结果时保留已有档位：已入库的档位不得因再次采集而丢失。
/// 采集结果自带档位时以它为准（记录里的凭据来自该档位）。
pub(super) fn inherit_existing_variant(existing: &Value, collected: &mut Value) {
    if get_str(collected, "variant").is_none() {
        if let Some(variant) = get_str(existing, "variant") {
            collected["variant"] = Value::String(variant);
        }
    }
}

/// 返回可用于 UID 缺失场景的真实邮箱。历史展示占位值不参与身份匹配。
pub(super) fn identity_email(account: &Value) -> Option<String> {
    let email = get_str(account, "email")?;
    if !email.contains('@')
        || email.eq_ignore_ascii_case("unknown")
        || email == "手动添加"
        || get_str(account, "nickname").as_deref() == Some(email.as_str())
        || get_str(account, "uid").as_deref() == Some(email.as_str())
    {
        return None;
    }
    Some(email.to_ascii_lowercase())
}

/// 按稳定身份将采集结果合并到账号列表，并返回最终持久化的账号。
///
/// 非空 UID 始终优先；仅当新账号没有 UID 时，才使用真实邮箱兜底。
/// 命中已有身份时保留本地 id，避免调用方持有的账号引用失效。
pub fn upsert_collected_account(accounts: &mut Vec<Value>, mut collected: Value) -> Value {
    let collected_uid = get_str(&collected, "uid");
    let collected_email = identity_email(&collected);
    let matches_identity = |existing: &Value| {
        if let Some(uid) = collected_uid.as_deref() {
            return get_str(existing, "uid").as_deref() == Some(uid);
        }
        collected_email
            .as_deref()
            .is_some_and(|email| identity_email(existing).as_deref() == Some(email))
    };

    let matching_indexes: Vec<usize> = accounts
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| matches_identity(existing).then_some(index))
        .collect();

    if let Some(&first_index) = matching_indexes.first() {
        let existing = &accounts[first_index];

        // WorkBuddy 5.6 加密态保护：本机重导入得到的是加密信封 token；若已有
        // 记录仍持有未过期的明文 token（OAuth 扫码所得），不得让信封覆盖明文
        // —— 否则 UI 每次自动 importLocal 都会把扫码凭据冲掉，签到/积分等
        // 需要明文 token 的功能随之失效。明文过期后才放行信封接管。
        if is_envelope(&collected, "access_token") && has_unexpired_plain_token(existing) {
            if get_str(existing, "id").is_some() {
                return existing.clone();
            }
            // 凭据保护短路时也修复缺失的本地 id，但不动任何 token 或展示字段。
            let mut preserved = existing.clone();
            preserved["id"] = get_str(&collected, "id")
                .map(Value::String)
                .unwrap_or_else(|| Value::String(uuid::Uuid::new_v4().to_string()));
            accounts[first_index] = preserved.clone();
            return preserved;
        }
        // 展示字段兜底：新采集为信封时保留已有记录的明文展示值。
        for key in ["nickname", "email", "enterpriseName"] {
            if is_envelope(&collected, key) {
                if let Some(v) = existing.get(key) {
                    collected[key] = v.clone();
                }
            }
        }
        // 本地展示字段（备注 / 显示选择）不来自官方采集：采集结果缺失时保留已有值。
        for key in ["note", "displayField"] {
            if collected.get(key).is_none() {
                if let Some(v) = existing.get(key) {
                    collected[key] = v.clone();
                }
            }
        }

        if let Some(existing_id) = get_str(existing, "id") {
            collected["id"] = Value::String(existing_id);
        } else if get_str(&collected, "id").is_none() {
            // 同 uid 的历史记录也可能缺 id；命中覆盖分支时同样维持账号库不变量。
            collected["id"] = Value::String(uuid::Uuid::new_v4().to_string());
        }
        if get_str(&collected, "uid").is_none() {
            if let Some(existing_uid) = existing.get("uid").cloned() {
                collected["uid"] = existing_uid;
            }
        }
        if let Some(created_at) = existing.get("createdAt").cloned() {
            collected["createdAt"] = created_at;
        }
        inherit_existing_variant(existing, &mut collected);

        for index in matching_indexes.into_iter().rev() {
            accounts.remove(index);
        }
        accounts.insert(first_index.min(accounts.len()), collected.clone());
    } else {
        // 追加分支同样保证入库记录带 id（账号库不允许无 id 记录）。
        if get_str(&collected, "id").is_none() {
            collected["id"] = Value::String(uuid::Uuid::new_v4().to_string());
        }
        accounts.push(collected.clone());
    }

    collected
}

/// 使用统一身份规则保存采集到的账号。
pub fn save_collected_account(collected: Value) -> std::io::Result<Value> {
    let mut accounts = load_accounts();
    let saved = upsert_collected_account(&mut accounts, collected);
    save_accounts(&accounts)?;
    Ok(saved)
}
