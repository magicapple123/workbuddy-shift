//! spool 行的解码：把「来源指令行 + 原始 payload」还原成一个带 `agent_source` 的事件。
//!
//! `companion-hook-spool.jsonl` 有两类写入者，必须共用同一份语义（issue #20 的
//! 升级路径要求旧队列里未消费的事件继续被补采）：
//!
//! - **脚本写入者**（v0.4.3 起 WorkBuddy / CodeBuddy IDE 的 hook）：纯 shell 只会
//!   追加字节，无法安全地拼装 JSON（拼接会破坏引号 / 转义）。所以每条事件先写一行
//!   来源指令 `@@workbuddy-shift-hook-source workbuddy domestic`，紧接着原样写 payload。
//!   一个事件**独占一个文件**（见 [`crate::hook_inbox`]），所以这里的跨行累积只是
//!   为了容忍宿主写多行 JSON，而不是为了拼接并发写入。
//! - **旧 exe 写入者**（v0.4.2 及更早，以及仍然走 exe 的 codex）：单行 JSON，且已自带
//!   `agent_source` / `agent_edition`，解码时原样放行。
//!
//! 两种格式共同的三条规则：
//!
//! 1. **脚本声明的来源一定会盖上，不为「payload 形状不对」整帧丢弃**。宿主实际都发
//!    object，但非 object 的 payload（数组 / 标量）不该被静默丢弃：它会被包装成
//!    `{"agent_source":…,"agent_edition":…,"payload":<原值>}` 后照常投递。理由是
//!    来源已经由脚本明确声明，这一帧本身是合规的；而「先剥掉来源再投递」会被
//!    `ingest_hook` 兜底成 codex（`adapters::hook_agent`），那才是真正的来源串味。
//! 2. **旧格式（没有来源指令行）的非 object payload 仍然不投递**。没有声明就没有来源
//!    可用，硬投上去只会污染 codex 会话，丢掉一条本来就不合规的事件更可接受。
//! 3. **能重同步**。交错写入 / 进程被强杀都会留下永远拼不上的残帧；此时只要「当前
//!    这一行」自己能解析成 object，就放弃残帧、照常投递这一行（多行 JSON 的任意单行
//!    都不会自成 object，误判风险极低）。

use serde_json::Value;

/// 来源指令行前缀：脚本生成端（`hook_script`）与解码端共用这一个定义。
pub const SOURCE_DIRECTIVE: &str = "@@workbuddy-shift-hook-source";

/// 跨行 payload 的累积上限：超过它仍不是合法 JSON 就放弃，避免畸形输入长期占用缓冲。
const MAX_PENDING_BYTES: usize = 256 * 1024;

/// 一行输入的解析结果。
#[derive(Debug)]
pub enum Frame {
    /// 解析出一个完整事件（已按脚本声明补上来源字段）。
    Event(Value),
    /// 新格式的跨行 payload 还没写完，等下一行。
    Incomplete,
    /// 正常但不产出事件的输入（来源指令行、空行）。
    Skipped,
    /// 损坏行 / 无来源的非 object payload：调用方可以记一条诊断日志。
    Invalid,
}

/// 一帧声明的来源（脚本按 `(source, edition)` 各生成一份，值来自编译期字面量）。
#[derive(Debug, Default, PartialEq)]
struct SourceTag {
    source: String,
    edition: Option<String>,
}

impl SourceTag {
    /// 识别来源指令行；不是指令行返回 `None`。
    fn parse(line: &str) -> Option<Self> {
        let rest = line.strip_prefix(SOURCE_DIRECTIVE)?;
        let mut parts = rest.split_whitespace();
        let source = parts.next().filter(|s| !s.is_empty())?;
        Some(Self {
            source: source.to_string(),
            edition: parts
                .next()
                .filter(|edition| !edition.is_empty())
                .map(str::to_string),
        })
    }

    /// 给事件盖上来源（见文件头的第 1 条规则）。
    fn stamp(&self, value: Value) -> Value {
        stamp_source(value, &self.source, self.edition.as_deref())
    }
}

