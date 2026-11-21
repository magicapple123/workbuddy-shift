//! 发版后验收用的「真·应用内升级」触发链路（**仅 CI 使用**）。
//!
//! `release-smoke.yml` 的 Windows 格要验的是「0.2.4 的用户点更新 → 下载 → 签名校验 → 安装 →
//! 0.2.5」这条真实链路，而不是「再装一遍再覆盖」。桌面壳没有这个入口，因此新增一个**隐藏、
//! 双重闸门**的启动参数 `--smoke-self-update`：闸门满足后本模块在后台依次走 `check` →
//! `start_download` → 轮询到 `ReadyToRestart` → `restart`，把每一步写进 JSON Lines 供 CI 断言。
//!
//! **默认零影响**：[`maybe_spawn`] 的第一条语句就是闸门判定，不满足即 `return`；其后的任何一行
//! （读 env、解析日志路径、spawn 任务、磁盘 / 网络 / 窗口操作）都不会执行。三条闸门：① 存在
//! **整参相等**的 `--smoke-self-update`（禁止子串匹配，与 `tray::is_silent_startup` 同口径）；
//! ② `WB_SHIFT_SMOKE_SELF_UPDATE == "1"`；③ 非截图演示模式。
//!
//! **不进用户文档**：桌面端没有 `--help`；本参数不写进 `README.md` / `docs/cli.md` / `FAQ.md`，
//! 只在 `docs/ARCHITECTURE.md` 的内部小节登记。不注册到 autostart 参数、不读写
//! `github_config.json` 的开关、不落「已升级」标记——唯一的落盘是追加式日志文件。
//!
//! **退出码**（CI **不**用它判通过：Windows 上 NSIS 会让进程以不确定码退出，退出码只用于失败
//! 归类）：见下方 `EXIT_*` 常量。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};

use wb_shift_core::modules::config::store_dir;
use wb_shift_core::modules::update::APP_VERSION;

use super::{check, restart, snapshot, start_download, UpdatePhase};

/// 触发参数。**整参相等**匹配，不做子串匹配。
pub const SELF_UPDATE_ARG: &str = "--smoke-self-update";
/// 二次确认环境变量（值为 `"1"` 才生效）。
pub const SMOKE_ENV: &str = "WB_SHIFT_SMOKE_SELF_UPDATE";
/// 日志路径覆盖：CI 指定绝对路径；缺省落在 `<store_dir>/smoke-self-update.jsonl`。
pub const LOG_ENV: &str = "WB_SHIFT_SMOKE_LOG";
/// 日志文件缺省名（相对 `store_dir`）。
pub const LOG_FILE_NAME: &str = "smoke-self-update.jsonl";

// 退出码：0 走完（Windows 常被安装器接管）/ 10 已是最新（CI 判 H）/ 11 检查失败（A 或 G/J）/
// 12 下载或签名失败（G 或 A）/ 13 安装失败（E）/ 14 环境不可用（日志不可写）/ 20 平台不支持。
pub const EXIT_OK: i32 = 0;
pub const EXIT_UP_TO_DATE: i32 = 10;
pub const EXIT_CHECK_FAILED: i32 = 11;
pub const EXIT_DOWNLOAD_FAILED: i32 = 12;
pub const EXIT_INSTALL_FAILED: i32 = 13;
pub const EXIT_UNAVAILABLE: i32 = 14;
pub const EXIT_UNSUPPORTED_PLATFORM: i32 = 20;

// `phase` 固定词表（**新增必须同步** `docs/ARCHITECTURE.md` 与 `scripts/smoke/assert-update-log.py`）。
pub const PHASE_START: &str = "start";
pub const PHASE_CHECK: &str = "check";
pub const PHASE_CHECK_OK: &str = "check_ok";
pub const PHASE_UP_TO_DATE: &str = "up_to_date";
pub const PHASE_DOWNLOAD_START: &str = "download_start";
pub const PHASE_DOWNLOAD_PROGRESS: &str = "download_progress";
pub const PHASE_DOWNLOAD_DONE: &str = "download_done";
pub const PHASE_INSTALL_START: &str = "install_start";
pub const PHASE_INSTALL_OK: &str = "install_ok";
pub const PHASE_RELAUNCH: &str = "relaunch";
pub const PHASE_DONE: &str = "done";
pub const PHASE_ERROR: &str = "error";
pub const PHASE_ABORT: &str = "abort";

