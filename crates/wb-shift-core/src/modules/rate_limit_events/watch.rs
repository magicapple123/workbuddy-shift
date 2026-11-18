//! 事件消费与对外入口：按 offset 逐行消费、轮转、watcher 与查询入口。

use super::parse::*;
use super::state::*;
use super::*;
use crate::modules::locks;

/// 消费到该字节数后轮转事件文件，避免无限增长（一次事件一行，常规使用远低于此）。
pub(super) const COMPACT_AFTER_BYTES: u64 = 1024 * 1024;

/// watcher 轮询间隔：一次 `stat`，hook 事件要求秒级可见（AC1）。
pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// 消费
// ---------------------------------------------------------------------------

/// 事件文件是否还有未消费的字节（一次 `stat`；watcher 与查询路径共用）。
pub(super) fn has_pending_bytes(events: &Path, offset: u64) -> bool {
    std::fs::metadata(events)
        .map(|metadata| metadata.len() != offset)
        .unwrap_or(false)
}

/// 只取到最后一个换行为止的完整行；返回（行集合，已消费字节数）。
///
/// 写到一半的行必须留给下一轮：客户端可能在任意时刻 append，半行 JSON 解析不出任何东西，
/// 直接消费会把这一行吃掉。
pub(super) fn complete_lines(bytes: &[u8]) -> (Vec<String>, u64) {
    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return (Vec::new(), 0);
    };
    let consumed = (last_newline + 1) as u64;
    let mut text = String::from_utf8_lossy(&bytes[..last_newline]).into_owned();
    // Windows PowerShell 5.1 的 `Add-Content -Encoding UTF8` **创建文件时**会写 BOM：
    // 首行带 `\u{feff}` 前缀，serde_json 解析必失败 ⇒ hook 装好后的**第一个**限额事件
    // 会被静默丢弃（2026-09-18 本机实证）。只在文本开头剥一次，开销可忽略。
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    let lines = text
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    (lines, consumed)
}

/// 把一批行入账到状态；返回是否新增了条目。
pub(super) fn ingest_lines(
    ctx: &IngestContext,
    state: &mut State,
    lines: &[String],
    now: i64,
) -> bool {
    let mut added = false;
    for line in lines {
        let Some(event) = parse_quota_line(line) else {
            continue;
        };
        let Some(source) = source_of(&event.transcript_path, &ctx.source_roots) else {
            continue;
        };
        let Some(account_id) = attribute(&event, source, ctx, now) else {
            continue;
        };
        merge_entry(
            &mut state.events,
            StoredEntry {
                account_id,
                model: event.model.clone(),
                reset_at: event.reset_at,
                first_seen_at: now,
                hit_count: 1,
            },
        );
        added = true;
    }
    if added {
        state.last_event_at = Some(now);
    }
    added
}

/// 同一次事件（同账号 + 同模型 + 同恢复时刻）只保留一条，命中行数累加。
pub(super) fn merge_entry(events: &mut Vec<StoredEntry>, entry: StoredEntry) {
    if let Some(existing) = events.iter_mut().find(|existing| {
        existing.account_id == entry.account_id
            && existing.model == entry.model
            && existing.reset_at == entry.reset_at
    }) {
        existing.hit_count += entry.hit_count;
        existing.first_seen_at = existing.first_seen_at.min(entry.first_seen_at);
        return;
    }
    events.push(entry);
}

/// 清掉已过官方恢复时刻的条目，并转成台账条目。
pub(super) fn live_entries(state: &mut State, now: i64) -> Vec<Resolved> {
    state.events.retain(|entry| entry.reset_at > now);
    state
        .events
        .iter()
        .map(|entry| Resolved {
            account_id: entry.account_id.clone(),
            model: entry.model.clone(),
            reset_at: entry.reset_at,
            first_seen_at: entry.first_seen_at,
            hit_count: entry.hit_count,
        })
        .collect()
}

/// 消费新字节；返回 `None` 表示「没有新字节」，`Some(changed)` 表示已消费（changed = 有新条目）。
///
/// 偏移与条目一并持久化：进程重启后不重复入账，也不重放已消费的事件。
pub(super) fn consume_with(ctx: &IngestContext, state: &mut State, now: i64) -> Option<bool> {
    let bytes = std::fs::read(&ctx.events).ok()?;
    // 文件被外部截断/替换（编辑器保存、用户手删）时从头再来一次：重复条目会被 merge 吞掉。
    let truncated = (bytes.len() as u64) < state.offset;
    if truncated {
        state.offset = 0;
        let _ = save_state(&ctx.state_path, state);
    }
    if (bytes.len() as u64) == state.offset {
        return None;
    }
    let (lines, consumed) = complete_lines(&bytes[state.offset as usize..]);
    let mut changed = false;
    if consumed > 0 {
        changed = ingest_lines(ctx, state, &lines, now);
        state.offset += consumed;
    }
    let rotated = compact_if_needed(ctx, state, now);
    state.events.retain(|entry| entry.reset_at > now);
    let _ = save_state(&ctx.state_path, state);
    Some(changed || rotated)
}

