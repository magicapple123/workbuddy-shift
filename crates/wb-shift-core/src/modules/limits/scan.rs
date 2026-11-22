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
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
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

// --- VS Code 插件宿主 agent 日志（#118 / #188）--------------------------------

/// VS Code 用户数据目录名（官方版与其 Insiders 变体，按此顺序探测）。
const VSCODE_USER_DIRS: [&str; 2] = ["Code", "Code - Insiders"];

/// 扩展数据根下的日志段名（`…/CodeBuddyExtension/Logs/<客户端>/`）。
const EXTENSION_LOG_DIR_NAME: &str = "Logs";

/// 扩展日志段下的客户端目录名（VS Code 插件）。
const EXTENSION_CLIENT_DIR_NAME: &str = "VSCode";

/// 插件宿主日志的粗筛窗口（字节）：可能是几十 MB 的完整会话记录，只嗅探首尾。
const AGENT_SNIFF_BYTES: u64 = 256 * 1024;

/// 插件宿主日志目录的递归深度上限（`exthost/<插件>/<日期>/…`）。
const AGENT_MAX_DEPTH: usize = 10;

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
        // 插件宿主日志：`logs/<会话>/window<N>/exthost/**` 或扩展目录树，按文件 mtime 收窗。
        LogFormat::Agent => agent_log_files(root, cutoff_ms),
    };
    scan_files(files, format, auth)
}

/// 多根版本：插件宿主的候选根**可能同时存在多个**（VS Code 用户日志树 + 扩展目录日志），
/// 逐个枚举后按文件路径去重再统一解析（同一份日志被两条路径指到时只算一次）。
pub(super) fn collect_events_in(
    roots: &[PathBuf],
    format: LogFormat,
    auth: AuthMarker,
) -> Vec<Event> {
    let cutoff_ms = now_ms() - WINDOW_DAYS as i64 * DAY_MS;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let found = match format {
            LogFormat::WorkBuddy => windowed_log_files(root, cutoff_ms),
            LogFormat::Ide => ide_log_files(root, cutoff_ms),
            LogFormat::Agent => agent_log_files(root, cutoff_ms),
        };
        for file in found {
            if seen.insert(file.clone()) {
                files.push(file);
            }
        }
    }
    scan_files(files, format, auth)
}

/// 候选文件 → 逐文件粗筛/解析 → 去重。
fn scan_files(files: Vec<PathBuf>, format: LogFormat, auth: AuthMarker) -> Vec<Event> {
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

// ---------------------------------------------------------------------------
// VS Code 插件宿主 agent 日志（#118 / #188）
// ---------------------------------------------------------------------------

/// 插件宿主 agent 日志根（探测不到 → 空集合）。
///
/// **所有存在的候选根都要扫**（union），不是「挑第一个存在的」：VS Code 用户日志树与扩展
/// 目录日志会同时存在且内容互不覆盖 —— 前者是宿主进程日志（只有它带 `exthost/` 分段），
/// 后者是扩展自己写的日志（不经 VS Code 用户数据目录）。只挑第一个时，只要
/// `Code/logs` 存在就永远读不到扩展目录那一份，扩展侧的事件会整体消失。
/// 口径与 [`super::vscode_ext::log_roots`] 一致（同样按「全部候选」收集）。
///
/// 探测不到**不是错误**：本机不一定装了 VS Code，装了也不一定装了 CodeBuddy 插件。
/// 这种情况下该来源贡献 0 条事件，整轮扫描照常完成（不能因此判定失败，也不能 panic）。
pub(crate) fn agent_log_roots() -> Vec<PathBuf> {
    let home = crate::modules::config::home_dir();
    agent_log_roots_from(&agent_log_candidates(
        &home,
        dirs::data_dir().as_deref(),
        dirs::data_local_dir().as_deref(),
    ))
}

/// 候选日志根（按优先级），**纯函数**：家目录与平台数据根都由调用方注入，便于用临时目录测试。
///
/// 规则（优先级从高到低）：
/// 1. `<VS Code 用户数据>/logs` —— Windows `%APPDATA%/Code{,- Insiders}/logs`、
///    macOS `~/Library/Application Support/Code{,- Insiders}/logs`、
///    Linux `~/.config/Code{,- Insiders}/logs`（官方版先于 Insiders）；
/// 2. `<扩展数据根>/CodeBuddyExtension/Logs/VSCode` —— 退路（扩展自己写的日志）。
pub(crate) fn agent_log_candidates(
    home: &Path,
    roaming: Option<&Path>,
    local_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Some(roaming) = roaming {
            for dir in VSCODE_USER_DIRS {
                candidates.push(roaming.join(dir).join(LOG_DIR_NAME));
            }
        }
        if let Some(local_data) = local_data {
            candidates.push(extension_logs_root(local_data));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let support = home.join("Library").join("Application Support");
        for dir in VSCODE_USER_DIRS {
            candidates.push(support.join(dir).join(LOG_DIR_NAME));
        }
        candidates.push(extension_logs_root(&support));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let config = home.join(".config");
        for dir in VSCODE_USER_DIRS {
            candidates.push(config.join(dir).join(LOG_DIR_NAME));
        }
        candidates.push(extension_logs_root(&home.join(".local").join("share")));
    }
    // 部分平台的注入参数没有用上（例如 Windows 不用 `home`）：显式引用一次，
    // 避免 `-D warnings` 报未使用变量，也不引入 `#[allow]`。
    let _ = (home, roaming, local_data);
    candidates
}

/// 扩展数据根下的插件宿主日志根（`<data_root>/CodeBuddyExtension/Logs/VSCode`）。
fn extension_logs_root(data_root: &Path) -> PathBuf {
    data_root
        .join("CodeBuddyExtension")
        .join(EXTENSION_LOG_DIR_NAME)
        .join(EXTENSION_CLIENT_DIR_NAME)
}

/// 候选里**所有真实存在**的日志根（按候选顺序，去重）；一个都不存在 → 空集合
/// （该来源贡献 0 条）。
///
/// 不做「取第一个」的短路：见 [`agent_log_roots`] 的解释。
pub(crate) fn agent_log_roots_from(candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in candidates {
        if path.is_dir() && !roots.contains(path) {
            roots.push(path.clone());
        }
    }
    roots
}

/// 插件宿主 agent 日志的候选文件。
///
/// 两种布局都兼容（同一份日志在不同安装形态下的落点不同）：
/// - VS Code 用户日志树：`<root>/<会话>/window<N>/exthost/<插件>/**`（宿主进程的日志夹在这
///   棵树下，插件自己的子目录再深一层也能收到）；
/// - 扩展目录树：`<root>/**`（没有 `exthost` 分段）。
///
/// 都只收窗口内（按文件 mtime）的常规文件，并先做一次「限额文案」首尾嗅探：
/// `exthost/` 下还有**其它扩展**的日志，不能让它们整份进内存。
///
/// `exthost/` 下**先按插件目录过滤**再递归（[`is_codebuddy_extension_dir`]）：
/// `ms-python.python`、`github.copilot-chat` 之类的日志与限额无关，只按文件 mtime 收窗会把
/// 它们全收进候选 —— 既拖慢扫描，也让「限额文案嗅探」在大文件上白做一遍。
pub(crate) fn agent_log_files(root: &Path, cutoff_ms: i64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if has_exthost_tree(root) {
        for session in subdirs(root) {
            for window in subdirs(&session) {
                for plugin in subdirs(&window.join("exthost")) {
                    if !is_codebuddy_extension_dir(&plugin) {
                        continue;
                    }
                    walk_agent_logs(&plugin, 0, cutoff_ms, &mut files);
                }
            }
        }
    } else {
        walk_agent_logs(root, 0, cutoff_ms, &mut files);
    }
    files
}

/// 是否 CodeBuddy 扩展自己的日志目录。
///
/// 判定复用 `vscode_ext` 的扩展 marketplace id（大小写不敏感**包含**），不写死目录名清单：
/// marketplace id 在不同版本里大小写与后缀都变过（`Tencent-Cloud.coding-copilot`、
/// `coding-copilot-*`），前缀或全等匹配都会把本插件判成「别的扩展」。
fn is_codebuddy_extension_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.to_ascii_lowercase()
                .contains(crate::modules::vscode_ext::EXTENSION_ID)
        })
}