/// 词表全集（13 个，顺序即预期流程顺序；`error` / `abort` 只在异常路径出现）。
pub const PHASES: [&str; 13] = [
    PHASE_START,
    PHASE_CHECK,
    PHASE_CHECK_OK,
    PHASE_UP_TO_DATE,
    PHASE_DOWNLOAD_START,
    PHASE_DOWNLOAD_PROGRESS,
    PHASE_DOWNLOAD_DONE,
    PHASE_INSTALL_START,
    PHASE_INSTALL_OK,
    PHASE_RELAUNCH,
    PHASE_DONE,
    PHASE_ERROR,
    PHASE_ABORT,
];

/// 下载完成判定：轮询快照间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 下载阶段总预算（安装包约 100 MB，CDN 通常 <90s；10 分钟是「肯定失败」的兜底）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `download_progress` 写日志间隔（不让 JSONL 变成逐 chunk 刷屏）。
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// 三条闸门的实测结果。拆成字段而不是一个 `bool`，便于单测逐条断言，也便于排障说清「哪条没满足」。
/// `arg` = 存在整参相等的 `--smoke-self-update`；`env` = env 值为 `"1"`；
/// `demo` = 截图演示模式（为 `true` 时**否决**：演示态禁止真实网络/安装）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub arg: bool,
    pub env: bool,
    pub demo: bool,
}

impl Gate {
    /// 三条同时满足才放行。
    pub fn is_open(self) -> bool {
        self.arg && self.env && !self.demo
    }
}

#[cfg(test)]
impl Gate {
    /// 未满足的闸门名（**仅单测使用**：生产路径刻意不写任何日志 / 文件，保持「默认启动零行为
    /// 变化」，否则「只带参数不带 env」也会多一行 stderr）。
    pub fn blocked_by(self) -> Vec<&'static str> {
        let mut blocked = Vec::new();
        if !self.arg {
            blocked.push("arg");
        }
        if !self.env {
            blocked.push("env");
        }
        if self.demo {
            blocked.push("demo");
        }
        blocked
    }
}

/// 参数列表里是否存在**整参相等**的触发参数。
///
/// 不用 `contains`：`--smoke-self-update-x` / `--no-smoke-self-update` 都会被字串匹配命中，
/// 把普通启动带进真升级流程（设计 R8）。
pub fn has_exact_arg<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == SELF_UPDATE_ARG)
}

/// 计算三条闸门（纯函数，不碰宿主，便于单测）。
pub fn gate(args: &[String], env_value: Option<&str>, demo: bool) -> Gate {
    Gate {
        arg: has_exact_arg(args.iter().map(String::as_str)),
        env: env_value == Some("1"),
        demo,
    }
}

/// 读环境变量（`var` 失败 = 未设置）。
fn env_smoke_flag() -> Option<String> {
    std::env::var(SMOKE_ENV).ok()
}

/// 一行日志。字段名与顺序即契约（`scripts/smoke/assert-update-log.py` 按此断言）。
/// `ts` 是 ISO 8601 UTC（秒精度）；`seq` 从 0 单调递增；`version` 无 `v` 前缀；
/// `detail` 是 `{bytes, percent, installerExit, logPath}` 的子集；`pid` 跨重启区分进程。
#[derive(Debug, Serialize)]
struct SmokeRecord {
    ts: String,
    seq: u64,
    phase: &'static str,
    version: Option<String>,
    #[serde(rename = "currentVersion")]
    current_version: String,
    url: Option<String>,
    error: Option<String>,
    detail: Option<Value>,
    pid: u32,
}

/// 追加式 JSONL 写手：每行 `flush`，保证进程被安装器杀掉时已写的不丢。
struct SmokeLog {
    file: File,
    seq: u64,
    current_version: &'static str,
    pid: u32,
}

