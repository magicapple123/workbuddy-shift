//! settings.json 与环境变量：token/env 读写、区域环境、运行时缓存与状态同步。

use super::helper::*;
use super::state::*;
use super::*;

pub(super) fn settings_path() -> PathBuf {
    home_dir().join(SETTINGS_DIR).join(SETTINGS_FILE)
}

pub(super) fn clean_bearer_token(token: &str) -> &str {
    let token = token.trim();
    if token == "Bearer" {
        return "";
    }
    token.strip_prefix("Bearer ").unwrap_or(token).trim()
}

pub(super) fn settings_env_token(value: &Value) -> Option<&str> {
    value
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get(CODEBUDDY_AUTH_TOKEN))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

pub(super) fn process_env_token_present() -> bool {
    std::env::var_os(CODEBUDDY_AUTH_TOKEN)
        .is_some_and(|token| !token.to_string_lossy().trim().is_empty())
}

pub(super) fn process_internet_environment() -> Option<String> {
    std::env::var(CODEBUDDY_INTERNET_ENVIRONMENT)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn ensure_no_process_env_override() -> Result<(), String> {
    if process_env_token_present() {
        return Err(auth_config_error(
            "环境阶段",
            "检测到进程环境变量 CODEBUDDY_AUTH_TOKEN；它会覆盖 settings.json，请先删除该用户或系统环境变量并重启应用与 CodeBuddy CLI",
        ));
    }
    Ok(())
}

pub(super) fn ensure_region_env_compatible(variant: WbVariant) -> Result<(), String> {
    let Some(value) = process_internet_environment() else {
        return Ok(());
    };
    let is_cn_env =
        value.eq_ignore_ascii_case(CN_INTERNET_ENVIRONMENT) || value.eq_ignore_ascii_case("ioa");
    let is_ai_env = value.eq_ignore_ascii_case(AI_INTERNET_ENVIRONMENT)
        || value.eq_ignore_ascii_case("external");
    let conflict = match variant {
        WbVariant::Cn => !is_cn_env,
        WbVariant::Ai => is_cn_env || !(is_ai_env || value.is_empty()),
    };
    if conflict {
        return Err(auth_config_error(
            "环境阶段",
            "检测到进程环境变量 CODEBUDDY_INTERNET_ENVIRONMENT 与所选账号档位冲突；它会覆盖 settings.json，请先删除该用户或系统环境变量并重启应用与 CodeBuddy CLI",
        ));
    }
    Ok(())
}

pub(super) fn env_object_mut(
    value: &mut Value,
) -> Result<&mut serde_json::Map<String, Value>, String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "CodeBuddy settings.json 顶层不是对象".to_string())?;
    let env = object.entry("env").or_insert_with(|| json!({}));
    env.as_object_mut()
        .ok_or_else(|| "CodeBuddy settings.json 的 env 字段不是对象".to_string())
}

pub(super) fn apply_cli_region_env(value: &mut Value, variant: WbVariant) -> Result<(), String> {
    let env = env_object_mut(value)?;
    match variant {
        WbVariant::Cn => {
            env.insert(
                CODEBUDDY_INTERNET_ENVIRONMENT.to_string(),
                json!(CN_INTERNET_ENVIRONMENT),
            );
            env.remove(CODEBUDDY_BASE_URL);
        }
        WbVariant::Ai => {
            env.insert(
                CODEBUDDY_INTERNET_ENVIRONMENT.to_string(),
                json!(AI_INTERNET_ENVIRONMENT),
            );
            env.insert(
                CODEBUDDY_BASE_URL.to_string(),
                json!(AI_CLI_OPENAI_BASE_URL),
            );
        }
    }
    Ok(())
}

pub(super) fn cli_local_storage_dir_for_settings(settings: &Path) -> PathBuf {
    settings.parent().unwrap_or(settings).join("local_storage")
}

pub(super) fn cli_cache_filename(key: &str) -> &'static str {
    match key {
        CLI_ENV_CACHE_KEY => "entry_3bab4ce61838088127d444e4cc042d6d.info",
        CLI_ENDPOINT_CACHE_KEY => "entry_933d5543e80177622c17a73869c0fad7.info",
        CLI_PRODUCT_CACHE_KEY => "entry_604f48c944053e01d9546675443286c1.info",
        _ => "entry_unknown.info",
    }
}

pub(super) fn write_cli_json_string_cache(path: &Path, value: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            auth_config_error(
                "配置阶段",
                "无法创建 CodeBuddy CLI 缓存目录，请检查用户目录权限",
            )
        })?;
    }
    let content = serde_json::to_string(&json!(value))
        .map_err(|_| auth_config_error("配置阶段", "无法生成 CodeBuddy CLI 缓存"))?;
    atomic_write(path, &content)
        .map_err(|_| auth_config_error("配置阶段", "无法写入 CodeBuddy CLI 缓存，请检查文件权限"))
}

