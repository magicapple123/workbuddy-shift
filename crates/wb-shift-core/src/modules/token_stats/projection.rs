//! 解码口径与响应投影：用量/时间戳/模型字段、分组合计与明细行。

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Usage {
    pub(super) input: u64,
    pub(super) output: u64,
    pub(super) read: u64,
    pub(super) write: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Totals {
    pub(super) usage: Usage,
    pub(super) records: u64,
}

#[derive(Clone, Debug)]
pub(super) struct SessionTotals {
    pub(super) key: String,
    pub(super) title: Option<String>,
    pub(super) project: String,
    pub(super) session_id: String,
    pub(super) totals: Totals,
}

/// 展示总量：`input` 已包含 cache reads，因此只追加 output 与 cache write，
/// 避免把缓存命中重复计入。聚合与请求明细共用这一口径。
pub(super) fn usage_total(usage: Usage) -> u64 {
    usage
        .input
        .saturating_add(usage.output)
        .saturating_add(usage.write)
}

impl Totals {
    pub(super) fn add(&mut self, usage: Usage) {
        self.usage.input = self.usage.input.saturating_add(usage.input);
        self.usage.output = self.usage.output.saturating_add(usage.output);
        self.usage.read = self.usage.read.saturating_add(usage.read);
        self.usage.write = self.usage.write.saturating_add(usage.write);
        self.records = self.records.saturating_add(1);
    }

    pub(super) fn value(&self) -> Value {
        let cache_hit_rate =
            (self.usage.input > 0).then(|| self.usage.read as f64 / self.usage.input as f64);
        // `input` already includes cache reads; expose the same headline total
        // used by the dashboard without double-counting the cached portion.
        let total = usage_total(self.usage);
        json!({
            "total": total,
            "input": self.usage.input,
            "output": self.usage.output,
            "cacheRead": self.usage.read,
            "cacheWrite": self.usage.write,
            "uncachedInput": self.usage.input.saturating_sub(self.usage.read),
            "records": self.records,
            "cacheHitRate": cache_hit_rate,
        })
    }
}

/// 一次模型调用的明细行。只携带脱敏标识与 token 数字：不含消息正文、
/// 工具参数、绝对路径或认证信息。
#[derive(Clone, Debug)]
pub(super) struct RequestRow {
    pub(super) timestamp: i64,
    pub(super) model: String,
    pub(super) project: String,
    pub(super) session_id: String,
    pub(super) title: Option<String>,
    pub(super) usage: Usage,
    /// 思考过程 token 数（`output` 的子集），仅用于拆分「思考 / 回复」。
    pub(super) thinking: u64,
}

impl RequestRow {
    pub(super) fn value(&self) -> Value {
        json!({
            "timestamp": self.timestamp,
            "model": self.model,
            "project": self.project,
            "sessionId": self.session_id,
            "title": self.title,
            "input": self.usage.input,
            "output": self.usage.output,
            "cacheRead": self.usage.read,
            "cacheWrite": self.usage.write,
            // 与 `Totals::value()` 同一口径：input 已含 cacheRead，
            // 未命中部分即两者之差，下溢时饱和为 0。
            "uncachedInput": self.usage.input.saturating_sub(self.usage.read),
            // 回复内容 = max(0, output - thinking) 由前端做，这里只给原始计数；
            // 实测存在 thinking > output 的异常记录，故前端必须饱和减。
            "thinking": self.thinking,
            "total": usage_total(self.usage),
        })
    }
}

/// Read a non-negative integer from a JSON number or string.
pub(super) fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map(|n| n as u64)
        })
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
}

pub(super) fn field(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(number))
}

pub(super) fn positive_field(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(number).filter(|value| *value > 0))
}

pub(super) fn cached_input_field(object: &Map<String, Value>) -> u64 {
    // Providers have emitted both flat aliases and OpenAI-compatible nested
    // details. Prefer a positive flat alias so a stale `cache_read...: 0`
    // field cannot hide a populated `prompt_cache_hit_tokens` value.
    positive_field(
        object,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "prompt_cache_hit_tokens",
            "cached_tokens",
        ],
    )
    .or_else(|| {
        object
            .get("prompt_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| positive_field(details, &["cached_tokens"]))
    })
    .or_else(|| {
        object
            .get("inputTokensDetails")
            .and_then(Value::as_array)
            .and_then(|details| {
                details.iter().find_map(|detail| {
                    detail
                        .as_object()
                        .and_then(|detail| positive_field(detail, &["cached_tokens"]))
                })
            })
    })
    .unwrap_or(0)
}