impl SmokeLog {
    /// 打开（不存在则创建、存在则追加）。
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建日志目录失败：{error}"))?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("打开日志失败：{error}"))?;
        Ok(Self {
            file,
            seq: 0,
            current_version: APP_VERSION,
            pid: std::process::id(),
        })
    }

    /// 写一行。写失败只打到 stderr——此时没有更可靠的通道，但不 panic。
    fn write(
        &mut self,
        phase: &'static str,
        version: Option<&str>,
        url: Option<&str>,
        error: Option<&str>,
        detail: Option<Value>,
    ) {
        // 词表闸门：`PHASES` 是 CI 断言的唯一依据。注意 `debug_assert!` 在 release 构建下
        // **不生效**，而 CI 跑的是 release 产物：线上这层闸门等于不存在，真正兜住的是常量
        // （`PHASES`）与单测（`smoke/tests.rs` 断言词表不含未知值）。新增/改名 phase 必须
        // 同步 `docs/ARCHITECTURE.md` 与 `scripts/smoke/assert-update-log.py`。
        debug_assert!(PHASES.contains(&phase), "未登记的 phase: {phase}");
        let record = SmokeRecord {
            ts: iso8601_utc(now_epoch_secs()),
            seq: self.seq,
            phase,
            version: version.map(str::to_string),
            current_version: self.current_version.to_string(),
            url: url.map(str::to_string),
            error: error.map(str::to_string),
            detail,
            pid: self.pid,
        };
        self.seq = self.seq.saturating_add(1);
        match serde_json::to_string(&record) {
            Ok(line) => {
                if let Err(error) = writeln!(self.file, "{line}").and_then(|()| self.file.flush()) {
                    eprintln!("[smoke] 写日志失败：{error}");
                }
            }
            Err(error) => eprintln!("[smoke] 序列化日志失败：{error}"),
        }
    }

    /// 写一条「只有关键三要素」的记录（无 detail、无 error），减少调用处的噪声。
    fn mark(&mut self, phase: &'static str, version: Option<&str>, url: Option<&str>) {
        self.write(phase, version, url, None, None);
    }

    /// 写一条失败记录：`phase` 恒为 [`PHASE_ERROR`]，`error` 必填。
    fn fail_at(&mut self, version: Option<&str>, url: Option<&str>, error: &str) {
        self.write(PHASE_ERROR, version, url, Some(error), None);
    }
}

/// 日志落点：`WB_SHIFT_SMOKE_LOG` 优先，否则 `<store_dir>/smoke-self-update.jsonl`。
fn log_path() -> PathBuf {
    match std::env::var_os(LOG_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => store_dir().join(LOG_FILE_NAME),
    }
}

/// 日志里是否已有终态记录（`install_ok` / `done`）。serde_json 默认紧凑输出，仍兼容带空格的写法。
///
/// NSIS 装完可能自动拉起新版本进程，而 `restart()` 在 macOS / Linux 会把
/// `--smoke-self-update` 与环境变量一起转给新进程（`relaunch_app_inner` 只剔 `--hidden`）。
/// 新进程若写同一个日志会再跑一遍并把 `start` 插进去，污染 CI 的序列断言，因此先看终态。
fn already_finished(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.contains("\"phase\":\"install_ok\"")
        || text.contains("\"phase\":\"done\"")
        || text.contains("\"phase\": \"install_ok\"")
        || text.contains("\"phase\": \"done\"")
}

/// 当前 epoch 秒。
fn now_epoch_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        // 系统时钟早于 1970（不可能，但不能 panic）：按 0 处理。
        Err(_) => 0,
    }
}

/// epoch 秒 → `YYYY-MM-DDTHH:MM:SSZ`。刻意不引 `chrono`（桌面 crate 未启用）：日期换算用
/// Howard Hinnant 的 `civil_from_days`，二十来行足够。
fn iso8601_utc(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let seconds_of_day = epoch_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// 自 1970-01-01 起的天数 → `(年, 月, 日)`（公历，含闰年规则）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 平台白名单：只有三端桌面有「应用内升级后的重启」语义。
fn platform_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

/// 若三条闸门都满足，则后台跑一遍真实自更新流程；否则**立即返回，零副作用**。
/// 调用点：`lib.rs` 的 `setup()` 里，紧随 `tray::setup_startup_visibility` 之后（必须在事件
/// 循环启动前 spawn，且不能打断正常启动时序）。
pub fn maybe_spawn<R: Runtime>(app: &AppHandle<R>) {
    // 第一条**判定**即闸门（此前仅一次 argv 收集）：不满足直接返回，其后零副作用（设计 §3.3）。
    let args: Vec<String> = std::env::args().collect();
    let decision = gate(
        &args,
        env_smoke_flag().as_deref(),
        crate::is_screenshot_demo(),
    );
    if !decision.is_open() {
        return;
    }

    let path = log_path();
    if already_finished(&path) {
        eprintln!("[smoke] 日志已有终态记录，跳过本轮（重启后的新进程不重复验收）");
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let code = run(&app, &path).await;
        eprintln!("[smoke] 自更新流程结束，退出码 {code}");
        // CI 专用：让进程以约定码退出，driver 据此做失败归类（不作通过判定）。
        std::process::exit(code);
    });
}

