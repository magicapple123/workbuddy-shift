//! 展示与鉴权字段：显示名/元数据、显示更新、信封错误与鉴权头。

use super::identity::*;
use super::store::*;
use super::*;
use crate::modules::config::now_ms;

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

/// 账号展示名（**全局唯一口径**，见 R5-07）：备注优先。
///
/// 顺序：`displayField == "phone"` 且有手机号 → 手机号；否则
/// 备注 → 昵称 → 邮箱 → uid → id → `unknown`。
///
/// - 「备注优先」是产品决策：用户自己写的备注必须压过官方昵称 / 邮箱；
/// - `displayField == "nickname"` 不再压制备注（存量数据里被钉在昵称上的账号也会跟着备注走）；
/// - `displayField == "phone"` 且没有手机号时继续往下走（不会因缺少手机号而显示空）。
///
/// 该函数同时驱动积分统计 / 官方用量 / 积分账本写入 / 签到日志展示字段 / CLI 当前账号名，
/// 改动必须与前端 `src/lib/account-display.ts` 的 `displayName()` 逐条一致。
pub fn account_display_name(acc: &Value) -> String {
    let by_phone = if get_str(acc, "displayField").as_deref() == Some("phone") {
        profile_phone_number(acc)
    } else {
        None
    };
    by_phone
        .or_else(|| get_str(acc, "note"))
        .or_else(|| get_str(acc, "nickname"))
        .or_else(|| get_str(acc, "email"))
        .or_else(|| get_str(acc, "uid"))
        .or_else(|| get_str(acc, "id"))
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
        // 用户自定义标记（R5-04）：结构体，不能走 `display_value`（对象会被折叠成 null）。
        "mark": mark_meta(acc),
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

/// 标记文本长度上限（字符数），与前端 `AccountMarkDialog` 的 maxLength 一致（R5-04）。
pub const MARK_TEXT_MAX_CHARS: usize = 16;

/// 标记允许的语义色（与前端 `AccountMarkColor` 一一对应，顺序固定用于单测与文档）。
pub const MARK_COLORS: [&str; 5] = ["warning", "danger", "info", "success", "neutral"];

/// 校验并规范化一条标记对象（`{text, color, createdAt, expiresAt}`）。
///
/// - `text`：trim 后 1–16 个字符，含换行直接拒绝（纯文本展示，不解析 Markdown）；
/// - `color`：`MARK_COLORS` 之一；
/// - `expiresAt`：`null`（不自动过期）或晚于 `now` 的毫秒数，过去时间报「到期时间必须晚于当前时间」；
/// - `createdAt`：由调用方注入（写入取当前时刻；读取沿用记录内已有值），客户端传值一律忽略。
///
/// 读取路径（[`mark_meta`]）传 `now = i64::MIN`：已过期的标记仍然要下发，前端才能显示
/// 「已过期」并让用户续期；写入路径才做「必须晚于当前」的校验。
fn normalize_mark(raw: &Value, created_at: i64, now: i64) -> Result<Value, String> {
    let object = raw.as_object().ok_or_else(|| "标记格式无效".to_string())?;
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("标记内容不能为空".to_string());
    }
    if text.chars().count() > MARK_TEXT_MAX_CHARS {
        return Err(format!("标记不能超过 {MARK_TEXT_MAX_CHARS} 个字符"));
    }
    if text.contains('\n') || text.contains('\r') {
        return Err("标记不能包含换行".to_string());
    }
    let color = object.get("color").and_then(Value::as_str).unwrap_or_default();
    if !MARK_COLORS.contains(&color) {
        return Err("标记颜色无效".to_string());
    }
    let expires_at = match object.get("expiresAt") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Number(number)) => {
            let ms = number.as_i64().ok_or_else(|| "到期时间无效".to_string())?;
            if ms <= now {
                return Err("到期时间必须晚于当前时间".to_string());
            }
            Value::Number(ms.into())
        }
        Some(_) => return Err("到期时间无效".to_string()),
    };
    Ok(json!({
        "text": text,
        "color": color,
        "createdAt": created_at,
        "expiresAt": expires_at,
    }))
}

/// 下发用的标记字段：任何一项不合法（缺 `createdAt`、文本为空、色值未知…）整体返回 `null`。
///
/// 旧账号库没有 `mark` 字段 → 返回 `null`，等价于「无标记」，历史数据零迁移。
pub fn mark_meta(acc: &Value) -> Value {
    let Some(object) = acc.get("mark").and_then(Value::as_object) else {
        return Value::Null;
    };
    let Some(created_at) = object.get("createdAt").and_then(Value::as_i64) else {
        return Value::Null;
    };
    normalize_mark(&Value::Object(object.clone()), created_at, i64::MIN).unwrap_or(Value::Null)
}

/// 更新账号的本地展示字段：`note`（字符串或 null=清空）、`displayField` 与
/// `mark`（标记对象或 null=解除）。
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
    // 标记（R5-04）：对象 = 覆盖保存（单账号只保留一个，重复保存即覆盖），
    // `null` = 解除标记。`createdAt` 由后端生成，客户端传值忽略。
    let mark_update = match patch.get("mark") {
        None => None,
        Some(Value::Null) => Some(Value::Null),
        Some(value) => {
            let now = now_ms();
            Some(normalize_mark(value, now, now)?)
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
    if let Some(mark) = mark_update {
        obj.insert("mark".to_string(), mark);
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
