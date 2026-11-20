//! AI 友好 CLI 子命令（R5-10，上游 issue #180）。
//!
//! 面向「主控 AI 直接调用命令行」的场景：一次拿到整个账号池的账号、积分、限额与
//! 活跃会话，自行决定等任务结束后切号。约定（与 GUI 无关，只服务 CLI）：
//!
//! - **stdout 只输出 JSON**：单行紧凑 JSON，可直接 `JSON.parse` / `jq` 解析；
//!   进度与警告一律走 stderr（`[credits] 3/12 …`）。
//! - 所有输出都带 `schemaVersion: 1`，字段变化时递增。
//! - 字段一律经 [`account::account_meta`] 或显式白名单读取，
//!   **禁止**出现 `access_token` / `refresh_token` / `auth_raw`。
//! - 退出码见 [`EXIT_OK`] / [`EXIT_NOT_FOUND`] / [`EXIT_USAGE`] / [`EXIT_REFUSED`] /
//!   [`EXIT_FATAL`]。
//!
//! 子命令分布：`accounts` / `limits` / `status` 与共享 helper 在本文件，
//! 网络查询在 [`credits`]，只读排查在 [`inspect`]，切号在 [`switch`]。
//!
//! 只读边界：`accounts` / `limits` / `status` / `inspect` 不写任何本地状态。
//! `credits` 实时查询复用 core 的 [`wb_shift_core::modules::credits::get_credit_expiry`]，
//! 该函数会把观察值追加到 `credit_usage` 快照账本（与 GUI 查积分同一条路径，
//! 也是 `--cached` 的数据来源），除此之外不写账号库。

use serde_json::{json, Value};
use std::collections::HashMap;

use wb_shift_core::modules::{account, codebuddy_cli, config, limits};

pub(crate) mod credits;
mod inspect;
mod switch;

#[cfg(test)]
mod tests;

/// 输出契约版本号。
pub(crate) const SCHEMA_VERSION: i64 = 1;

/// 成功（含 `partial: true`）。
pub(crate) const EXIT_OK: i32 = 0;
/// 目标不存在（`switch` / `inspect` 给的 id 在账号池与会话里都找不到）。
pub(crate) const EXIT_NOT_FOUND: i32 = 1;
/// 用法错误（未知子命令 / 未知开关 / 缺少参数 / 参数值非法）。
pub(crate) const EXIT_USAGE: i32 = 2;
/// 拒绝执行（有活跃 CLI 会话时的切号请求）。
pub(crate) const EXIT_REFUSED: i32 = 3;
/// 致命错误（账号库读不出 / 切号执行失败等）。
pub(crate) const EXIT_FATAL: i32 = 4;

/// `credits` 实时查询的整体时间预算（秒）：验收要求 12 个账号 ≤ 8s。
pub(crate) const DEFAULT_CREDITS_TIMEOUT_SECS: u64 = 8;
/// 实时查询的并发上限。账号多时不要为了提高覆盖率无限加并发——官方接口会限流。
pub(crate) const MAX_CREDITS_CONCURRENCY: usize = 4;
/// `--timeout` 上限：拦住把 CLI 挂死几十分钟的误用。
pub(crate) const MAX_CREDITS_TIMEOUT_SECS: u64 = 120;

/// 需要吃一个值的选项名（其余 `--x` 一律按无值开关处理）。
///
/// `variant` / `port` 是既有的全局选项，写在这里只是为了不让它们的值被当成位置参数。
const VALUE_OPTIONS: [&str; 3] = ["timeout", "variant", "port"];

/// 子命令参数解析结果：位置参数 + 无值开关 + 有值选项。
///
/// 只解析子命令**之后**的参数（`main` 用 `subcommand_index` 切好再传进来），
/// 不重复实现全局选项的跳过规则。
pub(crate) struct Options {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    positional: Vec<String>,
}

