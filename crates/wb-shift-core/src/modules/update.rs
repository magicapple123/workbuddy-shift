//! 自动更新：检查公开 GitHub Releases 版本 + 更新源配置。
//!
//! 对照 server.py `load_github_config` / `save_github_config` /
//! `compare_versions` / `update_check`。下载安装走 tauri-plugin-updater（整包更新）。
//!
//! 版本检查不走 GitHub API（避免 60 次/小时/IP 限流）：
//! 1. 主端点：release 资产的 updater manifest（下载不计 API 配额），三条候选 **并行** 请求；
//! 2. 兜底端点：`/releases/latest` 的 302 `Location` 头解析 tag；
//! 3. 成功结果进程级缓存 6 小时，缓存命中不发网络请求；
//! 4. 失败结果按 10 分钟起指数退避（上限 6 小时）负缓存，`force=true` 绕过。
//!
//! 单次检查预算：三条候选各 8s（并行）+ 302 兜底 8s = 最坏 16s；宿主层另有 20s 总预算。

use semver::Version;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::modules::config::{
    atomic_write, http_request_raw, http_request_with_proxy, now_secs, store_dir,
};
use crate::modules::error_log;
use crate::modules::locks;

/// 应用当前版本（来自 Cargo.toml package.version）。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 本项目自己的发布仓库。
///
/// 必须与 `src-tauri/tauri.conf.json` 的 `plugins.updater.endpoints` 保持一致：
/// 这里决定「查哪个仓库的版本、下载链接指向哪」，endpoints 决定实际下载安装；
/// 两边一旦分叉，用户会被提示升级到上游版本，点一下就把本应用换成原版。
pub const GITHUB_OWNER: &str = "magicapple123";
pub const GITHUB_REPO: &str = "workbuddy-shift";

/// 成功结果缓存有效期（6 小时）。自动轮询（30 分钟）命中缓存，不发网络请求；
/// 设置页手动检查传 force=true 绕过缓存强制刷新。
const CACHE_TTL_SECS: i64 = 6 * 60 * 60;

/// 单条 manifest 请求超时（三条候选并行，总耗时约等于单条）。
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(8);
/// 302 兜底请求超时。
const FALLBACK_TIMEOUT: Duration = Duration::from_secs(8);
/// 失败负缓存起点（10 分钟），按尝试次数指数退避。
const FAILURE_BASE_BACKOFF_SECS: i64 = 10 * 60;
/// 失败负缓存上限（6 小时）。
const FAILURE_MAX_BACKOFF_SECS: i64 = 6 * 60 * 60;

/// `github_config.json` 里「启动时检查更新」的字段名。
///
/// 与前端 `GithubConfig.autoCheck` 逐字一致（camelCase）：配置对象整体在两端之间往返，
/// 键名一旦分叉，设置页开关就会静默失效。
const AUTO_CHECK_KEY: &str = "autoCheck";

/// 发布说明最大保留字符数。
///
/// Release notes 常常是一整篇 changelog，全量过 IPC 再塞进卡片没有意义：卡片只展示前
/// 三行，「查看完整说明」走浏览器打开 Release 页。按**字符**截断（不是字节），
/// 中文说明被切在多字节中间会变成乱码。
pub const NOTES_MAX_CHARS: usize = 8000;

/// 进程级内存缓存，只缓存 ok=true 的结果；失败不写缓存。
struct CachedCheck {
    checked_at: i64,
    value: Value,
}

static CACHE: Mutex<Option<CachedCheck>> = Mutex::new(None);

/// 失败负缓存：`(失败时刻, 连续失败次数, 上次错误结果)`。
///
/// 直连被重置时一次完整检查要花十几秒；没有负缓存的话，30 分钟轮询 + 用户反复点
/// 会把同一份失败重跑很多遍，日志也会被刷满。
struct FailureCheck {
    failed_at: i64,
    attempts: u32,
    value: Value,
}

static FAILURE: Mutex<Option<FailureCheck>> = Mutex::new(None);

