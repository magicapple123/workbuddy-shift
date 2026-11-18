//! JSONL 数据源：逐行子树解码、来源收集器与多根聚合。

use super::*;

// ---------------------------------------------------------------------------
// 逐行解码：只取统计需要的子树
// ---------------------------------------------------------------------------

/// `message` / `providerData` 这类「只关心其中几个键」的对象。
///
/// 手写 `Visitor` 换来两件事：
/// 1. 任意非对象形状（字符串、数组、数字）都按「没有这些键」处理，与「整行解析成
///    `Value` 再取键」的行为一致，不会把本来能解析的行误判成解析错误；
/// 2. 无关的大字段（`content`、`reasoning`）走 [`IgnoredAny`]，零分配。
///
/// 重复键保留最后一次出现的值，同样与 `serde_json` 的 `Value` 一致。
#[derive(Debug, Default)]
pub(super) struct SubObject {
    pub(super) usage: Option<Value>,
    pub(super) model: Option<Value>,
    pub(super) raw_usage: Option<Value>,
}

impl<'de> Deserialize<'de> for SubObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SubObjectVisitor)
    }
}

pub(super) struct SubObjectVisitor;

impl<'de> Visitor<'de> for SubObjectVisitor {
    type Value = SubObject;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("任意 JSON 值")
    }

    fn visit_map<A>(self, mut map: A) -> Result<SubObject, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = SubObject::default();
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            match key.as_ref() {
                "usage" => object.usage = Some(map.next_value()?),
                "model" => object.model = Some(map.next_value()?),
                "rawUsage" => object.raw_usage = Some(map.next_value()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(object)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<SubObject, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(SubObject::default())
    }

    fn visit_bool<E>(self, _: bool) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }

    fn visit_i64<E>(self, _: i64) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }

    fn visit_u64<E>(self, _: u64) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }

    fn visit_f64<E>(self, _: f64) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }

    fn visit_str<E>(self, _: &str) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }

    fn visit_unit<E>(self) -> Result<SubObject, E>
    where
        E: serde::de::Error,
    {
        Ok(SubObject::default())
    }
}

/// 单行 JSONL 里参与统计的全部字段。
///
/// **不**把整行建成 `serde_json::Value`。本地日志里消息正文与工具参数占了约七成
/// 字节，整行建成 `Value` 会把它们全部分配出来再丢掉：本项目实测 839 MB 日志，
/// 整行 `Value` 解析 ~2.8 s，只取需要的子树 ~1.07 s（两者都已含读盘）。
///
/// 保留的子树仍然是 `Value`，所以后面 `usage_from` / `timestamp_from` /
/// `model_from` / `thinking_from` 的解码口径与整行解析逐字相同。
#[derive(Debug, Default)]
pub(super) struct JsonlLine {
    pub(super) timestamp: Option<Value>,
    pub(super) ts: Option<Value>,
    pub(super) cwd: Option<Value>,
    pub(super) ai_title: Option<Value>,
    pub(super) summary: Option<Value>,
    pub(super) usage: Option<Value>,
    pub(super) model: Option<Value>,
    pub(super) message: Option<SubObject>,
    pub(super) provider_data: Option<SubObject>,
}

impl<'de> Deserialize<'de> for JsonlLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonlLineVisitor)
    }
}

pub(super) struct JsonlLineVisitor;

impl<'de> Visitor<'de> for JsonlLineVisitor {
    type Value = JsonlLine;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("任意 JSON 值")
    }

    fn visit_map<A>(self, mut map: A) -> Result<JsonlLine, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut line = JsonlLine::default();
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            match key.as_ref() {
                "timestamp" => line.timestamp = Some(map.next_value()?),
                "ts" => line.ts = Some(map.next_value()?),
                "cwd" => line.cwd = Some(map.next_value()?),
                "aiTitle" => line.ai_title = Some(map.next_value()?),
                "summary" => line.summary = Some(map.next_value()?),
                "usage" => line.usage = Some(map.next_value()?),
                "model" => line.model = Some(map.next_value()?),
                "message" => line.message = Some(map.next_value()?),
                "providerData" => line.provider_data = Some(map.next_value()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(line)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<JsonlLine, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(JsonlLine::default())
    }

    fn visit_bool<E>(self, _: bool) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }

    fn visit_i64<E>(self, _: i64) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }

    fn visit_u64<E>(self, _: u64) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }

    fn visit_f64<E>(self, _: f64) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }

    fn visit_str<E>(self, _: &str) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }

    fn visit_unit<E>(self) -> Result<JsonlLine, E>
    where
        E: serde::de::Error,
    {
        Ok(JsonlLine::default())
    }
}

