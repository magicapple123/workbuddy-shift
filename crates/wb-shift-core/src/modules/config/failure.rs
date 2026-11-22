//! 传输层失败响应的统一分类：把「响应不是预期的 JSON」收敛成结构化错误（issue #189）。
//!
//! # 为什么放在这里
//!
//! [`http_request`](super::http::http_request) / [`http_request_with_proxy`](super::http::http_request_with_proxy)
//! 是**所有**上游请求的唯一收口点：积分、限额、签到、旅行、企业配额、刷新令牌、OAuth
//! 全部经过它。分类器长在这条边界上，十几个调用点才不需要各写一遍判据——而「每个调用点
//! 各写一遍」正是 issue #189 的原始症状：只有刷新路径认得 HTML 错误页，其它入口把整页
//! 网页（官方登录页或网关 5xx 错误页）当错误文案透给了界面。
//!
//! 放在 `config` 而不是 `refresh` 还有第二个原因：刷新分级
//! （`refresh::classify_refresh_failure`）必须与这里共用同一份判据——「响应是一张网页」
//! 永远不等于「登录被吊销」（issue #94 的教训）——而 `refresh` 本来就依赖 `config`，
//! 反过来放会让依赖方向倒挂（[`crate::modules::config`] 不得依赖刷新链路）。
//!
//! # 四档
//!
//! | 档位 | 判据 | 用户可见文案 |
//! | --- | --- | --- |
//! | [`FailureKind::LoginExpired`] | HTML 且像登录页（`login` / `signin` / `oauth` …） | 登录已失效，需要重新登录 |
//! | [`FailureKind::GatewayHtml`] | HTML 但不是登录页（网关 / 代理错误页） | 服务端返回了网页而不是数据，可能是代理或网关拦截 |
//! | [`FailureKind::Upstream`] | JSON 里有 `error` / `message` | 原样使用（官方给的人话） |
//! | [`FailureKind::Malformed`] | 空响应 / 非 JSON 文本 | 返回格式异常（HTTP xxx） |
//!
//! # 不回显原始正文
//!
//! 任何用户可见文案里都**不允许**出现 `<html`、`<!DOCTYPE` 或大段标签：界面会把
//! 一整页网页撑成一坨红字，用户既读不懂也拿不到下一步动作。原始正文这里只做两件事：
//! ① 用 [`strip_markup`] 去标签、② 截到 [`DIAGNOSTIC_LIMIT`] 个字符，拼成
//! [`diagnostic_summary`] 写进日志；界面只拿 [`FailureKind::message`] 的固定文案。
//!
//! # 与刷新分级的关系
//!
//! 这里只定「文案与分类」，不定「是否要求重新登录」：那只由
//! `refresh::classify_refresh_failure` 决定（`Revoked` 才写 `needs_relogin`）。
//! 两边通过 [`FailureKind::as_wire`] / [`is_html_failure`] 这一对字段通信，
//! 刷新分级读到 `failureKind` 就知道「这是一张网页」，从而维持原来的「暂时失败」。

use serde_json::{json, Value};

use super::http::normalize_error_body;

/// 诊断摘要的字符上限（不是字节：中文一个字算一个）。
///
/// 只进日志，不进界面；120 字符足够看清「哪个网关、什么标题」，又不至于把日志行撑爆。
pub const DIAGNOSTIC_LIMIT: usize = 120;

/// `failureKind` 字段名：结构化失败对象里携带档位的键。
///
/// 用独立字段而不是往 `message` 里塞标记，是为了让「用户看到的文案」与「机器判定」
/// 各走一条线：改文案不会改判定，改判定也不会把内部标记漏到界面上。
pub const FAILURE_KIND_FIELD: &str = "failureKind";

/// `httpStatus` 字段名：结构化失败对象里携带 HTTP 状态码的键（排障用）。
pub const HTTP_STATUS_FIELD: &str = "httpStatus";

/// 失败档位（四档，见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 服务端给的是网页，且像官方登录页：登录态已失效，需要重新登录。
    LoginExpired,
    /// 服务端给的是网页，但不是登录页：代理 / 网关拦下来了。
    GatewayHtml,
    /// 服务端给的是 JSON，里面有人话（`error` / `message`）：直接用它。
    Upstream,
    /// 空响应、非 JSON 文本：连人话都没有。
    Malformed,
}

