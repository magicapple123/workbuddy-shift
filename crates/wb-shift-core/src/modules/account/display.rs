//! 展示与鉴权字段：显示名/元数据、显示更新、信封错误与鉴权头。

use super::identity::*;
use super::store::*;
use super::*;

/// 从 profile_raw 读取官方手机号（仅接受非空字符串）。
///
/// 手机号不落库、每次实时读取，避免与官方数据产生第二份副本。
pub(super) fn profile_phone_number(acc: &Value) -> Option<String> {
    acc.get("profile_raw")
        .and_then(|p| p.get("phoneNumber"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 账号展示名：按本地 `displayField` 选择（note / phone / nickname），
/// 兜底 email → nickname → uid → unknown。
///
/// `displayField` 缺失或取值异常时行为与改造前一致（email → nickname → uid）。
pub fn account_display_name(acc: &Value) -> String {
    let by_field = match get_str(acc, "displayField").as_deref() {
        Some("note") => get_str(acc, "note"),
        Some("phone") => profile_phone_number(acc),
        _ => None,
    };
    by_field
        .or_else(|| get_str(acc, "email"))
        .or_else(|| get_str(acc, "nickname"))
        .or_else(|| get_str(acc, "uid"))
        .unwrap_or_else(|| "unknown".to_string())
}

/// 账号的展示元数据（不泄露 token）。对照 server.py `account_meta`。
/// 展示字段一律走 `display_value`：WorkBuddy 5.6 起 nickname/phoneNumber 可能是
/// 加密信封对象，裸透传会导致前端 React error #31（白屏）。
pub fn account_meta(acc: &Value) -> Value {
    json!({
        "id": display_value(acc, "id"),
        "uid": display_value(acc, "uid"),
        "email": display_value(acc, "email"),
        "nickname": display_value(acc, "nickname"),
        "phoneNumber": profile_phone_number(acc).map(Value::String).unwrap_or(Value::Null),
        "note": display_value(acc, "note"),
        "displayField": display_value(acc, "displayField"),
        "enterpriseName": display_value(acc, "enterpriseName"),
        "expiresAt": display_value(acc, "expiresAt"),
        "refreshExpiresAt": display_value(acc, "refreshExpiresAt"),
        "refreshedAt": display_value(acc, "refreshedAt"),
        "createdAt": display_value(acc, "createdAt"),
        "needsRelogin": acc.get("needs_relogin").and_then(|v| v.as_bool()) == Some(true),
        "needsReloginReason": display_value(acc, "needs_relogin_reason"),
        // 企业版标记（issue #167）：读取时判定，供前端展示「企业版」徽标，历史数据零迁移。
        "isEnterprise": is_enterprise_account(acc),
        // 档位随元数据下发，供宿主按档位过滤列表（缺省国内版，历史数据零迁移）。
        "variant": variant_of(acc).as_str(),
    })
}

/// 取非空字符串字段；空/缺失返回 None。
pub fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 展示型字段安全读取：标量原样返回；对象/数组（如 WorkBuddy 5.6 引入的
/// `{$wbEncrypted, envelope}` 加密信封）折叠为 Null，避免对象漏进前端
/// 被当作 React 子节点渲染导致整树卸载（白屏）。
pub fn display_value(acc: &Value, key: &str) -> Value {
    match acc.get(key) {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null)) => v.clone(),
        _ => Value::Null,
    }
}

/// 备注长度上限（字符数），与前端输入框 maxLength 保持一致。
pub(super) const NOTE_MAX_CHARS: usize = 24;

/// 更新账号的本地展示字段：`note`（字符串或 null=清空）与 `displayField`。
///
/// 只触碰本地展示字段，不修改 uid / token / profile_raw 等官方数据。
/// 先整体校验再写入，避免部分更新。返回更新后的 `account_meta`。
pub fn update_account_display(account_id: &str, patch: &Value) -> Result<Value, String> {
    let mut accounts = load_accounts();
    let updated = update_account_display_in(&mut accounts, account_id, patch)?;
    save_accounts(&accounts).map_err(|error| error.to_string())?;
    Ok(account_meta(&updated))
}

/// 更新账号展示字段的内存实现（不落盘，便于单测）。
///
/// 校验失败时不修改任何记录；成功时返回更新后的完整记录。
pub fn update_account_display_in(
    accounts: &mut [Value],
    account_id: &str,
    patch: &Value,
) -> Result<Value, String> {
    let index = accounts
        .iter()
        .position(|a| get_str(a, "id").as_deref() == Some(account_id))
        .ok_or_else(|| "账号不存在".to_string())?;

    let note_update = match patch.get("note") {
        None => None,
        Some(Value::Null) => Some(Value::Null),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Some(Value::Null)
            } else if trimmed.chars().count() > NOTE_MAX_CHARS {
                return Err(format!("备注不能超过 {NOTE_MAX_CHARS} 个字符"));
            } else {
                Some(Value::String(trimmed.to_string()))
            }
        }
        Some(_) => return Err("备注格式无效".to_string()),
    };
    let field_update = match patch.get("displayField") {
        None => None,
        Some(value) => {
            let field = value.as_str().unwrap_or("");
            if !matches!(field, "nickname" | "phone" | "note") {
                return Err("显示字段无效".to_string());
            }
            Some(Value::String(field.to_string()))
        }
    };

    let obj = accounts[index]
        .as_object_mut()
        .ok_or_else(|| "账号记录损坏".to_string())?;
    if let Some(note) = note_update {
        obj.insert("note".to_string(), note);
    }
    if let Some(field) = field_update {
        obj.insert("displayField".to_string(), field);
    }

    Ok(accounts[index].clone())
}

/// WorkBuddy 5.6 加密信封凭据的可读错误：`access_token` 为信封形态时返回提示文案。
///
/// 信封 token 无法解出明文，不能用于签到 / 积分 / 旅行等 API 请求；此前会经
/// [`build_auth_headers`] 的 `unwrap_or_default()` 兜底成空 `Bearer`，被网关
/// 401 后再把 HTML 错误页原样回显到界面（issue #94）。需要账号身份的请求
/// 发出前应先用本函数短路。
pub fn envelope_token_error(account: &Value) -> Option<String> {
    if is_envelope(account, "access_token") {
        return Some(
            "该账号凭据为 WorkBuddy 加密信封态，无法直接调用签到 / 积分 / Token 统计等接口；\
             切换功能不受影响，如需上述功能请删除该账号后改用「OAuth 扫码添加」获取明文凭据。"
                .to_string(),
        );
    }
    None
}

/// 构造与官方对齐的请求头。对照 server.py `build_auth_headers`。
///
/// 注意：`access_token` 为加密信封对象时 `get_str` 取不到值，这里会产出空
/// `Bearer`——调用方必须先用 [`envelope_token_error`] 拦截，不要把空凭据
/// 真的发出去（issue #94）。
pub fn build_auth_headers(account: &Value) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!(
            "Bearer {}",
            get_str(account, "access_token").unwrap_or_default()
        ),
    );
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    if let Some(uid) = get_str(account, "uid") {
        headers.insert("X-User-Id".to_string(), uid);
    }
    if let Some(eid) = enterprise_id_of(account) {
        headers.insert("X-Enterprise-Id".to_string(), eid.clone());
        headers.insert("X-Tenant-Id".to_string(), eid);
    }
    if let Some(domain) = get_str(account, "domain") {
        headers.insert("X-Domain".to_string(), domain);
    }
    headers
}
