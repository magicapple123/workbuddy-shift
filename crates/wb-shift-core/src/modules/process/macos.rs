#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use super::run_cmd_timeout;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::modules::config;
use crate::modules::variant::WbVariant;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// macOS：进程枚举 / 关闭 / 启动 / app 路径动态探测
//
// 本地不变量（对齐 Windows 进程契约的自我约束，注释只描述本地规则）：
// - 一律用 `ps -axo pid=,args=` 全量输出，在 Rust 内做**大小写敏感**子串匹配；
//   不直接裸用 pgrep/pkill 字符串（pgrep -f 是大小写不敏感子串匹配，会把命令行
//   里恰好引用路径的无关进程一并命中，实测不可靠）。
// - 排除自身 pid 与 args 含本工具产物名（`workbuddy-shift` /
//   `workbuddy-shift-desktop`）的 PID（自排除）。
// - 「主进程（GUI）」= argv 含 `<app>/Contents/MacOS/`，仅用于 footer「运行中」
//   语义与启动成功校验；「包内任意进程」= argv 含 `<app>`（含 Contents/MacOS 与
//   Contents/Resources 下的守护子进程），用于 close 的最终清杀与 launch 前的保险
//   清杀（释放目标应用持有的 single-instance launcher 位）。
// - 包内清杀只按 PID 枚举后 `kill -9 <pid>...` 批量，不按名称子串匹配。
// ---------------------------------------------------------------------------

/// 优雅退出用的目标 bundle id（按档位；实测正确 id，曾用/错误 id 不是它）。
#[cfg(target_os = "macos")]
fn macos_quit_bundle_id(variant: WbVariant) -> &'static str {
    variant.macos_bundle_id()
}

/// 解析单行 `ps -axo pid=,args=` 输出为 (pid, args)。
/// ps 输出 pid 列无表头、可能带前导空格，args 保留原始大小写。
#[cfg(target_os = "macos")]
pub(crate) fn parse_ps_row(line: &str) -> Option<(u32, String)> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace())?;
    let (pid_s, rest) = line.split_at(split);
    let pid = pid_s.trim().parse::<u32>().ok()?;
    let args = rest.trim();
    if args.is_empty() {
        return None;
    }
    Some((pid, args.to_string()))
}

/// args 是否命中任一大小写敏感子串模式。
#[cfg(target_os = "macos")]
fn ps_row_matches_any(args: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| args.contains(p.as_str()))
}

/// 从 `ps -axo pid=,args=` 全量输出中收集命中模式的 (pid, args)。
/// 过滤：排除自身 pid；排除 args 含本工具产物名（`workbuddy-shift` /
/// `workbuddy-shift-desktop`）的 PID。
/// 结果按 pid 去重。残留误杀面仅剩「用户进程的 args 主动引用目标 .app 路径」
/// 这一刻意场景（对齐 Windows 契约记录的残余风险）。
#[cfg(target_os = "macos")]
pub(crate) fn filter_ps_rows(
    stdout: &str,
    patterns: &[String],
    self_pid: u32,
) -> Vec<(u32, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Some((pid, args)) = parse_ps_row(line) else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        if args.contains("workbuddy-shift") || args.contains("workbuddy-shift-desktop") {
            continue;
        }
        if !ps_row_matches_any(&args, patterns) {
            continue;
        }
        if seen.insert(pid) {
            out.push((pid, args));
        }
    }
    out
}

/// 主进程层匹配模式：解析到 app 路径时用 `` `<app>/Contents/MacOS` ``；
/// 探测失败回退本档位字面量。纯函数。
#[cfg(target_os = "macos")]
pub(super) fn macos_main_patterns(resolved_app: Option<&Path>, variant: WbVariant) -> Vec<String> {
    match resolved_app {
        Some(app) => vec![format!("{}/Contents/MacOS", app.display())],
        None => variant
            .macos_app_names()
            .iter()
            .map(|name| format!("{name}/Contents/MacOS"))
            .collect(),
    }
}

/// 包内层匹配模式：解析到 app 路径时用其完整路径；探测失败回退字面量
/// （本档位主名 + 全小写变体，防用户把 .app 目录改名）。纯函数。
#[cfg(target_os = "macos")]
fn macos_bundle_patterns(resolved_app: Option<&Path>, variant: WbVariant) -> Vec<String> {
    match resolved_app {
        Some(app) => vec![app.display().to_string()],
        None => variant
            .macos_app_names()
            .iter()
            .flat_map(|name| [(*name).to_string(), name.to_lowercase()])
            .collect(),
    }
}

