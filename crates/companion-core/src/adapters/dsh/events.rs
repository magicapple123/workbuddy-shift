//! DSH（DeepSeek Harness）会话日志的解析与事件映射。
//!
//! 日志是 **zstd 压缩的 JSONL**：`sessions/<project>/<session-dir>/session*.jsonl.zstd`。
//! 本机实测（desktop 0.2.2-rc.3，346 个会话 / 57.6 万条事件）：
//!
//! - header 是首行 `{"type":"session","version":4,"id":…,"cwd":…,"parentSession"?:…}`；
//!   子代理会话带 `parentSession`，主会话没有该字段。会话 ID 取 header 的 `id`，
//!   目录名既有 `session-<uuid>` 也有裸 `<uuid>`，不可反推。
//! - 每行 `{type, seq, time, data}`，`time` 是毫秒时间戳。
//! - `turn/end.data.reason.kind` 实测只有 `completed`(731) / `interrupted`(48) /
//!   `error`(24) / `aborted`(12) 四种。
//! - **没有任何审批/提问事件**：`approval/asked`、`agent/status`、`session/disposed`
//!   在全库 57.6 万条事件里一条都没有；只有 `approval/policy`(355) 与
//!   `permission/preset`(355)，两者都是启动时的**配置**，不是「等待用户确认」的请求。
//!   因此本适配器只交「进行中 / 已完成 / 空闲」三态，不实现 wait —— 详见
//!   `parse_approval_signal` 的说明。
use serde_json::Value;

/// 支持的日志格式版本白名单。
///
/// 3 个月内 DSH 从 v0 变到 v4（字段与文件名都变过），所以未知版本必须**明确报错**
/// 而不是猜字段：静默空转会让用户以为「没会话」，比报错更难排查。
pub const FORMAT_MIN: u32 = 1;
pub const FORMAT_MAX: u32 = 4;

/// 会话日志首行 header 中我们关心的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    /// 会话 ID（原样使用：主会话可能是 `session-<uuid>`，子会话是裸 uuid）。
    pub id: String,
    pub cwd: String,
    /// 非空表示这是子代理会话：不建档（与 Codex 子会话隐藏策略一致）。
    pub parent: Option<String>,
}

/// header 解析失败的三类原因，调用方的处理完全不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// 读不到或解不开首帧：文件可能正在写，下一轮重试。
    Unreadable,
    /// 格式版本不在白名单：停止解析该文件并在 health 里报错，**不猜字段**。
    Unsupported(u32),
    /// 首行不是合法 header：该文件永久跳过。
    Malformed,
}

impl HeaderError {
    pub fn message(&self) -> String {
        match self {
            Self::Unreadable => "DSH 会话日志不可读".into(),
            Self::Unsupported(v) => {
                format!("DSH 会话格式已更新（v{v}），请升级 WorkBuddy Shift")
            }
            Self::Malformed => "DSH 会话日志首行不是有效的 session header".into(),
        }
    }
}

/// 解析 header。返回 `Err` 时调用方不得为该文件建档。
pub fn parse_header(v: &Value) -> Result<Header, HeaderError> {
    if v["type"].as_str() != Some("session") {
        return Err(HeaderError::Malformed);
    }
    let version = v["version"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(HeaderError::Malformed)?;
    if !(FORMAT_MIN..=FORMAT_MAX).contains(&version) {
        return Err(HeaderError::Unsupported(version));
    }
    let id = v["id"].as_str().unwrap_or("").trim().to_owned();
    if id.is_empty() {
        return Err(HeaderError::Malformed);
    }
    Ok(Header {
        version,
        id,
        cwd: v["cwd"].as_str().unwrap_or("").trim().to_owned(),
        parent: v["parentSession"]
            .as_str()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
    })
}

/// `turn/end.data.reason.kind` → companion 的终止状态。
///
/// 清单给的是「completed → done、cancel/interrupt/abort → aborted、未知 → aborted
/// 且记 `endedBy`（避免误报 error）」。实测还观测到 `error`（24/815，且带
/// `reason.error.message`）：它是**明确的**错误 kind，不是未知值，因此映射成
/// `error` 而不是 `aborted`——那不算误报，反而是原先唯一会丢的信息。
pub fn end_status(kind: &str) -> (&'static str, Option<String>) {
    match kind {
        "completed" | "complete" | "done" => ("done", None),
        "error" | "failed" | "failure" => ("error", None),
        "cancelled" | "canceled" | "cancel" | "interrupted" | "interrupt" | "aborted" | "abort" => {
            ("aborted", None)
        }
        other => ("aborted", Some(other.to_owned())),
    }
}

/// 一条 DSH 日志行映射出的 companion 事件（尚未带上会话信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshEvent {
    /// `session/title`：标题（调用方负责截断到 240 字）。
    Title(String),
    /// `turn/start`。
    Start { round: String },
    /// `tool/call`：让「进行中」有内容可显示。
    Step { round: String, call_id: String, label: String },
    /// `turn/end` / `session/disposed`。
    End {
        round: String,
        status: &'static str,
        ended_by: Option<String>,
    },
}

