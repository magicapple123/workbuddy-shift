//! WorkBuddy/CLI 日志行解析：传输行、归因、时间戳、重置时刻与请求身份。

use super::*;

/// 回显行：WorkBuddy 会把工具输出 / 命令回显整段塞进 `[SandboxShell] ProcessOutput …
/// | content=…`（及 Sandbox 系列的 stdout/stderr 摘要），其中可能带着**别处日志的原文**
/// ——包括 429 行与其 requestId。按原样解析会把回显当成真事件（截断行还会错落归因到回显
/// 会话的模型上，2026-09-18 实证 glm 假 chip、hitCount 虚高）。
///
/// ⚠️ 只认**外层载体**：行首方括号标签段里的 `SandboxShell`，或标签之后正文开头的
/// `ProcessOutput` / `sandbox attempt output`。不按整行判定 —— 真实 429 正文
/// （IDE 的 `Agent execution failed` 带完整请求体）里出现 `stdout(` / `stderr(` /
/// `content=` 是常事，整行匹配会把真事件一起误删（2026-09-20 审查反例）。
pub(crate) fn is_transport_line(line: &str) -> bool {
    let (tags, body) = split_logger_tags(line);
    tags.contains(TRANSPORT_TAG)
        || body.starts_with(TRANSPORT_OUTPUT_MARKER)
        || body.starts_with(SANDBOX_OUTPUT_MARKER)
}

/// 把一行拆成「外层 logger 标签段」与「正文」。
///
/// 标签段 = 行首连续的 `[…]`（业务日志的 `[时间] [级别] [pid=…] [Tag]`）；正文可能原样
/// 引用别的日志行，两者必须分开看，否则正文里的载体字样会被误判成传输行。
pub(crate) fn split_logger_tags(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    loop {
        let mut index = cursor;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'[') {
            break;
        }
        let Some(close) = line[index..].find(']') else {
            break;
        };
        cursor = index + close + 1;
    }
    (&line[..cursor], line[cursor..].trim_start())
}

/// WorkBuddy 格式（含 CLI）的事件 id 与模型归因。
///
/// 事件 id 取行尾 `(requestId/sessionId)`，回退行内 `sessionId=`，再回退
/// **紧随其后**（`CLASSIFIER_FALLBACK_MS` 内）的分类器行；模型走
/// `requestId → model`（`[ModelProvider] Sending request`）→ 该会话最近一次
/// `resolved model=` → 未知（不猜）。
///
/// ⚠️ 分类器兜底必须带时间窗：会话日志会把任意文本原样回显进文件（诊断命令、
/// 工具输出），无时间窗时无身份的回显行会错归到「最近一次分类器会话」上
/// （2026-09-18 glm 假 chip 实证）。
pub(crate) fn workbuddy_attribution(
    line: &str,
    request_models: &HashMap<String, String>,
    session_models: &HashMap<String, String>,
    classifier_session: Option<&str>,
) -> (Option<String>, Option<String>) {
    let (request_id, session_id) = match request_pair(line) {
        Some((request, session)) => (Some(request), Some(session)),
        None => (None, None),
    };
    let session_id = session_id
        .or_else(|| field_after(line, "sessionId=").map(str::to_string))
        .or_else(|| classifier_session.map(str::to_string));
    let model = request_id
        .as_deref()
        .and_then(|request| request_models.get(request))
        .or_else(|| {
            session_id
                .as_deref()
                .and_then(|session| session_models.get(session))
        })
        .cloned();
    (session_id, model)
}

