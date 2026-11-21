//! workbuddy-shift CLI：npm 安装形态的入口。
//!
//! ```bash
//! workbuddy-shift              # 启动本地服务（仅 127.0.0.1）+ 打开浏览器 webui
//! workbuddy-shift serve        # 同默认行为；配合 --port / --no-open 使用
//! workbuddy-shift status       # 终端输出当前账号（--json 输出供 AI 解析的 JSON）
//! workbuddy-shift accounts     # 账号列表（JSON）
//! workbuddy-shift limits       # 各账号观察到的模型限额（JSON）
//! workbuddy-shift credits      # 各账号积分（JSON，默认实时）
//! workbuddy-shift inspect      # 整池只读体检（JSON）
//! workbuddy-shift switch <id>  # 切 CodeBuddy CLI 账号（--dry-run 只预览）
//! workbuddy-shift version      # 版本号
//! workbuddy-shift help         # 用法说明（--help / -h 等价）
//! ```
//!
//! AI 友好子命令（`accounts` / `limits` / `credits` / `inspect` / `switch`）的实现在
//! [`cli`]：stdout 只输出 JSON、进度走 stderr、退出码见 `docs/cli.md`。
//!
//! 生产路径（lib/bin 的非测试编译）禁止 unwrap/expect：宿主层只做协议适配，
//! 任何失败都必须变成稳定的错误文案。测试目标（cfg(test)）不受此限制。
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod api;
mod cli;
mod listen;

use serde_json::json;

use listen::{is_loopback, open_target, resolve_listen, LISTEN_ENV_VAR};
use wb_shift_core::modules::{
    account, auth_file, checkin, config, process, rotate, travel, update, variant::WbVariant,
};

/// CLI 用法说明（`help` / `--help` / `-h`）。
///
/// 默认监听 127.0.0.1：服务会读写本机账号与客户端配置，所以**命令行故意不提供改监听地址
/// 的开关**，避免被参数误改成对外暴露。只有部署形态（Docker）才用环境变量显式放开，
/// 解析规则见 `listen.rs`，部署与安全说明见 `docs/docker.md`。
const USAGE: &str = "\
workbuddy-shift — WorkBuddy 账号切换工具（本地 webui）

用法:
  workbuddy-shift [选项]          启动本地服务并打开浏览器（默认命令）
  workbuddy-shift serve [选项]    同默认行为
  workbuddy-shift status          终端输出当前账号；加 --json 输出 JSON
  workbuddy-shift accounts        账号列表（JSON，不含密钥）
  workbuddy-shift limits          各账号观察到的模型限额（JSON）
  workbuddy-shift credits         各账号积分（JSON，默认实时查询）
  workbuddy-shift inspect         整池只读体检：账号 + 积分 + 限额（JSON）
  workbuddy-shift switch <id>     切 CodeBuddy CLI 账号（--dry-run 只预览）
  workbuddy-shift version         输出版本号
  workbuddy-shift help            显示本帮助

选项:
  --port <端口>      监听端口（默认 57910；也可写 --port=<端口>；地址默认 127.0.0.1）
  --no-open          启动服务后不自动打开浏览器
  --variant <档位>   cn（默认）或 ai；仅影响 status 的档位选择

环境变量:
  WB_SHIFT_LISTEN         覆盖监听地址（默认 127.0.0.1）。可写 0.0.0.0、0.0.0.0:57910、
                          [::]:57910；--port 的优先级高于其中的端口，非法值回落 127.0.0.1
  WB_SHIFT_ALLOWED_HOSTS  额外放行的 Host 白名单（逗号分隔域名，默认空 = 仅本机回环）
  WB_SHIFT_HOME           覆盖家目录（数据落在其下的 .workbuddy-shift）

AI 友好子命令的选项:
  --json             stdout 只输出 JSON（accounts/limits/credits/inspect 始终输出 JSON）
  --cached           积分只读本地快照，不联网（credits / inspect）
  --timeout <秒>     实时积分查询的总预算，默认 8，上限 120（credits / inspect）
  --no-credits       跳过积分网络查询（inspect）
  --summary          强制整池汇总视图（inspect，默认行为）
  --dry-run          只输出「将会发生什么」，不写任何状态（switch）
  --force            有活跃 CLI 会话时仍然切号（会关闭 CLI 进程，当前会话中断且不自动重开）

退出码: 0 成功 / 1 目标不存在 / 2 用法错误 / 3 拒绝执行（有活跃 CLI 会话） / 4 致命错误
";

fn print_usage() {
    print!("{USAGE}");
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

/// 取第一个非选项子命令的**下标**（`status` / `version` / `help` / `serve` / AI 子命令）。
///
/// 全局选项允许写在子命令前面（如 `workbuddy-shift --variant ai status`）：
/// 解析时跳过 `--variant` / `--port` 及其值、`--variant=…` / `--port=…`、`--no-open`；
/// 没有子命令时返回 `None`（调用方按默认 `serve` 处理）。未知单词仍原样返回，
/// 由 `main` 落到改造前的 serve 行为（不新增对未知命令的拒绝，避免破坏既有调用）。
///
/// `args` 是去掉程序名后的参数列表（与单测里的 `args(...)` 同口径）。
fn subcommand_index(args: &[String]) -> Option<usize> {
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
        return Some(index);
    }
    None
}