/// 消费到轮转阈值时轮转事件文件：改名后再读旧 inode 的尾部增量，最后删除。
///
/// 不裁剪写入方正在追加的文件：`rename` 是原子的，改名**之前**的 append 都落在旧 inode 里，
/// 会被这里重新读到；改名之后的 append 落在新文件里，由下一轮消费。失败就放弃本次轮转
/// （文件继续增长，不丢事件）。
pub(super) fn compact_if_needed(ctx: &IngestContext, state: &mut State, now: i64) -> bool {
    if state.offset < COMPACT_AFTER_BYTES {
        return false;
    }
    // 还有未消费字节（例如末尾是写到一半的行）时不轮转，等下一轮读全。
    if has_pending_bytes(&ctx.events, state.offset) {
        return false;
    }
    let rotated = ctx.events.with_extension("jsonl.1");
    if std::fs::rename(&ctx.events, &rotated).is_err() {
        return false;
    }
    let Ok(bytes) = std::fs::read(&rotated) else {
        // 读不到就退回原名，保持「文件仍是事件源」的形态。
        let _ = std::fs::rename(&rotated, &ctx.events);
        return false;
    };
    let mut added = false;
    if (bytes.len() as u64) > state.offset {
        let (lines, _) = complete_lines(&bytes[state.offset as usize..]);
        added = ingest_lines(ctx, state, &lines, now);
    }
    let _ = std::fs::remove_file(&rotated);
    state.offset = 0;
    added
}

// ---------------------------------------------------------------------------
// 对外入口
// ---------------------------------------------------------------------------

/// 消费事件文件里的新事件；返回是否入账了新条目。
pub(crate) fn consume_pending() -> bool {
    let events = rate_limit_hook::events_path();
    let state_file = state_path();
    // 状态以事件文件为准：中毒时取回 offset/归因结果继续消费。最坏情况是重复消费同一段
    // 事件（同账号/同模型的 hit_count 偏大），但不会丢掉已经发生的限额。
    let mut guard = locks::recover(&STATE);
    let state = guard.get_or_insert_with(|| load_state(&state_file));
    // 快速路径：一次 stat，没有新字节就直接返回（watcher 每秒走这里）。
    if !has_pending_bytes(&events, state.offset) {
        return false;
    }
    consume_with(&IngestContext::real(), state, now_ms()).unwrap_or(false)
}

/// 当前有效的 hook 条目（已归因；过期条目在这里清掉）。
///
/// 顺带消费一次待处理事件：即使 watcher 线程没跑（例如 webui 侧未启用），
/// 查询路径也能拿到最新状态。
pub(crate) fn hook_entries(now: i64) -> Vec<Resolved> {
    let _ = consume_pending();
    // 限额状态以事件文件为准，内存态只是 offset + 归因结果：中毒时取回并继续，
    // 比丢弃条目（让限额提示消失）更接近真实状态。
    let mut guard = locks::recover(&STATE);
    let state = guard.get_or_insert_with(|| load_state(&state_path()));
    live_entries(state, now)
}

/// 最近一次入账的 hook 限额事件时刻（诊断用）。
pub fn last_event_at() -> Option<i64> {
    {
        let guard = locks::recover(&STATE);
        if let Some(state) = guard.as_ref() {
            return state.last_event_at;
        }
    }
    load_state(&state_path()).last_event_at
}

/// 启动 watcher：轮询事件文件，有新条目时回调（宿主据此 emit `rate-limits-updated`）。
///
/// 开关（`rate_limit_config.json`）只影响「是否通知前端」：信号照常入账，关闭期间
/// 发生的限额在重新打开开关后仍在（后端持有）。
pub fn spawn_watcher(emit: impl Fn() + Send + 'static) {
    std::thread::spawn(move || loop {
        if consume_pending() && rate_limit_enabled() {
            emit();
        }
        std::thread::sleep(POLL_INTERVAL);
    });
}

/// 限额监听开关（默认开启）。
pub(crate) fn rate_limit_enabled() -> bool {
    crate::modules::config::load_rate_limit_config()
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}
