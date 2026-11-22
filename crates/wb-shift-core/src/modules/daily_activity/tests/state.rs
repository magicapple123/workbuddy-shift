// 去重状态：文件结构与退化行为（损坏 → 空状态，不阻塞打卡）。

/// 状态文件结构：`version` + `accounts[key] = {date, completedAt, model}`。
#[test]
fn state_roundtrip_keeps_date_and_model() {
    let dir = state::tests_support::TempDir::new("state-roundtrip");
    let path = dir.join(state::STATE_FILE_NAME);

    assert!(!state::done_today(&path, "acc-1", "2026-11-22"));
    state::record_success(&path, "acc-1", "2026-11-22", "hy3", 1_763_800_000_000)
        .expect("写入状态");

    let raw = std::fs::read_to_string(&path).expect("状态文件应存在");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("状态文件是 JSON");
    assert_eq!(parsed["version"], serde_json::json!(1));
    assert_eq!(
        parsed["accounts"]["acc-1"]["date"],
        serde_json::json!("2026-11-22")
    );
    assert_eq!(
        parsed["accounts"]["acc-1"]["model"],
        serde_json::json!("hy3")
    );
    assert_eq!(
        parsed["accounts"]["acc-1"]["completedAt"],
        serde_json::json!(1_763_800_000_000_i64)
    );

    assert!(state::done_today(&path, "acc-1", "2026-11-22"));
    // 换一天就不算已完成。
    assert!(!state::done_today(&path, "acc-1", "2026-11-23"));
}

/// 损坏的状态文件当作空状态：读不到只多跑一次，不该让整轮失败。
#[test]
fn corrupted_state_file_degrades_to_empty() {
    let dir = state::tests_support::TempDir::new("state-corrupt");
    let path = dir.join(state::STATE_FILE_NAME);
    std::fs::write(&path, "{not json").expect("写入坏文件");

    let loaded = state::load(&path);
    assert!(loaded.accounts.is_empty());
    assert_eq!(loaded.version, state::STATE_VERSION);
    assert!(!state::done_today(&path, "acc-1", "2026-11-22"));
}

/// 多账号各自独立：写一个账号不会覆盖另一个。
#[test]
fn records_are_per_account() {
    let dir = state::tests_support::TempDir::new("state-multi");
    let path = dir.join(state::STATE_FILE_NAME);

    state::record_success(&path, "acc-1", "2026-11-22", "hy3", 1).expect("写入 acc-1");
    state::record_success(&path, "acc-2", "2026-11-22", "deepseek-v4.1-flash", 2)
        .expect("写入 acc-2");

    let loaded = state::load(&path);
    assert_eq!(loaded.accounts.len(), 2);
    assert_eq!(loaded.accounts["acc-2"].model, "deepseek-v4.1-flash");
    assert!(loaded.is_done_today("acc-1", "2026-11-22"));
    assert!(loaded.is_done_today("acc-2", "2026-11-22"));
}

/// 去重 key 优先 `id`，其次 `uid`；都空返回空串（上层据此跳过并给可读原因）。
#[test]
fn account_key_prefers_id_then_uid() {
    assert_eq!(
        state::account_key(&serde_json::json!({"id": "a", "uid": "u"})),
        "a"
    );
    assert_eq!(state::account_key(&serde_json::json!({"uid": "u"})), "u");
    assert_eq!(state::account_key(&serde_json::json!({"id": "  "})), "");
    assert_eq!(state::account_key(&serde_json::json!({})), "");
}

/// 缺 `id/uid` 的账号：跳过且不发请求。
#[tokio::test]
async fn account_without_key_is_unsupported() {
    let dir = state::tests_support::TempDir::new("state-nokey");
    let path = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy();
    let account = serde_json::json!({
        "variant": "ai",
        "access_token": "tok-ACCESS-SENTINEL"
    });

    let result = run_daily_activity_with_limits(
        &[account],
        &stub,
        &path,
        &super::protocol::tests_support::fast_limits(),
    )
    .await;

    assert_eq!(
        result["accounts"][0]["phase"],
        serde_json::json!("unsupported")
    );
    assert!(stub.is_empty());
}
