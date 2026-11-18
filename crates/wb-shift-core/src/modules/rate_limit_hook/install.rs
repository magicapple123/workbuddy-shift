//! 安装：事件条目 upsert、脚本写入、安装目标与备份刷新。

use super::status::*;
use super::*;

/// 取出可变的事件数组；结构不符（`hooks` 不是对象 / 事件不是数组）时报错而不是覆盖。
pub(super) fn event_entries<'a>(
    root: &'a mut Value,
    event: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let Some(object) = root.as_object_mut() else {
        return Err("配置根节点不是 JSON 对象".to_string());
    };
    let hooks = object
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err("`hooks` 不是 JSON 对象".to_string());
    };
    let list = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    list.as_array_mut()
        .ok_or_else(|| format!("`hooks.{event}` 不是数组"))
}

/// 插入（已存在则更新）本工具在某个事件下的条目：先摘掉旧的同源条目，再追加一条。
pub(super) fn upsert_event(
    root: &mut Value,
    event: &str,
    command: &str,
    marker: &str,
    legacy_markers: &[String],
) -> Result<(), String> {
    let entries = event_entries(root, event)?;
    entries.retain(|entry| !entry_is_ours(entry, marker, legacy_markers));
    entries.push(json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": command }],
    }));
    Ok(())
}

/// 移除本工具在全部已注册事件下的条目；空数组 / 空 `hooks` 对象一并摘掉。
pub(super) fn remove_event_entries(root: &mut Value, marker: &str, legacy_markers: &[String]) {
    let Some(object) = root.as_object_mut() else {
        return;
    };
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for event in HOOK_EVENTS {
        let Some(list) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
            continue;
        };
        list.retain(|entry| !entry_is_ours(entry, marker, legacy_markers));
        if list.is_empty() {
            hooks.remove(event);
        }
    }
    if hooks.is_empty() {
        object.remove("hooks");
    }
}

pub(super) fn pretty(root: &Value) -> String {
    serde_json::to_string_pretty(root).unwrap_or_default()
}

/// 安装（幂等）：生成脚本 + 在**存在客户端**的配置里注册 `Stop` / `FinalStop`，写前备份。
///
/// 「接入 hook」同时清除用户的「卸载过」标记（恢复默认接入语义）。
pub fn install_hook() -> Result<Value, String> {
    install_and_clear_opt_out(
        &HookLayout::default_layout(),
        &config::rate_limit_config_file(),
    )
}

/// 启动时的默认接入：`enabled && !hookOptOut && 存在任一客户端 && hook 未装全` 才安装。
///
/// 幂等、非阻塞、失败静默降级（下次启动重试）；返回是否改动了注册状态（调用方据此作废扫描缓存）。
/// 失败原因会落到 `error.log`：过去这里完全静默，模板被误判成「用户改过」时整条 hook 通路
/// 永久装不上、只能靠日志扫描兜底，谁也发现不了（2026-11-17 本机实证，缺 chip 两天）。
pub fn auto_install_on_startup() -> bool {
    let cfg = config::load_rate_limit_config();
    let enabled = cfg.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let opt_out = cfg
        .get("hookOptOut")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    auto_install_reporting(&HookLayout::default_layout(), enabled, opt_out, &|error| {
        crate::modules::error_log::record("backend", "限额 hook 启动安装失败", error);
    })
}

/// [`auto_install_reporting`] 的无上报版本（单测用：不得触碰真实 `~/.workbuddy-shift`）。
#[cfg(test)]
pub(super) fn auto_install_at(layout: &HookLayout, enabled: bool, opt_out: bool) -> bool {
    auto_install_reporting(layout, enabled, opt_out, &|_| {})
}

/// 自动接入的实际实现；`on_error` 只在「决定安装且安装失败」时调用一次。
pub(super) fn auto_install_reporting(
    layout: &HookLayout,
    enabled: bool,
    opt_out: bool,
    on_error: &dyn Fn(&str),
) -> bool {
    if !enabled || opt_out || !layout.any_target_exists() || layout.fully_installed() {
        return false;
    }
    match install_at(layout) {
        Ok(_) => true,
        Err(error) => {
            on_error(&error);
            // 部分目标失败时，只要有一处已经注册，扫描范围就已经变了。
            layout.registered_target_count() > 0
        }
    }
}

