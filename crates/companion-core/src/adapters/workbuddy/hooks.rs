//! WorkBuddy hook 配置合并、版本识别与设置文件定位。

use super::*;

pub const WORKBUDDY_HOOK_EVENTS: [&str; 10] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "Notification",
    "PreCompact",
];

pub(crate) fn settings_json(dir: PathBuf) -> PathBuf {
    dir.join("settings.json")
}

pub(crate) fn looks_like_workbuddy_home(dir: &Path) -> bool {
    dir.is_dir() && (dir.join("settings.json").exists() || dir.join("workbuddy.db").exists())
}

/// International WorkBuddy AI uses `~/.workbuddy-ai`; the China desktop app uses `~/.workbuddy`.
/// Both are live products, so hooks are installed independently when that edition is present.
pub fn workbuddy_settings_files(home: &Path, custom: &str) -> Vec<PathBuf> {
    if !custom.is_empty() {
        let dir = if let Some(rest) = custom.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(custom)
        };
        return if dir.is_dir() {
            vec![settings_json(dir)]
        } else {
            Vec::new()
        };
    }
    let mut files = Vec::new();
    let international = home.join(".workbuddy-ai");
    if international.is_dir() || international.join("settings.json").exists() {
        files.push(settings_json(international));
    }
    let domestic = home.join(".workbuddy");
    // A bare `~/.workbuddy` may only hold toolchain binaries for the international app.
    if looks_like_workbuddy_home(&domestic) {
        files.push(settings_json(domestic));
    }
    files
}

pub fn workbuddy_edition(settings_file: &Path) -> &'static str {
    match settings_file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(".workbuddy-ai") => "international",
        _ => "domestic",
    }
}

pub fn is_workbuddy_hook_command(command: &str) -> bool {
    command.contains("workbuddy-status.py")
        || (command.contains("companion-runtime")
            && command.contains(" hook")
            && command.contains("--source workbuddy"))
        || crate::adapters::legacy_hook_command(command)
}

pub fn merge_workbuddy_hooks(mut doc: Value, command: &str) -> Result<Value, String> {
    if !doc.is_object() {
        return Err("现有 WorkBuddy Hook 配置无效".into());
    }
    if doc["hooks"].is_null() {
        doc["hooks"] = json!({});
    }
    if !doc["hooks"].is_object() {
        return Err("现有 WorkBuddy Hook 配置无效".into());
    }
    for event in WORKBUDDY_HOOK_EVENTS {
        let mut groups = doc["hooks"][event].as_array().cloned().unwrap_or_default();
        for group in &mut groups {
            if let Some(hooks) = group["hooks"].as_array_mut() {
                hooks.retain(|h| !is_workbuddy_hook_command(&text(&h["command"])));
            } else if group.get("command").is_some()
                && is_workbuddy_hook_command(&text(&group["command"]))
            {
                *group = json!({"hooks": []});
            }
        }
        groups.retain(|g| {
            if g["hooks"].is_array() {
                !g["hooks"].as_array().unwrap().is_empty()
            } else {
                g.get("command")
                    .map(|c| !is_workbuddy_hook_command(&text(c)))
                    .unwrap_or(true)
            }
        });
        let mut h = json!({"type":"command","command":command,"timeout":3,"statusMessage":"WorkBuddy Shift"});
        if event != "SessionEnd" {
            h["async"] = json!(true);
        }
        groups.push(json!({"matcher":"","hooks":[h]}));
        doc["hooks"][event] = json!(groups);
    }
    Ok(doc)
}
