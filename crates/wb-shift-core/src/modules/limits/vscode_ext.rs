//! VS Code 插件限额日志探针（第三轮 #188 → 第四轮 13-A 重做）。
//!
//! 本模块**只做诊断采样**，不产出任何限额 chip。台账侧的插件来源由
//! `ScanSource::AgentLog` + `LogFormat::Agent` 承担（`ledger::cache::scan_agent_logs`）：
//! 扫的是插件宿主 agent 日志（`[2026/9/27 13:27:37.109]` 那种行），与本模块的
//! 「强信号采样」是两条独立路径——本模块保留下来是为了在真实环境里取证：
//! 上游到底往日志里写了什么、格式是否又变了。
//!
//! 采样规则（第四轮 13-A）：
//! - **增量读**：每个文件记录 `offset + size + mtime`，下次只读新增字节；文件变小/被替换
//!   才从头再来；
//! - **首次最多尾读 256 KB**：插件日志可能是几十 MB 的完整会话记录，不能整份读；
//! - **只认强信号**：`Status: 429` / `[HTTP STATUS] 429` / `RESOURCE_EXHAUSTED` /
//!   `usage exceeds frequency limit` / `超出频率限制` 五类，避免把普通报错当限额；
//! - **样本落盘**：`~/.workbuddy-shift/vscode_limit_samples.jsonl`，每行
//!   `{at, file, line}`，邮件/长十六进制 id/Bearer token 先脱敏。
//!
//! 这个模块**不判断**「VS Code 被限流了」：`supported` 只表示「日志里出现了值得解析的
//! 强信号」，账号卡不会据此显示任何 chip。

use super::scan::{contains, DAY_MS, WINDOW_DAYS};
use crate::modules::config::{atomic_write, now_ms, store_dir};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 上游 issue 编号（reporter 报告「VS Code 插件从未触发限额」）。
///
/// 只存编号，不拼完整 URL：`scripts/check-structure.mjs` 禁止旧产品名出现在代码里。
pub const UPSTREAM_ISSUE_NUMBER: u32 = 188;

/// 首次读一个文件时最多回看的字节数（尾读）。
const FIRST_READ_BYTES: u64 = 256 * 1024;

/// 单次扫描单文件最多读的字节数：防止长时间未打开应用后一次读几十 MB。
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// 一次探测最多看多少个文件（按 mtime 从新到旧）。
const MAX_FILES: usize = 32;

/// 目录遍历深度上限：`logs/<会话>/window<N>/exthost/<插件>/<日期>/`。
const MAX_DEPTH: usize = 6;

/// 状态文件（偏移 + 累计命中 + 最近样本）。
const STATE_FILE: &str = "vscode_limit_probe.json";

/// 样本文件（追加写）。
const SAMPLES_FILE: &str = "vscode_limit_samples.jsonl";

/// 一次扫描最多追加多少条样本（诊断够用，避免刷爆文件）。
const MAX_SAMPLES_PER_RUN: usize = 5;

/// 强信号：只有「429 + 限额上下文」才算，避免把无关 429 当成限流证据。
///
/// 本机实测 VS Code 日志里确实有 7 条 `Status: 429`，但全部是
/// `Unable to resolve optional EDU details. Status: 429`（EDU 资料接口的普通限频），
/// 与模型额度无关。若只看状态码，探针会误报「VS Code 有 7 次限流」。
fn is_strong_signal(line: &str) -> bool {
    if line.contains("RESOURCE_EXHAUSTED")
        || line.contains("usage exceeds frequency limit")
        || line.contains("超出频率限制")
    {
        return true;
    }
    let is_429 = line.contains("Status: 429") || line.contains("[HTTP STATUS] 429");
    if !is_429 {
        return false;
    }
    [
        "frequency",
        "exceed",
        "quota",
        "limit",
        "超出",
        "限流",
        "额度",
    ]
    .iter()
    .any(|marker| line.to_ascii_lowercase().contains(marker))
}

