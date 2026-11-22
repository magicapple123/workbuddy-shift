//! 控制台接口（`/console/as/conversations/`）：第 1 步建会话、第 2 步取沙箱，
//! 以及第 5 步的 `stop` / `delete` 收尾与腾活跃名额。
//!
//! 鉴权只用网页端形态：`Authorization: Bearer <account.access_token>` +
//! `X-User-Id: <uid>`，**不带**桌面端指纹（这是网页通道与签到链路的本质差别）。
//! 这些头里有 token，因此本文件的任何错误文案都不得包含请求头内容。

use serde_json::{json, Value};

use crate::modules::account::variant_of;

use super::super::scrub;
use super::super::transport::{HttpRequest, HttpResponse, Transport, WEB_USER_AGENT};
use super::{step, truncate, StepFailure, PROMPT_TEXT};

/// 会话集合端点，相对档位 origin（国际版 `https://www.workbuddy.ai`）。
const CONVERSATIONS_PATH: &str = "/console/as/conversations/";
/// 单账号同时存在的活跃会话上限（上游硬限制，用于错误文案）。
const ACTIVE_LIMIT: usize = 10;
/// 网页端 `Accept`（控制台 JSON 接口）。
const ACCEPT_WEB: &str = "application/json, text/plain, */*";

/// 沙箱连接信息（第 3 步要用）。**不实现 `Debug`**：`token` 在此。
pub(super) struct SessionTarget {
    pub(super) link: String,
    pub(super) token: String,
    pub(super) session_id: String,
    pub(super) cwd: String,
}

/// 收尾动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    /// 成功收尾：会话留在历史里，只改状态。
    Stop,
    /// 失败收尾：从网页端删掉，避免堆测试会话。
    Delete,
}

impl Action {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

/// 网页端出站头：只有 bearer 与 `X-User-Id`，没有桌面端指纹。
fn web_headers(account: &Value) -> Vec<(String, String)> {
    let token = account
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let uid = account
        .get("uid")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let origin = variant_of(account).api_endpoint();
    vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-User-Id".to_string(), uid.to_string()),
        ("Accept".to_string(), ACCEPT_WEB.to_string()),
        ("Origin".to_string(), origin.to_string()),
        ("Referer".to_string(), format!("{origin}/app")),
        ("User-Agent".to_string(), WEB_USER_AGENT.to_string()),
    ]
}

// ---------------------------------------------------------------------------
// 第 1 步：建会话
// ---------------------------------------------------------------------------

/// 建一条网页会话，返回会话 id；活跃名额满时先腾一条再重试一次。
///
/// 「建会话只是排队」：不接沙箱、不请求这一轮，agent 永远不会跑，会话会停在
/// `CREATING`。所以这一步成功**不等于**打卡成功。
pub(super) async fn create_conversation(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    model: &str,
    limits: &super::Limits,
) -> Result<String, StepFailure> {
    match post_create(transport, account, origin, model, limits).await {
        Ok(conversation) => Ok(conversation),
        Err(failure) if failure.blocked => {
            let freed = prune_oldest(transport, account, origin, limits).await;
            if freed == 0 {
                return Err(StepFailure::blocked(
                    "create",
                    format!(
                        "活跃会话已达上限（{ACTIVE_LIMIT}），且没有可自动结束的旧会话；\
                         请到 {origin}/app 结束旧对话后重试"
                    ),
                ));
            }
            match post_create(transport, account, origin, model, limits).await {
                Ok(conversation) => Ok(conversation),
                Err(second) => Err(StepFailure::blocked(
                    "create",
                    format!(
                        "活跃会话已达上限（{ACTIVE_LIMIT}），已结束 {freed} 条旧会话后仍失败: {}",
                        second.message
                    ),
                )),
            }
        }
        Err(failure) => Err(failure),
    }
}