#[derive(Default)]
pub(super) struct SourceCollector {
    pub(super) total: Totals,
    pub(super) models: HashMap<String, Totals>,
    pub(super) projects: HashMap<String, Totals>,
    pub(super) sessions: Vec<SessionTotals>,
    pub(super) daily: HashMap<String, Totals>,
    pub(super) daily_by_model: HashMap<String, HashMap<String, Totals>>,
    pub(super) hours: HashMap<String, Totals>,
    pub(super) parse_errors: u64,
    pub(super) coverage_start_at: Option<i64>,
    pub(super) coverage_end_at: Option<i64>,
    pub(super) session_key_counts: HashMap<String, usize>,
    pub(super) requests: Vec<RequestRow>,
    pub(super) collect_requests: bool,
}

impl SourceCollector {
    pub(super) fn note_parse_error(&mut self) {
        self.parse_errors = self.parse_errors.saturating_add(1);
    }

    /// 一次性计入若干条解析错误（缓存里存的是文件级计数，命中缓存也要照常累计）。
    pub(super) fn note_parse_errors(&mut self, errors: u64) {
        self.parse_errors = self.parse_errors.saturating_add(errors);
    }

    /// Append one request row. Once the in-memory hard limit is exceeded, keep
    /// only the newest window: the sort is stable, so rows sharing a timestamp
    /// keep their collection order and the oldest rows are dropped.
    pub(super) fn push_request(&mut self, row: RequestRow) {
        if !self.collect_requests {
            return;
        }
        self.requests.push(row);
        if self.requests.len() > REQUEST_COLLECT_LIMIT {
            self.requests
                .sort_by_key(|row| std::cmp::Reverse(row.timestamp));
            self.requests.truncate(REQUEST_WINDOW);
        }
    }

    /// 把一条已解码的记录并入各维度聚合。
    ///
    /// 时间戳、模型名、项目名都由调用方传入解码结果，这里不再回头读 `Value`：
    /// 逐条聚合是扫描阶段的热路径，重复解码与重复分配都在这里放大。
    pub(super) fn add(
        &mut self,
        usage: Usage,
        model_name: &str,
        project: &str,
        timestamp: Option<i64>,
    ) {
        self.total.add(usage);
        add_to(&mut self.models, model_name, usage);
        add_to(&mut self.projects, project, usage);
        if let Some(timestamp) = timestamp {
            if let Some((day, hour)) = local_buckets(timestamp) {
                add_to(&mut self.daily, &day, usage);
                if !self.daily_by_model.contains_key(model_name) {
                    self.daily_by_model
                        .insert(model_name.to_string(), HashMap::new());
                }
                if let Some(per_model) = self.daily_by_model.get_mut(model_name) {
                    add_to(per_model, &day, usage);
                }
                add_to(&mut self.hours, &hour, usage);
            }
            self.coverage_start_at = Some(
                self.coverage_start_at
                    .map_or(timestamp, |current| current.min(timestamp)),
            );
            self.coverage_end_at = Some(
                self.coverage_end_at
                    .map_or(timestamp, |current| current.max(timestamp)),
            );
        }
    }

    pub(super) fn push_session(
        &mut self,
        session_id: String,
        title: Option<String>,
        project: String,
        totals: Totals,
    ) {
        if totals.records == 0 {
            return;
        }
        let base_key = format!("{project} · {session_id}");
        let count = self.session_key_counts.entry(base_key.clone()).or_default();
        *count += 1;
        let key = if *count == 1 {
            base_key
        } else {
            format!("{base_key} · {}", *count)
        };
        self.sessions.push(SessionTotals {
            key,
            title,
            project,
            session_id,
            totals,
        });
    }

    pub(super) fn into_value(mut self, name: &str, files_scanned: usize) -> Value {
        let daily_by_model = self
            .daily_by_model
            .into_iter()
            .map(|(model, points)| (model, Value::Array(groups(points))))
            .collect::<Map<String, Value>>();
        let mut value = json!({
            "source": name,
            "summary": self.total.value(),
            "models": groups(self.models),
            "projects": groups(self.projects),
            "sessions": session_groups(self.sessions),
            "daily": groups(self.daily),
            "dailyByModel": daily_by_model,
            "hours": groups(self.hours),
            "filesScanned": files_scanned,
            "parseErrors": self.parse_errors,
            "coverageStartAt": self.coverage_start_at,
            "coverageEndAt": self.coverage_end_at,
        });
        // Only detail sources take the new key; every other source keeps its
        // previous response shape byte for byte.
        if self.collect_requests {
            self.requests
                .sort_by_key(|row| std::cmp::Reverse(row.timestamp));
            self.requests.truncate(REQUEST_WINDOW);
            value["requests"] = Value::Array(self.requests.iter().map(RequestRow::value).collect());
        }
        value
    }
}

