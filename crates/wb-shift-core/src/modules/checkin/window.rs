//! 时间段策略：窗口判定、目标时刻抽签、到点未办判定与下一轮延迟。

use super::cycle::*;
use super::logs::*;
use super::*;

/// 默认时间段 8:00–22:00，用于用户没有配置 `checkin_start` / `checkin_end` 的情况。
///
/// 为什么不保留「不限制」：不限制等于一轮把所有账号串行签完（实测 34 个账号 61 秒内
/// 提交 34 次、间隔 1.8s），这是最像脚本的行为指纹。退化成默认窗口后，账号会被
/// `due_accounts` 的确定性抽签分散到 8:00–22:00，签到时刻自然错峰。
pub(super) const DEFAULT_CHECKIN_WINDOW: (u32, u32) = (8 * 60, 22 * 60);

/// 「到点未办」的判定范围，决定是否允许跨日补签。
///
/// 两种口径的分歧只在「今天的目标还没到」的账号上：默认兜底窗口若沿用跨日补签，
/// 它会在今天 14:00 就顶替昨天那个 20:00 被判成到点未办，于是分散能力被完全抵消，
/// 又回到「一轮打完」。所以两种来源必须用不同口径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DueScope {
    /// 用户显式配置的窗口：允许跨日补签（昨日目标未回应 → 立即补一轮，既有能力）。
    CatchUpAcrossDays,
    /// 默认兜底窗口：只认今日目标。窗口内按各自的今日 target 到点处理；窗口外限量补签
    /// （每轮最多 `OUTSIDE_WINDOW_BATCH` 个），避免只在夜间开机的用户整天漏签。
    TodayOnly,
}

/// 自动调度本轮使用的窗口与到期口径。
pub(super) struct ScheduleWindow {
    pub(super) window: (u32, u32),
    pub(super) scope: DueScope,
}

/// 窗口生效条件：两个字段都能解析成 `"HH:MM"` 且 `start < end`（不支持跨午夜）。
///
/// 返回当日窗口的 `[start_minute, end_minute)`；否则 `None`（= 未配置/非法）。
/// `None` 只表示「用户没有给出可用的时间段」，调用方必须自己决定默认值：
/// 自动调度用 `schedule_window` 退化为 `DEFAULT_CHECKIN_WINDOW`，手动入口
/// （`manual_window_skip_rows`）仍按「未配置 = 不限制立即签」处理。
pub(super) fn window_minutes(cfg: &Value) -> Option<(u32, u32)> {
    let (start_hour, start_minute) = cfg
        .get("checkin_start")
        .and_then(Value::as_str)
        .and_then(parse_clock)?;
    let (end_hour, end_minute) = cfg
        .get("checkin_end")
        .and_then(Value::as_str)
        .and_then(parse_clock)?;
    let start = start_hour * 60 + start_minute;
    let end = end_hour * 60 + end_minute;
    (start < end).then_some((start, end))
}

/// 自动调度实际使用的窗口与口径：一次给出，避免两个调用点各判一次 `window_minutes`
/// 而出现「窗口用默认、口径用配置」的不一致。
///
/// 用户配置了合法窗口 → 沿用既有跨日补签；走到默认兜底窗口 → 只看今日。
pub(super) fn schedule_window(cfg: &Value) -> ScheduleWindow {
    match window_minutes(cfg) {
        Some(window) => ScheduleWindow {
            window,
            scope: DueScope::CatchUpAcrossDays,
        },
        None => ScheduleWindow {
            window: DEFAULT_CHECKIN_WINDOW,
            scope: DueScope::TodayOnly,
        },
    }
}

/// `now`（ms）的本地分钟是否落在窗口 `[start, end)` 内。
///
/// 与抽签 target 同口径：`end` 为开区间。时间戳无法解析成本地时刻时保守判为不在
/// 窗口内（真实调用方传入 `now_ms()`，不会走到这一支）。
pub(super) fn now_in_window(now: i64, win: (u32, u32)) -> bool {
    Local
        .timestamp_millis_opt(now)
        .single()
        .is_some_and(|dt| (win.0..win.1).contains(&(dt.hour() * 60 + dt.minute())))
}