pub(super) const CACHE_WRITE_KEYS: &[&str] = &[
    "cache_write_input_tokens",
    "cacheWriteInputTokens",
    "cache_creation_input_tokens",
    "prompt_cache_write_tokens",
];

pub(super) fn usage_fields(object: &Map<String, Value>) -> Usage {
    Usage {
        input: field(object, &["input_tokens", "inputTokens", "prompt_tokens"]).unwrap_or(0),
        output: field(
            object,
            &["output_tokens", "outputTokens", "completion_tokens"],
        )
        .unwrap_or(0),
        read: cached_input_field(object),
        write: positive_field(object, CACHE_WRITE_KEYS).unwrap_or(0),
    }
}

pub(super) fn usage_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value?.as_object().filter(|object| {
        // Input is the required anchor for a usage record. It may legitimately
        // be zero (for example a provider reports output-only retries), so do
        // not use `input > 0` as the validity check.
        field(object, &["input_tokens", "inputTokens", "prompt_tokens"]).is_some()
    })
}

/// Decode one record. Usage precedence is message.usage > providerData.usage >
/// top-level usage. Cache-write metadata may only exist on a non-selected
/// usage object or rawUsage, so those objects are consulted without counting
/// their input/output again.
///
/// 生产路径走 [`usage_from`]（逐行扫描时子树已经取好）；这个整行入口只给单测用，
/// 便于用一小段 JSON 直接钉住解码口径。
#[cfg(test)]
pub(super) fn usage(value: &Value) -> Option<Usage> {
    let provider = value.get("providerData");
    usage_from(
        value
            .get("message")
            .and_then(|message| message.get("usage")),
        provider.and_then(|data| data.get("usage")),
        value.get("usage"),
        provider.and_then(|data| data.get("rawUsage")),
    )
}

/// [`usage`] 的「子树已取好」形态：逐行扫描时只保留这几棵子树，
/// 判定顺序与口径和整行解析完全一致。
pub(super) fn usage_from(
    message_usage: Option<&Value>,
    provider_usage: Option<&Value>,
    top_usage: Option<&Value>,
    raw_usage: Option<&Value>,
) -> Option<Usage> {
    let candidates = [message_usage, provider_usage, top_usage];
    let selected = candidates.iter().copied().find_map(usage_object)?;
    let mut result = usage_fields(selected);

    if result.write == 0 {
        result.write = candidates
            .iter()
            .copied()
            .filter_map(|candidate| candidate.and_then(Value::as_object))
            .chain(raw_usage.and_then(Value::as_object))
            .find_map(|object| positive_field(object, CACHE_WRITE_KEYS))
            .unwrap_or(0);
    }

    // prompt_cache_miss_tokens is deliberately not a write alias: current
    // WorkBuddy/CodeBuddy logs use it for newly computed (uncached) input,
    // while their explicit cache-write fields may legitimately remain zero.

    Some(result)
}

pub(super) fn timestamp(value: &Value) -> Option<i64> {
    timestamp_from(value.get("timestamp"), value.get("ts"))
}

/// 时间戳：`timestamp` 优先，缺失时才看 `ts`。
///
/// 注意 `or` 而不是 `or_else(|| timestamp_of(ts))`：`timestamp` 存在但不是数字
/// （例如 `null`）时**不**回退到 `ts`，与早期「整行解析成 `Value` 再取键」的口径一致。
pub(super) fn timestamp_from(timestamp: Option<&Value>, ts: Option<&Value>) -> Option<i64> {
    timestamp.or(ts).and_then(|timestamp| {
        timestamp
            .as_i64()
            .or_else(|| timestamp.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| timestamp.as_str()?.trim().parse::<i64>().ok())
    })
}

/// 一条记录归属的「本地日期」与「星期-小时」桶。
///
/// 两个键由**同一次**时区换算得出：原实现分别走 `date()` / `hour()`，每条记录要做
/// 两次 `Local` 换算，而本地时区换算正是逐条聚合里最贵的一步。
pub(super) fn local_buckets(timestamp: i64) -> Option<(String, String)> {
    let local = chrono::DateTime::from_timestamp_millis(timestamp)?.with_timezone(&Local);
    Some((
        local.format("%Y-%m-%d").to_string(),
        format!(
            "{}-{}",
            local.weekday().num_days_from_monday(),
            local.hour()
        ),
    ))
}

/// 模型名：`providerData.model` 优先，其次顶层 `model`，都没有则 `UNKNOWN_MODEL`。
///
/// 返回借用而不是 `String`：同一批日志里模型名高度重复，由调用方决定是否驻留
/// （见 [`intern`]），避免每条记录都分配一次。
pub(super) fn model_from<'a>(
    provider_model: Option<&'a Value>,
    top_model: Option<&'a Value>,
) -> &'a str {
    provider_model
        .or(top_model)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(UNKNOWN_MODEL)
}