/// 脱敏：邮件、32 位以上十六进制 id、Bearer token。
fn redact(line: &str) -> String {
    let mut text = line.to_string();
    // 邮件
    while let Some(at) = text.find('@') {
        let start = text[..at]
            .rfind(|character: char| {
                character.is_whitespace() || character == '<' || character == '"'
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = text[at..]
            .find(|character: char| {
                character.is_whitespace()
                    || character == '"'
                    || character == '>'
                    || character == ','
            })
            .map(|offset| at + offset)
            .unwrap_or(text.len());
        if start >= at {
            break;
        }
        text.replace_range(start..end, "<email>");
        if !text.contains('@') {
            break;
        }
    }
    // 长十六进制 id（requestId / sessionId）
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_hexdigit() {
            let mut end = index;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                end += 1;
            }
            if end - index >= 32 {
                output.push_str("<id>");
            } else {
                output.push_str(&text[index..end]);
            }
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or('\0');
        output.push(character);
        index += character.len_utf8();
    }
    // Bearer token
    if let Some(position) = output.find("Bearer ") {
        let end = output[position + 7..]
            .find(|character: char| character.is_whitespace() || character == '"')
            .map(|offset| position + 7 + offset)
            .unwrap_or(output.len());
        output.replace_range(position + 7..end, "<token>");
    }
    output.chars().take(500).collect()
}

#[derive(Clone, Default)]
struct FileState {
    offset: u64,
    size: u64,
    mtime_ms: i64,
}

fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE)
}

fn samples_path() -> PathBuf {
    store_dir().join(SAMPLES_FILE)
}

fn load_state(path: &Path) -> (BTreeMap<String, FileState>, u64, i64, Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (BTreeMap::new(), 0, 0, Vec::new());
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return (BTreeMap::new(), 0, 0, Vec::new());
    };
    let mut files = BTreeMap::new();
    if let Some(object) = value.get("files").and_then(Value::as_object) {
        for (key, item) in object {
            files.insert(
                key.clone(),
                FileState {
                    offset: item.get("offset").and_then(Value::as_u64).unwrap_or(0),
                    size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                    mtime_ms: item.get("mtimeMs").and_then(Value::as_i64).unwrap_or(0),
                },
            );
        }
    }
    let hits = value.get("hits").and_then(Value::as_u64).unwrap_or(0);
    let scanned_at = value.get("scannedAt").and_then(Value::as_i64).unwrap_or(0);
    let samples = value
        .get("samples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (files, hits, scanned_at, samples)
}

