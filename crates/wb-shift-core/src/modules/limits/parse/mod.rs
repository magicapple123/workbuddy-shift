use super::scan::LONG_LINE_SNIFF_BYTES;
use super::scan::SDK_CONVERSATIONS_DIR;
use super::scan::SDK_LOG_DIR;
use chrono::{FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use std::collections::HashMap;
use std::path::Path;

/// CLI 启动鉴权行标记：`[FirstScreen] [AuthDoInitProbe] stage=… uid=<uuid>`。
const CLI_AUTH_MARKER: &str = "[AuthDoInitProbe]";

/// IDE 会话鉴权行标记：`[PulseServiceLifecycle] Auth session changed: …, uid=<uuid>`。
const IDE_AUTH_MARKER: &str = "[PulseServiceLifecycle] Auth session changed:";

/// IDE 会话级模型标记：`[ModelSelection] conversationId=<conv32>, mode=…, modelId=<m>`
/// 与 `[AcpAgent:<conv32>] … modelId=<m>` 共用同一个字段名（见 `ide_model_id`）。
const IDE_MODEL_FIELD: &str = "modelId=";

/// 模型字段上的未知哨兵值（见 `known_model`）：`auto` 是「会话尚未选定模型」
/// （IDE 的 `[AcpAgent:…] Model cache synced … source=new-session`、SDK 的 `method:sendPrompt`
/// 都会出现），`undefined` / `null` 是 JS 侧字段缺失的写法。
/// 都按未知处理——卡片绝不能显示 `auto` / `undefined`，也不能让它们覆盖已知值。
pub(crate) const MODEL_UNKNOWN: [&str; 3] = ["auto", "undefined", "null"];

/// IDE 模型兜底行标记：`[handleAuthError] modelId=<m>, …`（只在调用失败时出现）。
const IDE_AUTH_ERROR_MARKER: &str = "[handleAuthError]";

/// IDE 会话 id 的方括号 tag：`[AcpAgent:<conv32>]` / `[AcpConnection:<conv32>]`。
const IDE_ACP_AGENT_TAG: &str = "AcpAgent:";

const IDE_ACP_CONNECTION_TAG: &str = "AcpConnection:";

/// IDE 行首时间格式（本地时间，定长 23 字节）：`2026-09-17 10:28:26.730`。
const IDE_TS_FORMAT: &str = "%Y-%m-%d %H:%M:%S%.3f";

const IDE_TS_LEN: usize = 23;

/// 限额文案标记（粗筛与逐行解析共用）：中文（国内版）与英文（国际版）。
///
/// 同时是 hook 通路（`rate_limit_events.rs`）识别限额 payload 的唯一判据。
pub(crate) const QUOTA_MARKERS: [&str; 2] = ["超出频率限制", "usage exceeds frequency limit"];

/// 官方重置时刻的句式标记：`将在 <时间> UTC+8 重置` / `will reset at <时间> UTC+8`。
const RESET_MARKERS: [&str; 2] = ["将在 ", "will reset at "];

/// 更可靠的分类行标记：`[ACP Agent] refusal classified: …, category=quota`。
const CLASSIFIER_MARKER: &str = "refusal classified";

/// 分类行的配额判据（比 `httpStatus=429` 更贴近「限额」语义）。
const CLASSIFIER_CATEGORY: &str = "category=quota";

/// 分类器兜底的时间窗：合法场景是分类器行后毫秒级紧随的裸文案行（上游实测 1ms）；
/// 回显行与分类器行相隔分钟级，窗口一卡就出局。
const CLASSIFIER_FALLBACK_MS: i64 = 2_000;

/// 会话当前模型标记：`[ModelConfig] sessionId=…, resolved model=…`。
const RESOLVED_MODEL_MARKER: &str = "resolved model=";

/// SDK 会话日志里的「发起请求」标记：`method:sendPrompt {…"modelId":"<模型>"}`。
///
/// 这类行**不带会话 id**（只有 `instanceId`），会话身份只能来自文件名 —— 而它正是模型归因的盲区：
/// 上游只认业务日志的 `requestId → model` 与 `resolved model=`，SDK 侧两样都没有 ⇒ 事件被标「未知模型」。
/// 本机实测（2026-09-18 15:40:22）：那条「未知模型」实为 `hy4-preview-f`。
const SDK_SEND_PROMPT_MARKER: &str = "method:sendPrompt";

/// 回显行（工具输出 / 命令回显）的**外层载体**标记。
///
/// - `SandboxShell`：外层 logger 标签（`[SandboxShell] …`）；
/// - `ProcessOutput` / `sandbox attempt output`：标签之后正文开头的载体名。
///
/// 正文里出现 `stdout(` / `stderr(` / `content=` 不算（见 `is_transport_line`）。
const TRANSPORT_TAG: &str = "SandboxShell";

const TRANSPORT_OUTPUT_MARKER: &str = "ProcessOutput";

const SANDBOX_OUTPUT_MARKER: &str = "sandbox attempt output";

/// 业务日志行首时间格式（本地时间）：`9/17/2026, 12:20:31 AM.232`。
const BUSINESS_TS_FORMAT: &str = "%m/%d/%Y, %I:%M:%S %p%.3f";

/// 业务日志的另一种行首时间格式（24 小时制，本机实测）：`2026/9/18 15:46:30.242`。
/// 只认上面那一种时，这类行会被 `line_timestamp` 判为「无时间戳」而整行跳过 —— 表现为台账为空。
const BUSINESS_TS_ALT_FORMAT: &str = "%Y/%m/%d %H:%M:%S%.3f";

/// 插件宿主 agent 日志的日期/时刻两段（`2026/9/27` + `13:27:37.109`）。
///
/// 单独写的理由：月/日/时都可能是**一位数**（`2026/9/7 3:07:37.109`），定长切片会切错；
/// 先按空白切成「日期 token + 时刻 token」再各自解析才稳。
const AGENT_TS_DATE_FORMAT: &str = "%Y/%m/%d";
const AGENT_TS_CLOCK_FORMAT: &str = "%H:%M:%S%.3f";
const AGENT_TS_CLOCK_FORMAT_NO_MS: &str = "%H:%M:%S";

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 日志格式族：决定「行首时间戳 / 事件 id / 模型归因」三处适配。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogFormat {
    /// `[9/15/2026, 6:19:48 PM.755] … (requestId/sessionId)`：WorkBuddy 两档位与 CodeBuddy CLI。
    WorkBuddy,
    /// `2026-09-17 10:28:26.730 [info] …`：两个 CodeBuddy IDE。
    Ide,
    /// `[2026/9/27 13:27:37.109] …`：VS Code 插件宿主（CodeBuddy 扩展）的 agent 日志（#188）。
    ///
    /// 独立变体的价值不在时间戳：`BUSINESS_TS_ALT_FORMAT`（`%Y/%m/%d %H:%M:%S%.3f`）本来
    /// 就能解析同一串数字，两者解析结果相同。真正需要区分的是**文件枚举口径**——插件宿主的
    /// 日志落在 `logs/<会话>/window<N>/exthost/<插件>/` 下，要按 mtime 收窗 + 插件目录过滤
    /// （`scan::agent_log_files`），不能用日期目录枚举；另外这里**允许省略毫秒**
    /// （`AGENT_TS_CLOCK_FORMAT_NO_MS`），IDE 的定长前缀做不到。
    /// 事件 id / 模型归因与业务日志同族（同一套 agent 代码写出来的行），
    /// 因此 `scan_text_scoped` 里与 `WorkBuddy` 共用分支。
    Agent,
}

/// 鉴权行标记：顺序扫描时据此维护「事件前最近一次 uid」，是新来源唯一的时态可靠账号线索。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthMarker {
    /// WorkBuddy 两档位：日志里没有账号 uid，归因走 `sessions` 表。
    None,
    /// CodeBuddy CLI：`[AuthDoInitProbe] stage=… uid=<uuid>`。
    AuthDoInitProbe,
    /// 两个 CodeBuddy IDE：`[PulseServiceLifecycle] Auth session changed: … uid=<uuid>`。
    AuthSessionChanged,
    /// 插件宿主 agent 日志：**两种鉴权行都可能出现**（同一份扩展代码，登录探测与 Pulse
    /// 生命周期各写一份），任一命中即更新「当前 uid」。
    ExtensionAny,
}