/// 发一次建会话请求（失败按 `max_attempts` 重试，5xx / 传输错误才算可重试）。
async fn post_create(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    model: &str,
    limits: &super::Limits,
) -> Result<String, StepFailure> {
    let url = format!("{origin}{CONVERSATIONS_PATH}");
    let body = json!({
        "prompt": PROMPT_TEXT,
        "model": model,
        // 网页端建会话时固定带上这两项（抓包所得），保持请求形态一致。
        "conversationOrigin": "workbuddy-app",
        "plugins": [{"name": "weixinpay", "marketplace": "codebuddy-builtin"}],
    });
    let attempts = limits.max_attempts.max(1);
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let request =
            HttpRequest::post_json(url.clone(), body.clone()).headers(&web_headers(account));
        let response = match step("建会话", limits.step_timeout, transport.send(request)).await {
            Ok(response) => response,
            Err(error) => {
                last_error = error;
                if attempt < attempts {
                    tokio::time::sleep(limits.retry_delay).await;
                }
                continue;
            }
        };
        if is_active_limit(response.status, &response.body) {
            return Err(StepFailure::blocked(
                "create",
                format!(
                    "活跃会话已达上限: HTTP {} {}",
                    response.status,
                    response_message(&response)
                ),
            ));
        }
        if response.status >= 500 {
            last_error = format!("HTTP {}", response.status);
            if attempt < attempts {
                tokio::time::sleep(limits.retry_delay).await;
            }
            continue;
        }
        if response.status >= 400 {
            return Err(StepFailure::new(
                "create",
                format!(
                    "建会话 HTTP {}: {}",
                    response.status,
                    response_message(&response)
                ),
            ));
        }
        let payload = response.json().unwrap_or_else(|| json!({}));
        let code = payload.get("code").and_then(Value::as_i64);
        let conversation = payload
            .get("data")
            .and_then(|data| data.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if code == Some(10105) {
            return Err(StepFailure::blocked(
                "create",
                format!("活跃会话已达上限: {}", message_of(&payload)),
            ));
        }
        if matches!(code, Some(0) | None) && !conversation.is_empty() {
            return Ok(conversation);
        }
        return Err(StepFailure::new(
            "create",
            format!("建会话被拒: code={code:?} msg={}", message_of(&payload)),
        ));
    }
    Err(StepFailure::new(
        "create",
        format!("建会话失败: {last_error}"),
    ))
}

