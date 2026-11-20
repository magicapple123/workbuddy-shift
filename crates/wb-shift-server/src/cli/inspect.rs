//! `inspect` 子命令：一次拿到「整池体检」或「单个账号/会话」的只读排查信息。
//!
//! 两种形态：
//!
//! - `inspect --summary`（默认）：账号 + 积分 + 限额一次返回，供主控 AI 选号。
//! - `inspect <id>`：`id` 先按账号 `id`/`uid` 匹配，匹配不到再按会话 id 在账号池里
//!   反查归属账号。只读，不改任何状态。
//!
//! 任何形态都**不写**本地状态（`--no-credits` 之外唯一的网络活动是积分实时查询，
//! 其副作用与 GUI 查积分完全一致）。

use serde_json::{json, Value};

use wb_shift_core::modules::session;

use super::*;

/// `inspect [--summary] [<账号|会话 id>] [--cached] [--no-credits] [--timeout 秒]`。
pub(crate) async fn inspect_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json", "summary", "cached", "no-credits"]) {
        return code;
    }
    let credits_options = match credits::CreditsOptions::from_options(options) {
        Ok(options) => options,
        Err(error) => return emit_error(&error, EXIT_USAGE),
    };
    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let target = options.positional().first().cloned();
    match target {
        Some(target) if !options.has("summary") => {
            inspect_target(&target, &accounts, &credits_options).await
        }
        _ => inspect_summary(&accounts, options, &credits_options).await,
    }
}

/// `inspect --summary`：整池快照。
async fn inspect_summary(
    accounts: &[Value],
    options: &Options,
    credits_options: &credits::CreditsOptions,
) -> i32 {
    let ledger = limits::get_rate_limits();
    let limit_map = limits_by_account(&ledger);
    let cli_current = cli_current_id();

    // `--no-credits` 跳过网络：此时每个账号的 `credits` 是 `null`，`partial` 保持 false
    // （没有发起查询，不算「不完整」）。
    let credit_entries = if options.has("no-credits") {
        None
    } else {
        progress("[inspect] 汇总账号池 …");
        Some(credits::entries(accounts, credits_options).await)
    };
    let (credit_list, partial) = match credit_entries {
        Some((entries, partial)) => (Some(entries), partial),
        None => (None, false),
    };

    let items: Vec<Value> = accounts
        .iter()
        .enumerate()
        .map(|(index, account)| {
            let mut entry = account_entry(account, cli_current.as_deref());
            let account_limits = limit_map
                .get(&account_id_of(account))
                .cloned()
                .unwrap_or_default();
            let account_credits = credit_list
                .as_ref()
                .and_then(|entries| entries.get(index))
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(map) = entry.as_object_mut() {
                map.insert("credits".to_string(), account_credits);
                map.insert("limits".to_string(), Value::Array(account_limits));
            }
            entry
        })
        .collect();

    emit(
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "generatedAt": config::now_ms(),
            "partial": partial,
            "current": cli_current.map(Value::String).unwrap_or(Value::Null),
            "scannedAt": ledger.get("scannedAt").cloned().unwrap_or(Value::Null),
            "windowDays": ledger.get("windowDays").cloned().unwrap_or(Value::Null),
            "accounts": items,
        }),
        EXIT_OK,
    )
}

/// `inspect <id>`：单账号（或会话归属账号）的排查视图。
async fn inspect_target(
    target: &str,
    accounts: &[Value],
    credits_options: &credits::CreditsOptions,
) -> i32 {
    let cli_current = cli_current_id();
    let ledger = limits::get_rate_limits();
    let limit_map = limits_by_account(&ledger);

    if let Some(account) = accounts
        .iter()
        .find(|account| matches_account(account, target))
    {
        let (credit_entries, partial) =
            credits::entries(std::slice::from_ref(account), credits_options).await;
        let owner_id = account_id_of(account);
        let mut entry = account_entry(account, cli_current.as_deref());
        if let Some(map) = entry.as_object_mut() {
            map.insert(
                "credits".to_string(),
                credit_entries.first().cloned().unwrap_or(Value::Null),
            );
            map.insert(
                "limits".to_string(),
                Value::Array(limit_map.get(&owner_id).cloned().unwrap_or_default()),
            );
        }
        return emit(
            &json!({
                "schemaVersion": SCHEMA_VERSION,
                "kind": "account",
                "generatedAt": config::now_ms(),
                "partial": partial,
                "current": cli_current.map(Value::String).unwrap_or(Value::Null),
                "scannedAt": ledger.get("scannedAt").cloned().unwrap_or(Value::Null),
                "account": entry,
            }),
            EXIT_OK,
        );
    }

    // 账号里没有：按会话 id 在账号池里反查归属账号（只读 SQLite 查询）。
    match find_session_owner(target, accounts) {
        Some((index, found)) => {
            let account = accounts.get(index).cloned().unwrap_or_else(|| json!({}));
            let owner_id = account_id_of(&account);
            emit(
                &json!({
                    "schemaVersion": SCHEMA_VERSION,
                    "kind": "session",
                    "generatedAt": config::now_ms(),
                    "session": found,
                    "account": account_entry(&account, cli_current.as_deref()),
                    "limits": limit_map.get(&owner_id).cloned().unwrap_or_default(),
                }),
                EXIT_OK,
            )
        }
        None => emit_error(
            &format!(
                "未找到账号或会话「{target}」（账号池 {} 个账号）",
                accounts.len()
            ),
            EXIT_NOT_FOUND,
        ),
    }
}

/// 在账号池里找持有该会话 id 的账号，返回 `(账号下标, 会话条目)`。
///
/// 会话正文在 `~/.workbuddy*/workbuddy.db`，这里只读 core 的列表接口，
/// 不碰会话文件，也不写库。
fn find_session_owner(target: &str, accounts: &[Value]) -> Option<(usize, Value)> {
    for (index, account) in accounts.iter().enumerate() {
        let listed = session::list_sessions_for_account(account);
        let Some(items) = listed.get("sessions").and_then(Value::as_array) else {
            continue;
        };
        if let Some(found) = items
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(target))
        {
            return Some((index, found.clone()));
        }
    }
    None
}