pub(super) fn install_at(layout: &HookLayout) -> Result<Value, String> {
    // 一处客户端都没装：什么都不做（不建目录、不写配置、不生成脚本），不是错误。
    if !layout.any_target_exists() {
        return Ok(status_at(layout));
    }
    write_script(layout)?;
    let command = hook_command(&layout.script);
    let marker = layout.marker();
    let legacy_markers = layout.legacy_markers();
    let mut errors = Vec::new();
    for target in &layout.targets {
        // 只对**存在**的客户端写配置：不存在的数据根不创建目录 / 空 settings.json。
        if !target.exists() {
            continue;
        }
        if let Err(error) = install_target(layout, target, &command, &marker, &legacy_markers) {
            errors.push(format!("{}：{error}", target.settings.display()));
        }
    }
    if errors.is_empty() {
        Ok(status_at(layout))
    } else {
        Err(errors.join("；"))
    }
}

/// 写入 / 升级 hook 脚本。
///
/// - 已是当前模板 → 不写（避免重复安装改 mtime）；
/// - 是本工具的已知旧模板 → 升级重写；
/// - 内容未知（用户改过）→ **不覆盖**，返回错误让调用方明确报出。
pub(super) fn write_script(layout: &HookLayout) -> Result<(), String> {
    match layout.script_ownership() {
        ScriptOwnership::Current => return Ok(()),
        ScriptOwnership::Foreign => {
            return Err(format!(
                "{} 不是本工具生成的脚本（可能已被手动修改），已保留原文件、未覆盖",
                layout.script.display()
            ));
        }
        ScriptOwnership::Absent | ScriptOwnership::Outdated => {}
    }
    if let Some(parent) = layout.script.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    }
    std::fs::write(&layout.script, script_body(&layout.events))
        .map_err(|error| format!("写入 {} 失败：{error}", layout.script.display()))
}

pub(super) fn install_target(
    layout: &HookLayout,
    target: &HookTarget,
    command: &str,
    marker: &str,
    legacy_markers: &[String],
) -> Result<(), String> {
    let original = std::fs::read_to_string(&target.settings).ok();
    let current = match &original {
        Some(text) => Some(
            serde_json::from_str::<Value>(text)
                .map_err(|_| "不是合法 JSON，已跳过（未做任何改动）".to_string())?,
        ),
        None => None,
    };
    // 备份基线：本次安装「除了本工具条目之外」的内容。
    let baseline = match &current {
        Some(root) => {
            let mut clean = root.clone();
            remove_event_entries(&mut clean, marker, legacy_markers);
            clean
        }
        None => json!({}),
    };
    refresh_backup(
        layout,
        target,
        original.as_deref(),
        &baseline,
        marker,
        legacy_markers,
    )?;

    let mut root = current.unwrap_or_else(|| json!({}));
    for event in HOOK_EVENTS {
        upsert_event(&mut root, event, command, marker, legacy_markers)?;
    }
    let content = pretty(&root);
    if original.as_deref() == Some(content.as_str()) {
        return Ok(());
    }
    // 数据根已存在（`install_at` 的调用前提），这里不再为「不存在的客户端」补建目录。
    atomic_write(&target.settings, &content).map_err(|error| format!("写入失败：{error}"))?;
    Ok(())
}

/// 维护备份基线：已有且语义一致就保留；否则记录「当前减去本工具条目」的形态。
///
/// 首次安装（原文件没有我们的 marker）时基线取**原文件原始字节**，卸载可直接逐字节还原。
pub(super) fn refresh_backup(
    layout: &HookLayout,
    target: &HookTarget,
    original: Option<&str>,
    baseline: &Value,
    marker: &str,
    legacy_markers: &[String],
) -> Result<(), String> {
    let backup = layout.backup_path(target);
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    }
    let absent = layout.absent_mark_path(target);
    match original {
        None => {
            if !backup.exists() && !absent.exists() {
                std::fs::write(&absent, "")
                    .map_err(|error| format!("写入备份标记失败：{error}"))?;
            }
        }
        Some(text) => {
            let raw_is_clean = !text.contains(marker)
                && !legacy_markers
                    .iter()
                    .any(|legacy| text.contains(legacy.as_str()));
            if let Ok(existing) = std::fs::read_to_string(&backup) {
                let same = serde_json::from_str::<Value>(&existing).ok().as_ref() == Some(baseline);
                if same {
                    return Ok(());
                }
            }
            let content = if raw_is_clean {
                text.to_string()
            } else {
                pretty(baseline)
            };
            std::fs::write(&backup, content).map_err(|error| format!("写入备份失败：{error}"))?;
        }
    }
    Ok(())
}

/// 「接入 hook」：安装 + 清除 `hookOptOut`。
///
/// 用户意图先于结果记录：即便本次安装部分失败，也认为用户已重新接受默认接入。
pub(super) fn install_and_clear_opt_out(
    layout: &HookLayout,
    config_path: &Path,
) -> Result<Value, String> {
    let result = install_at(layout);
    let _ = config::set_rate_limit_hook_opt_out_at(config_path, false);
    result
}