fn failure_backoff_secs(attempts: u32) -> i64 {
    let exponent = attempts.saturating_sub(1).min(10);
    (FAILURE_BASE_BACKOFF_SECS * (1_i64 << exponent)).min(FAILURE_MAX_BACKOFF_SECS)
}

pub fn github_config_file() -> PathBuf {
    store_dir().join("github_config.json")
}

/// 读取更新源配置（兼容旧配置文件，但永不返回 token）。
pub fn load_github_config() -> Value {
    load_github_config_at(&github_config_file())
}

/// [`load_github_config`] 的显式路径版本（单测注入临时文件，绝不读写真实家目录）。
fn load_github_config_at(f: &Path) -> Value {
    let mut owner = GITHUB_OWNER.to_string();
    let mut repo = GITHUB_REPO.to_string();
    let mut proxy = String::new();
    let mut auto_check = true;
    let mut should_normalize = false;
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(f) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(value) = v.get("owner").and_then(|v| v.as_str()) {
                    if !value.trim().is_empty() {
                        owner = value.to_string();
                    }
                }
                if let Some(value) = v.get("repo").and_then(|v| v.as_str()) {
                    if !value.trim().is_empty() {
                        repo = value.to_string();
                    }
                }
                if let Some(value) = v.get("proxy").and_then(|v| v.as_str()) {
                    proxy = value.trim().to_string();
                }
                if let Some(value) = v.get(AUTO_CHECK_KEY).and_then(Value::as_bool) {
                    auto_check = value;
                }
                should_normalize = v.get("token").is_some();
            }
        }
    }
    let normalized = json!({
        "owner": owner,
        "repo": repo,
        "proxy": proxy,
        "autoCheck": auto_check,
    });
    if should_normalize {
        let _ = atomic_write(
            f,
            &serde_json::to_string_pretty(&normalized).unwrap_or_default(),
        );
    }
    normalized
}

/// 读配置文件里的 [`AUTO_CHECK_KEY`]；文件不存在 / 字段缺失返回 None。
fn read_auto_check_at(f: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(f).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get(AUTO_CHECK_KEY).and_then(Value::as_bool)
}

/// [`auto_check_enabled`] 的显式路径版本（单测用）。
fn auto_check_enabled_at(f: &Path) -> bool {
    read_auto_check_at(f).unwrap_or(true)
}

/// 后台是否允许自动检查更新（默认开启）。
///
/// 后台轮询每轮都重读一次：设置页关掉「启动时检查更新」后不需要重启进程。
pub fn auto_check_enabled() -> bool {
    auto_check_enabled_at(&github_config_file())
}

/// 保存更新源配置；公开仓库不需要也不保存 GitHub token。
pub fn save_github_config(cfg: &Value) -> std::io::Result<()> {
    save_github_config_at(&store_dir(), &github_config_file(), cfg)
}

/// [`save_github_config`] 的显式路径版本（单测注入临时目录，绝不写真实配置）。
///
/// 前端保存代理时只提交 `proxy`，不带 [`AUTO_CHECK_KEY`]：这里必须沿用文件里已有的值，
/// 否则「保存代理」会把「启动时检查更新」静默重置成默认开启。
fn save_github_config_at(dir: &Path, f: &Path, cfg: &Value) -> std::io::Result<()> {
    let owner = cfg
        .get("owner")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(GITHUB_OWNER);
    let repo = cfg
        .get("repo")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(GITHUB_REPO);
    let proxy = cfg
        .get("proxy")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let auto_check = cfg
        .get(AUTO_CHECK_KEY)
        .and_then(Value::as_bool)
        .or_else(|| read_auto_check_at(f))
        .unwrap_or(true);
    let clean = json!({
        "owner": owner,
        "repo": repo,
        "proxy": proxy,
        "autoCheck": auto_check,
    });
    std::fs::create_dir_all(dir)?;
    atomic_write(f, &serde_json::to_string_pretty(&clean).unwrap_or_default())
}