/// 停掉最旧的一条活跃会话，腾出一个名额。返回腾出的条数（0 或 1）。
async fn prune_oldest(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    limits: &super::Limits,
) -> usize {
    let url = format!("{origin}{CONVERSATIONS_PATH}?page=1&pageSize=50");
    let request = HttpRequest::get(url).headers(&web_headers(account));
    let Ok(response) = step("列出会话", limits.step_timeout, transport.send(request)).await
    else {
        return 0;
    };
    let Some(list) = response
        .json()
        .and_then(|payload| payload.get("data").cloned())
        .and_then(|data| data.get("conversations").cloned())
        .and_then(|value| value.as_array().cloned())
    else {
        return 0;
    };
    // 列表按时间倒序，从尾部找最旧的活跃会话。
    for item in list.iter().rev() {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty() || !matches!(status, "working" | "creating" | "running") {
            continue;
        }
        if finish_conversation(transport, account, origin, id, Action::Stop, limits).await {
            return 1;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// 第 2 步：取沙箱地址 / token
// ---------------------------------------------------------------------------

/// 取沙箱信息。沙箱起得慢时会短暂返回空 `link/token`，因此按 `max_attempts` 有界重试。
pub(super) async fn fetch_session(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    conversation: &str,
    limits: &super::Limits,
) -> Result<SessionTarget, StepFailure> {
    let url = format!("{origin}{CONVERSATIONS_PATH}{conversation}/session");
    let attempts = limits.max_attempts.max(1);
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let request = HttpRequest::get(url.clone()).headers(&web_headers(account));
        let response = match step("取沙箱", limits.step_timeout, transport.send(request)).await {
            Ok(response) => response,
            Err(error) => {
                last_error = error;
                if attempt < attempts {
                    tokio::time::sleep(limits.retry_delay).await;
                }
                continue;
            }
        };
        if !response.is_success() {
            last_error = format!("HTTP {}", response.status);
            if attempt < attempts {
                tokio::time::sleep(limits.retry_delay).await;
            }
            continue;
        }
        let Some(data) = response
            .json()
            .and_then(|payload| payload.get("data").cloned())
        else {
            last_error = "响应不是 JSON".to_string();
            if attempt < attempts {
                tokio::time::sleep(limits.retry_delay).await;
            }
            continue;
        };
        let link = first_str(&data, &["link", "endpoint"]);
        let token = first_str(&data, &["token"]);
        if link.is_empty() || token.is_empty() {
            last_error = "沙箱未就绪（没有 link/token）".to_string();
            if attempt < attempts {
                tokio::time::sleep(limits.retry_delay).await;
            }
            continue;
        }
        let session_id = {
            let value = first_str(&data, &["sessionId", "session_id"]);
            if value.is_empty() {
                conversation.to_string()
            } else {
                value
            }
        };
        let cwd = {
            let value = first_str(&data, &["cwd"]);
            if value.is_empty() {
                "/workspace".to_string()
            } else {
                value
            }
        };
        return Ok(SessionTarget {
            link,
            token,
            session_id,
            cwd,
        });
    }
    Err(StepFailure::new(
        "session",
        format!("取沙箱失败: {last_error}"),
    ))
}

// ---------------------------------------------------------------------------
// 第 4 步：查会话状态
// ---------------------------------------------------------------------------

/// 查一次会话状态（`data.status`）。
pub(super) async fn conversation_status(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    conversation: &str,
    limits: &super::Limits,
) -> Result<String, String> {
    let url = format!("{origin}{CONVERSATIONS_PATH}{conversation}");
    let request = HttpRequest::get(url).headers(&web_headers(account));
    let response = step("查询会话状态", limits.step_timeout, transport.send(request)).await?;
    if !response.is_success() {
        return Err(format!("HTTP {}", response.status));
    }
    response
        .json()
        .and_then(|payload| payload.get("data").cloned())
        .and_then(|data| data.get("status").cloned())
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| "会话响应没有 data.status".to_string())
}

// ---------------------------------------------------------------------------
// 第 5 步：收尾
// ---------------------------------------------------------------------------

/// 对会话执行收尾动作；返回是否被服务端接受（失败只作为提示，不改判本轮结果）。
pub(super) async fn finish_conversation(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    conversation: &str,
    action: Action,
    limits: &super::Limits,
) -> bool {
    let url = format!(
        "{origin}{CONVERSATIONS_PATH}{conversation}/{}",
        action.as_str()
    );
    let attempts = limits.max_attempts.max(1);
    for attempt in 1..=attempts {
        let request = HttpRequest::post_json(url.clone(), json!({})).headers(&web_headers(account));
        if let Ok(response) = step(
            action.as_str(),
            limits.step_timeout,
            transport.send(request),
        )
        .await
        {
            if response.is_success() {
                let code = response
                    .json()
                    .and_then(|payload| payload.get("code").and_then(Value::as_i64));
                if matches!(code, Some(0) | None) {
                    return true;
                }
            }
        }
        if attempt < attempts {
            tokio::time::sleep(limits.retry_delay).await;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 依次尝试多个键，返回第一个非空字符串。
fn first_str(data: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| data.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// 建会话是否因为「活跃会话名额满」被拒（429 + 10105 / 英文提示）。
fn is_active_limit(status: u16, body: &str) -> bool {
    if status != 429 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    body.contains("10105") || lower.contains("maximum number of active")
}

/// 从 JSON 响应里提取业务 `msg`（截断 + 脱敏）。
fn message_of(payload: &Value) -> String {
    let text = payload
        .get("msg")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    truncate(&scrub(text))
}

/// 兜底文案：JSON 里没有 `msg`、或 `msg` / 正文本身就是一整页网页时用分类文案。
///
/// 本模块有自己的 reqwest 封装（沙箱域名 DoH 兜底），不经过
/// `config::http_request`，但错误文案同样会进「本轮失败」提示，因此**共用**
/// `config::failure` 的判据：绝不把 `response.body` 截一段原文回显（可能是整页
/// HTML 登录页或网关 502 页，issue #189）。
fn response_message(response: &HttpResponse) -> String {
    let direct = response
        .json()
        .map(|payload| message_of(&payload))
        .unwrap_or_default();
    if !direct.is_empty() && !crate::modules::config::contains_markup(&direct) {
        return direct;
    }
    crate::modules::config::user_message_for_body(response.status, &response.body)
}