impl FailureKind {
    /// 写进 `failureKind` 字段的稳定取值。
    ///
    /// 取值刻意用 snake_case 英文：它是机器契约（刷新分级、日志、将来的诊断导出都读它），
    /// 不能跟着中文文案一起改。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::LoginExpired => "login_expired",
            Self::GatewayHtml => "gateway_html",
            Self::Upstream => "upstream",
            Self::Malformed => "malformed",
        }
    }

    /// 从 `failureKind` 字段还原档位；未知取值返回 `None`。
    ///
    /// 未知取值一律按「不知道」处理而不是 panic：老版本客户端 / 新版本服务端混跑时，
    /// 读到不认识的档位只该退化成「没有额外信息」，不该把整次请求炸掉。
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim() {
            "login_expired" => Some(Self::LoginExpired),
            "gateway_html" => Some(Self::GatewayHtml),
            "upstream" => Some(Self::Upstream),
            "malformed" => Some(Self::Malformed),
            _ => None,
        }
    }

    /// 该档位是否代表「服务端给了一张网页」。
    ///
    /// 刷新分级用这一条把 HTML 401/502 排除在 `Revoked` 之外（issue #94）。
    pub fn is_html(self) -> bool {
        matches!(self, Self::LoginExpired | Self::GatewayHtml)
    }

    /// 用户可见文案：说清「发生了什么」并给出下一步，**不含**任何原始正文。
    ///
    /// `status` 只在 [`FailureKind::Malformed`] 用得上（那里没有任何信息可讲，
    /// 状态码是唯一能帮用户判断「谁错了」的东西）。
    pub fn message(self, status: u16) -> String {
        match self {
            Self::LoginExpired => "登录已失效，需要重新登录".to_string(),
            Self::GatewayHtml => "服务端返回了网页而不是数据，可能是代理或网关拦截".to_string(),
            Self::Upstream => "上游未给出可读原因".to_string(),
            Self::Malformed => format!("返回格式异常（HTTP {status}）"),
        }
    }
}

/// 登录页特征词（小写比较）。
///
/// 刻意窄：只收「这张网页本身就是登录入口」的词。`authorization` 一类**不能**收——
/// nginx 的 `401 Authorization Required` 是网关错误页，收进来会把代理故障误报成
/// 「登录已失效」，用户会白重登一次（正是 issue #94 要避免的误判方向）。
const LOGIN_MARKERS: [&str; 8] = [
    "login", "signin", "sign-in", "sign_in", "oauth", "passport", "登录", "扫码",
];

/// 标记（HTML/XML）特征：「这段文本是网页而不是数据」。
///
/// 除了直接以 `<` 开头，还要认「正文前面有半句英文」的形态（`Error: <html>…`），
/// 因此对常见开合标签做一次包含判断。
const MARKUP_MARKERS: [&str; 7] = [
    "<html",
    "<!doctype",
    "<head",
    "<body",
    "</",
    "<meta",
    "<script",
];