/// `data.turn` → 轮次 ID。缺字段时退回 `None`，调用方按时间戳兜底。
fn round_of(data: &Value) -> Option<String> {
    data["turn"]
        .as_u64()
        .map(|n| format!("turn:{n}"))
        .or_else(|| {
            data["turn"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| format!("turn:{s}"))
        })
}

/// 把一条日志行映射成 companion 事件；不关心的事件（`step/start` 等）返回 `None`。
///
/// `step/start` 实测 5.5 万条，映射出去会淹没事件流，按清单忽略。
pub fn map_event(v: &Value) -> Option<DshEvent> {
    match v["type"].as_str()? {
        "session/title" => v["data"]["title"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| DshEvent::Title(t.chars().take(240).collect())),
        "turn/start" => Some(DshEvent::Start {
            round: round_of(&v["data"]).unwrap_or_else(|| "turn:unknown".into()),
        }),
        "tool/call" => {
            let call_id = v["data"]["callId"].as_str().unwrap_or("").trim().to_owned();
            let label: String = v["data"]["name"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(200)
                .collect();
            if call_id.is_empty() {
                return None;
            }
            Some(DshEvent::Step {
                round: round_of(&v["data"]).unwrap_or_else(|| "turn:unknown".into()),
                call_id,
                label,
            })
        }
        "turn/end" => {
            let kind = v["data"]["reason"]["kind"].as_str().unwrap_or("");
            let (status, ended_by) = end_status(kind);
            Some(DshEvent::End {
                round: round_of(&v["data"]).unwrap_or_else(|| "turn:unknown".into()),
                status,
                ended_by,
            })
        }
        // v4 未观测到，按 issue 的事件名做容错：收不到不影响主流程。
        "session/disposed" | "agent/disposed" => Some(DshEvent::End {
            round: "turn:unknown".into(),
            status: "aborted",
            ended_by: Some("disposed".into()),
        }),
        _ => None,
    }
}

/// 「等待确认」信号的探测结果。
///
/// 目前只有单测在用：v4 日志没有可接入的信号，这里留骨架等后续版本。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalSignal {
    /// 用户的一条审批请求，进入 wait 态。
    Asked { call_id: String, tool: String },
    /// 审批已有结果，回到 running。
    Settled { call_id: String },
}

