//! 扫描范围与缓存：来源/范围枚举、根探测、扫描节流缓存与来源扫描。

use super::payload::*;
use super::*;
use crate::modules::locks;

/// CodeBuddy CLI 日志根（`~/.codebuddy/logs`；三平台同构，`home_dir()` 已跨平台）。
pub(crate) fn cli_logs_root() -> PathBuf {
    home_dir().join(CLI_DATA_DIR).join(LOG_DIR_NAME)
}

/// CodeBuddy CLI 轮换状态文件（`~/.codebuddy-rotate/state.json`）。
pub(crate) fn cli_state_path() -> PathBuf {
    home_dir().join(CLI_ROTATE_DIR).join(CLI_STATE_FILE)
}

// ---------------------------------------------------------------------------
// 扫描缓存与范围（hook 通路接入后：逐来源判定）
// ---------------------------------------------------------------------------

/// 日志扫描的最小间隔：与前端节流口径一致（hook 已安装时 IDE 日志仍按 5 分钟扫一次）。
///
/// 前端在「距上次扫描 ≥ 5 分钟」时才发起请求，后端这一层是不依赖前端行为的兜底：
/// 事件驱动的拉取、页面反复开关都不会触发额外的全量扫描。
pub(crate) const SCAN_MIN_INTERVAL_MS: i64 = 5 * 60 * 1000;

/// 逐来源扫描范围：
///
/// - CLI / WorkBuddy(x)：数据根存在 **且** 该处配置未注册本工具 hook —— 某处注册失败
///   （非法 JSON / 用户手删条目）时只有那一处回退日志扫描，其余来源继续走事件通路；
/// - IDE(x)：`scan_ide_logs` 开启 **且** 数据根存在才扫（IDE 的 429 不触发任何事件，
///   日志是它唯一的数据源；开关关闭时两个 IDE 一律不入范围）。
pub(crate) fn scan_scope(roots: &ScanRoots, scan_ide_logs: bool) -> ScanScope {
    let mut scope = ScanScope::NONE;
    for (source, data_root, settings) in &roots.hook_sources {
        if needs_log_scan(data_root, settings, &roots.marker) {
            scope.insert(*source);
        }
    }
    if scan_ide_logs {
        for (source, data_root) in &roots.ide_sources {
            if data_root.is_dir() {
                scope.insert(*source);
            }
        }
    }
    scope
}

