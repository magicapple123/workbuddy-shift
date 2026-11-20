//! `switch` 子命令：预览 / 执行 CodeBuddy CLI 换号。
//!
//! 安全边界（R5-10 决策）：
//!
//! - **有活跃 CLI 会话时默认拒绝**（退出码 3）：活进程持的是旧 key，切了也不生效，
//!   还会破坏「活进程 key == 当前账号」的不变式。要切必须显式 `--force`，
//!   而 `--force` 会关闭 CLI 进程，**当前会话会中断且不自动重开**。
//! - `--dry-run` 只输出「将会发生什么」，**不写任何状态**（不关进程、不写 state.json、
//!   不改 settings.json），退出码恒为 0，由 `wouldRefuse` 字段说明是否会被拒绝。
//! - 输出不含 token：只用 `id` / `name`。

use serde_json::{json, Value};
use std::path::PathBuf;

use wb_shift_core::modules::codebuddy_cli;

use super::*;

/// CLI 会话存活判据（毫秒），与 core `codebuddy_cli` 的 `LIVE_SESSION_STALE_MS` 对齐。
///
/// core 只导出了布尔版 [`codebuddy_cli::has_live_session`]，没有会话清单导出；
/// `--dry-run` 要列出进程，只能按同一判据重读 `~/.codebuddy/sessions/*.json`。
/// **拒绝判定仍以 core 的 `has_live_session` 为准**，本文件的清单只用于展示。
/// （建议后续由 core 导出 `live_sessions()`，见 NOTES 的遗留项。）
const LIVE_SESSION_STALE_MS: i64 = 120_000;

/// `switch <id> [--dry-run] [--force]`。
pub(crate) fn switch_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json", "dry-run", "force"]) {
        return code;
    }
    let dry_run = options.has("dry-run");
    let force = options.has("force");

    let Some(requested) = options.positional().first().cloned() else {
        return emit_error(
            "switch 需要账号 id：workbuddy-shift switch <id> [--dry-run] [--force]",
            EXIT_USAGE,
        );
    };
    if options.positional().len() > 1 {
        return emit_error(
            &format!(
                "switch 只接受一个账号 id，多收到：{}",
                options.positional()[1..].join(" ")
            ),
            EXIT_USAGE,
        );
    }

    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let Some(target) = accounts
        .iter()
        .find(|account| matches_account(account, &requested))
    else {
        return emit_error(
            &format!(
                "账号不存在：{requested}（账号池 {} 个账号）",
                accounts.len()
            ),
            EXIT_NOT_FOUND,
        );
    };

    let target_id = account_id_of(target);
    let target_entry = json!({
        "id": target_id,
        "name": account::account_display_name(target),
    });
    let current_id = cli_current_id();
    let current_account = current_id.as_deref().and_then(|current| {
        accounts
            .iter()
            .find(|account| matches_account(account, current))
    });
    let current_entry = current_account
        .map(|account| {
            json!({
                "id": account_id_of(account),
                "name": account::account_display_name(account),
            })
        })
        .unwrap_or(Value::Null);
    let already_current = current_id
        .as_deref()
        .is_some_and(|current| matches_account(target, current));

    let now = config::now_ms();
    let live = live_sessions(now);
    // 拒绝判定用 core 的布尔门控（权威）；清单只用于展示"会影响哪些进程"。
    let running = codebuddy_cli::has_live_session(now);
    let would_refuse = running && !force;

    if dry_run {
        progress(&format!(
            "[switch] dry-run：目标 {}，当前 CLI 会话 {} 个（不写入任何状态）",
            target_id,
            live.len()
        ));
        return emit(
            &json!({
                "schemaVersion": SCHEMA_VERSION,
                "dryRun": true,
                "ok": true,
                "target": target_entry,
                "current": current_entry,
                "liveSessions": live,
                "willClose": live.len(),
                "running": running,
                "alreadyCurrent": already_current,
                "wouldRefuse": would_refuse,
                "reason": if would_refuse {
                    Value::String("有正在运行的 CodeBuddy CLI 会话；实际切号会被拒绝（需 --force）".to_string())
                } else {
                    Value::Null
                },
            }),
            EXIT_OK,
        );
    }

    if would_refuse {
        let count = live.len().max(1);
        progress(&format!(
            "[switch] 有 {count} 个 CodeBuddy CLI 会话在运行，等待任务结束或加 --force（会中断当前会话且不自动重开）"
        ));
        return emit(
            &json!({
                "schemaVersion": SCHEMA_VERSION,
                "dryRun": false,
                "ok": false,
                "refused": "live-session",
                "target": target_entry,
                "previous": current_entry,
                "liveSessions": live,
                "willClose": live.len(),
                "message": format!(
                    "有 {count} 个 CodeBuddy CLI 会话在运行；等任务结束后再切，或加 --force（当前会话会中断且不自动重开）"
                ),
            }),
            EXIT_REFUSED,
        );
    }

    // 已经是当前账号：不重复写 state / settings（避免无意义地关一次 CLI）。
    // `--force` 时仍然执行，作为"配置被改坏后重新套用"的修复通道。
    if already_current && !force {
        return emit(
            &json!({
                "schemaVersion": SCHEMA_VERSION,
                "dryRun": false,
                "ok": true,
                "alreadyCurrent": true,
                "previous": current_entry,
                "target": target_entry,
                "closed": 0,
                "message": "已经是当前 CodeBuddy CLI 账号，未做任何改动",
            }),
            EXIT_OK,
        );
    }

    progress(&format!("[switch] 正在切换到 {target_id} …"));
    match codebuddy_cli::switch_active_account(&target_id) {
        Ok(result) => emit(
            &json!({
                "schemaVersion": SCHEMA_VERSION,
                "dryRun": false,
                "ok": true,
                "alreadyCurrent": already_current,
                "previous": current_entry,
                "target": target_entry,
                "closed": result
                    .get("closedProcessCount")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                "cliClosed": result.get("cliClosed").cloned().unwrap_or(Value::Null),
                "regionChanged": result.get("regionChanged").cloned().unwrap_or(Value::Null),
                "message": result.get("message").cloned().unwrap_or(Value::Null),
            }),
            EXIT_OK,
        ),
        Err(error) => {
            let code = if error.contains("账号不存在") {
                EXIT_NOT_FOUND
            } else {
                EXIT_FATAL
            };
            emit_error(&error, code)
        }
    }
}