/// 解析版本号（允许 `v` 前缀，语义按 semver：`0.2.1-rc.1 < 0.2.1`）。
///
/// 解析失败**不得**退化成「取数字前缀」：那会让 `0.2.1-rc1` 与 `0.2.0` 比较为相等，
/// 预发布版本对用户静默消失（第三轮第 19 条）。
pub fn parse_version(raw: &str) -> Result<Version, String> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    Version::parse(trimmed).map_err(|error| format!("版本号「{raw}」不是合法 semver：{error}"))
}

/// 版本比较（semver 规则）：`a > b` → Greater，相等 → Equal，`a < b` → Less。
///
/// 返回 `Err` 表示版本号不合法，调用方必须按「不可比较」处理（`hasUpdate=false`
/// + 明确文案），不得当成相等或最新。
pub fn compare_versions(a: &str, b: &str) -> Result<Ordering, String> {
    let left = parse_version(a)?;
    let right = parse_version(b)?;
    Ok(left.cmp(&right))
}

/// updater manifest 候选 URL（按优先级）。
///
/// 1. 合并后的 `latest.json`（含各平台）；
/// 2. 当前系统的 `latest-<os>-<arch>.json`；
/// 3. 兼容旧 Windows 安装包：它们仍请求 `latest-macos-<arch>.json`。
pub fn updater_manifest_urls(owner: &str, repo: &str, os: &str, arch: &str) -> Vec<String> {
    let os_slug = match os {
        "macos" | "darwin" => "macos",
        other => other,
    };
    let mut urls = vec![format!(
        "https://github.com/{owner}/{repo}/releases/latest/download/latest.json"
    )];
    urls.push(format!(
        "https://github.com/{owner}/{repo}/releases/latest/download/latest-{os_slug}-{arch}.json"
    ));
    if os_slug != "macos" {
        urls.push(format!(
            "https://github.com/{owner}/{repo}/releases/latest/download/latest-macos-{arch}.json"
        ));
    }
    urls.dedup();
    urls
}

fn manifest_headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("User-Agent".to_string(), "workbuddy-shift".to_string());
    headers
}

/// 单条 manifest 请求：8s 超时 + 失败写入 error.log（耗时 / 错误链）。
async fn fetch_manifest_at(url: String, proxy: Option<String>) -> Option<Value> {
    if url.is_empty() {
        return None;
    }
    let started = Instant::now();
    let headers = manifest_headers();
    let request = http_request_with_proxy(&url, "GET", None, Some(&headers), proxy.as_deref());
    match tokio::time::timeout(MANIFEST_TIMEOUT, request).await {
        Ok(resp) => {
            let version = resp.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if !version.trim().is_empty() {
                return Some(resp);
            }
            let message = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("更新清单解析失败");
            error_log::record(
                "backend",
                "更新清单不可用",
                &format!(
                    "url={url} 耗时={}ms 原因={message}",
                    started.elapsed().as_millis()
                ),
            );
            None
        }
        Err(_) => {
            error_log::record(
                "backend",
                "更新清单请求超时",
                &format!(
                    "url={url} 超时={}s 实际耗时={}ms",
                    MANIFEST_TIMEOUT.as_secs(),
                    started.elapsed().as_millis()
                ),
            );
            None
        }
    }
}

/// 并行拉取三条候选清单，按优先级返回第一个成功结果。
///
/// 并行是第 2 条的核心：串行时 3 × 30s 会把「一次检查」放大到 90s 以上。
async fn fetch_manifest_version(
    owner: &str,
    repo: &str,
    proxy: Option<&str>,
) -> Result<Value, String> {
    let urls = updater_manifest_urls(owner, repo, std::env::consts::OS, std::env::consts::ARCH);
    let proxy = proxy.map(str::to_string);
    let first = urls.first().cloned().unwrap_or_default();
    let second = urls.get(1).cloned().unwrap_or_default();
    let third = urls.get(2).cloned().unwrap_or_default();
    let (a, b, c) = tokio::join!(
        fetch_manifest_at(first, proxy.clone()),
        fetch_manifest_at(second, proxy.clone()),
        fetch_manifest_at(third, proxy),
    );
    a.or(b)
        .or(c)
        .ok_or_else(|| "更新清单不可达（候选地址全部失败）".to_string())
}