/// 去重前的原始命中行。
pub(crate) struct Hit {
    /// 事件 id：WorkBuddy/CLI 是 `sessionId`，IDE 是 `conversationId`。
    pub(super) session_id: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reset_at: i64,
    pub(super) occurred_at: i64,
    /// 该事件前最近一次鉴权行的账号 uid（WorkBuddy 两档位不采集）。
    pub(super) uid: Option<String>,
}

/// 去重后的一次限额事件。
pub(crate) struct Event {
    pub(super) session_id: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reset_at: i64,
    pub(super) first_seen_at: i64,
    pub(super) hit_count: u32,
    pub(super) uid: Option<String>,
}

impl Event {
    /// 合并同一次事件的重复书写：模型/会话 id/uid 保留任一有值的，首次出现时刻取最早，
    /// 命中行数累加。
    pub(super) fn merge(&mut self, other: Event) {
        if self.model.is_none() {
            self.model = other.model;
        }
        if self.session_id.is_none() {
            self.session_id = other.session_id;
        }
        if self.uid.is_none() {
            self.uid = other.uid;
        }
        self.first_seen_at = self.first_seen_at.min(other.first_seen_at);
        self.hit_count += other.hit_count;
    }
}

/// 逐行解析限额事件（文件已由粗筛确认命中）。
///
/// 顺序扫描是关键：命中限额行时，`session_models` / `conversation_models` 里恰好是「该会话
/// 在被限**之前**最近一次选用的模型」，`current_uid` 同理是「事件前最近一次鉴权账号」，
/// 因此不会归因成被限之后才切换到的模型或账号。
pub(super) fn scan_text(text: &str, format: LogFormat, auth: AuthMarker) -> Vec<Hit> {
    scan_text_scoped(text, format, auth, None)
}

