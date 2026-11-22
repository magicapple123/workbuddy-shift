//! 沙箱侧（ACP over streamable HTTP）：第 3 步把这一轮驱动起来，第 4 步轮询到
//! `completed`。
//!
//! ACP 是 JSON-RPC 2.0：先 `GET <link>` 建 SSE 通道（响应头 `Acp-Connection-Id`），
//! 再 `POST <link>` 依次发 `initialize` → `session/load` → `session/set_model` →
//! `session/prompt`，每条都带同一个 `Acp-Connection-Id`。SSE 通道必须持续被读，
//! 否则服务端会判连接空闲——[`EventStream`] 的 `Drop` 负责取消读流任务。

use serde_json::{json, Value};

use crate::modules::daily_activity::scrub;

use super::super::transport::{EventStream, HttpRequest, HttpResponse, Transport};
use super::console::SessionTarget;
use super::{step, truncate, StepFailure, PROMPT_TEXT};

/// ACP 协议版本。
const PROTOCOL_VERSION: i64 = 1;
/// 网页端 `Accept`（ACP 普通请求：JSON 或 SSE 都可能）。
const ACCEPT_ACP: &str = "application/json, text/event-stream";

/// 接上沙箱，依次发四条 JSON-RPC 请求，返回**必须在整轮期间持有**的 SSE 句柄。
///
/// 返回值里的 `model_applied` 表示 `session/set_model` 是否被接受：失败不致命
/// （本轮按会话默认模型跑），但结果里要如实标注，否则会谎报「用 hy3 免费跑完」。
pub(super) async fn acp_open_turn(
    transport: &dyn Transport,
    target: &SessionTarget,
    model: &str,
    limits: &super::Limits,
) -> Result<(EventStream, bool), StepFailure> {
    // 1) 建立 SSE 通道，响应头里带 `Acp-Connection-Id`。
    let request = HttpRequest::get(target.link.clone())
        .header("Accept", "text/event-stream")
        .header("Authorization", format!("Bearer {}", target.token));
    let stream = step(
        "打开 SSE 通道",
        limits.step_timeout,
        transport.open_stream(request),
    )
    .await
    .map_err(|error| StepFailure::new("turn", error))?;

    // 2) 按网页端顺序发四条 JSON-RPC 请求，每条都带 `Acp-Connection-Id`。
    let calls: [(&'static str, Value, i64, bool); 4] = [
        (
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                },
            }),
            1,
            true,
        ),
        (
            "session/load",
            json!({
                "sessionId": target.session_id,
                "cwd": target.cwd,
                "mcpServers": [],
            }),
            2,
            true,
        ),
        (
            "session/set_model",
            json!({"sessionId": target.session_id, "modelId": model}),
            3,
            false,
        ),
        (
            "session/prompt",
            json!({
                "sessionId": target.session_id,
                "prompt": [{"type": "text", "text": PROMPT_TEXT}],
            }),
            4,
            true,
        ),
    ];
    let attempts = limits.max_attempts.max(1);
    let mut model_applied = false;
    for (method, params, id, fatal) in calls {
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut last_error = String::new();
        let mut succeeded = false;
        for attempt in 1..=attempts {
            let request = HttpRequest::post_json(target.link.clone(), body.clone())
                .header("Content-Type", "application/json")
                .header("Accept", ACCEPT_ACP)
                .header("Acp-Connection-Id", stream.connection_id())
                .header("Authorization", format!("Bearer {}", target.token));
            match step(method, limits.step_timeout, transport.send(request)).await {
                Ok(response) if response.status == 200 || response.status == 202 => {
                    match jsonrpc_error(&response) {
                        // 200 也可能带 JSON-RPC error：这一步实际没成。
                        Some(error) => last_error = error,
                        None => {
                            succeeded = true;
                            break;
                        }
                    }
                }
                Ok(response) => last_error = format!("HTTP {}", response.status),
                Err(error) => last_error = error,
            }
            if attempt < attempts {
                tokio::time::sleep(limits.retry_delay).await;
            }
        }
        if !succeeded {
            if fatal {
                return Err(StepFailure::new(
                    "turn",
                    format!("{method} 失败: {last_error}"),
                ));
            }
            // `session/set_model` 失败不致命：本轮按会话默认模型跑，如实标注即可。
            continue;
        }
        if method == "session/set_model" {
            model_applied = true;
        }
    }
    Ok((stream, model_applied))
}

/// 轮询会话状态直到 `completed`（或失败 / 超时）。
pub(super) async fn wait_completed(
    transport: &dyn Transport,
    account: &Value,
    origin: &str,
    conversation: &str,
    limits: &super::Limits,
) -> Result<String, StepFailure> {
    let deadline = tokio::time::Instant::now() + limits.turn_timeout;
    let mut status = String::new();
    let mut failures = 0_u32;
    loop {
        if tokio::time::Instant::now() >= deadline {
            let shown = if status.is_empty() {
                "未知".to_string()
            } else {
                status.clone()
            };
            return Err(StepFailure::new(
                "poll",
                format!(
                    "会话在 {} ms 内没有跑完（最后状态={shown}）",
                    limits.turn_timeout.as_millis()
                ),
            ));
        }
        match super::console::conversation_status(transport, account, origin, conversation, limits)
            .await
        {
            Ok(next) => {
                failures = 0;
                status = next;
                if status == "completed" {
                    return Ok(status);
                }
                if is_terminal_failure(&status) {
                    return Err(StepFailure::new(
                        "poll",
                        format!("会话状态={status}，本轮不计分"),
                    ));
                }
            }
            Err(error) => {
                // 查询失败可能是网络抖动：容忍到上限，但绝不无限轮询。
                failures += 1;
                if failures >= limits.max_status_failures.max(1) {
                    return Err(StepFailure::new(
                        "poll",
                        format!("连续 {failures} 次查询会话状态失败: {error}"),
                    ));
                }
            }
        }
        tokio::time::sleep(limits.poll_interval).await;
    }
}

/// 终止态且**不会**计分的会话状态。
fn is_terminal_failure(status: &str) -> bool {
    matches!(status, "failed" | "error" | "cancelled" | "canceled")
}

/// 200 但带 JSON-RPC error 时的错误文案。
fn jsonrpc_error(response: &HttpResponse) -> Option<String> {
    let payload = response.json()?;
    let error = payload.get("error")?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("未知错误");
    let code = error.get("code").and_then(Value::as_i64);
    Some(truncate(&scrub(&format!("{message}（code={code:?}）"))))
}