/// 目录下所有子目录（读不到就是空集，不是错误）。
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

/// 是否是 VS Code 用户日志树：存在 `<会话>/window<N>/exthost` 这一层。
fn has_exthost_tree(root: &Path) -> bool {
    subdirs(root).iter().any(|session| {
        subdirs(session).iter().any(|window| {
            let name = window
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name.starts_with("window") && window.join("exthost").is_dir()
        })
    })
}

/// 递归收集窗口内的常规文件（限额文案嗅探通过才进候选）。
fn walk_agent_logs(dir: &Path, depth: usize, cutoff_ms: i64, out: &mut Vec<PathBuf>) {
    if depth > AGENT_MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_agent_logs(&path, depth + 1, cutoff_ms, out);
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if !modified_ms(&metadata).is_some_and(|modified| modified >= cutoff_ms) {
            continue;
        }
        if sniff_quota_markers(&path, metadata.len()) {
            out.push(path);
        }
    }
}

/// 首尾窗口嗅探限额文案：整份读 `exthost` 下所有扩展的日志是不可接受的（几十 MB）。
///
/// 首窗够了（日志是追加写、限额行落在开头附近时才可能命中），尾窗兜住「文件很大且
/// 事件刚写在末尾」这类常见形态；两处都没有就跳过 —— 与 `scan_file` 的粗筛口径一致。
fn sniff_quota_markers(path: &Path, size: u64) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let head_limit = size.min(AGENT_SNIFF_BYTES);
    let mut head = Vec::new();
    if file
        .by_ref()
        .take(head_limit)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    if has_quota_marker(&head) {
        return true;
    }
    if size <= AGENT_SNIFF_BYTES {
        return false;
    }
    if file
        .seek(SeekFrom::Start(size - AGENT_SNIFF_BYTES))
        .is_err()
    {
        return false;
    }
    let mut tail = Vec::new();
    if file
        .by_ref()
        .take(AGENT_SNIFF_BYTES)
        .read_to_end(&mut tail)
        .is_err()
    {
        return false;
    }
    has_quota_marker(&tail)
}

/// 字节级粗筛：是否含任一限额文案（与 `scan_file` 同一套标记）。
fn has_quota_marker(bytes: &[u8]) -> bool {
    QUOTA_MARKERS
        .iter()
        .any(|marker| contains(bytes, marker.as_bytes()))
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