/// 带「文件级会话 id」的解析入口。
///
/// SDK 会话日志（`logs/<日期>/sdk/conversations/<会话UUID>.log`）的 `method:sendPrompt`
/// 行不带会话 id，只能靠**文件名**确定会话；顺序扫描保证该模型是「本事件之前最近一次」。
pub(super) fn scan_text_scoped(
    text: &str,
    format: LogFormat,
    auth: AuthMarker,
    file_session: Option<&str>,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut session_models: HashMap<String, String> = HashMap::new();
    let mut request_models: HashMap<String, String> = HashMap::new();
    let mut classifier_session: Option<(String, Option<i64>)> = None;
    let mut conversation_models: HashMap<String, String> = HashMap::new();
    let mut auth_error_model: Option<String> = None;
    let mut current_uid: Option<String> = None;
    for line in text.lines() {
        // 传输行（工具输出 / 命令回显的载体）对上下文采集与限额检测都一律跳过。
        if is_transport_line(line) {
            continue;
        }
        if let Some(uid) = auth_uid(line, auth) {
            current_uid = Some(uid.to_string());
        }
        match format {
            // 插件宿主 agent 日志与 WorkBuddy 格式同族：同一套 `requestId → model` /
            // `resolved model=` / 分类器兜底线索（扩展的 agent 就是这套代码写出来的行）。
            LogFormat::WorkBuddy | LogFormat::Agent => {
                if let Some(model) = model_after(line, RESOLVED_MODEL_MARKER) {
                    if let Some(session) = field_after(line, "sessionId=") {
                        session_models.insert(session.to_string(), model.to_string());
                    }
                }
                if let Some(request) = field_after(line, "requestId=") {
                    if let Some(model) = field_model(line, "model=") {
                        request_models.insert(request.to_string(), model.to_string());
                    }
                }
                // SDK 会话日志的模型线索（行内无会话 id ⇒ 用文件名）；顺序扫描 ⇒ 取到事件前的最近一次。
                // 未知值（空 / `auto` / `undefined` / `null`）不写映射：既不能显示成模型名，
                // 也不能覆盖此前已知的值（2026-09-20 审查反例：SDK 的 auto 被显示为模型名）。
                if let Some(session) = file_session {
                    if line.contains(SDK_SEND_PROMPT_MARKER) {
                        if let Some(model) = json_field(line, "modelId").filter(|m| known_model(m))
                        {
                            session_models.insert(session.to_string(), model.to_string());
                        }
                    }
                }
                // 分类器行自带 `sessionId=`；裸文案行的兜底只在紧随其后（时间窗内）有效，
                // 相隔分钟级的回显行不得借道（2026-09-18 glm 假 chip 实证）。
                if line.contains(CLASSIFIER_MARKER) && line.contains(CLASSIFIER_CATEGORY) {
                    if let Some(session) = field_after(line, "sessionId=") {
                        classifier_session =
                            Some((session.to_string(), line_timestamp(line, format)));
                    }
                }
            }
            LogFormat::Ide => {
                // 会话级模型映射：`[ModelSelection] conversationId=…, modelId=…` 与
                // `[AcpAgent:<conv>] … modelId=…`。`modelId=auto` 视为未知（不覆盖已有值）。
                if line.contains(IDE_MODEL_FIELD) {
                    if let Some(model) = ide_model_id(line) {
                        if let Some(conversation) = ide_session_id(line) {
                            conversation_models.insert(conversation.to_string(), model.to_string());
                        }
                    }
                }
                // 文件级兜底：`[handleAuthError]` 只在调用失败时出现，因此该值不会跨会话陈旧。
                if line.contains(IDE_AUTH_ERROR_MARKER) {
                    if let Some(model) = ide_model_id(line) {
                        auth_error_model = Some(model.to_string());
                    }
                }
            }
        }
        if !QUOTA_MARKERS.iter().any(|marker| line.contains(marker)) {
            continue;
        }
        // `Agent execution failed` 的响应头里有本次请求的账号 uid，是鉴权行缺失时的同级
        // 补充。只对用 uid 归因的来源有意义（WorkBuddy 两档位走 `sessions` 表，不采集 uid）；
        // 该行可能含完整请求体，因此只做定长窗口嗅探。
        if auth != AuthMarker::None && current_uid.is_none() {
            current_uid = json_field(line, "x-user-id").map(str::to_string);
        }
        let Some(reset_at) = parse_reset_at(line) else {
            continue;
        };
        // 没有发生时刻就无法参与去重：宁可丢这一行，也不猜一个时间。
        let Some(occurred_at) = line_timestamp(line, format) else {
            continue;
        };
        let (session_id, model) = match format {
            LogFormat::WorkBuddy | LogFormat::Agent => {
                // 分类器兜底只认「紧随其后」的裸文案行：quota 行发生在分类器行之后
                // 且在时间窗内才允许借道（上游实测 1ms；回显行相隔分钟级 ⇒ 出局）。
                let classifier = classifier_session.as_ref().and_then(|(session, ts)| {
                    let ts = (*ts)?;
                    (occurred_at >= ts && occurred_at - ts <= CLASSIFIER_FALLBACK_MS)
                        .then_some(session.as_str())
                });
                workbuddy_attribution(line, &request_models, &session_models, classifier)
            }
            LogFormat::Ide => {
                ide_attribution(line, &conversation_models, auth_error_model.as_deref())
            }
        };
        // WorkBuddy 两档位的限额行必须有事件身份（行尾 `(requestId/sessionId)` 或 `sessionId=`）：
        // 它们的会话日志会把任意文本原样回显进文件（诊断命令、工具输出），无身份的行只能落到
        // 分类器兜底上 —— 账号/模型都会错归（2026-09-18 glm 假 chip 实证）。宁可少显示。
        //
        // 只对 WorkBuddy 两档位成立：CLI 与它们共用同一格式，但归因走日志内鉴权 uid /
        // 轮换状态文件，缺 `session_id` 不代表事件不可靠 —— 不能统一短路（2026-09-20 审查）。
        // 插件宿主 agent 日志（`LogFormat::Agent`）同理：它没有 `sessions` 表可查，
        // 归因链与 CLI 一致（uid → 状态文件），因此不适用这条短路。
        if format == LogFormat::WorkBuddy && auth == AuthMarker::None && session_id.is_none() {
            continue;
        }
        hits.push(Hit {
            session_id,
            model,
            reset_at,
            occurred_at,
            uid: current_uid.clone(),
        });
    }
    hits
}