/// 手动批量签到的窗口跳过行：仅在 `respect_window` 且窗口生效且当前在窗口外时返回
/// `Some`（逐账号 `skipped` / `outside_checkin_window`）；否则返回 `None`，调用方走
/// 原有「立即签」路径。
///
/// 返回 `Some` 时整轮不查询状态、不提交、不写日志：行内不携带任何请求结果。
pub(super) fn manual_window_skip_rows(
    accounts: &[Value],
    cfg: &Value,
    respect_window: bool,
    now: i64,
) -> Option<Vec<Value>> {
    if !respect_window {
        return None;
    }
    let win = window_minutes(cfg)?;
    if now_in_window(now, win) {
        return None;
    }
    Some(
        accounts
            .iter()
            .map(|acc| {
                json!({
                    "accountId": acc.get("id").cloned().unwrap_or(Value::Null),
                    "email": account_display_name(acc),
                    "result": "skipped",
                    "error": Value::Null,
                    "reason": "outside_checkin_window",
                })
            })
            .collect(),
    )
}

/// 本地日期 `day` 的相邻日（`delta` 为 ±1 天）。
pub(super) fn shift_day(day: &str, delta: i64) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let days = chrono::Days::new(delta.unsigned_abs());
    let shifted = if delta < 0 {
        date.checked_sub_days(days)?
    } else {
        date.checked_add_days(days)?
    };
    Some(shifted.format("%Y-%m-%d").to_string())
}

/// 某账号在 `day` 的目标分钟：窗口内按 (本地日期, 账号) 确定性抽签。
///
/// 同一天重启不变、不同账号/用户天然错峰；不引入新依赖（`rand` 已移除）。
pub(super) fn target_minute(day: &str, account_key: &str, start: u32, end: u32) -> u32 {
    let mut hasher = DefaultHasher::new();
    day.hash(&mut hasher);
    account_key.hash(&mut hasher);
    start + (hasher.finish() % u64::from(end - start)) as u32
}

/// `day` 当天第 `minute` 分钟的本地时刻（ms）。
pub(super) fn target_at(day: &str, minute: u32) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(minute / 60, minute % 60, 0)?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp_millis())
}

/// 某账号在 `day` 的目标时刻（ms）。
pub(super) fn target_ms_for_day(day: &str, account: &Value, win: (u32, u32)) -> Option<i64> {
    target_at(day, target_minute(day, &account_key(account), win.0, win.1))
}

/// 最近一个已应到的目标时刻：未到今日目标时取昨日目标。
pub(super) fn latest_target_ms(now: i64, account: &Value, win: (u32, u32)) -> Option<i64> {
    let today = date_str(Some(now));
    let today_target = target_ms_for_day(&today, account, win)?;
    if now >= today_target {
        return Some(today_target);
    }
    target_ms_for_day(&shift_day(&today, -1)?, account, win)
}

/// 下一个目标时刻（今日未到则今日，已过则明日），用于计算睡眠时长。
pub(super) fn next_target_ms(now: i64, account: &Value, win: (u32, u32)) -> Option<i64> {
    let today = date_str(Some(now));
    let today_target = target_ms_for_day(&today, account, win)?;
    if now < today_target {
        return Some(today_target);
    }
    target_ms_for_day(&shift_day(&today, 1)?, account, win)
}

/// 该账号在 `since` 之后是否已有 `success` / `already` 日志（即该窗口是否已被回应）。
pub(super) fn logs_answered_window(logs: &[Value], account: &Value, since: i64) -> bool {
    let stable_id = account
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let display = account_display_name(account);
    logs.iter().any(|entry| {
        if entry
            .get("ts")
            .and_then(Value::as_i64)
            .is_none_or(|ts| ts < since)
        {
            return false;
        }
        if !matches!(
            entry.get("result").and_then(Value::as_str),
            Some("success" | "already")
        ) {
            return false;
        }
        match stable_id {
            // 稳定 id 是真源：只认 accountId，避免同一展示名互相误判。
            Some(id) => entry.get("accountId").and_then(Value::as_str) == Some(id),
            // 无 id 的账号写日志时 accountId 为空（null / 缺字段 / 空串），只能按展示名匹配。
            None => {
                entry
                    .get("accountId")
                    .and_then(Value::as_str)
                    .is_none_or(|id| id.is_empty())
                    && entry.get("email").and_then(Value::as_str) == Some(display.as_str())
            }
        }
    })
}

/// 窗口模式下该账号是否「到点未办」：最近一次目标时刻之后没有任何 success/already。
///
/// 「最近一次目标时刻」会跨日回退（今天还没到点 → 取昨天的目标），所以这是**允许跨日
/// 补签**的口径，只用于用户显式配置的窗口。
pub(super) fn account_due(now: i64, account: &Value, win: (u32, u32), logs: &[Value]) -> bool {
    match latest_target_ms(now, account, win) {
        Some(latest) => !logs_answered_window(logs, account, latest),
        // 日期无法解释（极端时区 / DST 缺口）：保守当作到点未办。
        None => true,
    }
}