#[cfg(target_os = "macos")]
fn ps_all_rows() -> String {
    match run_cmd_timeout("ps", &["-axo", "pid=,args="], 5) {
        Some(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        None => String::new(),
    }
}

/// 按模式枚举命中进程（含自排除），去重后返回 (pid, args)。
#[cfg(target_os = "macos")]
pub(crate) fn macos_rows_by_patterns(patterns: &[String]) -> Vec<(u32, String)> {
    filter_ps_rows(&ps_all_rows(), patterns, std::process::id())
}

/// 按模式枚举命中 PID（含自排除）。
#[cfg(target_os = "macos")]
pub(crate) fn macos_pids_by_patterns(patterns: &[String]) -> Vec<u32> {
    macos_rows_by_patterns(patterns)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect()
}

/// 从主进程 argv 提取 `.app` bundle 路径。
/// argv 形如 `<dir>/<Name>.app/Contents/MacOS/<binary>`，取首个 `.app/Contents/MacOS`
/// 出现位置、截到 `.app` 结尾。shell 调用层保持薄，不做复杂解析。
#[cfg(target_os = "macos")]
pub(crate) fn extract_app_bundle_from_args(args: &str) -> Option<PathBuf> {
    let idx = args.find(".app/Contents/MacOS")?;
    let path = &args[..idx + 4]; // 含 `.app`
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// macOS app bundle 谓词：目录存在且含 Contents/Info.plist。
#[cfg(target_os = "macos")]
pub(crate) fn is_app_bundle(path: &Path) -> bool {
    path.is_dir() && path.join("Contents").join("Info.plist").is_file()
}

/// 常见安装位置候选：/Applications、~/Applications × 本档位 app 名。
/// 纯函数（不访问文件系统），供探测与单测复用。
#[cfg(target_os = "macos")]
fn app_bundle_candidates(home: &Path, variant: WbVariant) -> Vec<PathBuf> {
    let home_apps = home.join("Applications");
    let bases = [Path::new("/Applications"), home_apps.as_path()];
    let mut out = Vec::new();
    for base in bases {
        for name in variant.macos_app_names() {
            let p = base.join(name);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// 命中即写缓存；已是同一路径则不重复写（mac 判 app bundle 目录）。
#[cfg(target_os = "macos")]
fn persist_macos_app_cache(path: &Path, variant: WbVariant) {
    if !is_app_bundle(path) {
        return;
    }
    if config::load_workbuddy_exe_cache(variant).as_deref() == Some(path) {
        return;
    }
    let _ = config::save_workbuddy_exe_cache(variant, path);
}

/// 运行中主进程路径探测：扫主进程层字面量命中行，提取 `.app` 并校验为 bundle。
#[cfg(target_os = "macos")]
fn macos_running_app_path(variant: WbVariant) -> Option<PathBuf> {
    let patterns = macos_main_patterns(None, variant);
    for (_pid, args) in macos_rows_by_patterns(&patterns) {
        if let Some(p) = extract_app_bundle_from_args(&args) {
            if is_app_bundle(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// mdfind 按 bundle id 探测应用路径（Spotlight 不可用/未命中则静默跳过）。
#[cfg(target_os = "macos")]
fn macos_mdfind_app_path(variant: WbVariant) -> Option<PathBuf> {
    let query = format!(
        "kMDItemCFBundleIdentifier == '{}'c",
        macos_quit_bundle_id(variant)
    );
    let out = run_cmd_timeout("mdfind", &[query.as_str()], 5)?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

/// macOS app 路径动态探测（对齐 Windows 契约风格）。
///
/// 顺序：运行中主进程 → 缓存（mac 判 app bundle 目录）→ 常见位置
/// （/Applications、~/Applications × 本档位 app 名）→ mdfind bundle id。
/// 命中即写缓存；缓存指向已不存在的 bundle 则丢弃并继续。
/// 全部失败返回 None，由调用方决定默认路径 / 字面量回退。
#[cfg(target_os = "macos")]
pub(super) fn macos_workbuddy_app_path_resolved(variant: WbVariant) -> Option<PathBuf> {
    if let Some(p) = macos_running_app_path(variant) {
        persist_macos_app_cache(&p, variant);
        return Some(p);
    }
    if let Some(cached) = config::load_workbuddy_exe_cache(variant) {
        if is_app_bundle(&cached) {
            return Some(cached);
        }
        config::clear_workbuddy_exe_cache(variant);
    }
    let home = config::home_dir();
    for p in app_bundle_candidates(&home, variant) {
        if is_app_bundle(&p) {
            persist_macos_app_cache(&p, variant);
            return Some(p);
        }
    }
    if let Some(p) = macos_mdfind_app_path(variant) {
        if is_app_bundle(&p) {
            persist_macos_app_cache(&p, variant);
            return Some(p);
        }
    }
    None
}

/// macOS：解析 WorkBuddy app 路径；全部失败回退本档位默认 app
/// （供启动失败文案与包模式回退使用，与改造前的默认值等价）。
#[cfg(target_os = "macos")]
pub fn macos_workbuddy_app_path(variant: WbVariant) -> PathBuf {
    macos_workbuddy_app_path_resolved(variant).unwrap_or_else(|| variant.macos_default_app_path())
}

/// `kill -9` 按 PID 批量强杀；不按字符串匹配。失败仅打日志。
#[cfg(target_os = "macos")]
pub(crate) fn kill_macos_pids(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    let owned: Vec<String> = std::iter::once("-9".to_string())
        .chain(pids.iter().map(|pid| pid.to_string()))
        .collect();
    let args: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    match run_cmd_timeout("kill", &args, 10) {
        Some(out) if !out.status.success() => {
            eprintln!(
                "[process] kill -9 failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        None => eprintln!("[process] kill -9 timed out"),
        _ => {}
    }
}

/// 轮询等待「按模式命中」的进程集合为空；超时返回仍存活的 pid。
#[cfg(target_os = "macos")]
pub(crate) fn wait_macos_patterns_empty(patterns: &[String], timeout: Duration) -> Vec<u32> {
    if patterns.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        let alive = macos_pids_by_patterns(patterns);
        if alive.is_empty() || Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 轮询等待「主进程层」消失；超时返回是否已消失。语义同 `wait_process_gone`，
/// 但使用显式主进程模式，避免每轮重新解析 app 路径。
#[cfg(target_os = "macos")]
pub(crate) fn wait_macos_main_gone(main_patterns: &[String], timeout: Duration) -> bool {
    wait_macos_patterns_empty(main_patterns, timeout).is_empty()
}

/// 关闭 WorkBuddy（macOS）：
/// 1) 用正确 bundle id 发 osascript 优雅退出（失败不阻塞）；
/// 2) 给主进程一段优雅窗口（≤8s）消失；
/// 3) 清杀「包内任意进程」残留（含 Contents/Resources 下守护子进程），释放
///    single-instance launcher 位；
/// 4) 轮询包内集合为空；仍残留则报错（含手动 kill 提示）。
///
/// 注意：**不**以「主进程不在」做早退 —— 仅守护子进程存活的场景必须执行清杀。
#[cfg(target_os = "macos")]
pub(super) fn close_workbuddy_macos(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let resolved = macos_workbuddy_app_path_resolved(variant);
    let main_patterns = macos_main_patterns(resolved.as_deref(), variant);
    let bundle_patterns = macos_bundle_patterns(resolved.as_deref(), variant);
    let remaining = || {
        timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100))
    };

    // 1) 优雅退出
    let quit_script = format!("quit app id \"{}\"", macos_quit_bundle_id(variant));
    let quit = run_cmd_timeout("osascript", &["-e", quit_script.as_str()], 10);
    match quit {
        Some(out) if !out.status.success() => {
            eprintln!(
                "[close] osascript quit failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        None => eprintln!("[close] osascript quit timed out"),
        _ => {}
    }

    // 2) 优雅窗口等待主进程消失
    let graceful = Duration::from_secs(8).min(remaining());
    let main_graceful = wait_macos_main_gone(&main_patterns, graceful);
    if !main_graceful {
        eprintln!("[close] graceful quit not effective, forcing bundle kill…");
    }

    // 3) 清杀包内残留（主进程 + 守护子进程）
    let bundle_pids = macos_pids_by_patterns(&bundle_patterns);
    if !bundle_pids.is_empty() {
        eprintln!("[close] killing {} bundle process(es)…", bundle_pids.len());
        kill_macos_pids(&bundle_pids);
    }

    // 4) 等包内集合为空（剩余预算）
    let leftover = wait_macos_patterns_empty(&bundle_patterns, remaining());
    if leftover.is_empty() {
        return Ok(());
    }
    let pids: Vec<String> = leftover.iter().map(|pid| pid.to_string()).collect();
    Err(format!(
        "WorkBuddy 进程无法完全关闭（残留进程: {}）。请手动执行: kill -9 {}",
        pids.join(", "),
        pids.join(" ")
    ))
}

/// 启动存活校验：500ms 轮询，总上限 ~30s。
/// - 主进程从未出现 → 超时 Err；
/// - 出现后持续存活 ≥10s → Ok；
/// - 出现过但随后消失（目标应用单例锁秒退特征 ~2s）→ 立即 Err。
///
/// progress 可选：出现/确认阶段推送心跳，避免"正在启动…"长时间静默被误判卡死。
#[cfg(target_os = "macos")]
fn validate_macos_startup(
    app: &Path,
    variant: WbVariant,
    progress: Option<&dyn Fn(&str)>,
) -> Result<(), String> {
    let say = |msg: &str| {
        if let Some(p) = progress {
            p(msg);
        }
    };
    let main_patterns = macos_main_patterns(Some(app), variant);
    let deadline = Instant::now() + Duration::from_secs(30);
    let sustain = Duration::from_secs(10);
    let beat_every = Duration::from_secs(2);
    let mut seen_at: Option<Instant> = None;
    let mut last_beat: Option<Instant> = None;
    while Instant::now() < deadline {
        let now = Instant::now();
        let alive = !macos_pids_by_patterns(&main_patterns).is_empty();
        if alive {
            match seen_at {
                None => {
                    seen_at = Some(now);
                    say("WorkBuddy 窗口已出现，正在确认稳定运行…");
                }
                Some(start) => {
                    if now.duration_since(start) >= sustain {
                        say("WorkBuddy 已确认运行。");
                        return Ok(());
                    }
                    // 心跳：出现后每 ~2s 推一次（首拍从 ~2s 起，不推 0s），
                    // 进度平滑又不至于每 500ms 刷屏
                    let since_seen = now.duration_since(start);
                    let due = since_seen >= beat_every
                        && last_beat
                            .map(|b| now.duration_since(b) >= beat_every)
                            .unwrap_or(true);
                    if due {
                        last_beat = Some(now);
                        say(&format!("已稳定运行 {}s / 10s…", since_seen.as_secs()));
                    }
                }
            }
        } else if seen_at.is_some() {
            return Err(
                "WorkBuddy 启动后立即退出，疑似残留单例锁。请手动打开一次 WorkBuddy 后再试。"
                    .to_string(),
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "WorkBuddy 启动超时，未能确认运行（路径: {}）。请手动打开排查。",
        app.display()
    ))
}

/// 启动 WorkBuddy（macOS）：路径存在性检查 → 保险清杀包内残留 → open -n -a
/// → 轮询确认主进程出现且持续存活（覆盖单例锁导致秒退的场景）。
///
/// progress 可选：清杀/启动/存活确认阶段推送心跳（见 `validate_macos_startup`）。
#[cfg(target_os = "macos")]
pub(super) fn launch_workbuddy_macos(
    variant: WbVariant,
    progress: Option<&dyn Fn(&str)>,
) -> Result<(), String> {
    let say = |msg: &str| {
        if let Some(p) = progress {
            p(msg);
        }
    };
    let app = macos_workbuddy_app_path(variant);
    if !is_app_bundle(&app) {
        return Err(format!(
            "未找到 WorkBuddy 应用（尝试路径: {}）。请先手动打开一次 WorkBuddy 后重试。",
            app.display()
        ));
    }

    // 保险清杀：包内残留（如持锁守护）非空则 kill -9 全部并等到空（5s）。
    let bundle_patterns = macos_bundle_patterns(Some(&app), variant);
    let bundle_pids = macos_pids_by_patterns(&bundle_patterns);
    if !bundle_pids.is_empty() {
        say("正在清理残留进程…");
        kill_macos_pids(&bundle_pids);
        let _ = wait_macos_patterns_empty(&bundle_patterns, Duration::from_secs(5));
    }

    // open -n -a <app>：同步执行并检查退出码
    say("正在启动 WorkBuddy…");
    let app_lossy = app.to_string_lossy();
    let open = run_cmd_timeout("open", &["-n", "-a", app_lossy.as_ref()], 10);
    match open {
        Some(out) if out.status.success() => {}
        Some(out) => {
            let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let reason = if reason.is_empty() {
                format!("open 退出码 {}", out.status.code().unwrap_or(-1))
            } else {
                reason
            };
            return Err(format!(
                "启动 WorkBuddy 失败: {reason}（路径: {}）",
                app.display()
            ));
        }
        None => {
            return Err(format!(
                "启动 WorkBuddy 失败: open 超时（路径: {}）",
                app.display()
            ));
        }
    }

    validate_macos_startup(&app, variant, progress)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_parse_ps_row_extracts_pid_and_args() {
        assert_eq!(
            parse_ps_row("  1234 /Applications/WorkBuddy.app/Contents/MacOS/WorkBuddy --foo"),
            Some((
                1234,
                "/Applications/WorkBuddy.app/Contents/MacOS/WorkBuddy --foo".to_string()
            ))
        );
        assert_eq!(
            parse_ps_row("4321  /usr/bin/swiftc"),
            Some((4321, "/usr/bin/swiftc".to_string()))
        );
        assert_eq!(parse_ps_row(""), None);
        assert_eq!(parse_ps_row("   "), None);
        assert_eq!(parse_ps_row("1235"), None); // 无 args
        assert_eq!(parse_ps_row("not-a-pid /x"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_pattern_fallback_selection() {
        // 主进程层：解析到路径 → 只用该路径的 Contents/MacOS；失败 → 字面量变体
        assert_eq!(
            macos_main_patterns(None, WbVariant::Cn),
            vec!["WorkBuddy.app/Contents/MacOS".to_string()]
        );
        assert_eq!(
            macos_main_patterns(None, WbVariant::Ai),
            vec!["WorkBuddy AI.app/Contents/MacOS".to_string()]
        );
        assert_eq!(
            macos_main_patterns(
                Some(Path::new("/Applications/WorkBuddy.app")),
                WbVariant::Cn
            ),
            vec!["/Applications/WorkBuddy.app/Contents/MacOS".to_string()]
        );

        // 包内层：解析到路径 → 只用完整路径；失败 → 字面量 + 全小写变体
        assert_eq!(
            macos_bundle_patterns(None, WbVariant::Cn),
            vec!["WorkBuddy.app".to_string(), "workbuddy.app".to_string()]
        );
        assert_eq!(
            macos_bundle_patterns(None, WbVariant::Ai),
            vec![
                "WorkBuddy AI.app".to_string(),
                "workbuddy ai.app".to_string()
            ]
        );
        assert_eq!(
            macos_bundle_patterns(
                Some(Path::new("/Applications/WorkBuddy.app")),
                WbVariant::Cn
            ),
            vec!["/Applications/WorkBuddy.app".to_string()]
        );
    }

    /// 两档位模式串必须互斥：跨档位命中会误杀另一版客户端。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_patterns_never_cross_match() {
        let cn_patterns = macos_main_patterns(None, WbVariant::Cn);
        let cn_bundle = macos_bundle_patterns(None, WbVariant::Cn);
        let ai_patterns = macos_main_patterns(None, WbVariant::Ai);
        let ai_bundle = macos_bundle_patterns(None, WbVariant::Ai);

        let cn_args = "/Applications/WorkBuddy.app/Contents/MacOS/WorkBuddy --foo";
        let ai_args = "/Applications/WorkBuddy AI.app/Contents/MacOS/WorkBuddy AI --foo";
        assert!(cn_patterns.iter().any(|p| cn_args.contains(p.as_str())));
        assert!(cn_bundle.iter().any(|p| cn_args.contains(p.as_str())));
        assert!(
            !ai_patterns.iter().any(|p| cn_args.contains(p.as_str())),
            "国内版进程不得命中国际版主进程模式"
        );
        assert!(
            !ai_bundle.iter().any(|p| cn_args.contains(p.as_str())),
            "国内版进程不得命中国际版包内模式"
        );

        assert!(ai_patterns.iter().any(|p| ai_args.contains(p.as_str())));
        assert!(ai_bundle.iter().any(|p| ai_args.contains(p.as_str())));
        assert!(
            !cn_patterns.iter().any(|p| ai_args.contains(p.as_str())),
            "国际版进程不得命中国内版主进程模式"
        );
        assert!(
            !cn_bundle.iter().any(|p| ai_args.contains(p.as_str())),
            "国际版进程不得命中国内版包内模式"
        );

        let ide_args = "/Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy";
        assert!(
            !cn_patterns.iter().any(|p| ide_args.contains(p.as_str())),
            "国际 CodeBuddy IDE 不得命中国内版 WorkBuddy 主进程模式"
        );
        assert!(
            !cn_bundle.iter().any(|p| ide_args.contains(p.as_str())),
            "国际 CodeBuddy IDE 不得命中国内版 WorkBuddy 包内模式"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ps_filter_matches_case_sensitively_and_excludes_self() {
        let self_pid = std::process::id();
        // 5002 用小写目录：pgrep -f 大小写不敏感会命中，这里必须不命中。
        // 5003 全大写引用：旧的 pgrep -f 不敏感匹配的典型反例，这里必须不命中。
        // 5005 args 含 workbuddy-shift：非自身 pid 也按自排除规则剔除。
        let stdout = format!(
            "{self_pid} /Applications/workbuddy-shift.app/Contents/MacOS/workbuddy-shift\n\
             5001 /Applications/WorkBuddy.app/Contents/MacOS/WorkBuddy --foo\n\
             5002 /Applications/workbuddy.app/Contents/MacOS/WorkBuddy\n\
             5003 /bin/zsh -c 'echo WORKBUDDY.APP/CONTENTS/MACOS mention'\n\
             5004 /Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy\n\
             5005 /opt/tool/workbuddy-shift/helper run\n"
        );
        let patterns = macos_main_patterns(None, WbVariant::Cn);
        let kept = filter_ps_rows(&stdout, &patterns, self_pid);
        let pids: Vec<u32> = kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(pids, vec![5001]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_bundle_filter_keeps_bundle_daemons_and_drops_self() {
        let self_pid = std::process::id();
        // 包内任意进程：主进程 + Contents/Resources 下守护 + 引用 bundle 的自家进程。
        // 5012 用小写目录名（对应字面量全小写变体）。5014 是无关 .app，不误杀。
        let stdout = format!(
            "{self_pid} /Applications/workbuddy-shift.app/Contents/MacOS/workbuddy-shift\n\
             5011 /Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/vendor/sandbox/5.5.5/sandbox-center --config x\n\
             5012 /Applications/workbuddy.app/Contents/Resources/.../sandbox-center\n\
             5013 node ~/.workbuddy/binaries/cliguard-daemon --app_bundle /Applications/WorkBuddy.app\n\
             5014 /Applications/SomeOther.app/Contents/MacOS/SomeOther\n"
        );
        let patterns = macos_bundle_patterns(None, WbVariant::Cn);
        let kept = filter_ps_rows(&stdout, &patterns, self_pid);
        let pids: Vec<u32> = kept.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(pids, vec![5011, 5012, 5013]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_extract_app_bundle_from_args() {
        assert_eq!(
            extract_app_bundle_from_args(
                "/Applications/WorkBuddy.app/Contents/MacOS/WorkBuddy --foo"
            ),
            Some(PathBuf::from("/Applications/WorkBuddy.app"))
        );
        assert_eq!(
            extract_app_bundle_from_args("/Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy"),
            Some(PathBuf::from("/Applications/CodeBuddy.app"))
        );
        assert_eq!(extract_app_bundle_from_args("/usr/bin/ssh host"), None);
        assert_eq!(extract_app_bundle_from_args(""), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_app_bundle_candidates_order() {
        let cands = app_bundle_candidates(Path::new("/Users/tester"), WbVariant::Cn);
        let s: Vec<String> = cands
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            s,
            vec![
                "/Applications/WorkBuddy.app".to_string(),
                "/Users/tester/Applications/WorkBuddy.app".to_string(),
            ]
        );

        let ai = app_bundle_candidates(Path::new("/Users/tester"), WbVariant::Ai);
        let s: Vec<String> = ai
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            s,
            vec![
                "/Applications/WorkBuddy AI.app".to_string(),
                "/Users/tester/Applications/WorkBuddy AI.app".to_string(),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_is_app_bundle_requires_dir_with_info_plist() {
        let dir = std::env::temp_dir().join(format!(
            "workbuddy-shift-app-bundle-test-{}",
            uuid::Uuid::new_v4()
        ));
        let app = dir.join("WorkBuddy.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents").join("Info.plist"), "<plist/>").unwrap();
        assert!(is_app_bundle(&app));

        let contents = app.join("Contents");
        assert!(!is_app_bundle(&contents)); // Contents 不是 bundle 根

        let plain = dir.join("plain-dir");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!is_app_bundle(&plain)); // 无 Info.plist

        assert!(!is_app_bundle(&dir.join("missing.app"))); // 不存在
        std::fs::remove_dir_all(dir).unwrap();
    }
}
