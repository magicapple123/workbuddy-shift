//! 会话/凭据桥接：会话 JSON 构建、secret token 解析与账号匹配。

#[allow(unused_imports)]
use super::status::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

/// 构造注入到 CN IDE 的会话 JSON（与 CN 客户端登录态写入结构一致）。
pub fn build_session_json(acc: &Value) -> String {
    let uid = get_str(acc, "uid").unwrap_or_default();
    let nickname = get_str(acc, "nickname").unwrap_or_default();
    let enterprise_id = get_str(acc, "enterpriseId")
        .or_else(|| get_str(acc, "enterprise_id"))
        .unwrap_or_default();
    let enterprise_name = get_str(acc, "enterpriseName")
        .or_else(|| get_str(acc, "enterprise_name"))
        .unwrap_or_default();
    let domain = codebuddy_domain_for(
        get_str(acc, "domain").unwrap_or_default().as_str(),
        account::variant_of(acc),
    );
    let refresh_token = get_str(acc, "refresh_token").unwrap_or_default();
    let access_token = get_str(acc, "access_token").unwrap_or_default();
    let token_type = get_str(acc, "token_type").unwrap_or_else(|| "Bearer".to_string());
    let expires_at = acc.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);

    json!({
        "id": "Tencent-Cloud.genie-ide-cn",
        "token": access_token,
        "refreshToken": refresh_token,
        "expiresAt": expires_at,
        "domain": domain,
        "accessToken": format!("{uid}+{access_token}"),
        "converted": true,
        "account": {
            "id": uid,
            "uid": uid,
            "label": nickname,
            "nickname": nickname,
            "enterpriseId": enterprise_id,
            "enterpriseName": enterprise_name,
            "pluginEnabled": true,
            "lastLogin": true,
        },
        "auth": {
            "accessToken": access_token,
            "refreshToken": refresh_token,
            "tokenType": token_type,
            "domain": domain,
            "expiresAt": expires_at,
            "expiresIn": expires_at,
            "refreshExpiresIn": 0,
            "refreshExpiresAt": 0,
            "lastRefreshTime": now_ms(),
        }
    })
    .to_string()
}

/// 从 secret 明文（JSON 或 `uid+token`）解析出 (uid, access_token)。
///
/// 同时供 CodeBuddy CN IDE 与 VS Code CodeBuddy 扩展（`vscode_ext`）复用。
pub(crate) fn parse_token_from_secret(secret: &str) -> Option<(Option<String>, String)> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let token = value
            .get("token")
            .or_else(|| value.get("access_token"))
            .or_else(|| value.get("accessToken"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("auth")
                    .and_then(|a| a.get("accessToken").or_else(|| a.get("access_token")))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })?;
        let uid = value
            .get("uid")
            .or_else(|| value.pointer("/account/uid"))
            .or_else(|| value.pointer("/account/id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // accessToken 可能是 `uid+token`
        if let Some((prefix, suffix)) = token.split_once('+') {
            let suffix = suffix.trim();
            if !suffix.is_empty() {
                let uid = uid.or_else(|| {
                    let p = prefix.trim();
                    if p.is_empty() {
                        None
                    } else {
                        Some(p.to_string())
                    }
                });
                return Some((uid, suffix.to_string()));
            }
        }
        return Some((uid, token));
    }
    if let Some((prefix, suffix)) = trimmed.split_once('+') {
        let suffix = suffix.trim();
        if !suffix.is_empty() {
            let uid = {
                let p = prefix.trim();
                if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                }
            };
            return Some((uid, suffix.to_string()));
        }
    }
    Some((None, trimmed.to_string()))
}

/// 在账号库中按 uid / access_token 匹配账号；供 CN IDE 与 VS Code 扩展复用。
pub(crate) fn match_account_for_token(uid: Option<&str>, token: &str) -> Option<Value> {
    let accounts = account::load_accounts();
    if let Some(uid) = uid.filter(|s| !s.is_empty()) {
        if let Some(acc) = accounts
            .iter()
            .find(|a| get_str(a, "uid").as_deref() == Some(uid))
        {
            return Some(acc.clone());
        }
    }
    accounts
        .into_iter()
        .find(|a| get_str(a, "access_token").as_deref() == Some(token))
}