/// 文本是否像一段标记（网页）而不是数据。
pub fn contains_markup(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with('<') {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    MARKUP_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// 去掉标记后压平空白，得到可写进日志的一小段摘要。
///
/// 只吞**成对**的 `<…>`：`<title>登录</title>` 只留下 `登录`，而正文里的
/// `limit a < b` 这种没有闭合 `>` 的小于号原样保留（否则会把真实文案的后半截吃掉）。
pub fn strip_markup(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len().min(512));
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '<' {
            // 有闭合的 `>` 就是标签，整段跳过；没有就按普通字符输出（见上面的说明）。
            if let Some(offset) = chars[index..].iter().position(|ch| *ch == '>') {
                index += offset + 1;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 排障用的脱敏摘要：`HTTP 401 · 401 Authorization Required`，限长 [`DIAGNOSTIC_LIMIT`]。
///
/// **只写日志，不进界面。** 复用 `normalize_error_body` 的既有口径（HTML 取 `<title>`，
/// 其余按 500 字符截断），再统一去标签并限长，保证「正文以外的东西」不会被带出来。
pub fn diagnostic_summary(status: u16, body: &str) -> String {
    let normalized = normalize_error_body(body);
    let cleaned = strip_markup(&normalized);
    let head: String = cleaned.trim().chars().take(DIAGNOSTIC_LIMIT).collect();
    if head.is_empty() {
        format!("HTTP {status} · 空响应体")
    } else {
        format!("HTTP {status} · {head}")
    }
}

/// 从响应体判定档位（纯函数）。
///
/// 只该用在「JSON 解析失败」的正文上：JSON 走 [`sanitize_error_payload`]，
/// 那里文案已经是人话（档位 [`FailureKind::Upstream`]）。
pub fn classify_body(body: &str) -> FailureKind {
    let text = body.trim();
    if text.is_empty() {
        return FailureKind::Malformed;
    }
    if !contains_markup(text) {
        // 不是网页、也不是 JSON（调用方只在解析失败时进来）：纯文本同样不能透传原文，
        // 一律按格式异常处理，只保留状态码。
        return FailureKind::Malformed;
    }
    let lower = text.to_ascii_lowercase();
    if LOGIN_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return FailureKind::LoginExpired;
    }
    FailureKind::GatewayHtml
}

/// 直接给「原始正文」时该用的用户可见文案（自建传输层的模块复用这一条）。
///
/// `daily_activity` 有自己的 reqwest 封装（沙箱域名 DoH 兜底），不经过
/// [`http_request`](super::http::http_request)，但错误文案同样会进界面，
/// 因此它也走同一个分类器而不是各自 `body.chars().take(160)`。
pub fn user_message_for_body(status: u16, body: &str) -> String {
    classify_body(body).message(status)
}

/// 非 JSON / 无可用正文的响应 → 结构化失败对象。**这是唯一出口。**
///
/// 返回形状：
///
/// ```json
/// {
///   "code": 502,
///   "message": "服务端返回了网页而不是数据，可能是代理或网关拦截",
///   "failureKind": "gateway_html",
///   "httpStatus": 502
/// }
/// ```
///
/// `code` 的选择有讲究：
/// - 非 2xx → 用 HTTP 状态码，`is_route_missing`（404 回落）与
///   `is_unauthorized`（401 刷新重试）等既有判据照旧生效；
/// - 2xx 却是网页（SPA 回退、网关 200）→ 用既有的 `-1`「本次没拿到可用 JSON」约定。
///   **绝不能保留 200**：那会让 `status_from_response` 之类把「网关返回的登录页」
///   读成「签到状态正常」。
pub fn failure_response(status: u16, body: &str) -> Value {
    let kind = classify_body(body);
    // 正文只在这里落一行日志；返回值里一个字节的原文都不带。
    eprintln!(
        "[http] 响应不是预期 JSON（{}）：{}",
        kind.as_wire(),
        diagnostic_summary(status, body)
    );
    json!({
        "code": if (200..300).contains(&status) {
            -1
        } else {
            i64::from(status)
        },
        "message": kind.message(status),
        FAILURE_KIND_FIELD: kind.as_wire(),
        HTTP_STATUS_FIELD: status,
    })
}

/// 该响应是否被判定成「服务端给的是一张网页」。
///
/// 刷新分级读它：HTML 错误页（含 401 登录页）属于**暂时失败**，不写 `needs_relogin`。
/// 只认 [`FAILURE_KIND_FIELD`]，不去读文案——文案是给人看的，随时可能改。
pub fn is_html_failure(response: &Value) -> bool {
    response
        .get(FAILURE_KIND_FIELD)
        .and_then(Value::as_str)
        .and_then(FailureKind::from_wire)
        .is_some_and(FailureKind::is_html)
}

/// 逐字段找「夹带标记的文案」，返回它的档位与那段原文（原文只用来写日志）。
fn payload_markup_kind(payload: &Value) -> Option<(FailureKind, &str)> {
    let holders = [Some(payload), payload.get("data")];
    for holder in holders.into_iter().flatten() {
        for key in ["message", "msg", "error"] {
            if let Some(text) = holder.get(key).and_then(Value::as_str) {
                if contains_markup(text) {
                    // 复用同一套判据：夹带网页的文案同样要区分登录页与网关页。
                    return Some((classify_body(text), text));
                }
            }
        }
    }
    None
}

/// 把一处对象里的 `message` / `msg` / `error` 换成分类文案；换到任意一个就返回 `true`。
fn rewrite_error_text(holder: &mut Value, kind: FailureKind, status: u16) -> bool {
    let Some(object) = holder.as_object_mut() else {
        return false;
    };
    let mut rewritten = false;
    for key in ["message", "msg", "error"] {
        let carries_markup = object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(contains_markup);
        if carries_markup {
            object.insert(key.to_string(), json!(kind.message(status)));
            rewritten = true;
        }
    }
    rewritten
}

/// 已解析的 JSON 错误载荷 → 去掉夹带的网页正文。
///
/// 有些网关把 HTML 塞进 JSON 的 `error` / `message`（`{"code":401,"message":"<html>…"}`），
/// 原样透出等于把一整页网页灌进界面。这里只做一件事：**当某条文案是网页时**，
/// 把它换成分类文案并打上 [`FAILURE_KIND_FIELD`]；业务字段（`code` / `data` 的结构）
/// 一个都不动，因此 404 回落、401 刷新重试等既有判定完全不变。
///
/// 没有夹带标记时**逐字返回原载荷**：绝大多数 JSON 错误是官方给的人话
/// （[`FailureKind::Upstream`]），一个字都不该改。
pub fn sanitize_error_payload(status: u16, payload: Value) -> Value {
    if !payload.is_object() {
        return payload;
    }
    let Some((kind, raw_text)) = payload_markup_kind(&payload) else {
        return payload;
    };
    eprintln!(
        "[http] JSON 错误文案里夹带网页正文（{}）：{}",
        kind.as_wire(),
        diagnostic_summary(status, raw_text)
    );
    let mut payload = payload;
    rewrite_error_text(&mut payload, kind, status);
    if let Some(data) = payload.get_mut("data") {
        rewrite_error_text(data, kind, status);
    }
    payload[FAILURE_KIND_FIELD] = json!(kind.as_wire());
    payload[HTTP_STATUS_FIELD] = json!(status);
    payload
}
