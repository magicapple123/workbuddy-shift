//! 五步流程：建会话 → 取沙箱地址 / token → ACP → 轮询到 `completed` → 收尾。
//!
//! # 协议来源
//!
//! 全部报文形态来自官网 web app 的抓包结论（对照上游 `docs/daily-activity.md`），
//! 这里是**逆向协议**：上游一旦收紧非官方客户端识别就可能失效，失效后要按最新抓包
//! 更新这一组文件，不要凭猜测加字段。
//!
//! # 文件分工
//!
//! | 文件 | 职责 |
//! | --- | --- |
//! | 本文件 | 编排（模型顺延、收尾对称）、结果类型、超时/重试上限 |
//! | [`console`] | 控制台接口：建会话 / 取沙箱 / 查状态 / `stop` / `delete` / 腾位 |
//! | [`acp`] | 沙箱侧：SSE 通道 + 四条 JSON-RPC + 轮询到 `completed` |
//!
//! # 收尾对称（硬约束）
//!
//! 上游对单账号同时存在的活跃会话有硬上限，建了必须收尾。因此「建会话成功」之后
//! 的所有步骤都被收进 [`drive_turn`]，**收尾调用点只有一个**（在
//! [`attempt_with_model`] 里）：成功走 `stop`、失败走 `delete`。任何提前返回
//! （取沙箱失败 / ACP 失败 / 轮询失败）都自然落到同一个收尾分支，不会留下孤立的
//! `working` 会话。
//!
//! # 不打印的秘密
//!
//! prompt 内容与 `token` 一律不进文案：`token` 只出现在请求头与 JSON-RPC 头里，
//! 错误信息只带步骤名、HTTP 状态码、业务 `msg`（截断）。涉及 URL 的文案统一过
//! [`crate::modules::daily_activity::scrub`]，避免把沙箱地址的 query 带出去。

mod acp;
mod console;

use std::future::Future;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::modules::account::variant_of;
use crate::modules::config::now_ms;
use crate::modules::variant::WbVariant;

use super::scrub;
use super::transport::Transport;

/// 打卡用的最小 prompt。**内容本身不敏感，但按约定不打印、不落盘。**
pub(super) const PROMPT_TEXT: &str = "Hi";

/// 错误文案里业务 `msg` 的最大长度（字符）。
pub(super) const MESSAGE_LIMIT: usize = 160;

/// 模型候选序列：`hy3` 免费，不可用（限流 / 下线）时才顺延到付费备选。
///
/// 官方国际版模型的积分倍率（`wb_catalog` 的 `credits` 字段，实测 2026-11）：
///
/// - `hy3` x0.00（免费，192k 上下文）；
/// - `deepseek-v4.1-flash` x0.11（300k，effort=high）。
///
/// 打卡只需要一句问候加一段短回复，默认用免费的 `hy3`。注意网页通道**无法**单独
/// 下调思考档位：ACP 只提供 `session/set_model`，没有 reasoning effort 方法，省钱
/// 只能换模型，不能调参。
pub const MODEL_CANDIDATES: [&str; 2] = ["hy3", "deepseek-v4.1-flash"];

/// 各步超时与重试上限。做成结构体是为了单测能把 `poll_interval` 压到毫秒级。
#[derive(Debug, Clone)]
pub struct Limits {
    /// 单步（建会话 / 取沙箱 / ACP 请求 / 查询状态 / 收尾）超时。
    pub step_timeout: Duration,
    /// 整轮（建会话之后等 `completed`）上限。
    pub turn_timeout: Duration,
    /// 轮询会话状态的间隔。
    pub poll_interval: Duration,
    /// 单步失败后的重试上限（**含**首次，至少 1）。
    pub max_attempts: u32,
    /// 查询状态允许的连续失败次数：网络抖动容忍，但不无限轮询。
    pub max_status_failures: u32,
    /// 重试之间的退避。
    pub retry_delay: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            step_timeout: Duration::from_secs(30),
            turn_timeout: Duration::from_secs(120),
            poll_interval: Duration::from_secs(3),
            max_attempts: 2,
            max_status_failures: 3,
            retry_delay: Duration::from_millis(200),
        }
    }
}