/// 完整流程。返回退出码（见模块文档）。
async fn run<R: Runtime>(app: &AppHandle<R>, path: &Path) -> i32 {
    let mut log = match SmokeLog::open(path) {
        Ok(log) => log,
        Err(error) => {
            // 闸门满足但环境不可用：连日志都写不了，只能走 stderr + 退出码。
            eprintln!("[smoke] {error}（path={}）", path.display());
            return EXIT_UNAVAILABLE;
        }
    };

    log.write(
        PHASE_START,
        None,
        None,
        None,
        Some(json!({ "logPath": path.display().to_string() })),
    );

    if !platform_supported() {
        log.write(
            PHASE_ABORT,
            None,
            None,
            Some("当前平台不支持应用内自更新验收"),
            None,
        );
        return EXIT_UNSUPPORTED_PLATFORM;
    }

    log.mark(PHASE_CHECK, None, None);
    let result = check(app, None, true).await;
    let release_url = result
        .get("releaseUrl")
        .and_then(Value::as_str)
        .map(str::to_string);

    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        let message = result
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| result.get("error").and_then(Value::as_str))
            .unwrap_or("检查更新失败（无错误文案）")
            .to_string();
        log.fail_at(None, release_url.as_deref(), &message);
        return EXIT_CHECK_FAILED;
    }

    let latest = result
        .get("latest")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let has_update = result.get("hasUpdate").and_then(Value::as_bool) == Some(true);

    if !has_update {
        // 0.2.4 认为自己是 latest = 版本号体系出了问题，CI 会判 H。
        log.mark(PHASE_UP_TO_DATE, latest.as_deref(), release_url.as_deref());
        return EXIT_UP_TO_DATE;
    }

    let Some(target) = latest.as_deref() else {
        log.fail_at(None, release_url.as_deref(), "有更新但没有目标版本号");
        return EXIT_CHECK_FAILED;
    };
    log.mark(PHASE_CHECK_OK, Some(target), release_url.as_deref());
    log.mark(PHASE_DOWNLOAD_START, Some(target), release_url.as_deref());
    if let Err(error) = start_download(app, None) {
        log.fail_at(Some(target), release_url.as_deref(), &error);
        return EXIT_DOWNLOAD_FAILED;
    }

    if let Err((code, message)) = wait_for_ready(&mut log, target, release_url.as_deref()).await {
        log.fail_at(Some(target), release_url.as_deref(), &message);
        return code;
    }
    log.write(
        PHASE_DOWNLOAD_DONE,
        Some(target),
        release_url.as_deref(),
        None,
        Some(json!({ "percent": 100 })),
    );

    log.mark(PHASE_INSTALL_START, Some(target), release_url.as_deref());
    match restart(app).await {
        // macOS / Linux 到这里意味着「已拉起新版本、本进程即将退出」；Windows 侧
        // `Update::install` 内部已 `exit`，正常走不到这里。
        Ok(()) => {
            for phase in [PHASE_INSTALL_OK, PHASE_RELAUNCH, PHASE_DONE] {
                log.mark(phase, Some(target), release_url.as_deref());
            }
            EXIT_OK
        }
        Err(error) => {
            log.fail_at(Some(target), release_url.as_deref(), &error);
            EXIT_INSTALL_FAILED
        }
    }
}

/// 轮询快照直到 `ReadyToRestart`；期间每 5 秒写一条 `download_progress`。
/// 失败返回 `(退出码, 原因)`：`Error` 阶段 → 12；超时 → 12（CI 归 A 或 E）。
async fn wait_for_ready(
    log: &mut SmokeLog,
    target: &str,
    url: Option<&str>,
) -> Result<(), (i32, String)> {
    let started = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        let current = snapshot();
        match current.phase {
            UpdatePhase::ReadyToRestart => return Ok(()),
            UpdatePhase::Error => {
                let message = current
                    .message
                    .unwrap_or_else(|| "下载阶段进入 error 但无错误文案".to_string());
                return Err((EXIT_DOWNLOAD_FAILED, message));
            }
            _ => {}
        }

        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            log.write(
                PHASE_DOWNLOAD_PROGRESS,
                Some(target),
                url,
                None,
                Some(json!({ "percent": current.percent })),
            );
        }

        if started.elapsed() >= DOWNLOAD_TIMEOUT {
            return Err((
                EXIT_DOWNLOAD_FAILED,
                format!(
                    "下载未在 {} 秒内完成（phase={:?}）",
                    DOWNLOAD_TIMEOUT.as_secs(),
                    current.phase
                ),
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests;
