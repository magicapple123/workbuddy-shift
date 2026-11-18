use super::parse::scan_text;
use super::parse::scan_text_scoped;
use super::parse::session_from_file_name;
use super::parse::AuthMarker;
use super::parse::Event;
use super::parse::Hit;
use super::parse::LogFormat;
use super::parse::QUOTA_MARKERS;
use crate::modules::config::now_ms;
use chrono::NaiveDate;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 日志根目录名（档位/IDE 数据根下）。
pub(super) const LOG_DIR_NAME: &str = "logs";

/// 扫描窗口：五个来源一致，取文件 mtime 在最近 2 天内的日志（固定，不做可配置）。
///
/// 判据用 mtime 而非目录名/会话名：日志文件当天创建后会写到**次日凌晨**（本机实测
/// `2026-09-15/` 的两个文件写到 9/16 07:59），按「最近 N 个目录」会漏掉跨天那批记录。
///
/// 本机实测（release）五个来源全量扫描约 115 ms：CLI 最重（候选 43MB / 7 文件，约 66 ms，
/// 因为 CLI 的业务日志是完整会话记录）、CN IDE 约 12 ms、WorkBuddy 两档位合计约 7 ms。
/// 仍远快于前端 60s 轮询间隔，因此**不建**增量索引或本地缓存层。
pub(super) const WINDOW_DAYS: usize = 2;

/// 一天的毫秒数（mtime 收窗用）。
pub(super) const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// CodeBuddy CLI 数据根目录名（三平台同构）。
pub(crate) const CLI_DATA_DIR: &str = ".codebuddy";

/// IDE 插件在 `exthost/` 下的日志目录名（两个 IDE 一致）。
///
/// 这是**唯一稳定**的过滤条件：文件名不固定（CN 侧 `腾讯云代码助手.log`，含 `.1.log` 轮转；
/// 国际版侧还出现过 `Tencent Cloud CodeBuddy.log`），不加该过滤会把候选集从 18MB 涨到 43MB。
pub(super) const IDE_LOG_DIR_NAME: &str = "Tencent-Cloud.coding-copilot";

/// 超长行的字段嗅探窗口（字节）。
///
/// IDE 的 `Agent execution failed` 行含完整请求体（本机实测 117 KB，研究报告另有 350 KB
/// 样本）：只对行首/行尾各一个定长窗口做字段嗅探，不对整行做全量正则或 JSON 反序列化。
pub(super) const LONG_LINE_SNIFF_BYTES: usize = 4 * 1024;

/// 同一事件的重置时刻相同时，发生时刻相差 ≤ 该值即视为同一次事件。
///
/// 一次 429 会在业务日志写 5–6 行、SDK 日志再写一份；本机实测 14 行只是 2 次事件，
/// 不做去重的话卡片会把 2 次事件显示成 14 条。
const MERGE_WINDOW_MS: i64 = 15_000;

/// SDK 会话日志的目录形态：`logs/<日期>/sdk/conversations/<会话 UUID>.log`。
pub(super) const SDK_LOG_DIR: &str = "sdk";

pub(super) const SDK_CONVERSATIONS_DIR: &str = "conversations";

/// 字节级粗筛：命中限额文案的文件才解码（整份文件逐行解码是本模块的主要开销）。
///
/// 用 Boyer–Moore–Horspool 的坏字符跳表：一次比较失败即可跳过多个字节，
/// 而不是退回到「每个偏移都比一次」。2 天窗口（47MB / 30 文件）实测把粗筛
/// 从 ~0.9s 降到 ~0.1s（debug 构建），是「扫描 < 1s」的主要保障。
pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some(&last) = needle.last() else {
        return false;
    };
    if haystack.len() < needle.len() {
        return false;
    }
    // 坏字符跳表：needle 中每个字节最后一次出现处到末尾的距离。
    let mut skip = [needle.len(); 256];
    for (index, &byte) in needle[..needle.len() - 1].iter().enumerate() {
        skip[byte as usize] = needle.len() - 1 - index;
    }
    let mut offset = 0;
    while offset + needle.len() <= haystack.len() {
        if haystack[offset + needle.len() - 1] == last
            && &haystack[offset..offset + needle.len()] == needle
        {
            return true;
        }
        offset += skip[haystack[offset + needle.len() - 1] as usize];
    }
    false
}

/// 窗口内的 WorkBuddy 格式日志文件（两档位与 CLI 共用；日期目录下可能还有
/// `sdk/conversations/` 一层）。
///
/// 两层过滤：
/// - 只认日期形态的目录名：`logs/` 下还有 `sdk`、`migration`、`Crash-Log`、`memwatch`
///   等非日期目录，整体忽略；
/// - 目录内**没有任何** mtime 在窗口内的 `.log` 时整目录跳过。
///
/// 判据是文件 mtime 而非目录名：日志文件当天创建后会写到**次日凌晨**（本机实测
/// `2026-09-15/` 的两个文件写到 9/16 07:59），按「最近 N 个目录」会漏掉跨天那批记录。
/// 目录名只用于排除非日志目录，不参与窗口判定。
pub(super) fn windowed_log_files(logs_root: &Path, cutoff_ms: i64) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(logs_root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let is_dated = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| NaiveDate::parse_from_str(name, "%Y-%m-%d").is_ok());
        if !is_dated {
            continue;
        }
        let mut candidates = Vec::new();
        log_files(&path, &mut candidates);
        files.extend(candidates.into_iter().filter(|file| {
            std::fs::metadata(file)
                .ok()
                .and_then(|metadata| modified_ms(&metadata))
                .is_some_and(|modified| modified >= cutoff_ms)
        }));
    }
    files
}

