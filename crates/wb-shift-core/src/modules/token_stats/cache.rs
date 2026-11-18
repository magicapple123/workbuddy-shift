//! 单文件解析结果缓存与扫描目标：文件指纹、进程内缓存与解析计数。

use super::*;

/// 明细中最多返回的请求条数（响应窗口）。
pub(super) const REQUEST_WINDOW: usize = 500;
/// 扫描期间明细缓存的内存硬上限；达到该值后立即裁剪到 `REQUEST_WINDOW`，
/// 使峰值内存恒定，不随历史总量增长。
pub(super) const REQUEST_COLLECT_LIMIT: usize = 4_000;

/// 逐行读取日志时的读缓冲。日志行可以很长（一条消息正文就有几十 KB），
/// 缓冲给小了会让 `read_until` 频繁扩容。
pub(super) const READ_BUFFER: usize = 64 * 1024;
/// 单文件解析结果缓存的记录条数上限。达到上限后新文件不再入缓存，
/// 内存上限恒定（一条记录约 80 字节，50 万条约 40 MB）。
pub(super) const CACHE_MAX_RECORDS: usize = 500_000;
/// 单文件解析结果缓存的条目数上限（防止看过几十万个文件的极端场景）。
pub(super) const CACHE_MAX_FILES: usize = 4_096;
/// 按文件 mtime 裁剪窗口外文件时容忍的时钟偏差。
///
/// 记录时间戳来自客户端时钟，理论上可能晚于落盘时间（系统时钟回拨），
/// 因此只有「mtime 加一天仍早于窗口起点」的文件才会被整份跳过。
pub(super) const MTIME_SLACK_MS: i64 = 86_400_000;
/// 缺少模型名时的占位符（与既有响应口径一致）。
pub(super) const UNKNOWN_MODEL: &str = "未知模型";

/// 单测用的解析次数计数器（按路径）：用来断言「指纹不变就该命中缓存，不该再解析」。
///
/// 按路径计数而不是全局计数：单测是并行跑的，别的用例解析别的文件不该影响这里的断言。
#[cfg(test)]
pub(super) static PARSE_COUNTS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

#[cfg(test)]
pub(super) fn note_parse_for_test(path: &Path) {
    let mut counts = PARSE_COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts
        .get_or_insert_with(HashMap::new)
        .entry(path.to_path_buf())
        .or_default() += 1;
}

#[cfg(test)]
pub(super) fn parse_count(path: &Path) -> usize {
    PARSE_COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|counts| counts.get(path).copied())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 单文件解析 + 进程内缓存
// ---------------------------------------------------------------------------

/// 一条待聚合的记录：只保留聚合、去重与明细需要的字段。
///
/// 与源文件体积无关（本机 839 MB 日志 → 约 6 万条、几 MB 内存），因此可以安全地
/// 在进程内长期缓存。
#[derive(Clone, Debug)]
pub(super) struct ParsedRecord {
    /// 记录时间戳；缺失时无法去重也无法进明细，但仍计入聚合。
    pub(super) timestamp: Option<i64>,
    pub(super) model: Arc<str>,
    /// 记录自带 `cwd` 的项目名；`None` 表示回退到文件所在的项目目录。
    ///
    /// 特意不把回退值烘焙进缓存：同一条路径可能从不同根被扫描（国际版有
    /// `projects/` 与 `sessions/` 两个根），回退值由聚合阶段决定。
    pub(super) project: Option<Arc<str>>,
    pub(super) usage: Usage,
    /// 思考过程 token 数（`output` 的子集），仅明细需要，随记录一起缓存。
    pub(super) thinking: u64,
}

/// 一个 JSONL 文件的解析产物。
///
/// **与时间窗无关**：窗口在聚合阶段才应用，所以 7 / 30 / 90 天与「全部」共用
/// 同一份缓存；解析错误也在这里计数，避免缓存命中后 `parseErrors` 变少。
#[derive(Debug, Default)]
pub(super) struct ParsedFile {
    pub(super) records: Vec<ParsedRecord>,
    /// 文件级标题：`aiTitle` 优先，其次 `summary`（整份文件读完才定稿）。
    pub(super) title: Option<String>,
    pub(super) parse_errors: u64,
}

