//! 每日去重状态：同一账号同一天只跑一次。
//!
//! # 文件
//!
//! `<store_root>/daily_activity_state.json`（`store_root` = `~/.workbuddy-shift`，开发
//! 构建被 `WB_SHIFT_HOME` 重定向到沙箱）：
//!
//! ```json
//! {
//!   "version": 1,
//!   "accounts": {
//!     "<账号 id>": { "date": "2026-11-22", "completedAt": 1763800000000, "model": "hy3" }
//!   }
//! }
//! ```
//!
//! **只记「哪天、用哪个模型成功过」**：不写 prompt、不写 token、不写沙箱地址，
//! 也不写会话内容。每个账号只保留最后一条成功记录（够用来判今天）。
//!
//! # 为什么要落盘
//!
//! 一次活跃会话会消耗上游额度（`hy3` 免费、`deepseek-v4.1-flash` 付费），重复跑没有
//! 任何收益。状态落盘后，进程重启、手动连点、宿主重启都不会重复消耗。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::modules::config::{atomic_write, store_dir};
use crate::modules::locks;

/// 状态文件名（相对 `store_dir()`）。
pub const STATE_FILE_NAME: &str = "daily_activity_state.json";
/// 当前文件版本（将来改结构时用它做迁移判断）。
pub const STATE_VERSION: u32 = 1;

/// 进程内串行化守卫：保证「读 → 合并 → 写」不被并发调用交错。
///
/// 只保护这一个 JSON 文件；跨进程并发（两个宿主同时手动触发）本轮不做文件锁，
/// 最坏结果是多跑一次，不会写坏文件（写入走 [`atomic_write`]）。
static STATE_LOCK: Mutex<()> = Mutex::new(());

/// 单个账号的成功记录。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountEntry {
    /// 成功那天的本地日期 `YYYY-MM-DD`。
    pub date: String,
    /// 成功时刻（毫秒时间戳），仅用于诊断。
    pub completed_at: i64,
    /// 实际使用的模型 id（`hy3` / `deepseek-v4.1-flash`）。
    pub model: String,
}

/// 去重状态文件内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActivityState {
    /// 文件版本。
    pub version: u32,
    /// 账号 key → 最后一次成功记录。
    pub accounts: BTreeMap<String, AccountEntry>,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            accounts: BTreeMap::new(),
        }
    }
}

impl ActivityState {
    /// 该账号是否在 `today`（本地日期）已经成功过。
    pub fn is_done_today(&self, account_key: &str, today: &str) -> bool {
        self.accounts
            .get(account_key)
            .map(|entry| entry.date == today)
            .unwrap_or(false)
    }

    /// 记一条成功（覆盖该账号的上一条）。
    pub fn mark_done(&mut self, account_key: &str, today: &str, model: &str, completed_at: i64) {
        self.version = STATE_VERSION;
        self.accounts.insert(
            account_key.to_string(),
            AccountEntry {
                date: today.to_string(),
                completed_at,
                model: model.to_string(),
            },
        );
    }
}

/// 状态文件路径。
pub fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE_NAME)
}

/// 今天的本地日期 `YYYY-MM-DD`（跨零点以调用时刻为准）。
pub fn local_today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 账号去重 key：优先 `id`（本机稳定标识），退到 `uid`；两者都空返回空串。
///
/// 不用 `access_token` 之类的凭据做 key，也不用邮箱：前者会进文件，后者可被改。
pub fn account_key(account: &Value) -> String {
    ["id", "uid"]
        .iter()
        .filter_map(|field| account.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// 读状态文件。
///
/// 文件缺失 / 损坏 / 结构不认 → 当作空状态：读不到状态只会多跑一次，不该让整轮
/// 直接失败（用户手动点一下却看到「状态文件坏了」毫无意义）。
pub fn load(path: &Path) -> ActivityState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<ActivityState>(&text).ok())
        .unwrap_or_default()
}

/// 写状态文件（原子写：临时文件 + rename）。
pub fn save(path: &Path, state: &ActivityState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建状态目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| format!("序列化状态失败: {e}"))?;
    atomic_write(path, &text).map_err(|e| format!("写入状态文件失败: {e}"))
}

/// 读盘判断该账号今天是否已完成。
pub fn done_today(path: &Path, account_key: &str, today: &str) -> bool {
    load(path).is_done_today(account_key, today)
}

/// 记一条成功（重新读盘后合并，避免覆盖进程内外的其它账号记录）。
pub fn record_success(
    path: &Path,
    account_key: &str,
    today: &str,
    model: &str,
    completed_at: i64,
) -> Result<(), String> {
    // 串行化守卫：`Mutex<()>` 只提供互斥、没有数据；中毒时取回内部值继续是安全的
    // （临界区只有「读盘 → 合并 → 原子写」，文件本身不会被写坏）。
    let _serial = locks::recover(&STATE_LOCK);
    let mut state = load(path);
    state.mark_done(account_key, today, model, completed_at);
    save(path, &state)
}

#[cfg(test)]
pub(super) mod tests_support {
    use super::*;

    /// 单测用临时目录（每个用例一个，互不干扰）。
    pub(crate) struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let unique = format!(
                "wb-daily-activity-{tag}-{}-{}",
                std::process::id(),
                chrono::Local::now()
                    .timestamp_nanos_opt()
                    .unwrap_or_default()
            );
            let path = std::env::temp_dir().join(unique);
            let _ = std::fs::create_dir_all(&path);
            Self { path }
        }

        pub(crate) fn join(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