/// 递归收集候选日志文件（日期目录下可能还有 `sdk/conversations/` 一层）。
fn log_files(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            log_files(&path, output);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("log") {
            output.push(path);
        }
    }
}

fn scan_file(path: &Path, format: LogFormat, auth: AuthMarker, hits: &mut Vec<Hit>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if !QUOTA_MARKERS
        .iter()
        .any(|marker| contains(&bytes, marker.as_bytes()))
    {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    match session_from_file_name(path) {
        Some(session) => hits.extend(scan_text_scoped(&text, format, auth, Some(&session))),
        None => hits.extend(scan_text(&text, format, auth)),
    }
}

/// 枚举 → 粗筛 → 解析 → 去重，返回某个来源的限额事件。
pub(super) fn collect_events(root: &Path, format: LogFormat, auth: AuthMarker) -> Vec<Event> {
    let cutoff_ms = now_ms() - WINDOW_DAYS as i64 * DAY_MS;
    let files = match format {
        // 日期目录形态校验 + 文件 mtime 收窗（目录名不参与窗口判定）。
        LogFormat::WorkBuddy => windowed_log_files(root, cutoff_ms),
        // IDE 会话目录下按插件目录过滤 + 文件 mtime 收窗。
        LogFormat::Ide => ide_log_files(root, cutoff_ms),
    };
    let mut hits = Vec::new();
    for file in files {
        scan_file(&file, format, auth, &mut hits);
    }
    dedupe(hits)
}

/// 两个 IDE 的候选日志文件：
/// `<root>/<YYYYMMDDTHHMMSS>/window<N>/exthost/Tencent-Cloud.coding-copilot/` 下的**所有
/// 常规文件**（不限文件名/扩展名），只保留 `mtime ≥ cutoff_ms` 的。
///
/// 两个关键决策（研究报告 §B1）：
/// - 会话目录名是**会话启动时间**，不是内容日期，且旧会话目录仍会被追加写入（本机那次 429
///   就落在第 3 新的会话目录里）。按目录名收窗会漏事件，必须按文件 mtime。
/// - 文件名不写死：CN 侧 `腾讯云代码助手.log`（含 `.1.log` 轮转），国际版侧还出现过
///   `Tencent Cloud CodeBuddy.log`；父目录名才是稳定的过滤条件。
pub(super) fn ide_log_files(root: &Path, cutoff_ms: i64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    // 日志根缺失（该 IDE 未安装/未使用）返回空集，不是错误。
    let Ok(sessions) = std::fs::read_dir(root) else {
        return files;
    };
    for session in sessions.flatten() {
        let session_dir = session.path();
        if !session_dir.is_dir() {
            continue;
        }
        let Ok(windows) = std::fs::read_dir(&session_dir) else {
            continue;
        };
        for window in windows.flatten() {
            let directory = window.path().join("exthost").join(IDE_LOG_DIR_NAME);
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                if modified_ms(&metadata).is_some_and(|modified| modified >= cutoff_ms) {
                    files.push(entry.path());
                }
            }
        }
    }
    files
}

/// 文件最后修改时刻（毫秒）；取不到即视为不在窗口内（宁可漏，不报错）。
fn modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    let since_epoch = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since_epoch.as_millis()).ok()
}

/// 两步去重：① `(事件 id, 重置时刻)` 相同即同一次事件；② 重置时刻相同且发生时刻相差
/// ≤ `MERGE_WINDOW_MS` 的记录再合并一次（业务日志与 SDK 日志各写一份，IDE 侧一次事件写
/// 8 行且只有部分行带事件 id，且两边的 id 未必都取得到）。
pub(super) fn dedupe(hits: Vec<Hit>) -> Vec<Event> {
    let mut groups: BTreeMap<(Option<String>, i64), Event> = BTreeMap::new();
    for hit in hits {
        let key = (hit.session_id.clone(), hit.reset_at);
        let event = Event {
            session_id: hit.session_id,
            model: hit.model,
            reset_at: hit.reset_at,
            first_seen_at: hit.occurred_at,
            hit_count: 1,
            uid: hit.uid,
        };
        match groups.entry(key) {
            Entry::Occupied(mut entry) => entry.get_mut().merge(event),
            Entry::Vacant(entry) => {
                entry.insert(event);
            }
        }
    }

    let mut sorted: Vec<Event> = groups.into_values().collect();
    sorted.sort_by_key(|event| (event.reset_at, event.first_seen_at));
    let mut merged: Vec<Event> = Vec::new();
    for event in sorted {
        let same_event = merged.last().is_some_and(|last| {
            last.reset_at == event.reset_at
                && event.first_seen_at - last.first_seen_at <= MERGE_WINDOW_MS
        });
        if same_event {
            if let Some(last) = merged.last_mut() {
                last.merge(event);
            }
        } else {
            merged.push(event);
        }
    }
    merged
}
