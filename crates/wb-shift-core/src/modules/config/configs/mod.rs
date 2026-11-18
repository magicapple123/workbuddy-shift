use super::paths::auto_rotate_config_file;
use super::paths::auto_rotate_logs_file;
use super::paths::checkin_config_file;
use super::paths::checkin_logs_file;
use super::paths::rate_limit_config_file;
use super::paths::store_dir;
use super::paths::travel_cache_file;
use super::paths::travel_config_file;
use super::store::atomic_write;
use super::store::norm_ts;
use super::store::now_ms;
use super::CHECKIN_LOG_KEEP_DAYS;
use super::CHECKIN_LOG_MAX_RECORDS;
use crate::modules::locks;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static CHECKIN_LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

static TRAVEL_CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Serialize travel-cache read-modify-write across depart and claim cycles.
pub fn with_travel_cache_lock<T>(f: impl FnOnce() -> T) -> T {
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续写入。
    let _guard = locks::recover(&TRAVEL_CACHE_WRITE_LOCK);
    f()
}

mod automation;
mod checkin;
mod travel;

pub use automation::{
    add_rotate_log, auto_rotate_notify_file, default_auto_rotate_config, default_rate_limit_config,
    load_auto_rotate_config, load_rate_limit_config, load_rate_limit_config_at, load_rotate_logs,
    save_auto_rotate_config, save_rate_limit_config_at, save_rotate_logs,
    set_rate_limit_hook_opt_out_at, try_consume_rotate_notify, ROTATE_LOG_MAX_RECORDS,
    ROTATE_NOTIFY_DAILY_LIMIT,
};
pub use checkin::{
    add_checkin_log, compact_checkin_logs, default_checkin_config, load_checkin_config,
    load_checkin_logs, parse_clock, save_checkin_config,
};
pub use travel::{
    default_travel_config, load_travel_cache, load_travel_config, save_travel_cache,
    save_travel_config,
};

#[allow(unused_imports)]
pub(crate) use automation::{
    local_date, rotate_notify_count_at, try_consume_rotate_notify_at, ROTATE_NOTIFY_FILE_NAME,
    ROTATE_NOTIFY_LOCK,
};
#[allow(unused_imports)]
pub(crate) use checkin::{
    apply_checkin_config, checkin_config_with_enabled, checkin_log_local_date,
    compact_checkin_logs_at, legacy_checkin_identity, legacy_default_checkin_config,
    load_checkin_config_at, merge_checkin_config, normalize_checkin_logs, resolve_checkin_config,
    save_checkin_logs_unlocked,
};
#[allow(unused_imports)]
pub(crate) use travel::{
    legacy_default_travel_config, load_travel_config_at, resolve_travel_config,
    travel_config_with_enabled,
};
