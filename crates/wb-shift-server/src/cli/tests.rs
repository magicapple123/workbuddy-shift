//! CLI 子命令的解析与 JSON 形状单测（R5-10）。
//!
//! 只测不联网、不依赖本机账号库的纯逻辑：参数解析、白名单字段、
//! 限额折叠与注册表文件名判定。真实数据的端到端验证见
//! `delivery/evidence/round5/R5-10/NOTES.md`。

use serde_json::json;

use super::switch::{live_sessions, registry_stem};
use super::*;

fn tokens(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn options_split_flags_values_and_positionals() {
    let options = Options::parse(&tokens(&[
        "--dry-run",
        "acct_1",
        "--timeout",
        "3",
        "--cached",
    ]));
    assert!(options.has("dry-run"));
    assert!(options.has("cached"));
    assert!(!options.has("force"));
    assert_eq!(options.value("timeout"), Some("3"));
    assert_eq!(options.positional(), ["acct_1".to_string()]);
}

#[test]
fn options_read_equals_form_and_global_options() {
    let options = Options::parse(&tokens(&["--timeout=5", "--json", "--variant", "ai"]));
    assert_eq!(options.value("timeout"), Some("5"));
    assert_eq!(options.value("variant"), Some("ai"));
    assert!(options.has("json"));
    // 全局选项的值不能被当成位置参数（否则 `inspect --summary --variant ai` 会去查 "ai"）。
    assert!(options.positional().is_empty());
}

#[test]
fn options_treat_unknown_value_option_as_flag() {
    let options = Options::parse(&tokens(&["--no-credits", "--summary"]));
    assert!(options.has("no-credits"));
    assert!(options.has("summary"));
    assert!(options.value("timeout").is_none());
}

#[test]
fn parse_timeout_defaults_and_validates() {
    assert_eq!(parse_timeout(None).unwrap(), DEFAULT_CREDITS_TIMEOUT_SECS);
    assert_eq!(parse_timeout(Some("3")).unwrap(), 3);
    assert_eq!(parse_timeout(Some(" 12 ")).unwrap(), 12);
    assert_eq!(
        parse_timeout(Some("9999")).unwrap(),
        MAX_CREDITS_TIMEOUT_SECS,
        "超上限要截断，避免把 CLI 挂死"
    );
    assert!(parse_timeout(Some("abc")).is_err());
    assert!(parse_timeout(Some("0")).is_err());
}

/// 账号条目必须是白名单：任何 token / auth_raw 都不能漏进 stdout。
#[test]
fn account_entry_never_leaks_secrets() {
    let account = json!({
        "id": "acct_1",
        "uid": "uid_1",
        "nickname": "小明",
        "email": "user@example.com",
        "variant": "cn",
        "needs_relogin": false,
        "expiresAt": 1_763_000_000_000i64,
        "refreshExpiresAt": 1_765_000_000_000i64,
        "access_token": "SECRET-ACCESS",
        "refresh_token": "SECRET-REFRESH",
        "auth_raw": { "token": "SECRET-RAW" },
        "profile_raw": { "phone": "13800000000" },
    });
    let entry = account_entry(&account, Some("acct_1"));
    let text = serde_json::to_string(&entry).unwrap();
    for forbidden in [
        "SECRET-ACCESS",
        "SECRET-REFRESH",
        "SECRET-RAW",
        "access_token",
        "refresh_token",
        "auth_raw",
    ] {
        assert!(!text.contains(forbidden), "输出泄漏了 {forbidden}：{text}");
    }
    assert_eq!(entry["id"], "acct_1");
    assert_eq!(entry["uid"], "uid_1");
    // 展示名的字段优先顺序由 core 的 `account_display_name` 决定，这里只要求非空。
    assert!(entry["name"].as_str().is_some_and(|name| !name.is_empty()));
    assert_eq!(entry["variant"], "cn");
    assert_eq!(entry["cliCurrent"], true);
    assert_eq!(entry["needsRelogin"], false);
    assert_eq!(entry["expiresAt"], 1_763_000_000_000i64);
}

/// 没有邮箱/备注时展示名必须回退到昵称（AI 靠它给人看的名字）。
#[test]
fn account_entry_name_falls_back_to_nickname() {
    let account = json!({ "id": "acct_1", "nickname": "小明" });
    assert_eq!(account_entry(&account, None)["name"], "小明");
}

/// `cliCurrent` 认 uid，也认「当前账号不在池里」的兜底。
#[test]
fn account_entry_matches_cli_current_by_id_or_uid() {
    let account = json!({ "id": "acct_1", "uid": "uid_1", "nickname": "小明" });
    assert_eq!(
        account_entry(&account, Some("uid_1"))["cliCurrent"],
        json!(true)
    );
    assert_eq!(
        account_entry(&account, Some("acct_other"))["cliCurrent"],
        json!(false)
    );
    assert_eq!(account_entry(&account, None)["cliCurrent"], json!(false));
}

#[test]
fn matches_account_accepts_id_and_uid_only() {
    let account = json!({ "id": "acct_1", "uid": "uid_1", "nickname": "小明" });
    assert!(matches_account(&account, "acct_1"));
    assert!(matches_account(&account, "uid_1"));
    assert!(!matches_account(&account, "小明"));
    assert!(!matches_account(&account, ""));
}

/// 没有限额记录的账号必须是不在 map 里（调用方输出 `[]`），不能凭空造条目。
#[test]
fn limits_by_account_drops_fields_and_keeps_empty_pool() {
    let payload = json!({
        "scannedAt": 1_763_000_000_000i64,
        "windowDays": 2,
        "accounts": [
            {
                "accountId": "acct_1",
                "limited": [
                    { "model": "deepseek-v4", "resetAt": 1_763_000_600_000i64, "firstSeenAt": 1, "hitCount": 3, "source": "codebuddy-cli" }
                ],
                "borrowed": null
            }
        ]
    });
    let map = limits_by_account(&payload);
    let entries = map.get("acct_1").expect("acct_1 应有条目");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["model"], "deepseek-v4");
    assert_eq!(entries[0]["resetAt"], 1_763_000_600_000i64);
    assert_eq!(entries[0]["source"], "codebuddy-cli");
    // 台账里的诊断字段不下发给 CLI（AI 只认 model / resetAt / source）。
    assert!(entries[0].get("hitCount").is_none());
    assert!(!map.contains_key("acct_2"));
}

#[test]
fn limits_by_account_tolerates_missing_payload() {
    assert!(limits_by_account(&json!({})).is_empty());
    assert!(limits_by_account(&json!({ "accounts": "broken" })).is_empty());
}

#[test]
fn registry_stem_accepts_pid_and_manual_files_only() {
    assert_eq!(registry_stem("1234.json"), Some("1234"));
    assert_eq!(registry_stem("manual-abc.json"), Some("manual-abc"));
    assert_eq!(registry_stem("manual-.json"), None);
    assert_eq!(registry_stem("locks.json"), None);
    assert_eq!(registry_stem("1234.json.bak"), None);
    assert_eq!(registry_stem(".json"), None);
}

/// 会话清单是只读的：目录不存在时返回空列表（不是错误、不 panic）。
/// 真实机器上没有 CLI 会话时就是这种形态；有会话时每条都必须带 `pid` / `startedAt`。
#[test]
fn live_sessions_returns_plain_records() {
    let live = live_sessions(config::now_ms());
    for item in &live {
        assert!(item.get("pid").is_some(), "缺少 pid：{item}");
        assert!(item.get("startedAt").is_some(), "缺少 startedAt：{item}");
    }
}