/// 单根统计：转发到多根实现，供 CodeBuddy CLI 这类固定单根的数据源使用。
/// `detail` additionally keeps a bounded window of per-call request rows; both
/// outputs share the same decode, cutoff, dedupe, and `subagents`-exclusion
/// pipeline so a detail row can never disagree with the aggregate totals.
pub(super) fn source(root: PathBuf, name: &str, cutoff: Option<i64>, detail: bool) -> Value {
    source_from_roots(std::slice::from_ref(&root), name, cutoff, detail)
}

/// 多根合并统计：同一档位下的多个 jsonl 根合并成**一个** source。
///
/// 国际版数据根与国内版不同构（实测无 `projects/`、只有 `sessions/`），因此按
/// 「存在的根」探测；一个都不存在时返回空集且不报错（前端显示空状态）。
pub(super) fn source_from_roots(
    roots: &[PathBuf],
    name: &str,
    cutoff: Option<i64>,
    detail: bool,
) -> Value {
    let mut targets: Vec<Target> = Vec::new();
    for root in roots {
        let mut found = Vec::new();
        files(root, &mut found);
        targets.extend(
            found
                .into_iter()
                .map(|file| Target::new(root.clone(), file)),
        );
    }
    let mut collector = SourceCollector {
        collect_requests: detail,
        ..SourceCollector::default()
    };

    // Copied/forked sessions replay the parent history (including usage
    // records with their original timestamps) into their own JSONL, so the
    // same request would otherwise be counted once per copy. Process files
    // oldest first and skip fingerprints already seen in an earlier file so
    // usage stays attributed to the original session.
    //
    // 排序键直接用扫描时取到的 mtime（同一份数据既用于排序也用于窗口裁剪），
    // 取不到 mtime 的文件排在最后，与原实现一致。
    targets.sort_by_key(|target| target.modified_ms.unwrap_or(i64::MAX));
    let mut seen: HashSet<(i64, u64, u64, u64, u64, Arc<str>)> = HashSet::new();

    for target in &targets {
        // 整份文件都早于窗口起点时不必读盘：记录时间戳不可能晚于落盘时间
        // （留一天余量吸收时钟回拨）。注意 `filesScanned` 仍按发现的文件总数
        // 返回，裁剪只影响读盘量。
        if target.is_outside(cutoff) {
            continue;
        }
        let session_id = target.session_id();
        let fallback_project = project_name(&target.root, &target.path);
        let Some(parsed) = parsed_file(&target.path) else {
            collector.note_parse_error();
            continue;
        };
        collector.note_parse_errors(parsed.parse_errors);

        let mut session_totals = Totals::default();
        // Detail rows are buffered per file so the file-scoped title can be
        // backfilled once the whole file has been read.
        let mut file_requests: Vec<RequestRow> = Vec::new();
        let mut session_project: Option<String> = None;

        for record in &parsed.records {
            // Records with a missing timestamp are excluded from a bounded
            // range rather than guessed from file mtime or browser time.
            if cutoff.is_some_and(|minimum| record.timestamp.is_none_or(|ts| ts < minimum)) {
                continue;
            }
            // Fingerprint = (timestamp, full usage, model). Records without a
            // timestamp cannot be fingerprinted and are counted as before; they
            // are also unsortable and therefore never appear in the detail
            // window. The interned model name is shared, so building the
            // fingerprint key costs no allocation.
            let duplicate = record.timestamp.is_some_and(|ts| {
                !seen.insert((
                    ts,
                    record.usage.input,
                    record.usage.output,
                    record.usage.read,
                    record.usage.write,
                    Arc::clone(&record.model),
                ))
            });
            if duplicate {
                continue;
            }
            let project = record.project.as_deref().unwrap_or(&fallback_project);
            if session_project.is_none() {
                session_project = Some(project.to_string());
            }
            session_totals.add(record.usage);
            collector.add(record.usage, &record.model, project, record.timestamp);
            if collector.collect_requests {
                if let Some(ts) = record.timestamp {
                    file_requests.push(RequestRow {
                        timestamp: ts,
                        model: record.model.to_string(),
                        project: project.to_string(),
                        session_id: session_id.clone(),
                        title: None,
                        usage: record.usage,
                        thinking: record.thinking,
                    });
                }
            }
        }

        // `aiTitle` can appear anywhere in the file (including after the usage
        // records), so the file title is only final once the file is read.
        let title = parsed.title.clone();
        if !file_requests.is_empty() {
            for row in &mut file_requests {
                row.title = title.clone();
            }
            for row in file_requests {
                collector.push_request(row);
            }
        }
        collector.push_session(
            session_id,
            title,
            session_project.unwrap_or(fallback_project),
            session_totals,
        );
    }

    collector.into_value(name, targets.len())
}
