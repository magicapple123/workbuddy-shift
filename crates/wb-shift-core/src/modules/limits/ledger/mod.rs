use super::parse::AuthMarker;
use super::parse::Event;
use super::parse::LogFormat;
use super::payload::build_payload;
use super::payload::Resolved;
use super::scan::collect_events;
use super::scan::CLI_DATA_DIR;
use super::scan::LOG_DIR_NAME;
use crate::modules::account;
use crate::modules::config::{home_dir, now_ms, store_dir};
use crate::modules::rate_limit_hook::{hook_marker, needs_log_scan, SETTINGS_FILE_NAME};
use crate::modules::session::{open_db, table_exists, workbuddy_db_path};
use crate::modules::variant::WbVariant;
use crate::modules::vscode_cn_inject::{codebuddy_ide_data_dir, CodeBuddyIdeFlavor};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// CodeBuddy CLI 轮换状态目录名（`rotate.rs` / `codebuddy_cli.rs` 的写入方）。
pub(crate) const CLI_ROTATE_DIR: &str = ".codebuddy-rotate";

/// 状态文件名（`rotate.rs` / `codebuddy_ide.rs` / `codebuddy_cn_ide.rs` 的写入方）。
///
/// 与写入方保持同名：这里只读回落的 `activeAccountId`（账号库的 `id`，不是 uid）。
pub(crate) const CLI_STATE_FILE: &str = "state.json";

pub(super) const IDE_STATE_FILE: &str = "codebuddy_ide.json";

const CN_IDE_STATE_FILE: &str = "codebuddy_cn_ide.json";

/// 一个日志来源（扫描范围的最小单位）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScanSource {
    /// WorkBuddy 客户端（两档位各算一个来源）。
    WorkBuddy(WbVariant),
    /// CodeBuddy CLI（与两个 IDE **共用** `~/.codebuddy/settings.json`）。
    Cli,
    /// CodeBuddy IDE 的一个形态（429 不触发任何事件；`scanIdeLogs` 开启且数据根存在才扫）。
    Ide(CodeBuddyIdeFlavor),
}

impl ScanSource {
    /// 对外来源标签（payload 的 `source` 字段）。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::WorkBuddy(WbVariant::Cn) => "workbuddy-cn",
            Self::WorkBuddy(WbVariant::Ai) => "workbuddy-ai",
            Self::Cli => "codebuddy-cli",
            Self::Ide(CodeBuddyIdeFlavor::Intl) => "codebuddy-ide",
            Self::Ide(CodeBuddyIdeFlavor::Cn) => "codebuddy-cn-ide",
        }
    }

    /// 位图里的位序（与 `ScanScope::ALL` 对应）。
    fn bit(self) -> u8 {
        match self {
            Self::WorkBuddy(WbVariant::Cn) => 0,
            Self::WorkBuddy(WbVariant::Ai) => 1,
            Self::Cli => 2,
            Self::Ide(CodeBuddyIdeFlavor::Intl) => 3,
            Self::Ide(CodeBuddyIdeFlavor::Cn) => 4,
        }
    }
}

/// 一次请求要扫的来源集合（位图）。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct ScanScope(u8);

impl ScanScope {
    /// 一个来源都不扫。
    pub(super) const NONE: Self = Self(0);
    /// 全量五源。生产路径的范围一律由 `scan_scope` 按来源算出（可能正好是它），
    /// 这里保留给缓存超集语义的测试用。
    #[cfg(test)]
    pub(super) const ALL: Self = Self(0b1_1111);

    pub(super) fn insert(&mut self, source: ScanSource) {
        self.0 |= 1 << source.bit();
    }

    pub(super) fn contains(self, source: ScanSource) -> bool {
        self.0 & (1 << source.bit()) != 0
    }

