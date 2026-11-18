//! 状态/日志：rotate_status 与 rotate_logs。

use super::*;

/// 轮换状态（配置 + 上次检查/切换 + 当前 CLI 账号），供前端展示。
pub fn rotate_status() -> Value {
    let cfg = load_auto_rotate_config();
    let cli_status = codebuddy_cli::status();
    let last_check = LAST_CHECK_AT.load(Ordering::SeqCst);
    let last_switch = LAST_SWITCH_AT.load(Ordering::SeqCst);
    json!({
        "config": cfg,
        "cliConfigured": cli_status.get("configured").cloned().unwrap_or(json!(false)),
        "activeAccountId": cli_status.get("activeAccountId").cloned().unwrap_or_default(),
        "activeAccountName": cli_status.get("activeAccountName").cloned().unwrap_or_default(),
        "lastCheckAt": (last_check > 0).then_some(last_check),
        "lastSwitchAt": (last_switch > 0).then_some(last_switch),
    })
}

/// 最近轮换日志（新→旧）。
pub fn rotate_logs() -> Vec<Value> {
    let mut logs = load_rotate_logs();
    logs.reverse();
    logs
}