/// 只看今日目标的到点判据（默认兜底窗口用），不跨日补签。
///
/// 与 `account_due` 的区别正是关键：今天的目标还没到的账号一律不 due——否则「今天
/// 20:00 才轮到我」的账号在 14:00 就会被昨天那轮顶替掉，默认窗口的分散能力归零。
///
/// 窗口内/外的分流在 `due_accounts` 里做（窗口外走限量补签），本函数只负责「今日、
/// 已到点、未回应」这一件事。
pub(super) fn account_due_today_only(
    now: i64,
    account: &Value,
    win: (u32, u32),
    logs: &[Value],
) -> bool {
    let today = date_str(Some(now));
    let Some(target) = target_ms_for_day(&today, account, win) else {
        // 日期无法解释（极端时区 / DST 缺口）：与 account_due 同口径，保守当作到点未办。
        return true;
    };
    if now < target {
        return false;
    }
    !logs_answered_window(logs, account, target)
}

/// 窗口外的限量补签上限（默认兜底口径专用）。
///
/// 为什么是 3：默认兜底下宿主是 30 分钟一轮（`next_cycle_delay` 未配置时保持既有节奏），
/// 3 × 2 = 每小时最多 6 个账号；22:00–次日 08:00 共 10 小时 → 最多 60 次提交，足够把
/// 30+ 账号的账号库补完，只在夜间开机的用户不再整天漏签。同时单轮上限把「批量提交」
/// 压得很低，不会回到 61 秒 34 次的脚本指纹。
pub(super) const OUTSIDE_WINDOW_BATCH: usize = 3;

/// 窗口外的补签候选：今日尚无 success/already 的账号，按今日 target 升序取前
/// `OUTSIDE_WINDOW_BATCH` 个。
///
/// 候选口径是「今日未回应」而不是「今日 target 已过」：跨零点后（例如 02:00）今日
/// 所有 target 都在未来，按「已过」筛会一个候补都没有，夜间补签退化成什么都不做。
/// 排序按今日 target 升序，保证补的顺序仍是「今天本该最早签的那几个」。
///
/// 无法解释本地日期（极端时区 / DST 缺口）时返回空：宁可这一轮不补，也不猜时间。
fn outside_window_batch(
    accounts: Vec<Value>,
    now: i64,
    win: (u32, u32),
    logs: &[Value],
) -> Vec<Value> {
    let today = date_str(Some(now));
    let Some(today_start) = target_at(&today, 0) else {
        return Vec::new();
    };
    let mut candidates: Vec<(i64, Value)> = accounts
        .into_iter()
        .filter_map(|acc| {
            if logs_answered_window(logs, &acc, today_start) {
                return None;
            }
            let target = target_ms_for_day(&today, &acc, win)?;
            Some((target, acc))
        })
        .collect();
    // 稳定排序：同一分钟的目标按账号库顺序，结果可复现。
    candidates.sort_by_key(|(target, _)| *target);
    candidates
        .into_iter()
        .take(OUTSIDE_WINDOW_BATCH)
        .map(|(_, acc)| acc)
        .collect()
}

/// 窗口生效时只保留「到点未办」的账号；`win` 为 `None`（不限制）时原样返回全部账号。
///
/// `scope` 决定是否跨日补签（见 `DueScope`）：用户配置窗口用 `CatchUpAcrossDays`，
/// 默认兜底窗口用 `TodayOnly`，否则补签会抵消默认窗口的错峰效果。
///
/// 默认兜底的窗口外分支在这里分派（而不是在 `DueScope` 里多开一档）：窗口内/外是**运行时
/// 时刻**决定的，不是配置属性；多开一档就要调用方自己算一次 `now_in_window`，两处判断
/// 迟早分叉。限量补签只作用于这一支，配置窗口（`CatchUpAcrossDays`）完全不受影响。
pub(super) fn due_accounts(
    accounts: Vec<Value>,
    now: i64,
    win: Option<(u32, u32)>,
    logs: &[Value],
    scope: DueScope,
) -> Vec<Value> {
    let Some(win) = win else {
        return accounts;
    };
    match scope {
        DueScope::CatchUpAcrossDays => accounts
            .into_iter()
            .filter(|acc| account_due(now, acc, win, logs))
            .collect(),
        // 窗口内：只处理今日已到点未回应的账号，不加数量上限（抽签本身已把它们分散到
        // 全天，同一分钟到点的账号本就不多）。
        DueScope::TodayOnly if now_in_window(now, win) => accounts
            .into_iter()
            .filter(|acc| account_due_today_only(now, acc, win, logs))
            .collect(),
        // 窗口外：限量补签，避免夜间开机的用户整天漏签，也不制造批量提交。
        DueScope::TodayOnly => outside_window_batch(accounts, now, win, logs),
    }
}

