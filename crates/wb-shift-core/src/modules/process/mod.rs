//! 进程控制：检测、关闭、启动 WorkBuddy。
//!
//! 对照 server.py `is_workbuddy_running` / `_wait_process_gone` /
//! `close_workbuddy` / `launch_workbuddy`。

// Windows 映像名/路径辅助在非 Windows 上只供单测使用。
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
// macOS 的 app 路径解析收敛在本模块内（`macos_workbuddy_app_path_*`），不再回引 auth_file。
#[cfg(target_os = "macos")]
use self::macos::close_workbuddy_macos;
#[cfg(target_os = "macos")]
use self::macos::launch_workbuddy_macos;
#[cfg(target_os = "macos")]
use self::macos::macos_main_patterns;
#[cfg(target_os = "macos")]
use self::macos::macos_workbuddy_app_path_resolved;
#[cfg(target_os = "windows")]
use self::windows::close_workbuddy_windows;
#[cfg(target_os = "windows")]
use self::windows::persist_workbuddy_exe;
#[cfg(target_os = "windows")]
use self::windows::windows_exe_names;
#[cfg(target_os = "windows")]
use self::windows::windows_workbuddy_process_rows;
#[cfg(not(target_os = "macos"))]
use crate::modules::auth_file;
use crate::modules::variant::WbVariant;
use std::io::Read;

#[cfg(target_os = "macos")]
mod macos;
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use self::macos::extract_app_bundle_from_args;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::filter_ps_rows;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::is_app_bundle;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::kill_macos_pids;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::macos_pids_by_patterns;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::macos_rows_by_patterns;
#[cfg(target_os = "macos")]
pub use self::macos::macos_workbuddy_app_path;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::wait_macos_main_gone;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::wait_macos_patterns_empty;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::existing_windows_drives;
pub(crate) use self::windows::parse_windows_display_icon;
pub(crate) use self::windows::parse_windows_process_rows;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::ps_output;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::wait_windows_pids_gone;
#[cfg(target_os = "windows")]
pub use self::windows::windows_default_app_path;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::windows_tasklist_image_rows;
#[cfg(target_os = "windows")]
pub use self::windows::windows_workbuddy_exe_path;
pub(crate) use self::windows::WindowsProcessRow;