/// 单账号的最终结果（逐账号返回，不抛整体错误）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRunResult {
    /// 账号 key（`id` 优先，退 `uid`）。
    pub account_id: String,
    /// 这一轮是否跑完（`completed`）。
    pub ok: bool,
    /// 今天已经做过、本轮直接跳过。
    pub skipped: bool,
    /// 结束时的阶段：`unsupported` / `skipped` / `create` / `session` / `turn` / `poll` / `done`。
    pub phase: String,
    /// 结果里标注**实际用的模型**（`session/set_model` 失败时见 `modelApplied`）。
    pub model: Option<String>,
    /// `session/set_model` 是否被服务端接受；`false` 表示本轮按会话默认模型跑。
    pub model_applied: Option<bool>,
    /// 尝试过的模型，按顺序。
    pub models_tried: Vec<String>,
    /// 会话最后状态（`completed` / `failed` / …）。
    pub status: Option<String>,
    /// 收尾动作：`stop`（成功）或 `delete`（失败）。
    pub cleanup: Option<String>,
    /// 收尾请求是否被服务端接受。
    pub cleanup_ok: Option<bool>,
    /// 本轮耗时（毫秒）。
    pub elapsed_ms: i64,
    /// 失败原因（已脱敏，不含 token / query）。
    pub error: Option<String>,
    /// 不影响结果的提示（例如状态文件写入失败、`stop` 收尾失败）。
    pub notice: Option<String>,
}

impl AccountRunResult {
    /// 档位 / 凭据不支持：未发任何请求。
    pub fn unsupported(account_id: String, error: impl Into<String>) -> Self {
        Self {
            account_id,
            phase: "unsupported".to_string(),
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// 今日已完成：未发任何请求。
    pub fn skipped(account_id: String) -> Self {
        Self {
            account_id,
            ok: true,
            skipped: true,
            phase: "skipped".to_string(),
            ..Default::default()
        }
    }

    fn from_attempt(attempt: Attempt, account_id: &str, models_tried: Vec<String>) -> Self {
        Self {
            account_id: account_id.to_string(),
            ok: attempt.ok,
            skipped: false,
            phase: attempt.phase.to_string(),
            model: Some(attempt.model),
            model_applied: Some(attempt.model_applied),
            models_tried,
            status: attempt.status,
            cleanup: attempt.cleanup.map(str::to_string),
            cleanup_ok: attempt.cleanup_ok,
            elapsed_ms: attempt.elapsed_ms,
            error: attempt.error,
            notice: attempt.notice,
        }
    }
}

/// 某一步的失败。
pub(super) struct StepFailure {
    /// 失败阶段。
    pub(super) phase: &'static str,
    /// 已脱敏的原因。
    pub(super) message: String,
    /// 活跃会话名额满：换模型也建不出会话，调用方应立即终止候选循环。
    pub(super) blocked: bool,
}

impl StepFailure {
    pub(super) fn new(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: scrub(&message.into()),
            blocked: false,
        }
    }

    pub(super) fn blocked(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: scrub(&message.into()),
            blocked: true,
        }
    }
}

/// 单个模型的尝试结果。
#[derive(Default)]
struct Attempt {
    model: String,
    ok: bool,
    blocked: bool,
    phase: &'static str,
    model_applied: bool,
    status: Option<String>,
    cleanup: Option<&'static str>,
    cleanup_ok: Option<bool>,
    error: Option<String>,
    notice: Option<String>,
    elapsed_ms: i64,
}

impl Attempt {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            phase: "create",
            ..Default::default()
        }
    }

    fn apply_failure(&mut self, failure: StepFailure) {
        self.ok = false;
        self.blocked = failure.blocked;
        self.phase = failure.phase;
        self.error = Some(failure.message);
    }
}

/// 对单个账号跑一次：按 [`MODEL_CANDIDATES`] 从便宜到贵依次尝试，成功即停。
///
/// 只有「活跃名额满且腾不出来」会提前终止——换模型也建不出会话。
pub async fn run_account(
    transport: &dyn Transport,
    account: &Value,
    account_id: &str,
    limits: &Limits,
) -> AccountRunResult {
    let started = now_ms();
    if variant_of(account) != WbVariant::Ai {
        return AccountRunResult::unsupported(
            account_id.to_string(),
            "每日活跃会话仅国际版可用；国内版走签到",
        );
    }

    let mut tried: Vec<String> = Vec::with_capacity(MODEL_CANDIDATES.len());
    let mut last: Option<Attempt> = None;
    for model in MODEL_CANDIDATES {
        tried.push(model.to_string());
        let attempt = attempt_with_model(transport, account, model, limits).await;
        let stop = attempt.ok || attempt.blocked;
        last = Some(attempt);
        if stop {
            break;
        }
    }
    let elapsed_ms = now_ms() - started;
    match last {
        Some(attempt) => AccountRunResult::from_attempt(attempt, account_id, tried),
        None => AccountRunResult {
            account_id: account_id.to_string(),
            phase: "create".to_string(),
            models_tried: tried,
            elapsed_ms,
            error: Some("没有可用的模型候选".to_string()),
            ..Default::default()
        },
    }
}