/// 记录本轮健康度：干净轮次把快速重试预算重置满。
pub(super) fn record_cycle_health(had_errors: bool) {
    LAST_CYCLE_HAD_ERRORS.store(had_errors, Ordering::Relaxed);
    if !had_errors {
        FAST_RETRY_BUDGET.store(FAST_RETRY_MAX, Ordering::Relaxed);
    }
}

/// 下一轮延迟的纯决策，返回 `(延迟, 剩余快速重试预算)`。
///
/// - 窗口未生效：既有 30 分钟节奏，预算不参与、不消耗（与改动前逐字一致）。
/// - 上一轮有 error 且预算未耗尽：5 分钟，并消耗一次预算。
/// - 否则：到最近一个目标时刻的剩余时长，夹在 `[1 分钟, 30 分钟]`；
///   上限同时充当窗口内的本地复核周期（只读本地状态，不发请求）。
pub(super) fn plan_next_delay(
    had_errors: bool,
    budget: u32,
    now: i64,
    accounts: &[Value],
    win: Option<(u32, u32)>,
) -> (Duration, u32) {
    let Some(win) = win else {
        return (CHECKIN_RECOVERY_INTERVAL, budget);
    };
    if had_errors && budget > 0 {
        return (FAST_RETRY_INTERVAL, budget - 1);
    }
    let nearest = accounts
        .iter()
        .filter_map(|acc| next_target_ms(now, acc, win))
        .min();
    let delay = match nearest {
        Some(target) => Duration::from_millis((target - now).max(0) as u64)
            .clamp(MIN_CYCLE_DELAY, CHECKIN_RECOVERY_INTERVAL),
        None => CHECKIN_RECOVERY_INTERVAL,
    };
    (delay, budget)
}

/// 宿主下一轮睡眠时长。策略全在 core，宿主只读这个时长，不解释 payload。
pub fn next_cycle_delay() -> Duration {
    let cfg = load_checkin_config();
    // 唤醒节奏仍只看用户配置的时间段：未配置时保持既有 30 分钟一轮，不因为
    // `run_checkin_cycle` 退化了默认窗口就把轮询提到分钟级——每次唤醒都要读账号库与
    // 签到日志，默认配置下没有理由加密唤醒。
    //
    // 与默认兜底口径配合后的实际效果：窗口内每轮只处理「过去 30 分钟内到点」的那批
    // 账号（TodayOnly 不补昨天的），34 个账号铺在 14 小时窗口里约每人 25 分钟一个；
    // 窗口外每轮再压到 `OUTSIDE_WINDOW_BATCH` 个。两种情况下单轮都是个位数量级，
    // 而不是一轮打完全部。
    let window = window_minutes(&cfg);
    // 窗口未生效时不必读账号库/日志：直接走既有 30 分钟节奏。
    let accounts = if window.is_some() {
        auto_checkin_accounts(load_accounts(), &cfg)
    } else {
        Vec::new()
    };
    let (delay, remaining) = plan_next_delay(
        LAST_CYCLE_HAD_ERRORS.load(Ordering::Relaxed),
        FAST_RETRY_BUDGET.load(Ordering::Relaxed),
        now_ms(),
        &accounts,
        window,
    );
    FAST_RETRY_BUDGET.store(remaining, Ordering::Relaxed);
    delay
}

/// 窗口模式下到点复核发现服务端「今日已签」→ 补一条 `already` 回执。
///
/// 回执让「本窗口已回应」跨重启成立，避免次日 00:0x 被误判为「窗口未回应」而提前签。
/// 只在该窗口尚无记录时补写（每账号每目标时刻至多一条），且仅窗口模式补写；
/// `checkin_account` / `run_checkin_all` 的公开 payload 不变。
pub(super) fn write_window_receipt(win: (u32, u32), account: &Value) {
    let now = now_ms();
    let Some(latest) = latest_target_ms(now, account, win) else {
        return;
    };
    if logs_answered_window(&load_checkin_logs(), account, latest) {
        return;
    }
    add_checkin_log(&json!({
        "ts": now,
        "accountId": account.get("id").cloned().unwrap_or(Value::Null),
        "email": account_display_name(account),
        "variant": variant_of(account).as_str(),
        "result": "already",
    }));
}
