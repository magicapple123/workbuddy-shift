//! 诊断输出：把「识别不到 / 映射不到」变成用户能自行判断的信息。
//!
//! 只输出**结构性**信息（配置根、子目录名、插件目录名、进程 PID 与映射结果），
//! 绝不输出 secret 值（`secret-storage.xml` 只报「文件是否存在于」）。

use super::detect::*;
use super::*;

/// status() 里的 `diagnostics` 字段：切换列表为空 / 映射失败时的第一现场。
///
/// 结构一旦变化要同步 `src/lib/types/clients.ts` 的 `JetbrainsDiagnostics`。
pub(super) fn diagnostics_json(targets: &[Target], running: &[RunningIde]) -> Value {
    let root = jetbrains_config_root();
    let root_exists = root.as_deref().is_some_and(Path::exists);
    let scanned: Vec<DirScan> = root
        .as_deref()
        .filter(|p| p.exists())
        .map(scan_config_root)
        .unwrap_or_default();
    let accepted: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    let plugin_dirs: Vec<&str> = targets
        .iter()
        .filter(|t| t.plugin_installed)
        .map(|t| t.name.as_str())
        .collect();
    json!({
        "configRoot": root.as_deref().map(|p| p.to_string_lossy()),
        "configRootExists": root_exists,
        // 子目录全量清单 + 为什么被拒（accepted=false），用于区分「没装插件」与「没识别到目录」。
        "scannedDirs": scanned.iter().map(DirScan::to_json).collect::<Vec<_>>(),
        "acceptedDirs": accepted,
        "pluginInstalledDirs": plugin_dirs,
        "running": running.iter().map(running_to_json).collect::<Vec<_>>(),
        "unmappedRunningCount": unmapped_running(running).len(),
    })
}

/// 单个运行实例的诊断条目（字段与 `JetbrainsRunningProcess` 对齐）。
fn running_to_json(inst: &RunningIde) -> Value {
    json!({
        "pid": inst.pid,
        "product": inst.product_name,
        "version": inst.version,
        "configDir": inst.config_dir_name,
        "mappingSource": inst.mapping_source.as_str(),
        "mappingNote": inst.mapping_note,
        "launchTarget": inst
            .launch_target
            .as_deref()
            .map(|p| p.to_string_lossy().to_string()),
    })
}

/// 运行中但映射不到配置目录的实例（切换前必须阻断的那批）。
pub(super) fn unmapped_running(running: &[RunningIde]) -> Vec<RunningIde> {
    running
        .iter()
        .filter(|inst| inst.config_dir_name.is_none())
        .cloned()
        .collect()
}

/// 供 `status()` 顶层输出：前端据此在切换前就提示「先退出 IDE」。
pub(super) fn unmapped_running_json(running: &[RunningIde]) -> Value {
    Value::Array(
        unmapped_running(running)
            .iter()
            .map(|inst| {
                json!({
                    "pid": inst.pid,
                    "product": inst.product_name,
                    "version": inst.version,
                    "path": inst
                        .launch_target
                        .as_deref()
                        .map(|p| p.to_string_lossy().to_string()),
                    "reason": inst.mapping_note,
                })
            })
            .collect(),
    )
}

/// 未映射运行实例的阻断文案（含进程清单，用户能定位该退出哪个 IDE）。
pub(super) fn unmapped_running_hint(unmapped: &[RunningIde]) -> String {
    let mut lines = vec![format!(
        "检测到 {} 个正在运行的 JetBrains IDE 无法映射到配置目录，已拒绝写入（否则写入会被 IDE 内存态覆盖）。请先完全退出以下进程后重试：",
        unmapped.len()
    )];
    for inst in unmapped {
        lines.push(format!("- {}", inst.describe()));
    }
    if let Some(note) = unmapped
        .iter()
        .find_map(|inst| inst.mapping_note.as_deref().filter(|n| !n.is_empty()))
    {
        lines.push(format!("原因：{note}"));
    }
    lines.join("\n")
}

/// 「已识别 N 个 IDE：A、B」——失败文案的公共后缀（区分「没识别到」与「操作错了」）。
pub(super) fn describe_detected(targets: &[Target]) -> String {
    if targets.is_empty() {
        let root = jetbrains_config_root()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "（无法定位）".to_string());
        return format!("已识别 0 个 JetBrains 配置目录（配置根：{root}）");
    }
    let names: Vec<String> = targets
        .iter()
        .map(|t| {
            if t.plugin_installed {
                format!("{}（已装插件）", t.name)
            } else {
                t.name.clone()
            }
        })
        .collect();
    format!(
        "已识别 {} 个 JetBrains 配置目录：{}",
        targets.len(),
        names.join("、")
    )
}