    /// `self` 是否覆盖 `other`（缓存命中判据：缓存范围必须是请求范围的超集）。
    pub(super) fn covers(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// 扫描范围判定的输入路径（显式传入：单测注入 tempdir，不触碰真实配置）。
pub(crate) struct ScanRoots {
    /// 需要按「数据根存在 且 该处未注册 hook」判定的来源：CLI 与 WorkBuddy 两档位。
    pub(super) hook_sources: Vec<(ScanSource, PathBuf, PathBuf)>,
    /// 只判「数据根存在」的来源：两个 IDE。
    pub(super) ide_sources: Vec<(ScanSource, PathBuf)>,
    /// 本工具 hook 脚本的绝对路径（配置里的 marker）。
    pub(super) marker: String,
}

impl ScanRoots {
    fn real() -> Self {
        let mut hook_sources = Vec::new();
        for variant in WbVariant::ALL {
            let root = variant.data_root();
            hook_sources.push((
                ScanSource::WorkBuddy(variant),
                root.clone(),
                root.join(SETTINGS_FILE_NAME),
            ));
        }
        let cli_root = home_dir().join(CLI_DATA_DIR);
        hook_sources.push((
            ScanSource::Cli,
            cli_root.clone(),
            cli_root.join(SETTINGS_FILE_NAME),
        ));
        let ide_sources = [CodeBuddyIdeFlavor::Intl, CodeBuddyIdeFlavor::Cn]
            .into_iter()
            .filter_map(|flavor| {
                codebuddy_ide_data_dir(flavor).map(|dir| (ScanSource::Ide(flavor), dir))
            })
            .collect();
        Self {
            hook_sources,
            ide_sources,
            marker: hook_marker(),
        }
    }
}

/// 全部账号当前的模型限额状态。
///
/// 两条通路合并：
/// - **hook 通路**（CLI / WorkBuddy 两档位）：事件驱动、实时归因，由 `rate_limit_events` 持有；
/// - **日志扫描**（两个 IDE 只走这条，受 `scanIdeLogs` 开关约束；CLI / WorkBuddy 只在「该处未注册 hook」时回退扫描）：
///   按 `SCAN_MIN_INTERVAL_MS` 节流，合并后的 `scannedAt` 是**最近一次真实扫描**的时刻
///   （前端据此节流）。
///
/// 不接收档位参数：扫描本身就是全局的，按档位调用会把同一份日志扫 N 遍。
pub fn get_rate_limits() -> Value {
    let now = now_ms();
    // 限额监听关闭时不再扫日志（hook 信号照常入账，由后端持有）。
    let enabled = crate::modules::rate_limit_events::rate_limit_enabled();
    // 「扫描 CodeBuddy IDE 日志」是独立开关：关闭后两个 IDE 一律不入 scope，
    // CLI / WorkBuddy 的逐来源判定（hook 未接上则回退日志）不受影响。
    let scan_ide_logs = scan_ide_logs_enabled();
    // 逐来源判定：不存在的客户端不参与扫描；已注册 hook 的来源交给事件通路。
    // 装 / 卸 hook 只作废缓存（`invalidate_scan_cache`），下一次按当前范围重算 ——
    // 安装使范围收窄、卸载使范围扩大，都不需要强制全来源，也不补扫接入前的历史。
    let scope = scan_scope(&ScanRoots::real(), scan_ide_logs);
    let (scanned_at, mut resolved) = cached_scan(scope, now, enabled);
    resolved.extend(crate::modules::rate_limit_events::hook_entries(now));
    // 13-B 开关：关闭后不再产出 borrowed（账号卡不显示「其它客户端限流至 …」）。
    let cross_client_infer = crate::modules::config::load_rate_limit_config()
        .get("crossClientInfer")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    build_payload(
        resolved,
        if scanned_at > 0 { scanned_at } else { now },
        now,
        cross_client_infer,
    )
}

/// hook 安装状态 + 运行期字段（最近一次 hook 事件时刻）。
///
/// 两个宿主（Tauri / Axum）共用这一份组装逻辑，避免各自维护同样的
/// `lastEventAt` 字段而产生响应形态漂移。
pub fn rate_limit_hook_status() -> Value {
    let mut status = crate::modules::rate_limit_hook::hook_status();
    status["lastEventAt"] = json!(crate::modules::rate_limit_events::last_event_at());
    // VS Code 插件能力探测（第三轮第 20 条）：只作诊断字段，前端类型不含它，
    // UI 的「暂不支持」是静态事实，不随探测结果变化 —— 否则探测失败会被误读成「支持」。
    status["vscodeExt"] = super::vscode_ext::probe();
    status
}

/// 保存限额监听配置，并同步日志扫描范围。
///
/// `scanIdeLogs` 变化 → 作废扫描缓存（`invalidate_scan_cache`）：下一次 `get_rate_limits()`
/// 按新 scope 重扫，关掉后不会再用旧缓存扫一次 IDE。
pub fn save_rate_limit_config(cfg: &Value) -> std::io::Result<()> {
    save_rate_limit_config_at(&crate::modules::config::rate_limit_config_file(), cfg)
}

/// [`save_rate_limit_config`] 的可注入路径版本（单测不得触碰真实 `~/.workbuddy-shift`）。
pub(crate) fn save_rate_limit_config_at(path: &Path, cfg: &Value) -> std::io::Result<()> {
    let before = crate::modules::config::load_rate_limit_config_at(path);
    crate::modules::config::save_rate_limit_config_at(path, cfg)?;
    let after = crate::modules::config::load_rate_limit_config_at(path);
    if scan_ide_logs_of(&before) != scan_ide_logs_of(&after) {
        invalidate_scan_cache();
    }
    Ok(())
}

mod cache;
mod payload;

pub use cache::invalidate_scan_cache;
pub(crate) use payload::account_by_session;

// 测试通过 `use super::*` 复用这些重导出；lib 目标下它们看起来未被使用。
#[allow(unused_imports)]
pub(crate) use cache::{
    cached_scan, cli_logs_root, cli_state_path, scan_ide_logs_enabled, scan_ide_logs_of,
    scan_scope, scan_sources, ScanCache, SCAN_CACHE, SCAN_MIN_INTERVAL_MS,
};
// 测试通过 `use super::*` 复用这些重导出；lib 目标下它们看起来未被使用。
#[allow(unused_imports)]
pub(crate) use payload::{account_id_by_uid, resolve, resolve_by_uid, ActiveAccount};