/// CLI 会话注册表目录（`~/.codebuddy/sessions`）。
fn sessions_dir() -> PathBuf {
    config::home_dir().join(".codebuddy").join("sessions")
}

/// 注册表文件名是否形态合法，合法时返回文件名主体。
///
/// 与 core 的 `is_session_registry_file` 同口径：`<pid>.json` 或 `manual-*.json`；
/// 该目录里客户端还会放别的东西，只有这两种形态才是会话记录。
pub(super) fn registry_stem(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".json")?;
    if stem.is_empty() {
        return None;
    }
    if stem.chars().all(|character| character.is_ascii_digit()) {
        return Some(stem);
    }
    if stem
        .strip_prefix("manual-")
        .is_some_and(|rest| !rest.is_empty())
    {
        return Some(stem);
    }
    None
}

/// 心跳新鲜的 CLI 会话清单（按 pid 升序）；读不到目录时返回空列表。
///
/// 只读：不探测 `kill(pid, 0)`、不清理过期注册表。
pub(super) fn live_sessions(now_ms: i64) -> Vec<Value> {
    let dir = sessions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut live: Vec<Value> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(stem) = registry_stem(name) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(heartbeat) = value.get("lastHeartbeat").and_then(Value::as_i64) else {
            continue;
        };
        // 心跳在未来（时钟回拨）时 now - hb 为负，仍算活着：宁可报告"有会话"也不误判为无。
        if now_ms - heartbeat > LIVE_SESSION_STALE_MS {
            continue;
        }
        let pid = stem
            .parse::<i64>()
            .ok()
            .or_else(|| value.get("pid").and_then(Value::as_i64));
        live.push(json!({
            "pid": pid,
            "startedAt": first_number(&value, &["startedAt", "startTime", "createdAt"]),
        }));
    }
    live.sort_by_key(|item| item.get("pid").and_then(Value::as_i64).unwrap_or(0));
    live
}

/// 依次尝试多个字段名取第一个整数（注册表里启动时刻的字段名跨版本不统一）。
fn first_number(value: &Value, keys: &[&str]) -> Value {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
        .map(|found| json!(found))
        .unwrap_or(Value::Null)
}
