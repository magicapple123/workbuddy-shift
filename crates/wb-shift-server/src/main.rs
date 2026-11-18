//! workbuddy-shift CLI：npm 安装形态的入口。
//!
//! ```bash
//! workbuddy-shift              # 启动本地服务（仅 127.0.0.1）+ 打开浏览器 webui
//! workbuddy-shift serve        # 同默认行为；配合 --port / --no-open 使用
//! workbuddy-shift status       # 终端输出当前账号
//! workbuddy-shift version      # 版本号
//! workbuddy-shift help         # 用法说明（--help / -h 等价）
//! ```

// 生产路径（lib/bin 的非测试编译）禁止 unwrap/expect：宿主层只做协议适配，
// 任何失败都必须变成稳定的错误文案。测试目标（cfg(test)）不受此限制。
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod api;

use serde_json::json;

use wb_shift_core::modules::{
    account, auth_file, checkin, config, process, rotate, travel, update, variant::WbVariant,
};

/// CLI 用法说明（`help` / `--help` / `-h`）。
///
/// 默认端口固定监听 127.0.0.1：服务会读写本机账号与客户端配置，不提供改监听地址的
/// 开关，避免被参数误改成对外暴露。
const USAGE: &str = "\
workbuddy-shift — WorkBuddy 账号切换工具（本地 webui）

用法:
  workbuddy-shift [选项]          启动本地服务并打开浏览器（默认命令）
  workbuddy-shift serve [选项]    同默认行为
  workbuddy-shift status          终端输出当前账号
  workbuddy-shift version         输出版本号
  workbuddy-shift help            显示本帮助

选项:
  --port <端口>      监听端口（默认 57910；也可写 --port=<端口>；始终只绑定 127.0.0.1）
  --no-open          启动服务后不自动打开浏览器
  --variant <档位>   cn（默认）或 ai；仅影响 status 的档位选择
";

fn print_usage() {
    print!("{USAGE}");
}

fn default_port() -> u16 {
    57910
}