/// 配置里的 `scanIdeLogs`（IDE 日志扫描开关）：缺失 / 类型不符按默认开启。
pub(crate) fn scan_ide_logs_of(cfg: &Value) -> bool {
    cfg.get("scanIdeLogs")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// 当前配置是否扫描两个 CodeBuddy IDE 的日志。
pub(crate) fn scan_ide_logs_enabled() -> bool {
    scan_ide_logs_of(&crate::modules::config::load_rate_limit_config())
}

pub(crate) struct ScanCache {
    pub(crate) at: i64,
    /// 本次缓存覆盖的来源范围。
    pub(crate) scope: ScanScope,
    pub(crate) entries: Vec<Resolved>,
}

impl ScanCache {
    /// 缓存能否满足请求：范围是请求的超集（全量缓存满足子范围请求）。
    pub(crate) fn covers(&self, requested: ScanScope, now: i64) -> bool {
        now - self.at < SCAN_MIN_INTERVAL_MS && self.scope.covers(requested)
    }
}

pub(crate) static SCAN_CACHE: Mutex<Option<ScanCache>> = Mutex::new(None);

/// 扫描缓存作废：下一次 `get_rate_limits()` 按**当前来源 scope** 重算。
///
/// 装 / 卸 hook 与 `scanIdeLogs` 这类范围变更共用这一条：安装使范围收窄
/// （该来源改走事件通路）、卸载使范围扩大（回到日志扫描），两种变化都由「按当前 scope
/// 重算」覆盖，不需要为任何一方强制全来源扫描 —— 也不为接入 hook 之前的历史补扫。
pub fn invalidate_scan_cache() {
    // 进程内可重建缓存：中毒时取回旧值并直接清空，下一次调用会重新扫描。
    *locks::recover(&SCAN_CACHE) = None;
}

/// 取一次扫描结果（命中缓存则不重扫）。
///
/// 命中要求缓存范围 ⊇ 请求范围；`allow_scan = false`（限额监听被关闭）时不重扫，
/// 只用已有缓存——关闭开关后不得再读日志。
pub(crate) fn cached_scan(
    requested: ScanScope,
    now: i64,
    allow_scan: bool,
) -> (i64, Vec<Resolved>) {
    // 进程内可重建缓存：中毒时取回旧值继续（命中即返回，未命中则重新扫描并覆盖）。
    let mut cache = locks::recover(&SCAN_CACHE);
    if let Some(cached) = cache.as_ref() {
        if cached.covers(requested, now) || !allow_scan {
            return (cached.at, cached.entries.clone());
        }
    }
    if !allow_scan {
        return (0, Vec::new());
    }
    let entries = scan_sources(requested);
    *cache = Some(ScanCache {
        at: now,
        scope: requested,
        entries: entries.clone(),
    });
    (now, entries)
}

/// 按范围扫描日志：只扫 `scope` 里的来源（已注册 hook 的来源由事件通路负责）。
pub(crate) fn scan_sources(scope: ScanScope) -> Vec<Resolved> {
    let mut resolved = Vec::new();
    // ① WorkBuddy 两档位：日期目录枚举 + `sessions` 表归因（现有行为不变）。
    for variant in WbVariant::ALL {
        if !scope.contains(ScanSource::WorkBuddy(variant)) {
            continue;
        }
        let events = collect_events(
            &variant.data_root().join(LOG_DIR_NAME),
            LogFormat::WorkBuddy,
            AuthMarker::None,
        );
        resolved.extend(resolve(variant, &events));
    }
    // ② CodeBuddy CLI：日志与 WorkBuddy 同格式（时间戳/事件 id/模型归因全部兼容），
    // 但会话不在 WorkBuddy 的 `sessions` 表里，归因走日志内鉴权 uid + 轮换状态文件回落。
    if scope.contains(ScanSource::Cli) {
        let uid_to_account = account_id_by_uid();
        let cli = collect_events(
            &cli_logs_root(),
            LogFormat::WorkBuddy,
            AuthMarker::AuthDoInitProbe,
        );
        resolved.extend(resolve_by_uid(
            &cli,
            &uid_to_account,
            &ActiveAccount::load(&cli_state_path()),
        ));
    }
    // ③ 两个 CodeBuddy IDE：同一份 Ide 格式与同一套归因，只有日志根与状态文件不同。
    //
    // **不扫** `CodeBuddyExtension/Logs/CodeBuddyIDE/`：它是插件宿主的跨 App 共享日志根，
    // 与 CN IDE 真身重复记录同一次 429（同一 requestId、行时间差 3 ms），扫它会重复展示
    // 且档位归属不清（PRD D1）。
    let mut ide_uid_to_account: Option<HashMap<String, String>> = None;
    for (flavor, state_file) in [
        (CodeBuddyIdeFlavor::Intl, IDE_STATE_FILE),
        (CodeBuddyIdeFlavor::Cn, CN_IDE_STATE_FILE),
    ] {
        if !scope.contains(ScanSource::Ide(flavor)) {
            continue;
        }
        // 该 IDE 的数据目录不可定位（平台不支持）时跳过，不是错误。
        let Some(data_dir) = codebuddy_ide_data_dir(flavor) else {
            continue;
        };
        let events = collect_events(
            &data_dir.join(LOG_DIR_NAME),
            LogFormat::Ide,
            AuthMarker::AuthSessionChanged,
        );
        let uid_to_account = ide_uid_to_account.get_or_insert_with(account_id_by_uid);
        resolved.extend(resolve_by_uid(
            &events,
            uid_to_account,
            &ActiveAccount::load(&store_dir().join(state_file)),
        ));
    }
    resolved
}