/// 给一个事件盖上来源，脚本链路与 exe 链路共用这一份语义。
///
/// object 就地补上 `agent_source` / `agent_edition`；非 object 的 payload 不丢弃、也不
/// 让 `value[key] = …` 在 serde_json 里 panic（那会让 hook 进程以 101 退出、宿主把这
/// 次操作判成失败），而是包装成
/// `{"agent_source":…,"agent_edition":…,"payload":<原值>}`。
/// 宿主实际都发 object，这里只是保证「来源已知」的事件不会因为形状异常而消失。
pub fn stamp_source(value: Value, source: &str, edition: Option<&str>) -> Value {
    let mut object = match value {
        Value::Object(map) => map,
        // 非 object 没有字段可撞名，`payload` 一定不会覆盖用户数据。
        other => serde_json::Map::from_iter([("payload".to_string(), other)]),
    };
    object.insert(
        "agent_source".to_string(),
        Value::String(source.to_string()),
    );
    if let Some(edition) = edition.filter(|edition| !edition.is_empty()) {
        object.insert(
            "agent_edition".to_string(),
            Value::String(edition.to_string()),
        );
    }
    Value::Object(object)
}

/// 解析一段**完整**内容（一个事件文件，或一次性喂进来的整块文本）。
///
/// 内容可以是「来源指令行 + payload」，也可以是旧格式的自带来源单行 JSON。
pub fn parse_frame(content: &str) -> Option<Value> {
    let (tag, body) = match content.split_once('\n') {
        Some((first, rest)) => match SourceTag::parse(first) {
            Some(tag) => (Some(tag), rest),
            None => (None, content),
        },
        None => (None, content),
    };
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    match tag {
        Some(tag) => Some(tag.stamp(value)),
        // 旧格式没有来源可用，非 object 的 payload 只能丢弃（文件头第 2 条规则）。
        None => value.is_object().then_some(value),
    }
}

/// 增量行解码器；`HookSpool` 持有一个实例，跨 `drain` 调用保留未完成的帧。
#[derive(Default)]
pub struct HookFrameDecoder {
    /// 当前帧声明的来源（`None` 表示这一行属于旧格式）。
    tag: Option<SourceTag>,
    /// 尚未解析成功的 payload 片段（只在本帧已有来源声明时累积）。
    pending: String,
}