/// 兜底端点：请求 `/releases/latest`，读 302 `Location` 头（形如
/// `.../releases/tag/v0.1.13`）解析 tag。不跟随重定向，避免拉到 HTML 页面。
/// 成功返回 tag，失败返回可读错误 + code（状态码或 -1）。
async fn fetch_latest_tag(
    owner: &str,
    repo: &str,
    proxy: Option<&str>,
) -> Result<String, (String, i64)> {
    let url = format!("https://github.com/{owner}/{repo}/releases/latest");
    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "text/html".to_string());
    headers.insert("User-Agent".to_string(), "workbuddy-shift".to_string());
    let started = Instant::now();
    let request = http_request_raw(&url, "GET", None, Some(&headers), proxy, false);
    let (status, resp_headers, body) = match tokio::time::timeout(FALLBACK_TIMEOUT, request).await {
        Ok(result) => result,
        Err(_) => {
            error_log::record(
                "backend",
                "更新兜底请求超时",
                &format!(
                    "url={url} 超时={}s 实际耗时={}ms",
                    FALLBACK_TIMEOUT.as_secs(),
                    started.elapsed().as_millis()
                ),
            );
            return Err(("检查更新超时，请检查网络或更新代理".to_string(), -1));
        }
    };

    if status == 0 {
        let msg = if body.trim().is_empty() {
            "网络请求失败".to_string()
        } else {
            body
        };
        error_log::record(
            "backend",
            "更新兜底请求失败",
            &format!(
                "url={url} 耗时={}ms 原因={msg}",
                started.elapsed().as_millis()
            ),
        );
        return Err((msg, -1));
    }
    if status == 404 {
        // 无正式 release 或仓库不存在。
        return Err(("未找到可用的发布版本".to_string(), 404));
    }
    let location = resp_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.clone());
    let location = match location {
        Some(location) => location,
        None => {
            return Err((
                format!("无法获取发布页跳转地址（HTTP {status}）"),
                status as i64,
            ))
        }
    };
    let tag = location.rsplit('/').next().unwrap_or("").trim().to_string();
    if tag.is_empty() || !location.contains("/releases/tag/") {
        return Err(("无法解析发布版本标签".to_string(), -1));
    }
    Ok(tag)
}

/// 清单里的 `name` 只有在「确实是发布标题」时才返回：
/// 等于 tag / latest / `v{latest}` 的值没有信息量，前端会渲染成重复版本号（第 3 条）。
fn meaningful_release_name(manifest: &Value, tag: &str, latest: &str) -> Option<String> {
    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    let prefixed = format!("v{latest}");
    if name == tag || name == latest || name == prefixed {
        return None;
    }
    Some(name.to_string())
}