impl Options {
    pub(crate) fn parse(tokens: &[String]) -> Self {
        let mut values: Vec<(String, String)> = Vec::new();
        let mut flags: Vec<String> = Vec::new();
        let mut positional: Vec<String> = Vec::new();
        let mut index = 0;
        while let Some(token) = tokens.get(index) {
            let Some(name) = token.strip_prefix("--") else {
                if token.starts_with('-') && token.len() > 1 {
                    // 短选项（`-h` 之类）按无值开关处理：本 CLI 没有短选项语义。
                    flags.push(token.trim_start_matches('-').to_string());
                } else {
                    positional.push(token.clone());
                }
                index += 1;
                continue;
            };
            if let Some((key, value)) = name.split_once('=') {
                values.push((key.to_string(), value.to_string()));
                index += 1;
                continue;
            }
            if VALUE_OPTIONS.contains(&name) {
                // 值缺失时把下一个 token 当值（越界得到空串），与既有 `--variant` 解析同口径。
                let value = tokens.get(index + 1).cloned().unwrap_or_default();
                values.push((name.to_string(), value));
                index += 2;
                continue;
            }
            flags.push(name.to_string());
            index += 1;
        }
        Self {
            values,
            flags,
            positional,
        }
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn positional(&self) -> &[String] {
        &self.positional
    }
}

/// 子命令入口：`main` 分发到这里，返回值即进程退出码。
///
/// `command` 由 `main::subcommand_index` 取出的第一个非选项词，`tokens` 是它之后的参数。
pub(crate) async fn run(command: &str, tokens: &[String]) -> i32 {
    let options = Options::parse(tokens);
    match command {
        "accounts" => accounts_command(&options),
        "limits" => limits_command(&options),
        // `status` 只有带 `--json` 才走这里；裸 `status` 仍是既有的终端文本输出。
        "status" => status_command(&options),
        "credits" => credits::credits_command(&options).await,
        "inspect" => inspect::inspect_command(&options).await,
        "switch" => switch::switch_command(&options),
        other => {
            eprintln!("[cli] 未知子命令「{other}」，可用：accounts / limits / credits / status / inspect / switch");
            EXIT_USAGE
        }
    }
}

/// `status` 是否要求 JSON 输出。
///
/// 其余新命令始终输出 JSON（`--json` 只是为对称而接受），只有 `status` 需要区分：
/// 裸 `status` 保持改造前的终端文本，AI 必须显式加 `--json` 才拿到 JSON。
pub(crate) fn wants_json(tokens: &[String]) -> bool {
    tokens.iter().any(|token| token == "--json")
}

/// 输出一行 JSON 到 stdout，并返回给定退出码。
pub(crate) fn emit(value: &Value, code: i32) -> i32 {
    match serde_json::to_string(value) {
        Ok(text) => {
            println!("{text}");
            code
        }
        Err(error) => {
            eprintln!("[cli] JSON 序列化失败：{error}");
            EXIT_FATAL
        }
    }
}

/// 统一错误输出：stderr 给人看，stdout 仍是可解析的 JSON 信封。
pub(crate) fn emit_error(error: &str, code: i32) -> i32 {
    eprintln!("[cli] {error}");
    emit(
        &json!({ "schemaVersion": SCHEMA_VERSION, "ok": false, "error": error }),
        code,
    )
}

/// 进度/警告一律走 stderr，避免污染 stdout 的纯 JSON。
pub(crate) fn progress(message: &str) {
    eprintln!("{message}");
}

/// 校验未知开关；返回 `Some(退出码)` 表示已经输出错误、调用方应直接返回。
pub(crate) fn reject_unknown(options: &Options, known: &[&str]) -> Option<i32> {
    let unknown = options
        .flags
        .iter()
        .map(String::as_str)
        .find(|flag| !known.contains(flag))?;
    Some(emit_error(
        &format!("未知选项「--{unknown}」，可用：{}", describe_flags(known)),
        EXIT_USAGE,
    ))
}

fn describe_flags(known: &[&str]) -> String {
    if known.is_empty() {
        return "（无）".to_string();
    }
    known
        .iter()
        .map(|flag| format!("--{flag}"))
        .collect::<Vec<String>>()
        .join(" / ")
}

/// 读取账号池；账号库「文件在但读不出 / 不是 JSON 数组」视为致命错误。
///
/// core 的 [`account::load_accounts`] 把「文件缺失」与「文件损坏」都折叠成空列表，
/// AI 无法区分「这个用户没有账号」与「账号库坏了」，所以这里补一层判定。
pub(crate) fn load_pool() -> Result<Vec<Value>, String> {
    if let Some(error) = accounts_store_error() {
        return Err(error);
    }
    Ok(account::load_accounts())
}

fn accounts_store_error() -> Option<String> {
    let path = config::accounts_file();
    match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => Some(format!("账号库 {} 是空文件", path.display())),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(_)) => None,
            Ok(_) => Some(format!("账号库 {} 不是 JSON 数组", path.display())),
            Err(error) => Some(format!(
                "账号库 {} 解析失败：{error}（可能正在被其它进程写入）",
                path.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!("账号库 {} 读取失败：{error}", path.display())),
    }
}

/// 账号 id（缺失时为空串）。
pub(crate) fn account_id_of(account: &Value) -> String {
    account::get_str(account, "id").unwrap_or_default()
}

/// 账号是否就是这个 id（`id` 或 `uid` 任一匹配）。
pub(crate) fn matches_account(account: &Value, account_id: &str) -> bool {
    account::get_str(account, "id").as_deref() == Some(account_id)
        || account::get_str(account, "uid").as_deref() == Some(account_id)
}

