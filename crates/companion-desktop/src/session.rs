//! 会话链接唤起：URL 解析、VS Code 历史桶解析与平台唤起按职责拆分。

mod open;
mod url;
#[cfg(any(target_os = "macos", test))]
mod vscode_bucket;

use url::session_target;

#[tauri::command]
pub async fn open_session_url(url: String) -> Result<(), String> {
    let target = session_target(&url)?;
    tauri::async_runtime::spawn_blocking(move || open::open_session_target(target))
        .await
        .map_err(|e| e.to_string())?
}
