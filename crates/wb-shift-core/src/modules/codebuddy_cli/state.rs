//! CLI 路径与会话状态：状态文件、会话注册表心跳、账号索引与 status。

use super::helper::*;
use super::settings::*;
use super::*;

pub(super) fn rotate_dir() -> PathBuf {
    home_dir().join(ROTATE_DIR)
}

pub(super) fn state_path() -> PathBuf {
    rotate_dir().join(STATE_FILE)
}

/// CLI 会话注册表目录（`~/.codebuddy/sessions`）。
///
/// **唯一**拼接点：轮换的存活门控与限额归因都从这里取路径，不要再各写一份
/// `home_dir().join(".codebuddy").join("sessions")`。
pub(crate) fn sessions_dir() -> PathBuf {
    home_dir().join(SETTINGS_DIR).join(SESSIONS_DIR_NAME)
}

/// 注册表文件名形态（对齐客户端 `PID_FILE_PATTERN`）：`<pid>.json` 或 `manual-*.json`。
///
/// 客户端还会在该目录放别的东西，只有这两种形态才是会话记录。
pub(super) fn is_session_registry_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    if stem.chars().all(|character| character.is_ascii_digit()) && !stem.is_empty() {
        return true;
    }
    stem.strip_prefix("manual-")
        .is_some_and(|rest| !rest.is_empty())
}

/// 单份注册表是否心跳新鲜；缺 `lastHeartbeat` / 类型不符 → false（跳过）。
///
/// `lastHeartbeat` 在未来（时钟回拨、客户端时钟偏差）时 `now - hb` 为负，仍然算活着 ——
/// 保守侧：宁可不切，也不要关掉一个可能活着的进程。
pub(super) fn heartbeat_is_live(value: &Value, now_ms: i64) -> bool {
    value
        .get("lastHeartbeat")
        .and_then(Value::as_i64)
        .is_some_and(|heartbeat| now_ms - heartbeat <= LIVE_SESSION_STALE_MS)
}

/// `dir` 下是否存在心跳新鲜的 CLI 会话；`now_ms` 由调用方给定（便于单测与复用同一时刻）。
///
/// - 只认 `^(\d+|manual-.+)\.json$`；坏 JSON / 缺 `lastHeartbeat` 的文件跳过；
/// - 目录不存在 → `false`（不是错误，未接入 CLI 的机器就是这种形态）；
/// - 目录存在但读不了 → `eprintln!` 告警后 `false`：**不能**因为读不到就永久卡住轮换，
///   真出现"旧进程还活着"时由归因侧的丢弃兜底。
///
/// 刻意**不**做 `kill(pid, 0)` / `ps` 探测：客户端自己会清理死进程的注册表文件，
/// 心跳过期已经足够，另行探测只会在跨平台用户权限差异上引入新的失败面。
pub(super) fn has_live_session_in(dir: &Path, now_ms: i64) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "[codebuddy-cli] 无法读取会话注册表目录 {}：{error}；本次按无会话处理",
                dir.display()
            );
            return false;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_session_registry_file)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if heartbeat_is_live(&value, now_ms) {
            return true;
        }
    }
    false
}

/// 是否存在活着的 CodeBuddy CLI 会话（`~/.codebuddy/sessions/*.json`，心跳新鲜）。
///
/// 自动轮换的存活门控用这个判据：只要返回 true 就不切账号（活进程持的是旧 key，
/// 切了也不生效，还会破坏「活进程 key == 当前账号」的不变式）。
pub fn has_live_session(now_ms: i64) -> bool {
    has_live_session_in(&sessions_dir(), now_ms)
}

pub(super) fn load_state() -> Value {
    read_json_file(&state_path())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

pub(super) fn account_index(accounts: &[Value], account_id: &str) -> Option<(usize, String)> {
    accounts.iter().enumerate().find_map(|(index, account)| {
        let matches = account.get("id").and_then(Value::as_str) == Some(account_id)
            || account.get("uid").and_then(Value::as_str) == Some(account_id);
        if !matches {
            return None;
        }
        let canonical_id = account
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| account_id.to_string());
        Some((index, canonical_id))
    })
}

pub(super) fn state_account_index(state: &Value, accounts: &[Value]) -> Option<(usize, String)> {
    if accounts.is_empty() {
        return None;
    }
    if let Some(active_id) = state.get("activeAccountId").and_then(Value::as_str) {
        if let Some(found) = account_index(accounts, active_id) {
            return Some(found);
        }
    }

    let index = state
        .get("active")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .rem_euclid(accounts.len() as i64) as usize;
    let account = accounts.get(index)?;
    let id = account.get("id").and_then(Value::as_str)?.to_string();
    Some((index, id))
}

pub(super) fn account_index_by_token(accounts: &[Value], token: &str) -> Option<(usize, String)> {
    let expected = clean_bearer_token(token);
    if expected.is_empty() {
        return None;
    }
    accounts.iter().enumerate().find_map(|(index, account)| {
        let actual = account
            .get("access_token")
            .and_then(Value::as_str)
            .map(clean_bearer_token)?;
        if actual != expected {
            return None;
        }
        let id = account.get("id").and_then(Value::as_str)?.to_string();
        Some((index, id))
    })
}

/// 返回脱敏的 CLI 轮换状态，不返回 token 或 helper 内容。
pub fn status() -> Value {
    let accounts = account::load_accounts();
    let state = load_state();
    let settings = read_json_file(&settings_path()).unwrap_or_else(|| json!({}));
    let env_token = settings_env_token(&settings);
    let env_configured = env_token.is_some();
    let environment_override = cfg!(windows) && process_env_token_present();
    let active = if cfg!(windows) {
        env_token.and_then(|token| account_index_by_token(&accounts, token))
    } else {
        state_account_index(&state, &accounts)
    };
    let expected_active = state_account_index(&state, &accounts);
    let configured = if cfg!(windows) {
        env_configured && !environment_override
    } else {
        helper_is_configured()
    };
    let migration_required = if cfg!(windows) {
        environment_override || (!env_configured && helper_is_configured())
    } else {
        helper_migration_required()
    };
    // 脱节判定拆成两个互斥状态：真脱节 vs 保活刷新中的正常中间态。
    // 后者若也报脱节，用户会在每次启动看到「认证脱节」并反复点按钮。
    let (sync_pending, sync_in_progress) = classify_sync_state(
        cfg!(windows),
        env_configured,
        active.is_none(),
        expected_active.is_some(),
        account_store_written_within(ACCOUNTS_SYNC_GRACE_MS),
    );
    json!({
        "configured": configured,
        "authMode": if cfg!(windows) { "settings-env" } else { "api-key-helper" },
        "environmentOverride": environment_override,
        "helperCurrent": if cfg!(windows) { env_configured } else { helper_is_current() },
        "migrationRequired": migration_required,
        "syncPending": sync_pending,
        "syncInProgress": sync_in_progress,
        "settingsPresent": settings_path().is_file(),
        "helperPresent": helper_path().map(|path| path.is_file()).unwrap_or(false),
        "helperSupportsAccountIds": helper_supports_account_ids(),
        "activeIndex": active.as_ref().map(|(index, _)| *index),
        "activeAccountId": active.as_ref().map(|(_, id)| id),
        "activeAccountName": active.as_ref().and_then(|(_, id)| account::find_account(id).map(|account| account::account_display_name(&account))),
        "activeAccountVariant": active.as_ref().and_then(|(index, _)| accounts.get(*index)).map(WbVariant::from_account).map(WbVariant::as_str),
        "accountCount": accounts.len(),
        "statePath": state_path().to_string_lossy(),
    })
}