/// 思考过程 token 数：优先 `providerData.rawUsage.completion_thinking_tokens`，
/// 回退到 OpenAI 形状的 `completion_tokens_details.reasoning_tokens`，都没有则为 0。
///
/// 只读取计数字段：`providerData.reasoning` 是思考**正文**，属隐私红线，
/// 任何情况下都不得读取或返回。
pub(super) fn thinking_from(raw_usage: Option<&Value>) -> u64 {
    let Some(raw_usage) = raw_usage.and_then(Value::as_object) else {
        return 0;
    };
    field(raw_usage, &["completion_thinking_tokens"])
        .or_else(|| {
            raw_usage
                .get("completion_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| field(details, &["reasoning_tokens"]))
        })
        .unwrap_or(0)
}

pub(super) fn files(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type()` 直接来自目录项，不需要额外 stat；`is_dir()` 在 Windows 上
        // 每个条目都要多一次系统调用。软链接仍按目录跟随，与原实现一致。
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let path = entry.path();
        if is_dir || path.is_dir() {
            // Subagent logs duplicate parent-session context and are not part
            // of either product's primary usage accounting.
            if path.file_name().and_then(|name| name.to_str()) != Some("subagents") {
                files(&path, output);
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            output.push(path);
        }
    }
}

pub(super) fn project_name(root: &Path, file: &Path) -> String {
    let name = file
        .strip_prefix(root)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| component.as_os_str().to_str())
        .filter(|name| !name.is_empty() && !name.ends_with(".jsonl"));
    match name {
        // Product directories commonly encode the complete absolute path.
        // Returning that would leak a user name and parent directories.
        Some(name) if !name.starts_with("Users-") && !name.starts_with("home-") => name.to_string(),
        _ => "未知项目".to_string(),
    }
}

/// 记录自带 `cwd` 时取它的 basename；取不到时返回 `None`，由调用方回退到文件
/// 所在的项目目录。返回借用而不是 `String`：项目名同样高度重复，统一由
/// [`intern`] 驻留。
pub(super) fn record_project(cwd: Option<&Value>) -> Option<&str> {
    cwd.and_then(Value::as_str)
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 120)
}

pub(super) fn non_empty_text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 把重复出现的名字驻留成一份 `Arc<str>`：模型名、项目名在日志里高度重复，
/// 每条记录各存一个 `String` 会让逐条聚合变成分配器瓶颈。
pub(super) fn intern(cache: &mut HashMap<String, Arc<str>>, value: &str) -> Arc<str> {
    if let Some(existing) = cache.get(value) {
        return Arc::clone(existing);
    }
    let interned: Arc<str> = Arc::from(value);
    cache.insert(value.to_string(), Arc::clone(&interned));
    interned
}

pub(super) fn groups(groups: HashMap<String, Totals>) -> Vec<Value> {
    let mut values: Vec<_> = groups
        .into_iter()
        .map(|(key, totals)| {
            let mut value = totals.value();
            value["key"] = json!(key);
            value
        })
        .collect();
    values.sort_by_key(|right| std::cmp::Reverse(total_value(right)));
    values
}

pub(super) fn session_groups(sessions: Vec<SessionTotals>) -> Vec<Value> {
    let mut values: Vec<_> = sessions
        .into_iter()
        .map(|session| {
            let mut value = session.totals.value();
            value["key"] = json!(session.key);
            value["title"] = json!(session.title);
            value["project"] = json!(session.project);
            value["sessionId"] = json!(session.session_id);
            value
        })
        .collect();
    values.sort_by_key(|right| std::cmp::Reverse(total_value(right)));
    values
}

pub(super) fn total_value(value: &Value) -> u64 {
    value
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            value
                .get("input")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(value.get("output").and_then(Value::as_u64).unwrap_or(0))
                .saturating_add(value.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0))
        })
}

/// 往分组表里累加：命中时完全不分配（`HashMap<String, _>` 可以按 `&str` 查），
/// 只有第一次见到这个键才分配一次 `String`。
///
/// 逐条聚合里模型名/项目名/日期都是高度重复的键，`entry(key.to_string())` 会让每条
/// 记录都分配一次，这里换成「先查后插」。
pub(super) fn add_to(groups: &mut HashMap<String, Totals>, key: &str, usage: Usage) {
    if let Some(totals) = groups.get_mut(key) {
        totals.add(usage);
        return;
    }
    groups.entry(key.to_string()).or_default().add(usage);
}
