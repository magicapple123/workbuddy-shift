//! 卸载：移除事件条目、opt-out 记录与目标恢复。

use super::install::*;
use super::status::*;
use super::*;

/// 卸载：移除三处配置里属于本工具的条目（可逐字节还原），并清理本工具生成的脚本。
///
/// 「卸载 hook」同时记下用户的拒绝（`hookOptOut = true`），之后不再自动接入。
pub fn uninstall_hook() -> Result<Value, String> {
    uninstall_and_opt_out(
        &HookLayout::default_layout(),
        &config::rate_limit_config_file(),
    )
}

/// 「卸载 hook」：移除注册条目 + 记录 `hookOptOut`（用户拒绝自动接入）。
///
/// 两件事必须一起做，避免「配置已还原但下次启动又被自动装回」的状态不一致。
pub(super) fn uninstall_and_opt_out(
    layout: &HookLayout,
    config_path: &Path,
) -> Result<Value, String> {
    let result = uninstall_at(layout);
    let _ = config::set_rate_limit_hook_opt_out_at(config_path, true);
    result
}

pub(super) fn uninstall_at(layout: &HookLayout) -> Result<Value, String> {
    let marker = layout.marker();
    let legacy_markers = layout.legacy_markers();
    let mut errors = Vec::new();
    for target in &layout.targets {
        if let Err(error) = uninstall_target(layout, target, &marker, &legacy_markers) {
            errors.push(format!("{}：{error}", target.settings.display()));
        }
    }
    // 脚本只在本工具生成的那一份（当前模板或已知旧模板）时删除；用户改过就保留，不做猜测。
    if matches!(
        layout.script_ownership(),
        ScriptOwnership::Current | ScriptOwnership::Outdated
    ) {
        let _ = std::fs::remove_file(&layout.script);
    }
    // 旧版 cmd 脚本（2026-09-18 弃用）一并清理，但**必须内容匹配已知生成版本**：
    // 只看 `is_file()` 会把用户改写过的 hook.cmd 当作本工具产物删掉（2026-09-20 审查反例）。
    let legacy_script = layout.script.with_file_name(LEGACY_CMD_NAME);
    if std::fs::read_to_string(&legacy_script).is_ok_and(|content| content == LEGACY_CMD_BODY) {
        let _ = std::fs::remove_file(&legacy_script);
    }
    if errors.is_empty() {
        Ok(status_at(layout))
    } else {
        Err(errors.join("；"))
    }
}

pub(super) fn uninstall_target(
    layout: &HookLayout,
    target: &HookTarget,
    marker: &str,
    legacy_markers: &[String],
) -> Result<(), String> {
    let Some(original) = std::fs::read_to_string(&target.settings).ok() else {
        return Ok(());
    };
    let Ok(mut root) = serde_json::from_str::<Value>(&original) else {
        // 损坏的配置不动：宁可留着 marker，也不覆盖用户（或客户端）写坏的内容。
        return Ok(());
    };
    remove_event_entries(&mut root, marker, legacy_markers);
    let cleaned = pretty(&root);

    if let Ok(bytes) = std::fs::read_to_string(layout.backup_path(target)) {
        if serde_json::from_str::<Value>(&bytes).ok().as_ref() == Some(&root) {
            // 语义与备份一致 = 安装之后没有别的改动 → 直接写回原始字节。
            if bytes != original {
                atomic_write(&target.settings, &bytes)
                    .map_err(|error| format!("还原失败：{error}"))?;
            }
            return Ok(());
        }
    } else if layout.absent_mark_path(target).exists() && root == json!({}) {
        // 安装前该配置不存在，卸载后内容为空 → 恢复「不存在」。
        std::fs::remove_file(&target.settings)
            .map_err(|error| format!("删除空配置失败：{error}"))?;
        return Ok(());
    }

    if cleaned != original {
        atomic_write(&target.settings, &cleaned).map_err(|error| format!("写入失败：{error}"))?;
    }
    Ok(())
}