/// 清单里的发布说明：trim 后按 [`NOTES_MAX_CHARS`] 字符截断。
///
/// 老 Release 的清单没有 `notes` 字段（或只有空白）→ 返回 None，前端据此只显示
/// 版本号与日期，不留一块空白说明区。
fn manifest_notes(manifest: &Value) -> Option<String> {
    let raw = manifest.get("notes").and_then(|v| v.as_str())?.trim();
    if raw.is_empty() {
        return None;
    }
    let truncated: String = raw.chars().take(NOTES_MAX_CHARS).collect();
    let trimmed = truncated.trim_end().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 记录一次失败（负缓存），返回对外的错误结果。
fn remember_failure(value: Value) -> Value {
    let now = now_secs();
    // 负缓存只是省流量的优化，锁中毒时丢掉旧记录继续返回本次错误即可。
    let mut guard = locks::recover(&FAILURE);
    let attempts = guard.as_ref().map(|entry| entry.attempts + 1).unwrap_or(1);
    *guard = Some(FailureCheck {
        failed_at: now,
        attempts,
        value: value.clone(),
    });
    value
}

/// 查询最新 Release，与本地版本对比。对照 server.py `update_check`。
///
/// `force=true` 绕过成功缓存与失败负缓存（设置页手动检查）；否则：
/// - 6 小时内的成功结果直接返回，不发网络请求；
/// - 10 分钟起指数退避（上限 6 小时）内直接返回上次错误，不重跑完整链路。
pub async fn update_check(proxy: Option<&str>, force: bool) -> Value {
    let now = now_secs();
    if !force {
        // 版本查询缓存：下一次请求会整体覆盖，锁中毒时取回旧值继续即可。
        if let Some(cached) = locks::recover(&CACHE).as_ref() {
            if now - cached.checked_at < CACHE_TTL_SECS {
                return cached.value.clone();
            }
        }
        if let Some(failure) = locks::recover(&FAILURE).as_ref() {
            if now - failure.failed_at < failure_backoff_secs(failure.attempts) {
                return failure.value.clone();
            }
        }
    }

    let cfg = load_github_config();
    let owner = cfg
        .get("owner")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let repo = cfg
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let configured_proxy = cfg
        .get("proxy")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let proxy = proxy
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or(configured_proxy);
    let release_url = format!("https://github.com/{owner}/{repo}/releases/latest");
    let current = APP_VERSION.to_string();

    // 主端点：updater manifest（release 资产下载，不计 GitHub API 配额）。
    match fetch_manifest_version(&owner, &repo, proxy).await {
        Ok(manifest) => {
            let version = manifest
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let latest = version.strip_prefix('v').unwrap_or(version).to_string();
            let tag = format!("v{latest}");
            let release_name = meaningful_release_name(&manifest, &tag, &latest);
            let published_at = manifest
                .get("pub_date")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let notes = manifest_notes(&manifest);
            // 版本号不合法时不得猜：明确报「无法比较」，不给出可下载按钮。
            let has_update = match compare_versions(&latest, &current) {
                Ok(ordering) => ordering == Ordering::Greater,
                Err(error) => {
                    error_log::record("backend", "更新版本号无法比较", &error);
                    let value = json!({
                        "ok": false,
                        "current": current,
                        "latest": latest,
                        "latestTag": tag,
                        "hasUpdate": false,
                        "manifestAvailable": true,
                        "releaseUrl": release_url,
                        "checkedAt": now_secs(),
                        "error": error,
                        "message": format!("无法比较版本号：{error}"),
                    });
                    return remember_failure(value);
                }
            };
            let mut value = json!({
                "ok": true,
                "current": current,
                "latest": latest,
                "latestTag": tag,
                "hasUpdate": has_update,
                "manifestAvailable": true,
                "releaseUrl": release_url,
                "publishedAt": published_at,
                "notes": notes,
                "checkedAt": now_secs(),
            });
            if let Some(name) = release_name {
                value["releaseName"] = json!(name);
            }
            *locks::recover(&CACHE) = Some(CachedCheck {
                checked_at: now_secs(),
                value: value.clone(),
            });
            *locks::recover(&FAILURE) = None;
            value
        }
        Err(manifest_error) => {
            // 兜底端点：`/releases/latest` 的 302 Location 头解析 tag（仅 manifest 失败时）。
            match fetch_latest_tag(&owner, &repo, proxy).await {
                Ok(tag) => {
                    let latest = tag.strip_prefix('v').unwrap_or(&tag).to_string();
                    let has_update = match compare_versions(&latest, &current) {
                        Ok(ordering) => ordering == Ordering::Greater,
                        Err(error) => {
                            let value = json!({
                                "ok": false,
                                "current": current,
                                "latest": latest,
                                "latestTag": tag,
                                "hasUpdate": false,
                                "manifestAvailable": false,
                                "releaseUrl": release_url,
                                "checkedAt": now_secs(),
                                "error": error,
                                "message": format!("无法比较版本号：{error}"),
                            });
                            return remember_failure(value);
                        }
                    };
                    // manifest 不可达时只凭 tag 判断版本：能提示「有新版本」，
                    // 但下载大概率也会失败，前端据此提示先配代理。
                    let message = if has_update {
                        Some("已发现新版本，但更新清单不可达，请配置并保存更新代理后重试")
                    } else {
                        None
                    };
                    let mut value = json!({
                        "ok": true,
                        "current": current,
                        "latest": latest,
                        "latestTag": tag,
                        "hasUpdate": has_update,
                        "manifestAvailable": false,
                        "releaseUrl": release_url,
                        "checkedAt": now_secs(),
                    });
                    if let Some(message) = message {
                        value["message"] = json!(message);
                    }
                    *locks::recover(&CACHE) = Some(CachedCheck {
                        checked_at: now_secs(),
                        value: value.clone(),
                    });
                    value
                }
                Err((msg, code)) => {
                    let value = json!({
                        "ok": false,
                        "error": msg,
                        "message": format!("{msg}（code={code}）"),
                        "manifestAvailable": false,
                        "releaseUrl": release_url,
                    });
                    error_log::record(
                        "backend",
                        "检查更新失败",
                        &format!("manifest={manifest_error} fallback={msg} code={code}"),
                    );
                    remember_failure(value)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_manifest_urls_macos_skips_duplicate_fallback() {
        let urls = updater_manifest_urls(GITHUB_OWNER, GITHUB_REPO, "macos", "aarch64");
        assert_eq!(
            urls,
            vec![
                "https://github.com/magicapple123/workbuddy-shift/releases/latest/download/latest.json",
                "https://github.com/magicapple123/workbuddy-shift/releases/latest/download/latest-macos-aarch64.json",
            ]
        );
    }

    #[test]
    fn updater_manifest_urls_windows_keeps_macos_compat() {
        let urls = updater_manifest_urls(GITHUB_OWNER, GITHUB_REPO, "windows", "x86_64");
        assert_eq!(
            urls,
            vec![
                "https://github.com/magicapple123/workbuddy-shift/releases/latest/download/latest.json",
                "https://github.com/magicapple123/workbuddy-shift/releases/latest/download/latest-windows-x86_64.json",
                "https://github.com/magicapple123/workbuddy-shift/releases/latest/download/latest-macos-x86_64.json",
            ]
        );
    }

    /// 默认更新源必须是本项目自己的发布仓库（上游 changexbc 会让用户被"升级"回原版）。
    #[test]
    fn github_constants_point_at_own_repository() {
        assert_eq!(GITHUB_OWNER, "magicapple123");
        assert_eq!(GITHUB_REPO, "workbuddy-shift");
    }

    /// 单个用例独占的临时配置文件（绝不触碰真实 `~/.workbuddy-shift`）。
    struct TempConfig {
        dir: PathBuf,
        file: PathBuf,
    }

    impl TempConfig {
        /// `content` 为 `None` 表示连配置文件都不存在。
        fn new(name: &str, content: Option<&str>) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wb_shift_update_test_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("github_config.json");
            if let Some(content) = content {
                std::fs::write(&file, content).unwrap();
            }
            Self { dir, file }
        }

        fn raw(&self) -> String {
            std::fs::read_to_string(&self.file).unwrap()
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 用户自己配过别的更新源 → 原样保留，连文件都不改写（那是用户的选择）。
    #[test]
    fn load_github_config_keeps_custom_source() {
        let raw = r#"{"owner":"someone-else","repo":"their-repo","proxy":"http://127.0.0.1:7890"}"#;
        let temp = TempConfig::new("custom-source", Some(raw));
        let config = load_github_config_at(&temp.file);
        assert_eq!(config["owner"], "someone-else", "{config}");
        assert_eq!(config["repo"], "their-repo");
        assert_eq!(config["proxy"], "http://127.0.0.1:7890");
        assert_eq!(temp.raw(), raw, "自定义更新源不得被改写");
    }

    /// 配置里缺 `owner` → 落到默认常量；缺文件同样落到默认常量。
    #[test]
    fn load_github_config_defaults_when_owner_missing() {
        let temp = TempConfig::new("owner-missing", Some(r#"{"repo":"workbuddy-shift"}"#));
        let config = load_github_config_at(&temp.file);
        assert_eq!(config["owner"], GITHUB_OWNER, "{config}");
        assert_eq!(config["owner"], "magicapple123");

        let absent = TempConfig::new("no-file", None);
        let config = load_github_config_at(&absent.file);
        assert_eq!(config["owner"], "magicapple123", "{config}");
        assert_eq!(config["repo"], "workbuddy-shift");
    }

    /// 更新说明：trim、按字符截断、空说明返回 None（老 Release 没有 notes 字段）。
    #[test]
    fn manifest_notes_trims_truncates_and_skips_empty() {
        let manifest = json!({"version": "0.2.0", "notes": "  修复若干问题\n\n第二行  "});
        assert_eq!(
            manifest_notes(&manifest).as_deref(),
            Some("修复若干问题\n\n第二行")
        );

        assert_eq!(manifest_notes(&json!({"version": "0.2.0"})), None);
        assert_eq!(
            manifest_notes(&json!({"version": "0.2.0", "notes": "   "})),
            None
        );
        assert_eq!(
            manifest_notes(&json!({"version": "0.2.0", "notes": null})),
            None
        );

        let long = "改".repeat(NOTES_MAX_CHARS + 500);
        let notes =
            manifest_notes(&json!({"version": "0.2.0", "notes": long})).expect("截断后仍应有说明");
        assert_eq!(notes.chars().count(), NOTES_MAX_CHARS);
        assert!(
            notes.chars().all(|c| c == '改'),
            "按字符截断，不得切坏多字节字符"
        );
    }

    /// `autoCheck` 缺省为 true；显式关闭后 load / auto_check_enabled 都必须如实反映。
    #[test]
    fn auto_check_defaults_to_true_and_follows_config() {
        let absent = TempConfig::new("auto-check-default", None);
        assert_eq!(
            load_github_config_at(&absent.file)["autoCheck"],
            json!(true)
        );
        assert!(auto_check_enabled_at(&absent.file), "缺省应允许自动检查");

        let off = TempConfig::new("auto-check-off", Some(r#"{"autoCheck":false}"#));
        assert_eq!(load_github_config_at(&off.file)["autoCheck"], json!(false));
        assert!(!auto_check_enabled_at(&off.file), "关闭后不得自动检查");

        // 旧配置带 token 会被归一化改写，autoCheck 同样要保留。
        let legacy = TempConfig::new(
            "auto-check-legacy",
            Some(
                r#"{"owner":"magicapple123","repo":"workbuddy-shift","token":"","autoCheck":false}"#,
            ),
        );
        assert_eq!(
            load_github_config_at(&legacy.file)["autoCheck"],
            json!(false)
        );
        assert!(!auto_check_enabled_at(&legacy.file));
        assert!(!legacy.raw().contains("token"), "token 必须被清掉");
        assert!(
            legacy.raw().contains("\"autoCheck\": false"),
            "{}",
            legacy.raw()
        );
    }

    /// 保存代理时前端不提交 autoCheck：必须沿用旧值，不能重置回默认开启。
    #[test]
    fn save_github_config_preserves_auto_check() {
        let temp = TempConfig::new(
            "save-auto-check",
            Some(
                r#"{"owner":"magicapple123","repo":"workbuddy-shift","proxy":"","autoCheck":false}"#,
            ),
        );
        save_github_config_at(
            &temp.dir,
            &temp.file,
            &json!({"proxy": "http://127.0.0.1:7897"}),
        )
        .unwrap();
        let config = load_github_config_at(&temp.file);
        assert_eq!(config["proxy"], "http://127.0.0.1:7897");
        assert_eq!(config["autoCheck"], json!(false), "缺省字段必须沿用旧值");
        assert_eq!(config["owner"], "magicapple123");

        // 设置页开关显式提交时按提交值落盘。
        save_github_config_at(
            &temp.dir,
            &temp.file,
            &json!({"proxy": "", "autoCheck": true}),
        )
        .unwrap();
        let config = load_github_config_at(&temp.file);
        assert_eq!(config["autoCheck"], json!(true));
        assert_eq!(config["proxy"], "");
    }

    /// 没有任何配置文件时保存：autoCheck 落到默认 true。
    #[test]
    fn save_github_config_defaults_auto_check_when_absent() {
        let temp = TempConfig::new("save-auto-check-default", None);
        save_github_config_at(
            &temp.dir,
            &temp.file,
            &json!({"owner": "someone", "repo": "r"}),
        )
        .unwrap();
        let config = load_github_config_at(&temp.file);
        assert_eq!(config["autoCheck"], json!(true));
        assert_eq!(config["owner"], "someone");
    }

    #[test]
    fn compare_versions_orders_semver() {
        assert_eq!(compare_versions("0.1.18", "0.1.18"), Ok(Ordering::Equal));
        assert_eq!(compare_versions("0.1.19", "0.1.18"), Ok(Ordering::Greater));
        assert_eq!(compare_versions("v0.1.17", "0.1.18"), Ok(Ordering::Less));
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ok(Ordering::Greater));
    }

    /// 第 19 条的三种预发布场景：旧实现把非数字段整段丢弃，导致比较结果错误。
    #[test]
    fn compare_versions_handles_prerelease() {
        assert_eq!(
            compare_versions("0.2.1-rc1", "0.2.0"),
            Ok(Ordering::Greater)
        );
        assert_eq!(compare_versions("0.2.0-rc1", "0.2.0"), Ok(Ordering::Less));
        assert_eq!(
            compare_versions("1.0.0-beta.1", "1.0.0"),
            Ok(Ordering::Less)
        );
        assert_eq!(
            compare_versions("0.2.1-rc.1", "0.2.1"),
            Ok(Ordering::Less),
            "预发布必须小于同号正式版"
        );
        assert_eq!(
            compare_versions("0.2.1-rc.2", "0.2.1-rc.1"),
            Ok(Ordering::Greater)
        );
    }

    /// 非法版本号必须返回 Err，不能静默退化成数字前缀比较。
    #[test]
    fn compare_versions_rejects_invalid_versions() {
        let error = compare_versions("not-a-version", "0.2.0").unwrap_err();
        assert!(error.contains("不是合法 semver"), "{error}");
        assert!(
            compare_versions("0.2", "0.2.0").is_err(),
            "两段式版本不可比较"
        );
    }

    /// 发布标题与 tag / 版本号相同时不应输出（否则前端渲染成 `v0.2.0 · v0.2.0`）。
    #[test]
    fn release_name_is_dropped_when_it_duplicates_version() {
        let manifest = json!({"version": "0.2.0", "name": "v0.2.0"});
        assert_eq!(meaningful_release_name(&manifest, "v0.2.0", "0.2.0"), None);
        let manifest = json!({"version": "0.2.0", "name": "0.2.0"});
        assert_eq!(meaningful_release_name(&manifest, "v0.2.0", "0.2.0"), None);
        let manifest = json!({"version": "0.2.0"});
        assert_eq!(meaningful_release_name(&manifest, "v0.2.0", "0.2.0"), None);
        let manifest = json!({"version": "0.2.0", "name": "第三轮修复"});
        assert_eq!(
            meaningful_release_name(&manifest, "v0.2.0", "0.2.0").as_deref(),
            Some("第三轮修复")
        );
    }

    #[test]
    fn failure_backoff_grows_and_caps() {
        assert_eq!(failure_backoff_secs(1), 600);
        assert_eq!(failure_backoff_secs(2), 1200);
        assert_eq!(failure_backoff_secs(3), 2400);
        assert_eq!(failure_backoff_secs(99), FAILURE_MAX_BACKOFF_SECS);
    }
}
