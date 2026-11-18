//! 状态持久化：事件文件 offset + 已归因限额条目的内存/落盘状态。

use super::*;

/// 后端状态文件名（offset + 已归因的限额条目）。
pub(super) const STATE_FILE_NAME: &str = "rate_limit_state.json";

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

/// 一条已归因的限额事件（落盘形态）。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredEntry {
    pub(super) account_id: String,
    pub(super) model: Option<String>,
    pub(super) reset_at: i64,
    pub(super) first_seen_at: i64,
    pub(super) hit_count: u32,
}

/// 后端持有的状态：事件文件已读偏移 + 当前有效的限额条目。
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct State {
    pub(super) offset: u64,
    /// 最近一次入账的 hook 限额事件时刻（诊断用）。
    pub(super) last_event_at: Option<i64>,
    pub(super) events: Vec<StoredEntry>,
}

pub(super) static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE_NAME)
}

pub(super) fn load_state(path: &Path) -> State {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<State>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn save_state(path: &Path, state: &State) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(
        path,
        &serde_json::to_string_pretty(state).unwrap_or_default(),
    )
}