fn is_hex_id(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

/// 取 `<key>` 字段值（到 `,` / 空格 / 引号 / 右括号为止）。
///
/// 要求 key 前是字段边界，避免 `parentSessionId=` 之类的相似字段误命中。
fn field_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut search = 0;
    while let Some(offset) = line[search..].find(key) {
        let index = search + offset;
        let boundary = index == 0 || !is_field_char(line.as_bytes()[index - 1]);
        if boundary {
            let value = line[index + key.len()..]
                .split([',', ' ', '"', '}', ')'])
                .next()
                .unwrap_or_default()
                .trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
        search = index + key.len();
    }
    None
}

fn is_field_char(byte: u8) -> bool {
    // 点号也算字段字符：`requestOptions.model=` 不是 `model=` 字段。
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.'
}

/// 取 `<marker>` 之后的模型名。
fn model_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let start = line.find(marker)? + marker.len();
    let value = line[start..].split([',', ' ', ';']).next()?.trim();
    (!value.is_empty()).then_some(value)
}

/// 取字段边界上的 `<key>` 字段值（`model=` 与 IDE 的 `modelId=` 共用）。
///
/// 只认字段边界上的 key：`[ModelProvider] Sending request: agent=cli, model=<model>,
/// requestId=<id>` 命中，而 `requestOptions.model=` 不命中。
fn field_model<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut search = 0;
    while let Some(offset) = line[search..].find(key) {
        let index = search + offset;
        if index == 0 || !is_field_char(line.as_bytes()[index - 1]) {
            return model_after(&line[index..], key);
        }
        search = index + key.len();
    }
    None
}