/// 文件指纹：修改时间 + 大小。任一变化都说明内容可能变了，必须重解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Fingerprint {
    pub(super) modified: Option<SystemTime>,
    pub(super) size: u64,
}

pub(super) fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Fingerprint {
        modified: metadata.modified().ok(),
        size: metadata.len(),
    })
}

/// 进程内「路径 → 解析产物」缓存。
///
/// 只存在内存里：本工具唯一的落盘目录是它自己的沙箱目录，这里不需要序列化，
/// 进程退出即失效，不存在「缓存被写脏又跨进程复用」的风险。
pub(super) static FILE_CACHE: OnceLock<Mutex<FileCache>> = OnceLock::new();

#[derive(Default)]
pub(super) struct FileCache {
    pub(super) entries: HashMap<PathBuf, CachedFile>,
    /// 已缓存记录总数，用于给内存封顶。
    pub(super) records: usize,
}

impl FileCache {
    /// 写入一条缓存，返回是否真的写进去了。
    ///
    /// 封顶后**不再**缓存新文件：内存上限恒定，已缓存的照旧命中、也照旧允许覆盖
    /// （覆盖不改记录数上限，只是换一份内容）。
    pub(super) fn store(
        &mut self,
        path: &Path,
        fingerprint: Fingerprint,
        parsed: &Arc<ParsedFile>,
    ) -> bool {
        let is_known = self.entries.contains_key(path);
        if !is_known && self.entries.len() >= CACHE_MAX_FILES {
            return false;
        }
        let records_without_previous = match self.entries.get(path) {
            Some(previous) => self.records.saturating_sub(previous.parsed.records.len()),
            None => self.records,
        };
        if records_without_previous.saturating_add(parsed.records.len()) > CACHE_MAX_RECORDS {
            return false;
        }
        self.records = records_without_previous.saturating_add(parsed.records.len());
        self.entries.insert(
            path.to_path_buf(),
            CachedFile {
                fingerprint,
                parsed: Arc::clone(parsed),
            },
        );
        true
    }
}

pub(super) struct CachedFile {
    pub(super) fingerprint: Fingerprint,
    pub(super) parsed: Arc<ParsedFile>,
}