/// 写状态文件（诊断面板不依赖它成功，写失败只影响下次是否重读）。
fn save_state(
    path: &Path,
    files: &BTreeMap<String, FileState>,
    hits: u64,
    at: i64,
    samples: &[String],
) {
    let mut map = Map::new();
    for (key, state) in files {
        map.insert(
            key.clone(),
            json!({ "offset": state.offset, "size": state.size, "mtimeMs": state.mtime_ms }),
        );
    }
    let body = json!({
        "files": map,
        "hits": hits,
        "scannedAt": at,
        "samples": samples.iter().take(10).cloned().collect::<Vec<String>>(),
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = atomic_write(path, &serde_json::to_string(&body).unwrap_or_default());
}

/// VS Code 与 CodeBuddy 插件可能写限额日志的根目录（不存在就跳过）。
pub fn log_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            roots.push(PathBuf::from(appdata).join("Code").join("logs"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(
                PathBuf::from(local)
                    .join("CodeBuddyExtension")
                    .join("Logs")
                    .join("VSCode"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        let support = crate::modules::config::home_dir()
            .join("Library")
            .join("Application Support");
        roots.push(support.join("Code").join("logs"));
        roots.push(
            support
                .join("CodeBuddyExtension")
                .join("Logs")
                .join("VSCode"),
        );
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = crate::modules::config::home_dir();
        roots.push(home.join(".config").join("Code").join("logs"));
        roots.push(
            home.join(".local")
                .join("share")
                .join("CodeBuddyExtension")
                .join("Logs")
                .join("VSCode"),
        );
    }
    roots
}

/// 探测 VS Code 插件日志里的限额强信号（增量采样，不产出任何 chip）。
pub fn probe() -> Value {
    probe_at(&log_roots(), &state_path(), &samples_path(), now_ms())
}

/// 可注入路径版本（单测用临时目录，不触碰真实日志与用户状态）。
pub(crate) fn probe_at(
    roots: &[PathBuf],
    state_file: &Path,
    samples_file: &Path,
    now: i64,
) -> Value {
    let (mut files, previous_hits, _previous_scan, mut samples) = load_state(state_file);
    let existing: Vec<PathBuf> = roots.iter().filter(|root| root.is_dir()).cloned().collect();
    let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
    for root in &existing {
        collect_log_files(root, 0, now, &mut candidates);
    }
    candidates.sort_by_key(|item| std::cmp::Reverse(item.0));
    candidates.truncate(MAX_FILES);

    let mut scanned = 0usize;
    let mut new_hits = 0u64;
    let mut appended = 0usize;
    let mut new_samples: Vec<String> = Vec::new();

    for (mtime_ms, path) in &candidates {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        let size = metadata.len();
        let key = path.to_string_lossy().to_string();
        let previous = files.get(&key).cloned().unwrap_or_default();
        // 文件变小 / mtime 回退 → 被截断或替换，从头按尾读窗口重来。
        let start = if previous.size > 0 && size >= previous.size && *mtime_ms >= previous.mtime_ms
        {
            previous.offset.min(size)
        } else {
            size.saturating_sub(FIRST_READ_BYTES)
        };
        files.insert(
            key.clone(),
            FileState {
                offset: size,
                size,
                mtime_ms: *mtime_ms,
            },
        );
        if start >= size {
            continue;
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            continue;
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            continue;
        }
        let limit = (size - start).min(MAX_READ_BYTES);
        let mut buffer = Vec::new();
        if file.take(limit).read_to_end(&mut buffer).is_err() {
            continue;
        }
        scanned += 1;
        let text = String::from_utf8_lossy(&buffer);
        for line in text.lines() {
            if !is_strong_signal(line) {
                continue;
            }
            new_hits += 1;
            if new_samples.len() < MAX_SAMPLES_PER_RUN {
                new_samples.push(redact(line));
            }
        }
    }

    // 状态文件只留最近 10 条样本（诊断够用）。
    let mut all_samples = new_samples.clone();
    all_samples.append(&mut samples);
    all_samples.truncate(10);
    if !new_samples.is_empty() {
        if let Some(parent) = samples_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut handle) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(samples_file)
        {
            for line in &new_samples {
                let record = json!({
                    "at": now,
                    "file": "VS Code 插件日志",
                    "line": line,
                });
                let _ = writeln!(handle, "{record}");
                appended += 1;
            }
        }
    }
    let hits = previous_hits + new_hits;
    save_state(state_file, &files, hits, now, &all_samples);

    json!({
        "supported": hits > 0,
        "hits": hits,
        "newHits": new_hits,
        "files": files.len(),
        "scannedFiles": scanned,
        "lastScanAt": now,
        "samples": all_samples.iter().take(3).cloned().collect::<Vec<String>>(),
        "samplesPath": samples_file.to_string_lossy(),
        "appended": appended,
        "roots": existing.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
        "upstreamIssue": UPSTREAM_ISSUE_NUMBER,
        "reason": if hits > 0 {
            "日志里出现了 429 / RESOURCE_EXHAUSTED 等强信号，样本已落盘；仍需确认这些行能否解析出重置时刻"
        } else {
            "日志里没有限额强信号（插件宿主 agent 日志的台账扫描在 ledger::cache::scan_agent_logs，与本探针无关）"
        },
        // 只采样不判定：这个字段恒为 true，避免前端把「采样到强信号」误读成「已支持」。
        "samplesOnly": true,
    })
}

/// 收集窗口内的 `.log` 文件（按 mtime 收窗，与扫描器同一口径：2 天）。
fn collect_log_files(dir: &Path, depth: usize, now: i64, out: &mut Vec<(i64, PathBuf)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_log_files(&path, depth + 1, now, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(elapsed) = modified.duration_since(UNIX_EPOCH) else {
            continue;
        };
        let mtime_ms = elapsed.as_millis() as i64;
        if now - mtime_ms > WINDOW_DAYS as i64 * DAY_MS {
            continue;
        }
        // 粗筛：没有限额文案的文件不进候选（与扫描器同口径，避免打开设置页就扫全盘）。
        let head_limit = FIRST_READ_BYTES.min(meta.len());
        let Ok(bytes) = read_head(&path, head_limit) else {
            continue;
        };
        if !["429", "RESOURCE_EXHAUSTED", "超出频率限制"]
            .iter()
            .any(|marker| contains(&bytes, marker.as_bytes()))
        {
            continue;
        }
        out.push((mtime_ms, path));
    }
}

fn read_head(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    let mut limited = file.take(limit);
    limited.read_to_end(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("wb-shift-vscode-probe-{label}-{unique}"));
        std::fs::create_dir_all(&path).expect("创建临时目录");
        path
    }

    #[test]
    fn probe_is_incremental_and_only_reads_new_bytes() {
        let root = temp_dir("incremental");
        let log = root.join("session").join("window1").join("exthost");
        std::fs::create_dir_all(&log).expect("创建日志目录");
        let file = log.join("腾讯云代码助手.log");
        std::fs::write(&file, "agent started\nno quota here\n").expect("写日志");
        let state = root.join("state.json");
        let samples = root.join("samples.jsonl");

        let first = probe_at(std::slice::from_ref(&root), &state, &samples, now_ms());
        assert_eq!(first["hits"], 0);
        assert_eq!(first["scannedFiles"], 0, "无强信号的文件不进入扫描");

        // 追加一行强信号：第二次必须读到，且只读新增部分。
        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(handle, "[HTTP STATUS] 429 usage exceeds frequency limit").unwrap();
        drop(handle);

        let second = probe_at(std::slice::from_ref(&root), &state, &samples, now_ms());
        assert_eq!(second["newHits"], 1);
        assert_eq!(second["hits"], 1);
        assert_eq!(second["supported"], true);
        assert_eq!(second["scannedFiles"], 1);

        // 没有新字节：第三次不再命中。
        let third = probe_at(std::slice::from_ref(&root), &state, &samples, now_ms());
        assert_eq!(third["newHits"], 0);
        assert_eq!(third["hits"], 1);
        assert_eq!(third["scannedFiles"], 0);

        let text = std::fs::read_to_string(&samples).expect("样本文件");
        assert!(text.contains("429"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unrelated_429_is_not_a_quota_signal() {
        // 本机真实样本：EDU 资料接口的 429，不是模型限流。
        assert!(!is_strong_signal(
            "2026-11-19 18:29:15.800 [info] Unable to resolve optional EDU details. Status: 429 "
        ));
        assert!(is_strong_signal(
            "429 Too Many Requests: usage exceeds frequency limit"
        ));
        assert!(is_strong_signal("超出频率限制，将在 18:42 重置"));
    }

    #[test]
    fn probe_redacts_email_ids_and_tokens() {
        let line = r#"user a@b.com requestId 0123456789abcdef0123456789abcdef Authorization Bearer sk-secret-123"#;
        let redacted = redact(line);
        assert!(!redacted.contains("a@b.com"));
        assert!(!redacted.contains("0123456789abcdef0123456789abcdef"));
        assert!(!redacted.contains("sk-secret-123"));
        assert!(redacted.contains("<email>"));
        assert!(redacted.contains("<id>"));
        assert!(redacted.contains("<token>"));
    }

    #[test]
    fn probe_ignores_missing_roots_and_old_files() {
        let root = temp_dir("old");
        let log = root.join("old");
        std::fs::create_dir_all(&log).expect("创建日志目录");
        std::fs::write(log.join("x.log"), "[HTTP STATUS] 429\n").expect("写日志");
        let state = root.join("state.json");
        let samples = root.join("samples.jsonl");
        // now 往后推 10 天：2 天窗口之外的文件不参与。
        let value = probe_at(
            std::slice::from_ref(&root),
            &state,
            &samples,
            now_ms() + 10 * DAY_MS,
        );
        assert_eq!(value["scannedFiles"], 0);
        assert_eq!(value["hits"], 0);

        let missing = root.join("does-not-exist");
        let value = probe_at(&[missing], &state, &samples, now_ms());
        assert_eq!(value["roots"], json!([]));
        std::fs::remove_dir_all(&root).ok();
    }
}
