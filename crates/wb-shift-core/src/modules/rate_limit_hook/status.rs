//! hook 状态：事件路径/marker、日志扫描判定与 settings marker 查询、status_at。

use super::*;

/// hook 事件信号文件路径（后端消费方使用）。
pub fn events_path() -> PathBuf {
    HookLayout::default_layout().events
}

/// 本工具 hook 脚本的绝对路径（配置里的 marker；扫描范围判定用）。
pub(crate) fn hook_marker() -> String {
    HookLayout::default_layout().marker()
}

/// 该处客户端配置是否需要日志扫描：**数据根存在** 且 该配置未注册本工具 hook。
///
/// 逐来源判定（而不是「整体装了 hook 就不扫日志」）：某处配置注册失败 / 用户手删条目时，
/// 只有那一处回退日志扫描，其余来源继续走 hook。路径与 marker 全部显式传入（单测注入 tempdir）。
pub(crate) fn needs_log_scan(data_root: &Path, settings: &Path, marker: &str) -> bool {
    data_root.is_dir() && !settings_has_marker(settings, marker)
}

// ---------------------------------------------------------------------------
// 配置读写
// ---------------------------------------------------------------------------

pub(super) fn read_settings(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 条目是否属于本工具：嵌套格式里任一 command hook 的命令包含当前脚本路径。
///
/// `legacy_markers` 只在清理时传入，且必须都是本工具历史脚本的绝对路径
/// （旧 `hook.cmd`、上一代数据目录里的同名脚本，见 `HookLayout::legacy_markers`）。
/// 存在性判定不认旧形态，否则旧条目会挡住新脚本的自动迁移 —— 2026-09-18 实证。
pub(super) fn entry_is_ours(entry: &Value, marker: &str, legacy_markers: &[String]) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("type").and_then(Value::as_str) == Some("command")
                    && hook
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| {
                            command.contains(marker)
                                || legacy_markers
                                    .iter()
                                    .any(|legacy| command.contains(legacy.as_str()))
                        })
            })
        })
}

/// 配置里是否已注册本工具的 hook（脚本存在性由调用方另外判定）。
pub(super) fn config_has_marker(root: &Value, marker: &str) -> bool {
    root.get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| {
            HOOK_EVENTS.iter().any(|event| {
                hooks
                    .get(*event)
                    .and_then(Value::as_array)
                    .is_some_and(|entries| entries.iter().any(|e| entry_is_ours(e, marker, &[])))
            })
        })
}

/// 指定配置文件的 marker 命中判定。
///
/// 文件缺失 / 非法 JSON / 结构不符一律视为「未注册」：这些情况下事件不会到达，
/// 该来源必须回退日志扫描。
pub(crate) fn settings_has_marker(path: &Path, marker: &str) -> bool {
    read_settings(path).is_some_and(|root| config_has_marker(&root, marker))
}

// ---------------------------------------------------------------------------
// 安装 / 卸载 / 状态
// ---------------------------------------------------------------------------

/// hook 安装状态（含三处配置逐项结果）。
pub fn hook_status() -> Value {
    status_at(&HookLayout::default_layout())
}

pub(super) fn status_at(layout: &HookLayout) -> Value {
    let marker = layout.marker();
    let script_exists = layout.script.is_file();
    let mut configured = 0;
    let targets: Vec<Value> = layout
        .targets
        .iter()
        .map(|target| {
            // 不存在的数据根：不参与安装，也不看配置（它根本不该有配置）。
            let exists = target.exists();
            let installed = exists && target.registered(&marker);
            if installed {
                configured += 1;
            }
            json!({
                "label": target.label,
                "path": target.settings.to_string_lossy(),
                "exists": exists,
                "installed": installed,
            })
        })
        .collect();
    json!({
        "scriptPath": layout.script.to_string_lossy(),
        "scriptExists": script_exists,
        "eventsPath": layout.events.to_string_lossy(),
        "installed": script_exists && configured > 0,
        "targets": targets,
    })
}