pub(super) fn sync_cli_runtime_cache_at(
    storage_dir: &Path,
    variant: WbVariant,
) -> Result<(), String> {
    let env_path = storage_dir.join(cli_cache_filename(CLI_ENV_CACHE_KEY));
    let endpoint_path = storage_dir.join(cli_cache_filename(CLI_ENDPOINT_CACHE_KEY));
    let product_path = storage_dir.join(cli_cache_filename(CLI_PRODUCT_CACHE_KEY));
    // 产品包缓存按环境选 product.internal.json / product.json；切档位必须丢掉。
    let _ = std::fs::remove_file(&product_path);
    match variant {
        WbVariant::Cn => {
            write_cli_json_string_cache(&env_path, CN_INTERNET_ENVIRONMENT)?;
            write_cli_json_string_cache(&endpoint_path, CN_CLI_ENDPOINT)?;
        }
        WbVariant::Ai => {
            write_cli_json_string_cache(&env_path, AI_INTERNET_ENVIRONMENT)?;
            write_cli_json_string_cache(&endpoint_path, AI_CLI_ENDPOINT)?;
        }
    }
    Ok(())
}

pub(super) fn sync_cli_runtime_cache(settings: &Path, variant: WbVariant) -> Result<(), String> {
    sync_cli_runtime_cache_at(&cli_local_storage_dir_for_settings(settings), variant)
}

pub(super) fn write_settings_env_token(value: &mut Value, token: &str) -> Result<(), String> {
    let env = env_object_mut(value)?;
    env.insert(CODEBUDDY_AUTH_TOKEN.to_string(), json!(token));
    Ok(())
}

pub(super) fn persist_settings_at(settings: &Path, value: &Value) -> Result<(), String> {
    let content = serde_json::to_string_pretty(value)
        .map_err(|_| auth_config_error("配置阶段", "无法生成 CodeBuddy settings.json"))?;
    if let Some(parent) = settings.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            auth_config_error(
                "配置阶段",
                "无法创建 CodeBuddy 配置目录，请检查用户目录权限",
            )
        })?;
    }
    atomic_write(settings, &content).map_err(|_| {
        auth_config_error(
            "配置阶段",
            "无法写入 CodeBuddy settings.json，请检查文件权限",
        )
    })
}

pub(super) fn validate_persisted_env_token_at(
    settings: &PathBuf,
    expected_token: &str,
) -> Result<(), String> {
    let persisted = read_json_file(settings).ok_or_else(|| {
        auth_config_error("配置阶段", "写入后无法重新读取 CodeBuddy settings.json")
    })?;
    if settings_env_token(&persisted).map(clean_bearer_token)
        != Some(clean_bearer_token(expected_token))
    {
        return Err(auth_config_error(
            "配置阶段",
            "写入后的认证信息与所选账号不一致",
        ));
    }
    Ok(())
}

pub(super) fn prepare_settings_env_update(
    settings: &PathBuf,
    token: &str,
    variant: WbVariant,
) -> Result<(Option<String>, Value), String> {
    let previous = std::fs::read_to_string(settings).ok();
    let mut value = previous
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| "CodeBuddy settings.json 不是有效 JSON")?
        .unwrap_or_else(|| json!({}));
    write_settings_env_token(&mut value, clean_bearer_token(token))?;
    apply_cli_region_env(&mut value, variant)?;
    Ok((previous, value))
}

pub(super) fn persist_cli_region_env(variant: WbVariant) -> Result<(), String> {
    let settings = settings_path();
    let previous = std::fs::read_to_string(&settings).ok();
    let mut value = previous
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| "CodeBuddy settings.json 不是有效 JSON")?
        .unwrap_or_else(|| json!({}));
    apply_cli_region_env(&mut value, variant)?;
    persist_settings_at(&settings, &value).inspect_err(|_| {
        restore_file(&settings, previous.as_deref());
    })?;
    if let Err(error) = sync_cli_runtime_cache(&settings, variant) {
        restore_file(&settings, previous.as_deref());
        return Err(error);
    }
    Ok(())
}

pub(super) fn commit_settings_env_update(
    settings: &PathBuf,
    previous: Option<&str>,
    value: &Value,
    expected_token: &str,
    variant: WbVariant,
) -> Result<(), String> {
    if let Err(error) = persist_settings_at(settings, value)
        .and_then(|_| validate_persisted_env_token_at(settings, expected_token))
        .and_then(|_| sync_cli_runtime_cache(settings, variant))
    {
        restore_file(settings, previous);
        return Err(error);
    }
    Ok(())
}

