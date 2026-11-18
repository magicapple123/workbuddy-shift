//! 悬浮窗偏好设置与登录自启入口；平台自启实现见 `autostart` 子模块。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{Emitter, Manager};

mod autostart;

use autostart::{heal_login, login_state, managed, set_login, supported};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub avatar_style: String,
    pub visible_count: u8,
    pub animation: bool,
    #[serde(default = "default_size")]
    pub size: String,
}
fn default_size() -> String {
    "standard".into()
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            avatar_style: "animal".into(),
            visible_count: 8,
            animation: true,
            size: default_size(),
        }
    }
}
impl Preferences {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.avatar_style.as_str(), "animal" | "bot")
            || !(3..=16).contains(&self.visible_count)
            || !matches!(self.size.as_str(), "small" | "medium" | "standard")
        {
            return Err("悬浮窗设置无效".into());
        }
        Ok(())
    }
}
fn path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("rail-settings.json"))
}

fn response(app: &tauri::AppHandle, preferences: Preferences) -> Result<Value, String> {
    let mut value = serde_json::to_value(preferences).map_err(|e| e.to_string())?;
    value["autostartSupported"] = json!(supported(app));
    // Embedded hosts own their own login item, so the settings page hides the row.
    value["autostartManaged"] = json!(managed(app));
    value["autostart"] = json!(login_state(app)?);
    Ok(value)
}
#[tauri::command]
pub fn rail_settings_get(app: tauri::AppHandle) -> Result<Value, String> {
    heal_login(&app);
    let preferences = match std::fs::read(path(&app)?) {
        Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
        Err(e) => return Err(e.to_string()),
    };
    preferences.validate()?;
    response(&app, preferences)
}
#[tauri::command]
pub fn rail_settings_set(
    app: tauri::AppHandle,
    preferences: Preferences,
    autostart: bool,
) -> Result<Value, String> {
    preferences.validate()?;
    if autostart != login_state(&app)? {
        if !supported(&app) {
            return Err("当前运行环境不支持开机自启".into());
        }
        set_login(&app, autostart)?;
    }
    let file = path(&app)?;
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = file.with_extension("tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&preferences).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, file).map_err(|e| e.to_string())?;
    let value = response(&app, preferences)?;
    app.emit("companion-rail-settings", &value)
        .map_err(|e| e.to_string())?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_preferences_keep_the_existing_size() {
        let preferences: Preferences = serde_json::from_value(
            json!({"avatarStyle":"animal","visibleCount":8,"animation":true}),
        )
        .unwrap();
        assert_eq!(preferences.size, "standard");
        assert!(preferences.validate().is_ok());
    }
}