/// 模型值是否可用：非空且不是未知哨兵。
///
/// IDE 的 `modelId=` 与 SDK `method:sendPrompt` 的 `"modelId"` 共用同一套判据 ——
/// `auto` / `undefined` / `null` / 空值都不得作为模型名展示，也不得覆盖已知值。
fn known_model(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && !MODEL_UNKNOWN.contains(&value)
}

/// 取行首定长窗口（对齐到字符边界，不切碎多字节字符）。
fn head_window(line: &str) -> &str {
    let mut end = line.len().min(LONG_LINE_SNIFF_BYTES);
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

/// 取行尾定长窗口（对齐到字符边界）。
fn tail_window(line: &str) -> &str {
    let mut start = line.len().saturating_sub(LONG_LINE_SNIFF_BYTES);
    while start < line.len() && !line.is_char_boundary(start) {
        start += 1;
    }
    &line[start..]
}

/// 取 JSON 形态的字符串字段 `"<key>":"<value>"`。
///
/// IDE 的 `Agent execution failed` 行可能含完整请求体（本机实测 117 KB，研究报告另有
/// 350 KB 样本），因此短行整行嗅探、超长行只看行首与行尾两个定长窗口
/// （`requestBodyValues` 在行首、`responseHeaders` / `responseBody` 在行尾），
/// 不对整行做全量正则或 JSON 反序列化。
pub(super) fn json_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    if line.len() <= LONG_LINE_SNIFF_BYTES {
        return json_field_in(line, key);
    }
    json_field_in(head_window(line), key).or_else(|| json_field_in(tail_window(line), key))
}

/// 在给定文本里取 `"<key>":"<value>"`（两侧引号即字段边界，故 `"modelId"` 不会误命中 `model`）。
///
/// 兼容两种书写：未转义的 `"<key>":"<value>"` 与嵌在字符串里的转义形式
/// `\"<key>\":\"<value>\"`（IDE 的 `responseBody: "…"` 这类行），转义形式只是引号前多一个
/// 反斜杠。
///
/// 值必须以**闭合引号**结束才采纳：超长行只嗅探定长窗口，窗口边界可能正好落在值的中间，
/// 掐头去尾的半个模型名／半个 uid 一个都不能用（取不到就按「未知」处理，绝不猜）。
fn json_field_in<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(offset) = text[search..].find(key) {
        let index = search + offset;
        search = index + key.len();
        // 键的左侧必须是引号（转义形式下引号前还有一个反斜杠，与本次判断无关）。
        if index == 0 || bytes[index - 1] != b'"' {
            continue;
        }
        // 依次吃掉：键的右引号 → 冒号 → 值的左引号（两处都可能带转义反斜杠）。
        let mut cursor = index + key.len();
        if bytes.get(cursor) == Some(&b'\\') {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') || bytes.get(cursor + 1) != Some(&b':') {
            continue;
        }
        cursor += 2;
        if bytes.get(cursor) == Some(&b'\\') {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            continue;
        }
        let Some((value, _)) = text[cursor + 1..].split_once('"') else {
            continue;
        };
        // 转义形式下值的闭合引号写作 `\"`：那个反斜杠是转义记号，不属于值本身。
        let value = value.strip_suffix('\\').unwrap_or(value).trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// 取方括号 tag 里的会话 id：`[AcpAgent:<conv32>]` / `[AcpConnection:<conv32>]`。
///
/// 只认会话 id 形态：`[BaseAgent:craft]` 之类的 agent tag 必须忽略。
fn bracket_tag_after<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let mut search = 0;
    while let Some(offset) = line[search..].find(tag) {
        let index = search + offset;
        let value = line[index + tag.len()..].split(']').next()?.trim();
        if is_ide_session_id(value) {
            return Some(value);
        }
        search = index + tag.len();
    }
    None
}

mod ide_cli;
mod workbuddy;

pub(crate) use workbuddy::parse_reset_at;

// 测试通过 `use super::*` 复用这些重导出；lib 目标下它们看起来未被使用。
#[allow(unused_imports)]
pub(crate) use ide_cli::{
    ide_attribution, ide_conversation_id, ide_json_model, ide_model_id, ide_session_id,
    is_ide_session_id,
};
// 测试通过 `use super::*` 复用这些重导出；lib 目标下它们看起来未被使用。
#[allow(unused_imports)]
pub(crate) use workbuddy::{
    auth_uid, is_transport_line, line_timestamp, parse_datetime_prefix, parse_utc_offset,
    request_pair, session_from_file_name, split_logger_tags, workbuddy_attribution,
};