pub(super) fn read_json_file(path: &PathBuf) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(super) fn restore_file(path: &Path, previous: Option<&str>) {
    if let Some(previous) = previous {
        let _ = atomic_write(path, previous);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Windows 静态认证下，把当前 CLI 账号刷新后的 token 同步到 settings。
/// 仅当 settings 当前 token 能匹配该账号（或状态明确指向该账号）时写入，
/// 避免后台刷新覆盖用户刚刚手动选择的其他账号；失败只返回脱敏错误。
pub fn sync_windows_env_for_account(
    account_value: &Value,
    previous_access_token: Option<&str>,
) -> Result<bool, String> {
    if !cfg!(windows) {
        return Ok(false);
    }
    ensure_no_process_env_override()?;
    let settings = settings_path();
    let Some(current_settings) = read_json_file(&settings) else {
        return Ok(false);
    };
    let Some(current_token) = settings_env_token(&current_settings) else {
        return Ok(false);
    };
    let accounts = account::load_accounts();
    let state = load_state();
    let Some((_, active_id)) = state_account_index(&state, &accounts) else {
        return Ok(false);
    };
    let account_id = account_value.get("id").and_then(Value::as_str);
    if account_id != Some(active_id.as_str()) {
        return Ok(false);
    }
    let Some(updated_token) = account_value.get("access_token").and_then(Value::as_str) else {
        return Ok(false);
    };
    // settings 必须仍是刷新前 token（或已同步的新 token）；否则视为用户已切换/手工修改。
    let current = clean_bearer_token(current_token);
    let previous_matches = previous_access_token
        .map(clean_bearer_token)
        .is_some_and(|token| token == current);
    let already_synced = clean_bearer_token(updated_token) == current;
    // 例外：settings 里的 token 匹配不上账号库中的**任何**账号时，它是失效的孤儿值
    // （典型成因：历史上某次刷新没同步成功，或该账号被重新登录换了 token）。
    //
    // 这种情况必须允许用活跃账号的最新 token 覆盖，否则状态**不可自愈**：
    // settings 既不等于「刷新前 token」也不等于「刷新后 token」，于是每次刷新都在
    // 这里被挡掉，界面永远停在「认证脱节」，只能靠用户手动点按钮。
    //
    // 若它匹配到的是**其它**账号，说明用户手动切过去了，仍然不覆盖（保留原保护）。
    let current_is_orphan = !accounts.iter().any(|candidate| {
        clean_bearer_token(
            candidate
                .get("access_token")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ) == current
    });
    if !previous_matches && !already_synced && !current_is_orphan {
        return Ok(false);
    }
    if already_synced {
        return Ok(true);
    }
    let (previous, value) = prepare_settings_env_update(
        &settings,
        updated_token,
        WbVariant::from_account(account_value),
    )?;
    commit_settings_env_update(
        &settings,
        previous.as_deref(),
        &value,
        updated_token,
        WbVariant::from_account(account_value),
    )?;
    Ok(true)
}

/// 保活刷新后、settings 尚未同步的宽限窗口。
///
/// 账号库在此时间内被写过，说明大概率正处在「保活刚换完 token、settings 还没跟上」
/// 的正常中间态，此时不该提示用户「认证脱节」。
pub(super) const ACCOUNTS_SYNC_GRACE_MS: i64 = 30_000;

/// 账号库是否在最近 `window_ms` 内被写过（按 mtime 判断）。
pub(super) fn account_store_written_within(window_ms: i64) -> bool {
    let path = crate::modules::config::accounts_file();
    let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    match std::time::SystemTime::now().duration_since(modified) {
        Ok(age) => (age.as_millis() as i64) <= window_ms,
        // 时钟回拨等异常：按「刚写过」处理，宁可少报一次也不误报脱节。
        Err(_) => true,
    }
}

/// 由状态位推导 `(syncPending, syncInProgress)`。
///
/// - `syncPending`：真脱节——settings 的 token 反查不到账号，且已超出同步宽限窗口，
///   需要用户介入（点「更新 CLI 认证」）。
/// - `syncInProgress`：保活刚写完账号库、settings 尚未跟上的正常中间态，
///   稍候会自动完成，不该催用户操作。
///
/// 抽成纯函数是为了让边界（尤其是宽限窗口的临界点）可被单测固定住。
pub(super) fn classify_sync_state(
    windows: bool,
    env_configured: bool,
    active_missing: bool,
    expected_present: bool,
    recently_written: bool,
) -> (bool, bool) {
    let pending_raw = windows && env_configured && active_missing && expected_present;
    let in_progress = pending_raw && recently_written;
    let pending = pending_raw && !recently_written;
    (pending, in_progress)
}

pub(super) fn install_env_auth() -> Result<Value, String> {
    ensure_no_process_env_override()?;
    let settings = settings_path();
    let accounts = account::load_accounts();
    let state = load_state();
    let active = state_account_index(&state, &accounts)
        .and_then(|(index, _)| accounts.get(index))
        .or_else(|| accounts.first())
        .ok_or_else(|| auth_config_error("账号阶段", "当前没有可供 CodeBuddy CLI 使用的账号"))?;
    ensure_region_env_compatible(WbVariant::from_account(active))?;
    let token = settings_account_token(active)?;
    let (previous_settings, settings_value) =
        prepare_settings_env_update(&settings, token, WbVariant::from_account(active))?;
    commit_settings_env_update(
        &settings,
        previous_settings.as_deref(),
        &settings_value,
        token,
        WbVariant::from_account(active),
    )?;

    Ok(json!({
        "ok": true,
        "configured": true,
        "authMode": "settings-env",
        "helperPresent": helper_path().map(|path| path.is_file()).unwrap_or(false),
        "helperSupportsAccountIds": helper_supports_account_ids(),
        "verified": true,
        "message": "CodeBuddy CLI 认证配置已更新；当前运行会话不会切换，请由 ACP 重新加载会话或重启 CLI 后生效",
    }))
}