impl HookFrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 丢掉未完成的帧：消费位置回退、文件被截断或归档时调用。
    pub fn reset(&mut self) {
        self.tag = None;
        self.pending.clear();
    }

    /// 喂一行（不含换行符），按两种格式产出事件。
    pub fn push(&mut self, line: &str) -> Frame {
        if line.starts_with(SOURCE_DIRECTIVE) {
            self.begin(line);
            return Frame::Skipped;
        }
        // 旧格式：一行一条事件，自带来源。空行与损坏行都不产出事件。
        if self.tag.is_none() {
            self.pending.clear();
            if line.trim().is_empty() {
                return Frame::Skipped;
            }
            return match serde_json::from_str::<Value>(line) {
                Ok(value) if value.is_object() => Frame::Event(value),
                _ => Frame::Invalid,
            };
        }
        // 新格式：payload 可能跨行，累积到能解析为止。
        if self.pending.is_empty() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                return self.finish(value);
            }
            self.pending.push_str(line);
            return Frame::Incomplete;
        }
        self.pending.push('\n');
        self.pending.push_str(line);
        let parsed = serde_json::from_str::<Value>(&self.pending);
        if let Ok(value) = parsed {
            return self.finish(value);
        }
        if self.pending.len() > MAX_PENDING_BYTES {
            self.reset();
            return Frame::Invalid;
        }
        // 重同步：残帧拼不上时，当前这一行若自成 object 就放弃残帧、投递这一行。
        match serde_json::from_str::<Value>(line) {
            Ok(value) if value.is_object() => {
                self.pending.clear();
                self.finish(value)
            }
            _ => Frame::Incomplete,
        }
    }

    /// 开始一帧：残帧一律作废（hook 进程被强杀会留下没有 payload 的指令行）。
    fn begin(&mut self, line: &str) {
        self.pending.clear();
        self.tag = SourceTag::parse(line);
    }

    /// 用当前帧的来源给事件盖章，并结束这一帧。
    fn finish(&mut self, value: Value) -> Frame {
        self.pending.clear();
        match self.tag.take() {
            Some(tag) => Frame::Event(tag.stamp(value)),
            // 旧格式自带来源，原样放行。
            None => Frame::Event(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(frame: Frame) -> Value {
        match frame {
            Frame::Event(value) => value,
            other => panic!("期望完整事件，实际 {other:?}"),
        }
    }

    #[test]
    fn script_directive_stamps_source_and_edition() {
        let mut decoder = HookFrameDecoder::new();
        assert!(matches!(
            decoder.push("@@workbuddy-shift-hook-source workbuddy international"),
            Frame::Skipped
        ));
        let value = event(decoder.push(r#"{"hook_event_name":"Stop","session_id":"s-1"}"#));
        assert_eq!(value["agent_source"], "workbuddy");
        assert_eq!(value["agent_edition"], "international");
        assert_eq!(value["session_id"], "s-1");
        // 一帧只作用一条事件：下一条指令行到来前不得串味。
        assert!(matches!(decoder.push(""), Frame::Skipped));
        assert!(matches!(decoder.push("{broken"), Frame::Invalid));
    }

    #[test]
    fn legacy_single_line_events_keep_their_own_source() {
        let mut decoder = HookFrameDecoder::new();
        let value = event(decoder.push(r#"{"agent_source":"codebuddy-ide","session_id":"s-2"}"#));
        assert_eq!(value["agent_source"], "codebuddy-ide");
        assert!(value["agent_edition"].is_null());
        // 损坏行只丢自己，不影响后面的正常事件。
        assert!(matches!(decoder.push(r#"{"broken":"#), Frame::Invalid));
        let value = event(decoder.push(r#"{"agent_source":"workbuddy","session_id":"s-3"}"#));
        assert_eq!(value["session_id"], "s-3");
    }

    #[test]
    fn multi_line_payload_is_reassembled_under_one_frame() {
        let mut decoder = HookFrameDecoder::new();
        decoder.push("@@workbuddy-shift-hook-source codebuddy-ide domestic");
        assert!(matches!(decoder.push("{"), Frame::Incomplete));
        assert!(matches!(
            decoder.push(r#"  "session_id": "s-4","#),
            Frame::Incomplete
        ));
        assert!(matches!(
            decoder.push(r#"  "hook_event_name": "Stop""#),
            Frame::Incomplete
        ));
        let value = event(decoder.push("}"));
        assert_eq!(value["session_id"], "s-4");
        assert_eq!(value["agent_source"], "codebuddy-ide");
        assert_eq!(value["agent_edition"], "domestic");
    }

    #[test]
    fn a_truncated_frame_is_dropped_by_the_next_directive() {
        let mut decoder = HookFrameDecoder::new();
        decoder.push("@@workbuddy-shift-hook-source workbuddy domestic");
        assert!(matches!(
            decoder.push(r#"{"session_id":"#),
            Frame::Incomplete
        ));
        decoder.push("@@workbuddy-shift-hook-source workbuddy international");
        let value = event(decoder.push(r#"{"session_id":"s-5"}"#));
        assert_eq!(value["session_id"], "s-5");
        assert_eq!(value["agent_edition"], "international");
    }

    #[test]
    fn an_unterminated_frame_is_abandoned_before_it_grows_without_bound() {
        let mut decoder = HookFrameDecoder::new();
        decoder.push("@@workbuddy-shift-hook-source workbuddy domestic");
        assert!(matches!(decoder.push("{"), Frame::Incomplete));
        // 一次超过上限的追加后放弃整帧，下一条指令行重新开始。
        let huge = "x".repeat(MAX_PENDING_BYTES + 1);
        assert!(matches!(decoder.push(&huge), Frame::Invalid));
        decoder.push("@@workbuddy-shift-hook-source workbuddy domestic");
        let value = event(decoder.push(r#"{"session_id":"s-6"}"#));
        assert_eq!(value["session_id"], "s-6");
    }

    #[test]
    fn an_intact_event_after_a_torn_frame_is_still_delivered() {
        // 交错写入留下的残帧：残帧本身可以放弃，但后面字节完好的一条必须照常投递。
        let mut decoder = HookFrameDecoder::new();
        decoder.push("@@workbuddy-shift-hook-source workbuddy domestic");
        assert!(matches!(
            decoder.push(
                r#"{"session_id":"w-3","blob":"xxx@@workbuddy-shift-hook-source workbuddy domestic"#
            ),
            Frame::Incomplete
        ));
        assert!(matches!(decoder.push(r#"xxx"}"#), Frame::Incomplete));
        let next = event(decoder.push(r#"{"session_id":"w-1","blob":"yyy"}"#));
        assert_eq!(next["session_id"], "w-1");
        assert_eq!(next["agent_source"], "workbuddy");
        // 残帧已被丢弃，后续事件继续按行解析。
        let after = event(decoder.push(r#"{"session_id":"w-2"}"#));
        assert_eq!(after["session_id"], "w-2");
    }

    #[test]
    fn a_non_object_payload_is_wrapped_under_the_declared_source() {
        // 来源已经由脚本声明，所以形状异常也照常投递：包装成 `payload` 字段。
        // 直接丢弃会让一条本来合规的事件消失；而剥掉来源再投递会被兜底成 codex。
        let mut decoder = HookFrameDecoder::new();
        let directive = "@@workbuddy-shift-hook-source workbuddy domestic";
        decoder.push(directive);
        let array = event(decoder.push("[1,2,3]"));
        assert_eq!(array["agent_source"], "workbuddy");
        assert_eq!(array["agent_edition"], "domestic");
        assert_eq!(
            array["payload"],
            Value::Array(vec![Value::from(1), Value::from(2), Value::from(3)])
        );
        decoder.push(directive);
        let scalar = event(decoder.push(r#""text""#));
        assert_eq!(scalar["payload"], Value::String("text".to_string()));
        assert_eq!(scalar["agent_source"], "workbuddy");
        // 旧格式没有来源可盖：非 object 仍然不投递（否则会污染 codex 会话）。
        let mut legacy = HookFrameDecoder::new();
        assert!(matches!(legacy.push("[1,2,3]"), Frame::Invalid));
        assert!(matches!(legacy.push(r#""text""#), Frame::Invalid));
    }

    #[test]
    fn parse_frame_handles_tagged_and_self_described_events() {
        let tagged = parse_frame(
            "@@workbuddy-shift-hook-source codebuddy-ide international\n{\n  \"session_id\": \"a\"\n}\n",
        )
        .unwrap();
        assert_eq!(tagged["agent_source"], "codebuddy-ide");
        assert_eq!(tagged["agent_edition"], "international");
        assert_eq!(tagged["session_id"], "a");
        let legacy = parse_frame("{\"agent_source\":\"codex\",\"session_id\":\"b\"}\n").unwrap();
        assert_eq!(legacy["agent_source"], "codex");
        assert!(parse_frame("").is_none());
        assert!(parse_frame("@@workbuddy-shift-hook-source workbuddy domestik\n{oops").is_none());
        // 有来源声明的非 object payload 包装后投递；无来源的旧格式只丢自己。
        let wrapped = parse_frame("@@workbuddy-shift-hook-source workbuddy domestic\n[1]").unwrap();
        assert_eq!(wrapped["agent_source"], "workbuddy");
        assert_eq!(wrapped["payload"], Value::Array(vec![Value::from(1)]));
        assert!(parse_frame("[1]").is_none());
    }
}