/// 用指定模型完整跑一次，并**在唯一一处**做对称收尾。
async fn attempt_with_model(
    transport: &dyn Transport,
    account: &Value,
    model: &str,
    limits: &Limits,
) -> Attempt {
    let started = now_ms();
    let origin = variant_of(account).api_endpoint();
    let mut attempt = Attempt::new(model);

    // 第 1 步：建会话（活跃名额满时先腾一条再重试一次）。
    let conversation =
        match console::create_conversation(transport, account, origin, model, limits).await {
            Ok(conversation) => conversation,
            Err(failure) => {
                attempt.apply_failure(failure);
                attempt.elapsed_ms = now_ms() - started;
                return attempt;
            }
        };

    // 第 2–4 步：取沙箱 → ACP → 轮询。
    let driven = drive_turn(transport, account, origin, &conversation, model, limits).await;

    // 第 5 步：收尾。成功 `stop`（会话留在历史里，只改状态），失败 `delete`
    // （不在网页端堆一堆只说了「Hi」的测试会话）。
    match driven {
        Ok(done) => {
            let stopped = console::finish_conversation(
                transport,
                account,
                origin,
                &conversation,
                console::Action::Stop,
                limits,
            )
            .await;
            attempt.ok = true;
            attempt.phase = "done";
            attempt.status = Some(done.status);
            attempt.model_applied = done.model_applied;
            attempt.cleanup = Some(console::Action::Stop.as_str());
            attempt.cleanup_ok = Some(stopped);
            if !stopped {
                attempt.notice = Some(
                    "会话已完成（已计分），但 stop 收尾请求未被接受，可到网页端手动结束"
                        .to_string(),
                );
            }
        }
        Err(failure) => {
            let deleted = console::finish_conversation(
                transport,
                account,
                origin,
                &conversation,
                console::Action::Delete,
                limits,
            )
            .await;
            attempt.apply_failure(failure);
            attempt.cleanup = Some(console::Action::Delete.as_str());
            attempt.cleanup_ok = Some(deleted);
        }
    }
    attempt.elapsed_ms = now_ms() - started;
    attempt
}

/// 一轮跑完的结论。
struct TurnDone {
    status: String,
    model_applied: bool,
}

/// 第 2–4 步：取沙箱 → ACP → 轮询到 `completed`。
///
/// 中途任何 `?` 提前返回都会 drop 那个 SSE 句柄（后台读流任务被取消），
/// 收尾由调用方统一处理。
async fn drive_turn(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    conversation: &str,
    model: &str,
    limits: &Limits,
) -> Result<TurnDone, StepFailure> {
    // 第 2 步：取沙箱地址 / token。
    let target = console::fetch_session(transport, account, origin, conversation, limits).await?;
    // 第 3 步：接上沙箱把这一轮跑起来。
    let (stream, model_applied) = acp::acp_open_turn(transport, &target, model, limits).await?;
    // 第 4 步：轮询到 `completed`。`stream` 必须活到轮询结束——后台读流就是保活。
    let status = acp::wait_completed(transport, account, origin, conversation, limits).await?;
    drop(stream);
    Ok(TurnDone {
        status,
        model_applied,
    })
}

/// 给某个异步步骤套独立超时。
pub(super) async fn step<T, F>(label: &str, timeout: Duration, future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(format!("{label}超时（{} ms）", timeout.as_millis())),
    }
}

/// 截到 [`MESSAGE_LIMIT`] 个字符（按字符而非字节，避免切坏 UTF-8）。
pub(super) fn truncate(text: &str) -> String {
    text.chars().take(MESSAGE_LIMIT).collect()
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) mod tests_support {
    use super::*;

    /// 单测用的宽松上限：不等待真实轮询间隔。
    pub(crate) fn fast_limits() -> Limits {
        Limits {
            step_timeout: Duration::from_secs(2),
            turn_timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(1),
            max_attempts: 2,
            max_status_failures: 2,
            retry_delay: Duration::from_millis(1),
        }
    }
}