/// `logs/<日期>/sdk/conversations/<会话 UUID>.log` 的文件名就是会话 id。
///
/// 路径形态必须匹配（祖父目录 `sdk`、父目录 `conversations`）：业务日志是
/// `<工作区>__<hash>.log`，但别的目录下也可能出现 UUID 文件名 —— 只按文件名认领会把它们
/// 当成会话 id，把模型线索写进错误的会话（2026-09-20 审查：限定 SDK conversations 形态）。
pub(crate) fn session_from_file_name(path: &Path) -> Option<String> {
    let conversations = path.parent()?;
    if conversations.file_name()?.to_str()? != SDK_CONVERSATIONS_DIR {
        return None;
    }
    if conversations.parent()?.file_name()?.to_str()? != SDK_LOG_DIR {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let looks_like_uuid = stem.len() == 36
        && stem.matches('-').count() == 4
        && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    looks_like_uuid.then(|| stem.to_string())
}

// ---------------------------------------------------------------------------
// 行解析
// ---------------------------------------------------------------------------

/// 取行首时间戳（毫秒）。
///
/// WorkBuddy 格式：业务日志是本地时间，SDK 日志是 UTC（`2026-09-16T16:20:31.523Z`），
/// 两者都折算到同一绝对时间轴后再比较。IDE 格式：行首 `2026-09-17 10:28:26.730` 定长
/// 23 字节，本地时间、无时区标记。
pub(crate) fn line_timestamp(line: &str, format: LogFormat) -> Option<i64> {
    let trimmed = line.trim_start();
    let naive = match format {
        LogFormat::WorkBuddy => {
            let head = trimmed.split_whitespace().next().unwrap_or_default();
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(head) {
                return Some(parsed.timestamp_millis());
            }
            let rest = trimmed.strip_prefix('[')?;
            let end = rest.find(']')?;
            let stamp = rest[..end].trim();
            NaiveDateTime::parse_from_str(stamp, BUSINESS_TS_FORMAT)
                .or_else(|_| NaiveDateTime::parse_from_str(stamp, BUSINESS_TS_ALT_FORMAT))
                .ok()?
        }
        LogFormat::Ide => {
            let head = std::str::from_utf8(trimmed.as_bytes().get(..IDE_TS_LEN)?).ok()?;
            NaiveDateTime::parse_from_str(head, IDE_TS_FORMAT).ok()?
        }
    };
    // DST 回拨的那一小时是歧义的：取较早的一次，而不是丢掉这一行。
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|date| date.timestamp_millis())
}

/// 官方给出的重置时刻（毫秒）。原文形如 `将在 2026-09-17 17:59:27 UTC+8 重置` /
/// `will reset at 2026-09-14 10:59:21 UTC+8,`。
///
/// 直接采用原文值（含原文声明的时区偏移），不自建窗口模型。hook payload 的
/// `last_assistant_message` 走同一个入口，保证两条通路的时刻口径一致。
pub(crate) fn parse_reset_at(line: &str) -> Option<i64> {
    for marker in RESET_MARKERS {
        let Some(index) = line.find(marker) else {
            continue;
        };
        let rest = &line[index + marker.len()..];
        let Some(naive) = parse_datetime_prefix(rest) else {
            continue;
        };
        let Some(offset) = parse_utc_offset(rest) else {
            continue;
        };
        return offset
            .from_local_datetime(&naive)
            .single()
            .map(|date| date.timestamp_millis());
    }
    None
}

/// 取前缀里的 `YYYY-MM-DD HH:MM:SS`。
pub(crate) fn parse_datetime_prefix(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.get(..19)?, "%Y-%m-%d %H:%M:%S").ok()
}

/// 解析文案里的 `UTC±H[:MM]` 偏移。
pub(crate) fn parse_utc_offset(text: &str) -> Option<FixedOffset> {
    let index = text.find("UTC")?;
    let rest = &text[index + 3..];
    let sign = match rest.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = rest[1..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours.parse::<i32>().ok()?, minutes.parse::<i32>().ok()?),
        None => match digits.len() {
            1 | 2 => (digits.parse::<i32>().ok()?, 0),
            4 => (
                digits[..2].parse::<i32>().ok()?,
                digits[2..].parse::<i32>().ok()?,
            ),
            _ => return None,
        },
    };
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

/// 取行尾 `(requestId/sessionId)`：32 位 hex 请求 id + 带连字符的 36 位会话 uuid。
///
/// 括号里那串 hex 是 **requestId，不是账号**；账号归因只能走会话 id。
pub(crate) fn request_pair(line: &str) -> Option<(String, String)> {
    let mut found = None;
    let mut search = 0;
    while let Some(offset) = line[search..].find('(') {
        let start = search + offset + 1;
        let Some(end) = line[start..].find(')') else {
            break;
        };
        if let Some((request, session)) = line[start..start + end].split_once('/') {
            if is_hex_id(request, 32) && is_hex_id(session, 36) {
                found = Some((request.to_string(), session.to_string()));
            }
        }
        search = start + end;
    }
    found
}

/// 鉴权行给出的账号 uid（顺序扫描时维护「事件前最近一次」）。
///
/// `uid=none` 是「无会话 / 未登录」的哨兵值，必须忽略。
pub(crate) fn auth_uid(line: &str, auth: AuthMarker) -> Option<&str> {
    let marker = match auth {
        AuthMarker::None => return None,
        AuthMarker::AuthDoInitProbe => CLI_AUTH_MARKER,
        AuthMarker::AuthSessionChanged => IDE_AUTH_MARKER,
    };
    if !line.contains(marker) {
        return None;
    }
    let uid = field_after(line, "uid=")?;
    (!uid.eq_ignore_ascii_case("none")).then_some(uid)
}