/// 后台任务：自动签到启动即核验，之后按 core 计算的下一轮延迟睡眠（未设置
/// 签到时间段时固定 30 分钟）；自动轮换按配置间隔执行；
/// 限额 hook 信号每秒轮询一次、启动时后台默认接入。
fn spawn_background_loops() {
    tokio::spawn(async move {
        if let Err(error) = config::compact_checkin_logs() {
            eprintln!("[签到] 历史日志整理失败: {error}");
        }
        let _ = checkin::run_checkin_cycle(checkin::CheckinCycleMode::StartupVerify).await;
        loop {
            tokio::time::sleep(checkin::next_cycle_delay()).await;
            let _ = checkin::run_checkin_cycle(checkin::CheckinCycleMode::PeriodicRecovery).await;
        }
    });

    tokio::spawn(async move {
        let mut last_cycle_at: i64 = 0;
        loop {
            let cfg = config::load_auto_rotate_config();
            if cfg.get("enabled").and_then(|v| v.as_bool()) == Some(true) {
                let interval_minutes = cfg
                    .get("check_interval_minutes")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(5)
                    .max(1);
                let now = config::now_ms();
                if now - last_cycle_at >= interval_minutes * 60_000 {
                    last_cycle_at = now;
                    let _ = rotate::run_rotate_cycle().await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    // 派猫猫旅行：启动即派发，之后周期性补派（并重试 no-buddy / 瞬时错误）。
    tokio::spawn(async move {
        let _ = travel::run_travel_cycle().await;
        loop {
            tokio::time::sleep(travel::TRAVEL_RETRY_INTERVAL).await;
            let _ = travel::run_travel_cycle().await;
        }
    });

    // 旅行领取：启动立刻查一轮（避免重启后空等 15 分钟漏领），之后按周期检查。
    tokio::spawn(async move {
        let _ = travel::run_travel_claim_cycle().await;
        loop {
            tokio::time::sleep(travel::TRAVEL_CLAIM_INTERVAL).await;
            let _ = travel::run_travel_claim_cycle().await;
        }
    });

    // 限额 hook 信号：轮询 `~/.workbuddy-shift/hook-events.jsonl`，入账后由前端下次拉取可见。
    // webui 没有 Tauri 事件通道，因此不需要推送回调（桌面端见 src-tauri/src/lib.rs）。
    wb_shift_core::modules::rate_limit_events::spawn_watcher(|| {});

    // 默认接入：后台线程自动安装 hook（幂等、非阻塞、失败静默）；
    // 装上了就作废扫描缓存——扫描范围从全量收窄到「未注册的来源」。
    std::thread::spawn(|| {
        if wb_shift_core::modules::rate_limit_hook::auto_install_on_startup() {
            wb_shift_core::modules::limits::invalidate_scan_cache();
        }
    });
}

/// CLI 档位参数：`--variant ai` / `--variant=ai`；缺省国内版。
///
/// 与 Tauri 命令的可选 `variant` 参数、HTTP 路由的 query/body 字段同义。
fn variant_arg(args: &[String]) -> WbVariant {
    let raw = args.iter().enumerate().find_map(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--variant=") {
            return Some(value.to_string());
        }
        arg.eq("--variant")
            .then(|| args.get(index + 1).cloned().unwrap_or_default())
    });
    WbVariant::parse(raw.as_deref())
}

/// 取第一个非选项子命令（`status` / `version` / `help` / `serve`）。
///
/// 全局选项允许写在子命令前面（如 `workbuddy-shift --variant ai status`）：
/// 解析时跳过 `--variant` / `--port` 及其值、`--variant=…` / `--port=…`、`--no-open`；
/// 没有子命令时返回 `None`（调用方按默认 `serve` 处理）。未知单词仍原样返回，
/// 由 `main` 落到改造前的 serve 行为（不新增对未知命令的拒绝，避免破坏既有调用）。
///
/// `args` 是去掉程序名后的参数列表（与单测里的 `args(...)` 同口径）。
fn subcommand_of(args: &[String]) -> Option<&str> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if matches!(arg.as_str(), "--variant" | "--port") {
            // 选项 + 值一起跳过；值缺失时 index 越过末尾，循环自然结束。
            index += 2;
            continue;
        }
        if arg.starts_with("--variant=") || arg.starts_with("--port=") || arg == "--no-open" {
            index += 1;
            continue;
        }
        return Some(arg.as_str());
    }
    None
}

fn print_status(variant: WbVariant) {
    let auth = auth_file::read_auth_file(variant);
    let current = auth.as_ref().map(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        json!({
            "uid": account::display_value(&acct, "uid"),
            "nickname": account::display_value(&acct, "nickname"),
            "email": account::display_value(&acct, "email"),
        })
    });
    let running = process::is_workbuddy_running(variant);
    println!("workbuddy-shift v{}", update::APP_VERSION);
    println!("WorkBuddy 运行中: {}", if running { "是" } else { "否" });
    match current {
        Some(c) => {
            let name = c
                .get("nickname")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("email").and_then(|v| v.as_str()))
                .unwrap_or("未知");
            println!("当前账号: {name}");
        }
        None => println!("当前账号: 未登录"),
    }
    println!("账号数: {}", account::load_accounts().len());
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = subcommand_of(&args[1..]).unwrap_or("serve");
    match cmd {
        "status" => print_status(variant_arg(&args)),
        "version" | "--version" | "-V" => {
            println!("workbuddy-shift {}", env!("CARGO_PKG_VERSION"));
        }
        // 未识别为子命令时（无参数、serve、--port/--no-open 等选项）一律进入服务模式，
        // 保持改造前行为；help 必须在此之前拦下，否则 --help 会启动服务并打开浏览器。
        "help" | "--help" | "-h" => print_usage(),
        _ => serve(&args).await,
    }
}