/// `approval/asked` / `approval/settled` 的解析骨架 —— **当前不接入**。
///
/// 为什么不接：本机 346 个会话、57.6 万条事件里 `approval/asked` 一条都没有
/// （`approval/policy` 与 `permission/preset` 是启动配置，不是请求），projcache 的
/// `userQuestions.questions.active` 也实测恒为空。清单要求「实测有信号才接入」，
/// 没有信号时**不得**用「工具调用很久没结束」去猜等待（会误报）。
///
/// 一旦后续版本真发了这条事件，把这个函数接到 `map_event` 里并给 hub 发
/// `{"type":"wait"…}` / `{"type":"resolve"…}` 即可，状态机的其余部分已经支持 wait。
#[allow(dead_code)]
pub fn parse_approval_signal(v: &Value) -> Option<ApprovalSignal> {
    match v["type"].as_str()? {
        "approval/asked" => {
            let call_id = v["data"]["callId"].as_str().unwrap_or("").trim();
            if call_id.is_empty() {
                return None;
            }
            Some(ApprovalSignal::Asked {
                call_id: call_id.to_owned(),
                tool: v["data"]["tool"].as_str().unwrap_or("").trim().to_owned(),
            })
        }
        "approval/settled" | "approval/result" => {
            let call_id = v["data"]["callId"].as_str().unwrap_or("").trim();
            (!call_id.is_empty()).then(|| ApprovalSignal::Settled {
                call_id: call_id.to_owned(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_v4_header_and_detects_subagents() {
        let root = parse_header(&json!({"type":"session","version":4,"id":"session-b743",
            "cwd":"D:\\workbuddy-shift","isSeeded":false,"delegationDepth":0})).unwrap();
        assert_eq!(root.version, 4);
        assert_eq!(root.id, "session-b743");
        assert_eq!(root.cwd, "D:\\workbuddy-shift");
        assert_eq!(root.parent, None);
        let child = parse_header(&json!({"type":"session","version":4,"id":"febfee40",
            "cwd":"D:\\workbuddy-shift","parentSession":"session-b743","origin":"subagent"})).unwrap();
        assert_eq!(child.parent.as_deref(), Some("session-b743"));
    }

    #[test]
    fn unknown_format_version_is_an_error_not_an_empty_scan() {
        assert_eq!(
            parse_header(&json!({"type":"session","version":9,"id":"x"})).unwrap_err(),
            HeaderError::Unsupported(9)
        );
        assert_eq!(
            parse_header(&json!({"type":"session","id":"x"})).unwrap_err(),
            HeaderError::Malformed
        );
        assert_eq!(
            parse_header(&json!({"type":"turn/start","version":4})).unwrap_err(),
            HeaderError::Malformed
        );
        assert_eq!(
            parse_header(&json!({"type":"session","version":4,"id":" "})).unwrap_err(),
            HeaderError::Malformed
        );
        assert!(HeaderError::Unsupported(9).message().contains("v9"));
        // v1 历史会话也属于白名单，只是当前机器上没有。
        assert!(parse_header(&json!({"type":"session","version":1,"id":"old"})).is_ok());
    }

    #[test]
    fn maps_turns_tools_and_end_reasons() {
        assert_eq!(
            map_event(&json!({"type":"turn/start","data":{"turn":1}})),
            Some(DshEvent::Start { round: "turn:1".into() })
        );
        assert_eq!(
            map_event(&json!({"type":"tool/call","data":{"turn":3,"callId":"call_00_x","name":"run_code"}})),
            Some(DshEvent::Step {
                round: "turn:3".into(),
                call_id: "call_00_x".into(),
                label: "run_code".into()
            })
        );
        assert_eq!(
            map_event(&json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"completed"}}})),
            Some(DshEvent::End { round: "turn:1".into(), status: "done", ended_by: None })
        );
        assert_eq!(
            map_event(&json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"interrupted"}}})),
            Some(DshEvent::End { round: "turn:1".into(), status: "aborted", ended_by: None })
        );
        // 未知 kind 记进 endedBy，避免误报 error。
        assert_eq!(
            map_event(&json!({"type":"turn/end","data":{"turn":2,"reason":{"kind":"quarantined"}}})),
            Some(DshEvent::End {
                round: "turn:2".into(),
                status: "aborted",
                ended_by: Some("quarantined".into())
            })
        );
    }

    #[test]
    fn noisy_and_broken_lines_are_ignored() {
        assert_eq!(map_event(&json!({"type":"step/start","data":{"turn":1,"step":1}})), None);
        assert_eq!(map_event(&json!({"type":"unknown/future","data":{}})), None);
        assert_eq!(map_event(&json!({"type":"tool/call","data":{"name":"x"}})), None);
        assert_eq!(map_event(&json!({})), None);
        assert_eq!(map_event(&json!({"type":"approval/policy","data":{"policy":"never"}})), None);
    }

    #[test]
    fn titles_are_truncated_and_empty_titles_dropped() {
        let long = "标".repeat(400);
        let mapped = map_event(&json!({"type":"session/title","data":{"title":long}})).unwrap();
        match mapped {
            DshEvent::Title(t) => assert_eq!(t.chars().count(), 240),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(map_event(&json!({"type":"session/title","data":{"title":"  "}})), None);
    }

    #[test]
    fn approval_signal_parser_is_kept_for_a_future_version() {
        // 骨架可用，但当前 v4 日志里没有这些事件，所以适配器不会调用它。
        assert_eq!(
            parse_approval_signal(&json!({"type":"approval/asked","data":{"callId":"c1","tool":"Bash"}})),
            Some(ApprovalSignal::Asked { call_id: "c1".into(), tool: "Bash".into() })
        );
        assert_eq!(
            parse_approval_signal(&json!({"type":"approval/settled","data":{"callId":"c1"}})),
            Some(ApprovalSignal::Settled { call_id: "c1".into() })
        );
        assert_eq!(parse_approval_signal(&json!({"type":"approval/policy"})), None);
    }
}
