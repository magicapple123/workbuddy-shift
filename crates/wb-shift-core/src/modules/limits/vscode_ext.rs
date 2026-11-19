//! VS Code 插件限额能力探测（第三轮第 20 条，upstream issue #188）。
//!
//! 结论先写在这里，代码只是把结论变成可重跑的探测：
//!
//! VS Code 里的 CodeBuddy 插件**不在**现有五个 `ScanSource` 里，而且实测它的日志
//! 与两个 IDE 的格式不同——`腾讯云代码助手.log` 与
//! `CodeBuddyExtension/Logs/VSCode/<日期>/` 下都搜不到 `超出频率限制` /
//! `usage exceeds frequency limit`、也没有 `resolved model=` / `refusal classified` /
//! `将在 … UTC+8 重置` 这些解析器依赖的句式。也就是说：把日志根加进扫描范围**也不会**
//! 凭空产生限额条目，必须先由插件侧写出可解析的日志（或提供事件）。
//!
//! 因此本轮按「分支二」处理：UI 明示不支持并把需求转上游，同时保留这个探测函数——
//! 一旦上游补上可解析日志，`supported` 会自己变成 `true`，届时再接 `ScanSource`。
//!
//! 探测结果的判定口径与扫描器一致：**只认 `QUOTA_MARKERS` 的字节命中**，不做模糊猜测。

use super::parse::QUOTA_MARKERS;
use super::scan::{contains, DAY_MS, WINDOW_DAYS};
use crate::modules::config::now_ms;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 上游 issue 编号（第三轮第 20 条，reporter 报告「VS Code 插件从未触发限额」）。
///
/// 只存编号，不拼完整 URL：`scripts/check-structure.mjs` 禁止旧产品名出现在代码里，
/// 而派生来源仓库的路径必然含它；要贴完整链接需先在 `AGENTS.md` 第 7 节登记例外
/// （主理人决策，不在本条允许的改动范围内）。UI 文案同样只引用编号。
pub const UPSTREAM_ISSUE_NUMBER: u32 = 188;

/// 单文件粗筛上限：插件日志可能是几十 MB 的完整会话记录，只读头部做字节粗筛。
const MAX_BYTES_PER_FILE: u64 = 2 * 1024 * 1024;

/// 一次探测最多看多少个文件（按 mtime 从新到旧）；探测在设置页打开时触发，
/// 必须有一个可以写进代码的固定上限，而不是「把整棵树读完」。
const MAX_FILES: usize = 6;

/// 目录遍历深度上限：`logs/<会话>/window<N>/exthost/<插件>/<日期>/`。
const MAX_DEPTH: usize = 6;

/// VS Code 与 CodeBuddy 插件可能写限额日志的根目录（不存在就跳过）。
///
/// Windows 两处来自本机实测；macOS / Linux 的路径按各自平台的常规位置推导，
/// **未在真机验证**，探测函数对不存在的根是安全的（直接跳过）。
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

/// 探测 VS Code 插件是否写出了现有解析器能认的限额日志。
///
/// 返回给前端（`rate_limit_hook_status.vscodeExt`）的字段都只在诊断时有意义：
/// `supported=false` 才是当前事实，UI 不支持状态写死在卡片里，不依赖这个探测，
/// 否则「探测失败」会被误读成「支持但没触发」。
pub fn probe() -> Value {
    probe_roots(&log_roots(), now_ms())
}

/// [`probe`] 的可注入版本（单测给它临时目录，不触碰真实日志）。
pub(crate) fn probe_roots(roots: &[PathBuf], now: i64) -> Value {
    let existing: Vec<&PathBuf> = roots.iter().filter(|root| root.is_dir()).collect();
    let mut files: Vec<(i64, PathBuf)> = Vec::new();
    for root in &existing {
        collect_log_files(root, 0, now, &mut files);
    }
    // 新的先看：探测只是为了取证，不追求全量。
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_FILES);

    let mut scanned = 0usize;
    let mut hits = 0usize;
    for (_, path) in &files {
        let Ok(bytes) = read_head(path, MAX_BYTES_PER_FILE) else {
            continue;
        };
        scanned += 1;
        if QUOTA_MARKERS
            .iter()
            .any(|marker| contains(&bytes, marker.as_bytes()))
        {
            hits += 1;
        }
    }

    json!({
        "supported": hits > 0,
        "markerHits": hits,
        "scannedFiles": scanned,
        "roots": existing.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
        "upstreamIssue": UPSTREAM_ISSUE_NUMBER,
        "reason": if hits > 0 {
            "VS Code 插件日志里出现了限额文案，可以按两个 IDE 的方式接入 ScanSource"
        } else {
            "VS Code 插件日志里没有限额文案，现有解析器无法归因；需插件侧写出可解析日志或提供事件"
        },
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
        out.push((mtime_ms, path));
    }
}

/// 只读文件头 `limit` 字节：目标是字节级粗筛，不解析内容。
fn read_head(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.by_ref().take(limit).read_to_end(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("wb-shift-vscode-probe-{label}-{unique}"));
        std::fs::create_dir_all(&path).expect("创建临时目录");
        path
    }

    #[test]
    fn probe_reports_unsupported_without_markers() {
        let root = temp_dir("empty");
        let log = root.join("session").join("window1").join("exthost");
        std::fs::create_dir_all(&log).expect("创建日志目录");
        std::fs::write(
            log.join("腾讯云代码助手.log"),
            "agent started\nno quota here\n",
        )
        .expect("写日志");

        let value = probe_roots(&[root.clone()], now_ms());
        assert_eq!(value["supported"], json!(false));
        assert_eq!(value["markerHits"], json!(0));
        assert_eq!(value["scannedFiles"], json!(1));
        assert_eq!(value["upstreamIssue"], json!(UPSTREAM_ISSUE_NUMBER));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn probe_detects_quota_markers_when_present() {
        let root = temp_dir("hit");
        let log = root.join("20260101").join("window1").join("exthost");
        std::fs::create_dir_all(&log).expect("创建日志目录");
        std::fs::write(
            log.join("腾讯云代码助手.log"),
            "warn: 超出频率限制，将在 2026-01-01 12:00 UTC+8 重置\n",
        )
        .expect("写日志");

        let value = probe_roots(&[root.clone()], now_ms());
        assert_eq!(value["supported"], json!(true));
        assert_eq!(value["markerHits"], json!(1));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn probe_ignores_files_outside_the_window_and_missing_roots() {
        let root = temp_dir("window");
        let log = root.join("old");
        std::fs::create_dir_all(&log).expect("创建日志目录");
        let path = log.join("腾讯云代码助手.log");
        std::fs::write(&path, "超出频率限制\n").expect("写日志");

        // now 往前推 10 天：窗口（2 天）之外的文件不参与探测。
        let value = probe_roots(&[root.clone()], now_ms() + 10 * DAY_MS);
        assert_eq!(value["scannedFiles"], json!(0));
        assert_eq!(value["markerHits"], json!(0));

        // 不存在的根整体跳过，不 panic。
        let missing = root.join("does-not-exist");
        let value = probe_roots(&[missing], now_ms());
        assert_eq!(value["roots"], json!([]));

        std::fs::remove_dir_all(&root).ok();
    }
}