/// 创建子进程命令。Windows 上加 CREATE_NO_WINDOW，避免每次执行 tasklist/powershell
/// 等控制台命令时闪出 cmd 黑窗口（GUI 应用卡顿/跳动的主因）。
pub(crate) fn cmd_builder(program: impl AsRef<std::ffi::OsStr>) -> Command {
    // 只有 Windows cfg 分支会调用 creation_flags，非 Windows 下这个 mut 没有使用者。
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// 运行命令并等待退出，超时则 kill 并返回 None（对应 Python `subprocess.run(timeout=...)`）。
///
/// 注意：stdout/stderr 必须与等待并发读取——先等退出再读会在输出超过
/// 64KB 管道缓冲时死锁（`ps -axo` 全量输出在进程多的机器上很容易超过），
/// 子进程写满阻塞永不退出，最终被超时 kill 并返回 None。
pub(crate) fn run_cmd_timeout(program: &str, args: &[&str], timeout_secs: u64) -> Option<Output> {
    let mut child = cmd_builder(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;

    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut status = None;
    while status.is_none() {
        match child.try_wait() {
            Ok(Some(s)) => status = Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 管道随进程退出关闭，读线程随即结束。
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    Some(Output {
        status: status.unwrap(),
        stdout: out,
        stderr: err,
    })
}

fn image_stem(name: &str) -> &str {
    let name = name.trim();
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        name[..name.len() - 4].trim()
    } else {
        name
    }
}

/// Windows 路径可能含 `\`；在非 Windows 上 `Path::file_name` 不会按 `\` 切分。
fn image_name_from_path_str(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s).trim()
}

/// 本工具自身的映像名（忽略 .exe、大小写）。
///
/// 取**实际产出的两个二进制名**：CLI/服务端 `crates/wb-shift-server` 的 bin 名
/// `workbuddy-shift`，以及桌面端 `src-tauri` 的包名 `workbuddy-shift-desktop`。
pub(crate) fn is_self_image_name(name: &str) -> bool {
    let stem = image_stem(image_name_from_path_str(name));
    stem.eq_ignore_ascii_case("workbuddy-shift")
        || stem.eq_ignore_ascii_case("workbuddy-shift-desktop")
}

pub(crate) fn is_crashpad_helper_name(name: &str) -> bool {
    image_name_from_path_str(name)
        .to_ascii_lowercase()
        .contains("crashpad_handler")
}

/// WorkBuddy 是否在运行（按档位判定，两档位互不命中）。
pub fn is_workbuddy_running(variant: WbVariant) -> bool {
    #[cfg(target_os = "macos")]
    {
        // footer 语义 = GUI 主进程在运行：仅包内守护子进程存活不算运行中。
        // 使用解析出的主进程模式，解析失败回退字面量。
        let resolved = macos_workbuddy_app_path_resolved(variant);
        let patterns = macos_main_patterns(resolved.as_deref(), variant);
        !macos_pids_by_patterns(&patterns).is_empty()
    }
    #[cfg(target_os = "windows")]
    {
        !windows_workbuddy_process_rows(variant).is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let pattern = variant.linux_process_pattern();
        match cmd_builder("pgrep").args(["-f", pattern]).output() {
            Ok(out) => out.status.success() && !out.stdout.is_empty(),
            Err(_) => false,
        }
    }
}

/// 轮询等待 WorkBuddy 进程全部退出，返回是否已退出。对照 `_wait_process_gone`。
pub fn wait_process_gone(variant: WbVariant, timeout_secs: f64) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_secs);
    while Instant::now() < deadline {
        if !is_workbuddy_running(variant) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    !is_workbuddy_running(variant)
}

/// 关闭 WorkBuddy：优雅退出 → 超时后强杀 → 确认进程消失。对照 `close_workbuddy`。
pub fn close_workbuddy(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        close_workbuddy_windows(variant, timeout_secs)
    }
    #[cfg(target_os = "macos")]
    {
        close_workbuddy_macos(variant, timeout_secs)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if !is_workbuddy_running(variant) {
            return Ok(());
        }
        let pattern = variant.linux_process_pattern();
        let _ = run_cmd_timeout("pkill", &["-15", "-f", pattern], 10);
        if wait_process_gone(variant, (timeout_secs.min(5)) as f64) {
            return Ok(());
        }
        let _ = run_cmd_timeout("pkill", &["-9", "-f", pattern], 10);
        if wait_process_gone(variant, timeout_secs as f64) {
            return Ok(());
        }
        Err("WorkBuddy 进程无法关闭，请手动结束 workbuddy 进程".to_string())
    }
}

/// 启动 WorkBuddy。失败返回可读错误。对照 `launch_workbuddy`。
///
/// progress 可选：macOS 启动与存活确认阶段推送心跳（避免界面长时间静默）；
/// Windows/Linux 分支忽略该参数。
pub fn launch_workbuddy(variant: WbVariant, progress: Option<&dyn Fn(&str)>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        launch_workbuddy_macos(variant, progress)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = progress;
        let app = auth_file::workbuddy_app_path(variant);
        #[cfg(target_os = "windows")]
        {
            let default_exe = windows_exe_names(variant)[0].clone();
            let exe = if app
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            {
                app.clone()
            } else {
                app.join(default_exe)
            };
            if !exe.exists() {
                return Err(format!(
                    "未找到 WorkBuddy 程序（尝试路径: {}）。请在 Windows 上打开 WorkBuddy 后重试。",
                    exe.display()
                ));
            }
            persist_workbuddy_exe(&exe, variant);
            cmd_builder(&exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("启动 WorkBuddy 失败: {e}（路径: {}）", exe.display()))?;
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = cmd_builder(&app)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            Ok(())
        }
    }
}