/// 子命令名（`subcommand_index` 的便捷包装）。
///
/// 只有单测用得到（`main` 需要下标才能切出子命令之后的参数），因此只在测试目标里编译。
#[cfg(test)]
fn subcommand_of(args: &[String]) -> Option<&str> {
    subcommand_index(args)
        .and_then(|index| args.get(index))
        .map(String::as_str)
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
    let rest = &args[1..];
    let command_index = subcommand_index(rest);
    let cmd = command_index
        .and_then(|index| rest.get(index))
        .map(String::as_str)
        .unwrap_or("serve");
    // 子命令之后的参数交给 cli 自己解析（全局选项的跳过规则只在 subcommand_index 里实现一次）。
    let command_args: &[String] = command_index.map(|index| &rest[index + 1..]).unwrap_or(&[]);
    match cmd {
        // 裸 `status` 保持改造前的终端文本；`status --json` 走 AI 友好输出。
        "status" if cli::wants_json(command_args) => {
            std::process::exit(cli::run("status", command_args).await)
        }
        "status" => print_status(variant_arg(&args)),
        // AI 友好子命令：stdout 纯 JSON，退出码即进程退出码。
        "accounts" | "limits" | "credits" | "inspect" | "switch" => {
            std::process::exit(cli::run(cmd, command_args).await)
        }
        "version" | "--version" | "-V" => {
            println!("workbuddy-shift {}", env!("CARGO_PKG_VERSION"));
        }
        // 未识别为子命令时（无参数、serve、--port/--no-open 等选项）一律进入服务模式，
        // 保持改造前行为；help 必须在此之前拦下，否则 --help 会启动服务并打开浏览器。
        "help" | "--help" | "-h" => print_usage(),
        _ => serve(&args).await,
    }
}

/// `--port` 的取值（`--port 8080` 与 `--port=8080` 两种写法）。
///
/// 解析失败返回 `None`，交给默认值——与改造前一致（写错的 `--port` 不会让服务起不来，
/// 也不会连带丢掉 `WB_SHIFT_LISTEN` 里配置的地址）。
fn cli_port(args: &[String]) -> Option<u16> {
    args.iter().enumerate().find_map(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--port=") {
            return value.parse::<u16>().ok();
        }
        if arg == "--port" {
            return args
                .get(index + 1)
                .and_then(|value| value.parse::<u16>().ok());
        }
        None
    })
}

async fn serve(args: &[String]) {
    // 地址只来自 WB_SHIFT_LISTEN（默认 127.0.0.1），端口优先级 --port > 环境变量里的端口 >
    // 默认 57910；解析细节与非法值回落见 listen.rs。
    let raw_listen = std::env::var(LISTEN_ENV_VAR).ok();
    let (addr, listen_warning) = resolve_listen(raw_listen.as_deref(), cli_port(args));
    if let Some(warning) = listen_warning {
        eprintln!("{warning}");
    }
    // 白名单解析告警只打一次（缓存读环境变量，见 host_guard.rs）。
    if let Some(warning) = api::allowed_hosts_warning() {
        eprintln!("{warning}");
    }

    let app = api::router(api::shared_state());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "启动失败: 无法监听 {addr}（{e}）。可用 --port 指定其他端口，或检查 {LISTEN_ENV_VAR}。"
            );
            std::process::exit(1);
        }
    };

    // 横幅必须打印**真实绑定地址**：容器里绑 0.0.0.0 时用户需要看到这一点，
    // 而不是看到一个打不开的 http://0.0.0.0。浏览器打开的目标与横幅一致。
    let target = open_target(&addr);
    println!("workbuddy-shift v{}", update::APP_VERSION);
    if is_loopback(&addr) {
        println!("webui: http://{target}");
    } else {
        println!("监听地址: {addr}");
        println!("webui: http://{target}");
        println!(
            "安全提示: 已监听非回环地址，服务没有登录鉴权——请确保只在可信网络、\
             或带认证的反向代理之后暴露（说明见 docs/docker.md）。"
        );
        if api::allowed_hosts().is_empty() {
            println!(
                "提示: 未设置 WB_SHIFT_ALLOWED_HOSTS，用非回环域名访问会因 Host 校验返回 403\
                 （127.0.0.1 / localhost 不受影响）。"
            );
        }
    }
    println!("按 Ctrl+C 停止服务。");

    let no_open = args.iter().any(|a| a == "--no-open");
    if !no_open {
        open_browser(&target);
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
    use super::{subcommand_index, subcommand_of, variant_arg};
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

    /// AI 友好子命令不得落到 `serve`（否则 CLI 会挂住监听端口而不是输出 JSON）。
    #[test]
    fn ai_subcommands_are_recognized_before_serve() {
        for command in ["accounts", "limits", "credits", "inspect", "switch"] {
            assert_eq!(subcommand_of(&args(&[command])), Some(command));
        }
        assert_eq!(
            subcommand_of(&args(&["inspect", "--summary"])),
            Some("inspect")
        );
        assert_eq!(
            subcommand_of(&args(&["switch", "acct_1", "--dry-run"])),
            Some("switch")
        );
        assert_eq!(
            subcommand_of(&args(&["--variant", "ai", "credits", "--cached"])),
            Some("credits")
        );
        // 全局选项仍正常：`--port` 与其值被跳过，子命令下标指向真正的命令。
        let tokens = args(&["--port", "59000", "accounts", "--json"]);
        assert_eq!(subcommand_index(&tokens), Some(2));
        assert_eq!(subcommand_of(&tokens), Some("accounts"));
    }

    /// `main` 用下标切出子命令之后的参数：全局选项不能被带进子命令参数里。
    #[test]
    fn command_args_are_sliced_after_subcommand() {
        let tokens = args(&["--variant=ai", "switch", "acct_1", "--dry-run"]);
        let index = subcommand_index(&tokens).expect("应识别到 switch");
        assert_eq!(&tokens[index + 1..], &args(&["acct_1", "--dry-run"])[..]);
    }
}