async fn serve(args: &[String]) {
    let mut port = default_port();
    // 支持 `--port 8080` 与 `--port=8080` 两种写法（与 `subcommand_of` 的跳过规则一致，
    // 否则被当作全局选项跳过的 `--port=…` 会被静默忽略、落到默认端口）。
    if let Some(raw) = args.iter().enumerate().find_map(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--port=") {
            return Some(value);
        }
        arg.eq("--port")
            .then(|| args.get(index + 1).map(String::as_str).unwrap_or(""))
    }) {
        if let Ok(parsed) = raw.parse::<u16>() {
            port = parsed;
        }
    }

    let app = api::router(api::shared_state());
    let addr = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("启动失败: 端口 {port} 被占用或不可用（{e}）。可用 --port 指定其他端口。");
            std::process::exit(1);
        }
    };

    println!("workbuddy-shift v{}", update::APP_VERSION);
    println!("webui: http://{addr}");
    println!("按 Ctrl+C 停止服务。");

    let no_open = args.iter().any(|a| a == "--no-open");
    if !no_open {
        open_browser(&addr);
    }

    spawn_background_loops();

    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("服务异常退出: {error}");
        std::process::exit(1);
    }
}

fn open_browser(addr: &str) {
    let url = format!("http://{addr}");
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = std::process::Command::new("cmd");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：开浏览器不闪 cmd 窗
        }
        let _ = c.args(["/C", "start", &url]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::{subcommand_of, variant_arg};
    use wb_shift_core::modules::variant::WbVariant;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 不传档位时必须仍是国内版（改造前行为）。
    #[test]
    fn cli_variant_defaults_to_cn() {
        assert_eq!(variant_arg(&args(&["status"])), WbVariant::Cn);
        assert_eq!(variant_arg(&args(&["status", "--debug"])), WbVariant::Cn);
        assert_eq!(variant_arg(&args(&["status", "--variant"])), WbVariant::Cn);
        assert_eq!(variant_arg(&args(&["status", "cn"])), WbVariant::Cn);
        assert_eq!(
            variant_arg(&args(&["status", "--variant=cn"])),
            WbVariant::Cn
        );
    }

    #[test]
    fn cli_variant_reads_space_and_equals_forms() {
        assert_eq!(
            variant_arg(&args(&["status", "--variant", "ai"])),
            WbVariant::Ai
        );
        assert_eq!(
            variant_arg(&args(&["status", "--variant=ai"])),
            WbVariant::Ai
        );
        assert_eq!(
            variant_arg(&args(&["status", "--variant", "AI", "--no-open"])),
            WbVariant::Ai
        );
        assert_eq!(
            variant_arg(&args(&["--variant=ai", "status"])),
            WbVariant::Ai
        );
    }

    /// 子命令解析：全局选项可以写在子命令前面，跳过选项与其值后再取第一个单词。
    #[test]
    fn subcommand_skips_global_options() {
        assert_eq!(
            subcommand_of(&args(&["--variant", "ai", "status"])),
            Some("status")
        );
        assert_eq!(
            subcommand_of(&args(&["--variant=ai", "status"])),
            Some("status")
        );
        assert_eq!(subcommand_of(&args(&["--no-open"])), None);
        assert_eq!(subcommand_of(&args(&[])), None);
        assert_eq!(subcommand_of(&args(&["--port", "1234"])), None);
        assert_eq!(
            subcommand_of(&args(&["--port=1234", "version"])),
            Some("version")
        );
        assert_eq!(
            subcommand_of(&args(&["status", "--variant", "ai"])),
            Some("status")
        );
        assert_eq!(subcommand_of(&args(&["serve", "--no-open"])), Some("serve"));
        assert_eq!(subcommand_of(&args(&["--port"])), None);
        assert_eq!(subcommand_of(&args(&["--variant"])), None);
    }
}