/// 账号列表条目：白名单字段，绝不含 token / refresh_token / auth_raw。
///
/// `cli_current` 是 CodeBuddy CLI 的当前账号 id（**不是** GUI 当前账号）。
pub(crate) fn account_entry(account: &Value, cli_current: Option<&str>) -> Value {
    let meta = account::account_meta(account);
    json!({
        "id": meta.get("id").cloned().unwrap_or(Value::Null),
        "uid": meta.get("uid").cloned().unwrap_or(Value::Null),
        "name": account::account_display_name(account),
        "variant": meta.get("variant").cloned().unwrap_or(Value::Null),
        "cliCurrent": cli_current.is_some_and(|current| matches_account(account, current)),
        "needsRelogin": meta.get("needsRelogin").cloned().unwrap_or(Value::Bool(false)),
        "expiresAt": meta.get("expiresAt").cloned().unwrap_or(Value::Null),
        "refreshExpiresAt": meta.get("refreshExpiresAt").cloned().unwrap_or(Value::Null),
    })
}

/// CodeBuddy CLI 当前账号 id（读 core 的脱敏 status，不碰 token）。
pub(crate) fn cli_current_id() -> Option<String> {
    codebuddy_cli::status()
        .get("activeAccountId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 是否存在心跳新鲜的 CLI 会话（拒绝切号的判定基准）。
pub(crate) fn cli_running() -> bool {
    codebuddy_cli::has_live_session(config::now_ms())
}

/// `--timeout`：默认 8 秒；超上限时截断到 [`MAX_CREDITS_TIMEOUT_SECS`]。
pub(crate) fn parse_timeout(raw: Option<&str>) -> Result<u64, String> {
    let Some(text) = raw else {
        return Ok(DEFAULT_CREDITS_TIMEOUT_SECS);
    };
    let trimmed = text.trim();
    let seconds: u64 = trimmed
        .parse()
        .map_err(|_| format!("--timeout 需要整数秒，收到「{trimmed}」"))?;
    if seconds == 0 {
        return Err("--timeout 必须大于 0".to_string());
    }
    Ok(seconds.min(MAX_CREDITS_TIMEOUT_SECS))
}

/// 把 core 限额台账（[`limits::get_rate_limits`]）折叠成 `账号 id → 限额条目数组`。
///
/// 台账里**没有**记录的账号不会出现在 map 里，调用方据此输出 `limits: []`；
/// `[]` 只表示「没有观察到限额」，**不是**「模型可用」。
pub(crate) fn limits_by_account(payload: &Value) -> HashMap<String, Vec<Value>> {
    let mut map: HashMap<String, Vec<Value>> = HashMap::new();
    let Some(items) = payload.get("accounts").and_then(Value::as_array) else {
        return map;
    };
    for item in items {
        let Some(account_id) = item.get("accountId").and_then(Value::as_str) else {
            continue;
        };
        let Some(limited) = item.get("limited").and_then(Value::as_array) else {
            continue;
        };
        let entries: Vec<Value> = limited
            .iter()
            .map(|entry| {
                json!({
                    "model": entry.get("model").cloned().unwrap_or(Value::Null),
                    "resetAt": entry.get("resetAt").cloned().unwrap_or(Value::Null),
                    "source": entry.get("source").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        map.insert(account_id.to_string(), entries);
    }
    map
}

/// `accounts [--json]`：账号列表。
fn accounts_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json"]) {
        return code;
    }
    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let current = cli_current_id();
    let entries: Vec<Value> = accounts
        .iter()
        .map(|account| account_entry(account, current.as_deref()))
        .collect();
    emit(
        &json!({ "schemaVersion": SCHEMA_VERSION, "accounts": entries }),
        EXIT_OK,
    )
}

/// `limits [--json]`：各账号**观察到的**模型限额。
///
/// 账号池里的每个账号都有一条记录；没有观察到限额的账号输出 `limits: []`。
/// 不做跨账号推断，也不给「模型可用」结论。
fn limits_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json"]) {
        return code;
    }
    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let ledger = limits::get_rate_limits();
    let by_account = limits_by_account(&ledger);
    let entries: Vec<Value> = accounts
        .iter()
        .map(|account| {
            let limits = by_account
                .get(&account_id_of(account))
                .cloned()
                .unwrap_or_default();
            json!({ "id": account::get_str(account, "id").unwrap_or_default(), "limits": limits })
        })
        .collect();
    emit(
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "scannedAt": ledger.get("scannedAt").cloned().unwrap_or(Value::Null),
            // 观察窗口（天）：AI 据此判断「没记录」是窗口内没限流还是根本没扫到。
            "windowDays": ledger.get("windowDays").cloned().unwrap_or(Value::Null),
            "accounts": entries,
        }),
        EXIT_OK,
    )
}

/// `status --json`：CLI 当前账号 + 是否有运行中的会话 + 账号池大小。
fn status_command(options: &Options) -> i32 {
    if let Some(code) = reject_unknown(options, &["json"]) {
        return code;
    }
    let accounts = match load_pool() {
        Ok(accounts) => accounts,
        Err(error) => return emit_error(&error, EXIT_FATAL),
    };
    let current = cli_current_id();
    emit(
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "cli": {
                "current": current.map(Value::String).unwrap_or(Value::Null),
                "running": cli_running(),
            },
            "poolSize": accounts.len(),
        }),
        EXIT_OK,
    )
}