/// 取缓存；互斥锁中毒时照常使用里面的数据（只是解析结果，坏了重解析即可）。
pub(super) fn file_cache() -> MutexGuard<'static, FileCache> {
    FILE_CACHE
        .get_or_init(|| Mutex::new(FileCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn cache_lookup(path: &Path, fingerprint: Fingerprint) -> Option<Arc<ParsedFile>> {
    let cache = file_cache();
    let entry = cache.entries.get(path)?;
    (entry.fingerprint == fingerprint).then(|| Arc::clone(&entry.parsed))
}

pub(super) fn cache_store(path: &Path, fingerprint: Fingerprint, parsed: &Arc<ParsedFile>) {
    file_cache().store(path, fingerprint, parsed);
}

/// 读并解析一个 JSONL 文件。
///
/// 返回 `None` 仅表示「文件打不开」（调用方记一次解析错误）；单行解析失败只累加
/// `parse_errors`，不影响其它行。
///
/// 逐行用 [`BufRead::read_until`] 复用一个字节缓冲，而不是 `lines()`：后者为每一行
/// 分配一个 `String` 并做一次 UTF-8 校验，25 万行就是 25 万次分配。
pub(super) fn parse_jsonl_file(path: &Path) -> Option<ParsedFile> {
    #[cfg(test)]
    note_parse_for_test(path);
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::with_capacity(READ_BUFFER, file);
    let mut parsed = ParsedFile::default();
    let mut buffer: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut models: HashMap<String, Arc<str>> = HashMap::new();
    let mut projects: HashMap<String, Arc<str>> = HashMap::new();
    let mut ai_title: Option<String> = None;
    let mut summary: Option<String> = None;

    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                // 读到一半失败：记一次解析错误后收手。继续读会无限循环
                // （底层迭代器同样会一直返回错误）。
                parsed.parse_errors = parsed.parse_errors.saturating_add(1);
                break;
            }
        }
        // 与 `lines()` 对齐：去掉行尾 `\n`，以及紧随其后的一个 `\r`。
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
            if buffer.last() == Some(&b'\r') {
                buffer.pop();
            }
        }

        let Ok(line) = serde_json::from_slice::<JsonlLine>(&buffer) else {
            parsed.parse_errors = parsed.parse_errors.saturating_add(1);
            continue;
        };

        // 标题属于整份会话文件，且要在窗口判定之前读取：窗口外的标题仍要
        // 为窗口内的用量命名。`aiTitle` 优先于 `summary`，与出现顺序无关。
        if let Some(title) = non_empty_text(line.ai_title.as_ref()) {
            ai_title = Some(title);
        }
        if let Some(title) = non_empty_text(line.summary.as_ref()) {
            summary = Some(title);
        }

        let Some(usage) = usage_from(
            line.message
                .as_ref()
                .and_then(|message| message.usage.as_ref()),
            line.provider_data
                .as_ref()
                .and_then(|provider| provider.usage.as_ref()),
            line.usage.as_ref(),
            line.provider_data
                .as_ref()
                .and_then(|provider| provider.raw_usage.as_ref()),
        ) else {
            continue;
        };

        let model_name = model_from(
            line.provider_data
                .as_ref()
                .and_then(|provider| provider.model.as_ref()),
            line.model.as_ref(),
        );
        let timestamp = timestamp_from(line.timestamp.as_ref(), line.ts.as_ref());
        let project = record_project(line.cwd.as_ref());
        let thinking = thinking_from(
            line.provider_data
                .as_ref()
                .and_then(|provider| provider.raw_usage.as_ref()),
        );

        parsed.records.push(ParsedRecord {
            timestamp,
            model: intern(&mut models, model_name),
            project: project.map(|name| intern(&mut projects, name)),
            usage,
            thinking,
        });
    }

    parsed.title = ai_title.or(summary);
    Some(parsed)
}

/// 取一个文件的解析产物：命中缓存直接复用，未命中才真正读盘解析。
///
/// 解析前后各取一次指纹并比对：读取期间文件被追加或改写时不入缓存，
/// 免得把半份内容固化成「最新」。
pub(super) fn parsed_file(path: &Path) -> Option<Arc<ParsedFile>> {
    let before = fingerprint(path)?;
    if let Some(cached) = cache_lookup(path, before) {
        return Some(cached);
    }
    let parsed = Arc::new(parse_jsonl_file(path)?);
    if fingerprint(path) == Some(before) {
        cache_store(path, before, &parsed);
    }
    Some(parsed)
}

/// 待扫描的一个文件：根、路径，以及扫描时就取到的 mtime（毫秒）。
///
/// mtime 一次性取好并复用：既用于「复制会话按落地顺序处理」的排序，也用于窗口外
/// 文件的裁剪，避免每个文件多次 `metadata()`。
pub(super) struct Target {
    pub(super) root: PathBuf,
    pub(super) path: PathBuf,
    pub(super) modified_ms: Option<i64>,
}

impl Target {
    pub(super) fn new(root: PathBuf, path: PathBuf) -> Self {
        let modified_ms = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);
        Self {
            root,
            path,
            modified_ms,
        }
    }

    /// 文件是否整份落在时间窗之前（可跳过读盘）。
    ///
    /// mtime 未知时一律读，宁可多扫一个文件也不漏数据。
    pub(super) fn is_outside(&self, cutoff: Option<i64>) -> bool {
        match (cutoff, self.modified_ms) {
            (Some(cutoff), Some(modified)) => modified.saturating_add(MTIME_SLACK_MS) < cutoff,
            _ => false,
        }
    }

    pub(super) fn session_id(&self) -> String {
        self.path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("未知会话")
            .to_string()
    }
}
