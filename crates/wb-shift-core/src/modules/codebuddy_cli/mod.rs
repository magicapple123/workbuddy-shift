//! CodeBuddy CLI 账号轮换桥接。
//!
//! Windows 直接维护 `settings.json.env.CODEBUDDY_AUTH_TOKEN`，绕过 CLI
//! 执行 `apiKeyHelper` 时的路径兼容问题；macOS/Linux 继续使用 helper。
//! 两种模式都复用 workbuddy-shift 的 WorkBuddy 账号库，但保持独立的当前账号。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
// Instant 只在 macOS/Linux 的进程轮询路径里使用；Windows 上无条件导入会触发 unused_imports，
// 而 CI 的 setup-rust-toolchain 默认带 -D warnings，会直接编译失败。
#[cfg(not(target_os = "windows"))]
use std::time::Instant;

use crate::modules::account;
use crate::modules::config::{atomic_write, home_dir, now_ms};
use crate::modules::process;
use crate::modules::variant::WbVariant;

const ROTATE_DIR: &str = ".codebuddy-rotate";
const STATE_FILE: &str = "state.json";
/// macOS/Linux 直接配置带 Node shebang 的 helper.cjs。旧 Windows helper
/// 常量只用于识别已发布版本，便于状态迁移和非 Windows 兼容测试。
const HELPER_FILE: &str = "helper.cjs";
const LOGIC_FILE: &str = "helper.cjs";
const LEGACY_HELPER_FILE: &str = "helper.sh";
const LEGACY_WINDOWS_HELPER_FILE: &str = "helper.cmd";
const SETTINGS_DIR: &str = ".codebuddy";
const SETTINGS_FILE: &str = "settings.json";
/// CLI 运行中会话注册表目录名（`~/.codebuddy/sessions/<pid>.json`）。
const SESSIONS_DIR_NAME: &str = "sessions";
/// 会话存活判据：`now - lastHeartbeat <= 该值` 即视为活着。
///
/// 对齐客户端 `WORKER_HEARTBEAT_TIMEOUT_MS`（120_000）。客户端心跳是 30s 一次的
/// 纯定时器（与是否正在对话无关），所以这个判据只回答「有没有活着的 CLI 进程」，
/// 不回答「用户是否正在用」——不要拿它当"活跃保护"用。
const LIVE_SESSION_STALE_MS: i64 = 120_000;
const CODEBUDDY_AUTH_TOKEN: &str = "CODEBUDDY_AUTH_TOKEN";
const CODEBUDDY_INTERNET_ENVIRONMENT: &str = "CODEBUDDY_INTERNET_ENVIRONMENT";
const CODEBUDDY_BASE_URL: &str = "CODEBUDDY_BASE_URL";
const CN_INTERNET_ENVIRONMENT: &str = "internal";
/// 官网 IAM：国际版「不设，或 public」。必须写成明确值，不能只删 key：
/// CLI 启动会把 local_storage 里的 Environment-Cache 写进 process.env，
/// settings.json 缺省时就会继续走国内站。
const AI_INTERNET_ENVIRONMENT: &str = "public";
const CN_CLI_ENDPOINT: &str = "https://copilot.tencent.com";
const AI_CLI_ENDPOINT: &str = "https://www.codebuddy.ai";
/// CLI `resolveModelBaseURL`：若设置了 `CODEBUDDY_BASE_URL`，会原样当作 OpenAI
/// client `baseURL`，**不会**再拼 `/v2`。写成门户根地址
/// `https://www.codebuddy.ai` 会 POST `/chat/completions` 到官网 nginx，返回
/// `405 Not Allowed`（nginx/1.27.3）。国际版 OpenAI 兼容接口是 `${endpoint}/v2`。
const AI_CLI_OPENAI_BASE_URL: &str = "https://www.codebuddy.ai/v2";
/// CodeBuddy CLI ProductManager 写入 `~/.codebuddy/local_storage/entry_<md5(key)>.info`
/// 的固定 key。切换档位时必须改这两份缓存，否则 settings.json 改了 CLI 仍打国内站。
const CLI_ENV_CACHE_KEY: &str = "CodeBuddy-Environment-Cache";
const CLI_ENDPOINT_CACHE_KEY: &str = "CodeBuddy-Endpoint-Cache";
const CLI_PRODUCT_CACHE_KEY: &str = "CodeBuddy-Product-Cache";
const STANDARD_HELPER: &str = include_str!("../../../../../scripts/codebuddy-cli-helper.cjs");

mod helper;
mod settings;
mod state;
mod switch;

pub use settings::sync_windows_env_for_account;
pub(crate) use state::sessions_dir;
pub use state::{has_live_session, status};
pub use switch::{install_helper, switch_active_account};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use super::helper::*;
    use super::settings::*;
    use super::state::*;
    use super::switch::*;
    include!("tests/fixtures.rs");
    include!("tests/state.rs");
    include!("tests/settings.rs");
    include!("tests/helper.rs");
}
