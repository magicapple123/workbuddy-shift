//! Token 刷新、刷新失败分级与「需重新登录」标记的复核。
//!
//! 对照 server.py `refresh_account_token` / `ensure_fresh_token` /
//! `run_keepalive_cycle`。R5-08（上游 issue #190）在这条链路上补了两件事：
//!
//! 1. **刷新失败分级**（[`classify_refresh_failure`]）：只有「认证被吊销」才写
//!    `needs_relogin`。断网 / 代理挂掉 / 超时 / 5xx / 429 / HTML 错误页属于
//!    「暂时失败」，只写 `refresh_failed_at` / `refresh_failed_reason` 供诊断，
//!    保留原凭据并稍后重试。旧实现把 `code != 0 && code != 200` 一律当成
//!    「需重新登录」，一次网络抖动就会把可用账号永久标红。
//! 2. **认证恢复后的复核**（[`verify_relogin`]）：带标记的账号在任意已认证请求
//!    路径上触发一次侧路复核——真正打一次刷新接口，成功才清除标记。标记只在
//!    「续期成功」时清除，**不得**因为积分查询成功就清除（issue 明确要求）。
//!
//! 复核的三道闸都在本文件内：
//! - 单飞：进程内 `Mutex<HashSet<account_id>>`，同账号同时只有一次（[`VerifySlot`]）；
//! - 冷却：`needs_relogin_checked_at` + 退避阶梯（60s → 5min → 30min → … → 6h 上限）；
//! - 旁路：[`note_auth_success`] / [`ensure_fresh_token`] 只 `spawn` 复核，不 await，
//!   触发它的那次请求的返回值不受影响。会内联刷新的账号不再 spawn（[`should_spawn_verify`]），
//!   否则同一账号会并发打两次刷新接口，refresh token 轮换会让两次请求互相作废。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::modules::account::{build_auth_headers, upsert_account};
use crate::modules::config::{http_request, load_checkin_config, norm_ts, now_ms, RunFlagGuard};

static KEEPALIVE_RUNNING: AtomicBool = AtomicBool::new(false);
static RELOGIN_SWEEP_RUNNING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// R5-08 A：刷新失败分级
// ---------------------------------------------------------------------------

/// 上游「认证被吊销」业务码：`Invalid refresh token` / `Session doesn't have
/// required client`。两个档位都用这个码表示 refresh token 已不可用。
const CODE_REFRESH_REVOKED: i64 = 12153;

/// 刷新失败分类。**只有 [`RefreshFailure::Revoked`] 允许写 `needs_relogin`。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    /// 认证已被吊销（refresh token 失效 / 被拒 / 已过期）：必须重新登录。
    Revoked,
    /// 暂时失败（网络、代理、超时、5xx、429、HTML 错误页、响应缺字段）：
    /// 不写标记，保留原凭据，稍后自动重试。
    Temporary,
    /// 无法判断的业务码：保守按暂时失败处理，只写诊断字段。
    Unknown,
}

impl RefreshFailure {
    /// 该分类是否允许写「需重新登录」标记。
    pub fn marks_relogin(self) -> bool {
        matches!(self, Self::Revoked)
    }

    /// 写进诊断字段的分类名（日志/通知里可直接读）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Temporary => "temporary",
            Self::Unknown => "unknown",
        }
    }
}

/// 失败原因里「暂时失败」的说明：明确告诉用户会自动重试，避免被读成需要手动登录。
const TEMPORARY_HINT: &str = "暂时失败，稍后自动重试";

/// 从响应里取 `code`：兼容数字与数字字符串（网关偶发把 `code` 写成字符串）。
fn response_code(resp: &Value) -> Option<i64> {
    resp.get("code")
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .or_else(|| {
            resp.get("data").and_then(|data| {
                data.get("code").and_then(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
                })
            })
        })
}

/// 把响应里所有可读的错误文案拼成一段（`message` / `msg` / `data.*`）。
///
/// 分级只看响应本身，不推断上游故障原因（与上游 #191 的非目标一致）；因此这里
/// 不做任何「根据 code 反推文案」的事，只做取证。
fn response_message(resp: &Value) -> String {
    let mut text = String::new();
    for holder in [Some(resp), resp.get("data")] {
        let Some(holder) = holder else { continue };
        for key in ["message", "msg"] {
            if let Some(value) = holder.get(key).and_then(Value::as_str) {
                let value = value.trim();
                if !value.is_empty() {
                    text.push_str(value);
                    text.push(' ');
                }
            }
        }
    }
    text.trim().to_string()
}

/// 文案是否来自网关的 HTML 错误页（`normalize_error_body` 已归一化为该前缀）。
fn looks_like_html_error(message: &str) -> bool {
    let text = message.trim_start();
    text.starts_with('<') || text.contains("服务端返回 HTML 错误页")
}

/// 文案里是否出现吊销语义。
///
/// 这里的关键词都来自实测：`12153` 的两个官方文案（`invalid_grant: Invalid
/// refresh token`、`Session doesn't have required client`）以及网关的
/// `refresh token expired / revoked`。误判方向是「该提示时不提示」，因此宁可窄一点。
fn has_revocation_semantics(message: &str) -> bool {
    let text = message.to_lowercase();
    if [
        "invalid_grant",
        "invalid refresh token",
        "invalid_refresh_token",
        "required client",
        "revoked",
        "吊销",
        "已失效",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
    {
        return true;
    }
    // 「refresh token 过期」只有在明确指向 refresh token 时才算吊销：单纯
    // 「access token 过期」是正常续期场景，不能据此要求重新登录。
    (text.contains("refresh token") || text.contains("refresh_token"))
        && (text.contains("expired") || text.contains("过期") || text.contains("失效"))
}

/// 文案里是否出现「传输层/网关」语义（没有 code 时用来兜底判暂时失败）。
fn has_transport_semantics(message: &str) -> bool {
    let text = message.to_lowercase();
    [
        "timed out",
        "timeout",
        "deadline",
        "connection",
        "connect ",
        "dns",
        "proxy",
        "代理",
        "reset by peer",
        "unexpected eof",
        "tls",
        "certificate",
        "bad gateway",
        "gateway",
        "service unavailable",
        "too many requests",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
}

/// 刷新失败分级。**纯函数**：判据集中在这里，便于按实测补关键词。
///
/// 传入的应当是一次**失败**的刷新响应（`code` 不是 0/200）。成功码会被归到
/// [`RefreshFailure::Unknown`]，调用方不应依赖这一支。
pub fn classify_refresh_failure(resp: &Value) -> RefreshFailure {
    let code = response_code(resp);
    let message = response_message(resp);

    // 网关的 401/5xx 常返回整页 HTML（`http_request` 已归一化成
    // 「服务端返回 HTML 错误页：<title>」）。这既不是吊销也不是业务码 —— 先判掉，
    // 否则会被下面的 401/403 分支误判成「需重新登录」（issue #94 的教训）。
    if looks_like_html_error(&message) {
        return RefreshFailure::Temporary;
    }

    match code {
        // 刷新接口明确说 refresh token 不可用。
        Some(CODE_REFRESH_REVOKED) => return RefreshFailure::Revoked,
        // `http_request` 的传输错误约定：`{"code": -1, "message": ...}`。
        Some(-1) => return RefreshFailure::Temporary,
        // 限流与网关/服务端故障都是可重试的。
        Some(429) => return RefreshFailure::Temporary,
        Some(status) if (500..=599).contains(&status) => return RefreshFailure::Temporary,
        // 401/403：响应是 JSON 且不是 HTML 错误页 —— 刷新接口的鉴权拒绝。
        // 即使文案没写吊销关键词也按吊销处理：继续拿同一个 refresh token 重试
        // 没有意义，只会刷接口（冷却/退避仍然生效）。
        Some(401 | 403) => return RefreshFailure::Revoked,
        _ => {}
    }

    if has_revocation_semantics(&message) {
        return RefreshFailure::Revoked;
    }
    // 缺 `code`（`Value::Null`、非 JSON 响应体、字段被裁剪）或文案是传输层错误：
    // 无法确认凭据状态，按暂时失败处理，绝不给账号标「需重新登录」。
    if code.is_none() || has_transport_semantics(&message) {
        return RefreshFailure::Temporary;
    }
    // 其它业务码（例如网关指纹拦截 10085）：保守按暂时失败，只记诊断字段。
    RefreshFailure::Unknown
}

/// 失败响应 → 给人看的原因文案（分级 + code + 上游原话）。
fn refresh_failure_reason(failure: RefreshFailure, resp: &Value) -> String {
    let code = response_code(resp)
        .map(|code| code.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let message = response_message(resp);
    let message = if message.is_empty() {
        "上游未给出原因".to_string()
    } else {
        message
    };
    match failure {
        RefreshFailure::Revoked => format!("续期被拒（code={code}）：{message}"),
        RefreshFailure::Temporary => format!("续期{TEMPORARY_HINT}（code={code}）：{message}"),
        RefreshFailure::Unknown => {
            format!("续期结果未知（code={code}）：{message}，{TEMPORARY_HINT}")
        }
    }
}

// ---------------------------------------------------------------------------
// R5-08 A：标记的读写（纯内存，落盘由调用方负责）
// ---------------------------------------------------------------------------

/// 「需重新登录」标记的全部字段。清标记时必须一起清，否则会残留旧 reason /
/// 旧起点，前端 tooltip 会显示上一次失败的时间。
const RELOGIN_FIELDS: [&str; 5] = [
    "needs_relogin",
    "needs_relogin_reason",
    "needs_relogin_since",
    "needs_relogin_fail_count",
    "needs_relogin_last_result",
];

/// 暂时失败的诊断字段（不属于标记，清标记时一并清掉）。
const TEMPORARY_FIELDS: [&str; 2] = ["refresh_failed_at", "refresh_failed_reason"];

/// 账号是否带「需重新登录」标记。
pub fn has_relogin_marker(account: &Value) -> bool {
    account.get("needs_relogin").and_then(Value::as_bool) == Some(true)
}

/// 写入「需重新登录」标记（仅 [`RefreshFailure::Revoked`] / 缺 refresh token 时调用）。
///
/// 同时把 `needs_relogin_checked_at` 更新为本次尝试时间：刚失败的这一次本身就是
/// 一次「复核」，冷却从这里开始算，避免紧接着再打一次刷新接口。
fn mark_relogin_required(account: &mut Value, reason: String) {
    let now = now_ms();
    let already_marked = has_relogin_marker(account);
    let fail_count = account
        .get("needs_relogin_fail_count")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1;
    account["needs_relogin"] = json!(true);
    account["needs_relogin_reason"] = json!(reason);
    account["needs_relogin_fail_count"] = json!(fail_count);
    account["needs_relogin_checked_at"] = json!(now);
    if !already_marked {
        account["needs_relogin_since"] = json!(now);
    }
    if let Some(map) = account.as_object_mut() {
        for key in TEMPORARY_FIELDS {
            map.remove(key);
        }
    }
}

/// 记录一次「暂时失败」：**不动** `needs_relogin`（原标记保持原样）。
fn mark_temporary_failure(account: &mut Value, reason: String) {
    account["refresh_failed_at"] = json!(now_ms());
    account["refresh_failed_reason"] = json!(reason);
}

/// 清除「需重新登录」标记及其它刷新诊断字段（唯一清除点，只在续期成功时调用）。
fn clear_relogin_marker(account: &mut Value) {
    if let Some(map) = account.as_object_mut() {
        for key in RELOGIN_FIELDS {
            map.remove(key);
        }
        for key in TEMPORARY_FIELDS {
            map.remove(key);
        }
    }
}

// ---------------------------------------------------------------------------
// R5-08 B：复核的闸门（单飞 + 冷却 + 退避）
// ---------------------------------------------------------------------------

/// 首次复核的冷却窗口。
pub const RELOGIN_VERIFY_COOLDOWN_MS: i64 = 60_000;

/// 退避阶梯：60s → 5min → 30min → 1h → 2h → 4h → 6h（封顶）。
///
/// 吊销类失败连续发生时逐步拉长间隔：上游账号确实被吊销时不该每 60s 打一次接口。
const RELOGIN_BACKOFF_LADDER_MS: [i64; 7] = [
    60_000, 300_000, 1_800_000, 3_600_000, 7_200_000, 14_400_000, 21_600_000,
];

/// 当前账号的复核冷却时长（按连续失败次数取退避阶梯，上限 6h）。
pub fn verify_cooldown_ms(account: &Value) -> i64 {
    let fail_count = account
        .get("needs_relogin_fail_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let index = if fail_count <= 1 {
        0
    } else {
        (fail_count - 1).min(RELOGIN_BACKOFF_LADDER_MS.len() as i64 - 1) as usize
    };
    RELOGIN_BACKOFF_LADDER_MS[index]
}

/// 现在是否该复核这个账号（纯函数，`now` 由调用方注入便于单测）。
///
/// 条件：带标记 + 距上次复核（`needs_relogin_checked_at`）已超过冷却窗口。
/// 没有 `needs_relogin_checked_at`（历史标记）时视为「从未复核」，立即复核。
pub fn should_verify_relogin(account: &Value, now: i64) -> bool {
    if !has_relogin_marker(account) {
        return false;
    }
    let Some(last_checked_at) = account
        .get("needs_relogin_checked_at")
        .and_then(Value::as_i64)
    else {
        return true;
    };
    now.saturating_sub(last_checked_at) >= verify_cooldown_ms(account)
}

/// 复核前的判定结果。区分「为什么不做」而不是只给布尔值：日志、单测与将来
/// 的界面提示都要能区分「本来就不需要」「刚试过」「正在试」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyPlan {
    /// 该复核：带标记且已过冷却窗口。
    Proceed,
    /// 不带标记：正常账号，零额外请求。
    SkipNoMarker,
    /// 冷却中（含退避）：本轮不再打刷新接口。
    SkipCooldown,
}

/// 复核判定（纯函数，`now` 由调用方注入便于单测）。
///
/// 与 [`should_verify_relogin`] 同源但更细：`should_verify_relogin` 供批量扫描做
/// 过滤（只要布尔），本函数供 [`verify_relogin`] 给出可读的跳过原因。
pub fn verify_plan(account: &Value, now: i64) -> VerifyPlan {
    if !has_relogin_marker(account) {
        return VerifyPlan::SkipNoMarker;
    }
    if !should_verify_relogin(account, now) {
        return VerifyPlan::SkipCooldown;
    }
    VerifyPlan::Proceed
}

static VERIFY_SLOTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn verify_slots() -> &'static Mutex<HashSet<String>> {
    VERIFY_SLOTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 取在飞集合的锁。锁中毒时取回内部数据继续用：单飞是一层优化，
/// 不该因为一次 panic 变成全局不可用（`scripts/check-rust-locks.mjs` 同样禁止
/// 加锁后 unwrap/expect）。
fn lock_verify_slots() -> MutexGuard<'static, HashSet<String>> {
    verify_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 单飞令牌：拿到表示「本进程内该账号的复核由我负责」，Drop 时自动释放。
///
/// 并发合并的意义：账号页/积分页/签到/保活可能同时触发复核，没有这道闸就会
/// 对同一账号打多次 `/auth/token/refresh`（官方接口对并发刷新不友好）。
struct VerifySlot {
    account_id: String,
}

impl Drop for VerifySlot {
    fn drop(&mut self) {
        let mut slots = lock_verify_slots();
        slots.remove(&self.account_id);
    }
}

/// 尝试占用该账号的复核位；已被别人占用时返回 `None`（调用方应当安静跳过）。
fn try_acquire_verify_slot(account_id: &str) -> Option<VerifySlot> {
    let mut slots = lock_verify_slots();
    if !slots.insert(account_id.to_string()) {
        return None;
    }
    Some(VerifySlot {
        account_id: account_id.to_string(),
    })
}

/// 账号的稳定身份（`id` 优先，历史数据回退 `uid`）。
fn account_identity(account: &Value) -> Option<String> {
    for key in ["id", "uid"] {
        if let Some(value) = account.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 复核结束后回写复核时间与结果（`needs_relogin_checked_at` 是冷却的唯一依据）。
fn record_verify_result(account_id: &str, recovered: bool) {
    let Some(mut account) = crate::modules::account::find_account(account_id) else {
        return;
    };
    if recovered {
        // 成功复核后标记已由 `refresh_account_token` 清除；这里只补「上次复核」。
        clear_relogin_marker(&mut account);
    }
    account["needs_relogin_checked_at"] = json!(now_ms());
    account["needs_relogin_last_result"] = json!(if recovered { "ok" } else { "failed" });
    let _ = upsert_account(&account);
}

/// 复核一个带「需重新登录」标记的账号：真正打一次刷新接口，成功才清标记。
///
/// 返回值只用于日志/单测，**不是**业务结果：调用方（[`note_auth_success`]）不会
/// 等它，也不会改自己那次请求的返回值。
///
/// 顺序很重要：
/// 1. 先按 id 读档 → 不存在就跳过（不复活已删除账号）；
/// 2. 冷却检查（`needs_relogin_checked_at` + 退避）——被冷却时**不**打接口；
/// 3. 单飞占位——同账号并发只打一次；
/// 4. 占位后**重读**：期间账号可能被删、标记可能被别的路径清掉、展示字段
///    （备注 / 显示选择）可能被用户改过；重读后的对象作为刷新输入，
///    避免用旧快照覆盖这些字段；
/// 5. 复核 = `refresh_account_token`（吊销才写标记、成功清标记）；
/// 6. 回写 `needs_relogin_checked_at` 与结果。
pub async fn verify_relogin(account_id: &str) -> Value {
    let Some(account) = crate::modules::account::find_account(account_id) else {
        return json!({"accountId": account_id, "skipped": "missing"});
    };
    match verify_plan(&account, now_ms()) {
        VerifyPlan::Proceed => {}
        VerifyPlan::SkipNoMarker => {
            return json!({"accountId": account_id, "skipped": "no_marker"});
        }
        VerifyPlan::SkipCooldown => {
            return json!({"accountId": account_id, "skipped": "cooldown"});
        }
    }
    let Some(_slot) = try_acquire_verify_slot(account_id) else {
        return json!({"accountId": account_id, "skipped": "in_flight"});
    };
    let Some(current) = crate::modules::account::find_account(account_id) else {
        return json!({"accountId": account_id, "skipped": "deleted"});
    };
    if !has_relogin_marker(&current) {
        return json!({"accountId": account_id, "skipped": "no_marker"});
    }
    let verified = refresh_account_token(current).await;
    let recovered = !has_relogin_marker(&verified);
    record_verify_result(account_id, recovered);
    json!({
        "accountId": account_id,
        "verified": recovered,
        "skipped": Value::Null,
        "reason": verified
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// 旁路 spawn 一次复核：不阻塞调用方，也没有 tokio 运行时时安静跳过。
///
/// 同步上下文（例如 CLI 的一次性调用）里没有 runtime，此时复核留给下一次
/// 带 runtime 的触发点或宿主后台扫描（[`run_relogin_verify_sweep`]）。
fn spawn_verify_relogin(account_id: String) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = verify_relogin(&account_id).await;
        });
    }
}

/// 一次已认证请求成功后调用：带旧标记的账号顺带复核续期。
///
/// 未带标记的账号直接返回（零额外请求）——这是方案 1 的关键前提：正常账号不会
/// 因为这条钩子多打刷新接口。并发与频率由 [`verify_relogin`] 内部保证。
pub fn note_auth_success(account_id: &str) {
    let Some(account) = crate::modules::account::find_account(account_id) else {
        return;
    };
    if !has_relogin_marker(&account) {
        return;
    }
    spawn_verify_relogin(account_id.to_string());
}

/// 兜底扫描：给带标记的账号逐个复核（方案 2）。
///
/// 覆盖「只有后台任务在跑、没有前端请求」的场景。已过冷却的账号才处理，
/// 因此可以按任意间隔调用（建议 10 分钟）而不会产生额外请求。
pub async fn run_relogin_verify_sweep() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&RELOGIN_SWEEP_RUNNING) else {
        return json!({"skipped": "already_running"});
    };
    let now = now_ms();
    let targets: Vec<String> = crate::modules::account::load_accounts()
        .iter()
        .filter(|account| should_verify_relogin(account, now))
        .filter_map(account_identity)
        .collect();
    let mut results: Vec<Value> = Vec::with_capacity(targets.len());
    for account_id in targets {
        results.push(verify_relogin(&account_id).await);
    }
    json!({"checked": results.len(), "results": results})
}

// ---------------------------------------------------------------------------
// 刷新本体
// ---------------------------------------------------------------------------

/// 刷新接口 URL：域名与路径前缀都按账号自身档位选（缺档位字段 → 国内版，零回归）。
fn refresh_url(account: &Value) -> String {
    let variant = crate::modules::account::variant_of(account);
    format!(
        "{}{}/auth/token/refresh",
        variant.api_endpoint(),
        variant.api_prefix()
    )
}

/// 构造刷新请求头：鉴权头 + 刷新会话头。
///
/// `X-Auth-Refresh-Source: plugin` 必须保留：两个档位的官方客户端刷新时都发送该头，
/// 缺失时网关会把这次刷新判定成另一个 client 来源，国际版实测返回
/// `invalid_grant: Invalid refresh token`（code 12153）。抽成纯函数是为了让单测
/// 能把这个头固定住，避免将来被顺手删掉。
fn refresh_headers(account: &Value, refresh_token: &str) -> HashMap<String, String> {
    let mut headers = build_auth_headers(account);
    headers.insert("X-Refresh-Token".to_string(), refresh_token.to_string());
    headers.insert("X-Auth-Refresh-Source".to_string(), "plugin".to_string());
    headers
}

/// 刷新单账号 token（POST /v2/plugin/auth/token/refresh），成功则落盘并返回新账号。
///
/// 失败按 [`classify_refresh_failure`] 分级：
/// - `Revoked` / 缺 refresh token → 写 `needs_relogin`（确实无法续期）；
/// - `Temporary` / `Unknown` → **不写标记**，只写 `refresh_failed_at` /
///   `refresh_failed_reason`，保留原凭据等下一次重试。
///
/// 成功是**唯一**清除标记的地方（`needs_relogin` / reason / since / 退避计数一起清）。
pub async fn refresh_account_token(mut account: Value) -> Value {
    let previous_access_token = account
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let rt = account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    if rt.is_empty() {
        // 没有 refresh token 属于「确实无法续期」，按吊销处理。
        mark_relogin_required(
            &mut account,
            "缺少 refresh token，无法刷新，需重新登录".to_string(),
        );
        let _ = upsert_account(&account);
        return account;
    }

    let headers = refresh_headers(&account, &rt);
    // 刷新必须走账号自身档位的域名：把国际版 token 打到国内网关等于登录失效。
    let url = refresh_url(&account);
    let resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
    let recovered = apply_refresh_response(&mut account, &resp);
    let _ = upsert_account(&account);
    // Windows 不执行 apiKeyHelper；当前 CLI 账号刷新后同步 settings env。
    // 同步失败不阻断 WorkBuddy 保活；状态接口会根据 settings 与账号库是否
    // 一致显示“待同步”，避免把认证配置错误混入账号数据。
    // 只在续期成功时同步：暂时失败时 token 没变，同步是纯噪声。
    if recovered && cfg!(windows) {
        let _ = crate::modules::codebuddy_cli::sync_windows_env_for_account(
            &account,
            previous_access_token.as_deref(),
        );
    }
    account
}

/// 把一次刷新响应**应用到账号对象**上（纯逻辑，不做 IO，便于单测）。
///
/// 这是分级与标记的唯一落点，三条路径：
/// - 成功（`code` 为 0/200 且带 `accessToken`）→ 更新凭据 + 清除标记，返回 `true`；
/// - 失败 → [`classify_refresh_failure`] 分级：`Revoked` 写标记，其余只写诊断字段，
///   返回 `false`；
/// - `code` 是成功码但缺 `accessToken` → 无法确认凭据状态，按暂时失败处理（不写标记），
///   返回 `false`。
///
/// 返回值是本函数唯一表示「续期成功」的信号，调用方用它决定是否同步 CLI 配置。
fn apply_refresh_response(account: &mut Value, resp: &Value) -> bool {
    let code = response_code(resp).unwrap_or(-1);
    if code != 0 && code != 200 {
        let failure = classify_refresh_failure(resp);
        let reason = refresh_failure_reason(failure, resp);
        if failure.marks_relogin() {
            mark_relogin_required(account, reason);
        } else {
            // 断网 / 代理 / 5xx / 429 等：原标记保持原样，只记诊断字段。
            mark_temporary_failure(account, reason);
        }
        return false;
    }

    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let new_at = data
        .get("accessToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("access_token").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let Some(new_at) = new_at else {
        // 响应缺字段：无法确认凭据状态（可能是网关裁剪/错误页），按暂时失败处理，
        // 绝不给账号标「需重新登录」。
        mark_temporary_failure(
            account,
            format!("刷新响应缺少 accessToken（{TEMPORARY_HINT}）"),
        );
        return false;
    };

    account["access_token"] = json!(new_at);
    if let Some(new_rt) = data
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("refresh_token").and_then(|v| v.as_str()))
    {
        account["refresh_token"] = json!(new_rt);
    }
    // 官方接口只返回相对 expiresIn（秒），需换算为绝对时间戳
    let new_exp = norm_ts(data.get("expiresAt").or_else(|| data.get("expires_at")));
    let new_exp = match new_exp {
        Some(v) => Some(v),
        None => data
            .get("expiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_exp {
        account["expiresAt"] = json!(v);
    }
    let fallback_rt_exp = norm_ts(
        account
            .get("auth_raw")
            .and_then(|a| a.get("refreshExpiresAt")),
    );
    let mut new_rt_exp = norm_ts(
        data.get("refreshExpiresAt")
            .or_else(|| data.get("refresh_expires_at")),
    );
    if new_rt_exp.is_none() {
        new_rt_exp = fallback_rt_exp;
    }
    let new_rt_exp = match new_rt_exp {
        Some(v) => Some(v),
        None => data
            .get("refreshExpiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_rt_exp {
        account["refreshExpiresAt"] = json!(v);
    }
    account["refreshedAt"] = json!(now_ms());
    // 唯一清除点：续期成功才清「需重新登录」及全部刷新诊断字段。
    clear_relogin_marker(account);
    true
}

/// 惰性刷新：expiresAt 缺失或剩余 < lazy_refresh_hours 则刷新。返回最新账号。
///
/// R5-08：带「需重新登录」标记的账号在这里顺带触发一次侧路复核。选这里作为
/// 触发点的原因：积分查询（`credits::http::authenticated_post`）、签到轮次、
/// 旅行轮次都要先过这道闸，覆盖面比逐个在成功分支加钩子更广，而且不需要改动
/// 那些模块。复核是 `spawn` 出去的，本次请求的时延与返回值都不受影响。
///
/// **判定顺序很重要**：先算出「本次会不会内联刷新」。会内联刷新时**不再** spawn
/// 复核——那次内联刷新本身就是一次复核（成功清标记 / 吊销写标记），再 spawn 一次
/// 就等于对同一账号并发打两次 `/auth/token/refresh`；官方刷新带 refresh token
/// 轮换，并发刷新会互相把对方的 refresh token 作废，反而制造「需重新登录」。
pub async fn ensure_fresh_token(mut account: Value, cfg: &Value) -> Value {
    let lazy_h = cfg
        .get("lazy_refresh_hours")
        .and_then(|v| v.as_i64())
        .unwrap_or(24);
    let exp = account.get("expiresAt").and_then(|v| v.as_i64());
    let stale = match exp {
        Some(e) => now_ms() >= e || e - now_ms() < lazy_h * 3600 * 1000,
        None => true,
    };
    let has_rt = !account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty();
    let will_refresh = stale && has_rt;
    // 只有「本次不会内联刷新」且「确实有 refresh token 可试」时才旁路复核：
    // 没有 refresh token 的账号刷新必然失败，spawn 出去也只是白读一次账号库。
    if should_spawn_verify(stale, has_rt, has_relogin_marker(&account)) {
        if let Some(account_id) = account_identity(&account) {
            spawn_verify_relogin(account_id);
        }
    }
    if will_refresh {
        account = refresh_account_token(account).await;
    }
    account
}

/// `ensure_fresh_token` 是否应当旁路复核（纯函数，便于单测固定「内联刷新时不 spawn」）。
///
/// - `stale`：本次会不会内联刷新（token 过期 / 临期 / 缺 expiresAt）；
/// - `has_rt`：有 refresh token 可试；
/// - `marked`：带「需重新登录」标记。
///
/// 三条同时成立才复核：会内联刷新的账号由那次刷新自己完成复核，重复 spawn 会让
/// 同一账号并发打两次刷新接口，而 refresh token 轮换会因此互相作废。
fn should_spawn_verify(stale: bool, has_rt: bool, marked: bool) -> bool {
    !stale && has_rt && marked
}

/// 保活检查：每天由后台循环调用一次，默认（keepalive_days <= 0）无条件刷新
/// 全部带 refresh token 的账号；keepalive_days > 0 时仅刷新剩余不足该天数的账号。
///
/// 高频保活是为了避免官方服务端清理闲置的 refresh 会话——曾出现闲置数天后
/// 刷新返回 12153 invalid_grant（Session doesn't have required client）导致
/// 账号被迫重新登录。
///
/// 每个账号的结果分三档：`ok` / `failed`（吊销，需重新登录）/ `temporary_failed`
/// （断网、5xx 等，稍后自动重试，**不算**账号失效）。
pub async fn run_keepalive_cycle() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&KEEPALIVE_RUNNING) else {
        return json!({"skipped": "already_running"});
    };
    let cfg = load_checkin_config();
    let keep_days = cfg
        .get("keepalive_days")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let accounts = crate::modules::account::load_accounts();
    let total = accounts.len();
    let mut results: Vec<Value> = Vec::new();
    for mut acc in accounts {
        let exp = acc.get("expiresAt").and_then(|v| v.as_i64());
        let stale = keep_days <= 0
            || match exp {
                Some(e) => now_ms() >= e || e - now_ms() < keep_days * 24 * 3600 * 1000,
                None => true,
            };
        if !stale {
            continue;
        }
        if acc
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            mark_relogin_required(
                &mut acc,
                "缺少 refresh token，无法保活，需重新登录".to_string(),
            );
            let _ = upsert_account(&acc);
            results.push(json!({
                "email": crate::modules::account::account_display_name(&acc),
                "status": "missing_rt",
            }));
            continue;
        }
        let fresh = refresh_account_token(acc).await;
        let failed = has_relogin_marker(&fresh);
        let temporary = !failed
            && fresh
                .get("refresh_failed_at")
                .is_some_and(|value| value.is_i64());
        let status = if failed {
            "failed"
        } else if temporary {
            "temporary_failed"
        } else {
            "ok"
        };
        let reason = if failed {
            fresh.get("needs_relogin_reason")
        } else {
            fresh.get("refresh_failed_reason")
        };
        results.push(json!({
            "email": crate::modules::account::account_display_name(&fresh),
            "status": status,
            "error": if failed || temporary {
                reason.and_then(|v| v.as_str()).map(|s| s.to_string())
            } else {
                None
            },
        }));
    }
    json!({"checked": total, "refreshed": results})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    // -----------------------------------------------------------------------
    // 分级（R5-08 A）
    // -----------------------------------------------------------------------

    /// 12153 / invalid_grant / 401 吊销语义 → Revoked（只有这一类能写标记）。
    #[test]
    fn classify_revoked_failures() {
        let cases = [
            json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"}),
            json!({"code": 12153, "message": "Session doesn't have required client"}),
            json!({"code": 400, "message": "invalid_grant"}),
            json!({"code": 400, "message": "Invalid refresh token"}),
            json!({"code": 400, "message": "refresh token expired"}),
            json!({"code": 401, "message": "refresh token has been revoked"}),
            json!({"code": 403, "message": "refresh_token 已过期"}),
            // 字符串 code 也不能漏（网关偶发把 code 写成字符串）。
            json!({"code": "12153", "message": "invalid_grant"}),
        ];
        for resp in cases {
            assert_eq!(
                classify_refresh_failure(&resp),
                RefreshFailure::Revoked,
                "应当判为吊销：{resp}"
            );
        }
    }

    /// 断网 / 5xx / 429 / 超时 / HTML 错误页 → Temporary（绝不写标记）。
    #[test]
    fn classify_temporary_failures() {
        let cases = [
            // `http_request` 的传输错误约定。
            json!({"code": -1, "message": "error sending request for url (...) <- connection closed before message completed"}),
            json!({"code": -1, "message": "operation timed out"}),
            // 网关/服务端故障。
            json!({"code": 500, "message": "Internal Server Error"}),
            json!({"code": 502, "message": "Bad Gateway"}),
            json!({"code": 503}),
            json!({"code": 429, "message": "Too Many Requests"}),
            // 2xx 但响应体不是 JSON（`http_request` 返回 Null）。
            Value::Null,
            // 网关整页 HTML（401/502 都会这样）——必须优先按 HTML 判掉。
            json!({"code": 401, "message": "服务端返回 HTML 错误页：502 Bad Gateway"}),
            json!({"code": 502, "message": "<html><head><title>502</title></head></html>"}),
            // 缺 code 但文案是传输层错误。
            json!({"message": "dns error: failed to lookup address information"}),
        ];
        for resp in cases {
            assert_eq!(
                classify_refresh_failure(&resp),
                RefreshFailure::Temporary,
                "应当判为暂时失败：{resp}"
            );
        }
    }

    /// 其它业务码（无法判断）→ Unknown：保守按暂时失败处理，不写标记。
    #[test]
    fn classify_unknown_business_codes() {
        let resp = json!({"code": 10085, "message": "客户端指纹校验未通过"});
        let failure = classify_refresh_failure(&resp);
        assert_eq!(failure, RefreshFailure::Unknown);
        assert!(!failure.marks_relogin(), "Unknown 不得写 needs_relogin");
    }

    /// 只有 Revoked 允许写标记。
    #[test]
    fn only_revoked_marks_relogin() {
        assert!(RefreshFailure::Revoked.marks_relogin());
        assert!(!RefreshFailure::Temporary.marks_relogin());
        assert!(!RefreshFailure::Unknown.marks_relogin());
        assert_eq!(RefreshFailure::Temporary.label(), "temporary");
    }

    /// 原因文案必须能看出「续期被拒」与「稍后自动重试」的差别。
    #[test]
    fn reason_text_distinguishes_revoked_and_temporary() {
        let revoked = json!({"code": 12153, "message": "invalid_grant"});
        let text = refresh_failure_reason(classify_refresh_failure(&revoked), &revoked);
        assert!(text.contains("续期被拒"), "{text}");
        assert!(text.contains("12153"), "{text}");

        let offline = json!({"code": -1, "message": "connection refused"});
        let text = refresh_failure_reason(classify_refresh_failure(&offline), &offline);
        assert!(text.contains("暂时失败"), "{text}");
        assert!(text.contains("自动重试"), "{text}");
        assert!(!text.contains("重新登录"), "{text}");
    }

    // -----------------------------------------------------------------------
    // 标记读写：断网不标 / 401 标 / 成功清
    // -----------------------------------------------------------------------

    /// 断网（连接错误）不得写「需重新登录」，只写诊断字段。
    #[test]
    fn offline_failure_does_not_mark_relogin() {
        let mut account = json!({"id": "acc-offline", "access_token": "at", "refresh_token": "rt"});
        let resp = json!({"code": -1, "message": "error sending request <- connection refused"});
        let failure = classify_refresh_failure(&resp);
        assert!(!failure.marks_relogin());
        mark_temporary_failure(&mut account, refresh_failure_reason(failure, &resp));

        assert!(!has_relogin_marker(&account), "断网不得写 needs_relogin");
        assert!(account.get("needs_relogin_reason").is_none());
        assert!(account.get("refresh_failed_at").is_some());
        assert_eq!(
            account.get("refresh_failed_reason").and_then(Value::as_str),
            Some(refresh_failure_reason(failure, &resp).as_str())
        );
    }

    /// 401 吊销语义要写标记，并带上「续期被拒」的原因与起始时间。
    #[test]
    fn revoked_failure_marks_relogin() {
        let mut account = json!({"id": "acc-revoked", "access_token": "at", "refresh_token": "rt"});
        let resp = json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"});
        let failure = classify_refresh_failure(&resp);
        assert!(failure.marks_relogin());
        mark_relogin_required(&mut account, refresh_failure_reason(failure, &resp));

        assert!(has_relogin_marker(&account));
        let reason = account
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("续期被拒"), "{reason}");
        assert!(account.get("needs_relogin_since").is_some());
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(1)
        );
        // 刚失败这一次就是一次「复核」，冷却从这里起算。
        assert!(account.get("needs_relogin_checked_at").is_some());
    }

    /// 已有标记时遇到暂时失败：原标记与原因**保持原样**（不被临时错误覆盖）。
    #[test]
    fn temporary_failure_keeps_existing_marker() {
        let mut account = json!({
            "id": "acc-keep",
            "access_token": "at",
            "refresh_token": "rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
        });
        let resp = json!({"code": 502, "message": "Bad Gateway"});
        mark_temporary_failure(
            &mut account,
            refresh_failure_reason(classify_refresh_failure(&resp), &resp),
        );

        assert!(has_relogin_marker(&account));
        assert_eq!(
            account.get("needs_relogin_reason").and_then(Value::as_str),
            Some("续期被拒（code=12153）：invalid_grant"),
            "暂时失败不得覆盖原标记原因"
        );
        assert_eq!(
            account.get("needs_relogin_since").and_then(Value::as_i64),
            Some(1_700_000_000_000i64)
        );
        assert!(account.get("refresh_failed_at").is_some());
    }

    /// 续期成功后标记与全部诊断字段一起清掉，不留旧 reason / 旧起始时间。
    #[test]
    fn success_clears_relogin_marker() {
        let mut account = json!({
            "id": "acc-recovered",
            "access_token": "at",
            "refresh_token": "rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
            "needs_relogin_fail_count": 3,
            "needs_relogin_last_result": "failed",
            "refresh_failed_at": 1_700_000_100_000i64,
            "refresh_failed_reason": "续期暂时失败（code=-1）",
        });
        clear_relogin_marker(&mut account);

        assert!(!has_relogin_marker(&account));
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(account.get(*key).is_none(), "{key} 应当被清除：{account}");
        }
        // 凭据字段不受影响。
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("at")
        );
    }

    // -----------------------------------------------------------------------
    // 生产分支：刷新响应落到账号对象上的真实路径（`apply_refresh_response`）
    // -----------------------------------------------------------------------

    /// 断网（连接错误）走真实分支：不写标记、不动凭据、只留诊断字段。
    #[test]
    fn refresh_response_connection_error_does_not_mark_relogin() {
        let mut account = json!({
            "id": "acc-offline-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({
            "code": -1,
            "message": "error sending request for url (...) <- connection refused",
        });

        assert!(
            !apply_refresh_response(&mut account, &resp),
            "断网不算续期成功"
        );

        assert!(!has_relogin_marker(&account), "断网不得写 needs_relogin");
        assert!(
            account.get("needs_relogin_reason").is_none(),
            "断网不得写 needs_relogin_reason"
        );
        // 凭据必须原样保留：临时失败不该让账号不可用。
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("old-at")
        );
        assert_eq!(
            account.get("refresh_token").and_then(Value::as_str),
            Some("old-rt")
        );
        // 只留诊断字段。
        assert!(account
            .get("refresh_failed_at")
            .and_then(Value::as_i64)
            .is_some());
        let reason = account
            .get("refresh_failed_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("暂时失败"), "{reason}");
        assert!(
            account.get("refreshedAt").is_none(),
            "失败不得写 refreshedAt"
        );
    }

    /// 401（鉴权拒绝）走真实分支：写标记 + 「续期被拒」原因。
    #[test]
    fn refresh_response_unauthorized_marks_relogin() {
        let mut account = json!({
            "id": "acc-401-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({"code": 401, "message": "Unauthorized"});

        assert!(!apply_refresh_response(&mut account, &resp));

        assert!(has_relogin_marker(&account), "401 必须写 needs_relogin");
        let reason = account
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("续期被拒"), "{reason}");
        assert!(account.get("needs_relogin_since").is_some());
        // 冷却起点必须落下，否则复核会立刻重试。
        assert!(account.get("needs_relogin_checked_at").is_some());
        // 吊销时不保留「暂时失败」的诊断字段。
        assert!(account.get("refresh_failed_at").is_none());
    }

    /// 12153 吊销业务码走真实分支：同样写标记。
    #[test]
    fn refresh_response_revoked_code_marks_relogin() {
        let mut account = json!({"id": "acc-12153-live", "refresh_token": "old-rt"});
        let resp = json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"});

        assert!(!apply_refresh_response(&mut account, &resp));
        assert!(has_relogin_marker(&account));
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    /// 成功响应走真实分支：换发凭据 + **清除**旧标记，一次全清。
    #[test]
    fn refresh_response_success_clears_stale_marker() {
        let mut account = json!({
            "id": "acc-recover-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
            "needs_relogin_fail_count": 3,
            "needs_relogin_checked_at": 1_700_000_600_000i64,
            "needs_relogin_last_result": "failed",
            "refresh_failed_at": 1_700_000_700_000i64,
            "refresh_failed_reason": "续期暂时失败（code=-1）",
        });
        let resp = json!({
            "code": 0,
            "data": {"accessToken": "new-at", "refreshToken": "new-rt", "expiresIn": 7200},
        });
        let before = now_ms();

        assert!(
            apply_refresh_response(&mut account, &resp),
            "成功应返回 true"
        );

        assert!(!has_relogin_marker(&account), "成功必须清除标记");
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(account.get(*key).is_none(), "{key} 应当被清除：{account}");
        }
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("new-at")
        );
        assert_eq!(
            account.get("refresh_token").and_then(Value::as_str),
            Some("new-rt")
        );
        let expires_at = account
            .get("expiresAt")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            expires_at >= before + 7_200_000,
            "expiresIn 应换算成绝对时间戳：{expires_at}"
        );
        assert!(account.get("refreshedAt").and_then(Value::as_i64).is_some());
    }

    /// 成功码但缺 accessToken（网关裁剪 / 错误页）：按暂时失败，绝不写标记。
    #[test]
    fn refresh_response_success_without_access_token_is_temporary() {
        let mut account = json!({
            "id": "acc-hollow",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({"code": 0, "data": {}});

        assert!(
            !apply_refresh_response(&mut account, &resp),
            "缺 accessToken 不算成功"
        );

        assert!(!has_relogin_marker(&account), "缺字段不得写标记");
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("old-at")
        );
        let reason = account
            .get("refresh_failed_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("accessToken"), "{reason}");
    }

    /// 网关 HTML 错误页（502）：暂时失败，不写标记。
    #[test]
    fn refresh_response_html_error_page_is_temporary() {
        let mut account = json!({"id": "acc-html", "refresh_token": "old-rt"});
        let resp = json!({"code": 502, "message": "服务端返回 HTML 错误页：502 Bad Gateway"});

        assert!(!apply_refresh_response(&mut account, &resp));
        assert!(!has_relogin_marker(&account), "网关错误页不得写标记");
    }

    /// 已带标记时又遇暂时失败：标记与原因原样保留（复核失败不能反过来洗白或改写）。
    #[test]
    fn refresh_response_temporary_failure_keeps_existing_marker() {
        let mut account = json!({
            "id": "acc-marked-offline",
            "refresh_token": "old-rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_fail_count": 2,
        });
        let resp = json!({"code": -1, "message": "connection reset by peer"});

        assert!(!apply_refresh_response(&mut account, &resp));

        assert!(has_relogin_marker(&account));
        assert_eq!(
            account.get("needs_relogin_reason").and_then(Value::as_str),
            Some("续期被拒（code=12153）：invalid_grant"),
            "暂时失败不得覆盖原原因"
        );
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(2),
            "暂时失败不得累加吊销计数（否则退避会被网络抖动推长）"
        );
    }

    // -----------------------------------------------------------------------
    // 触发点闸门
    // -----------------------------------------------------------------------

    /// 复核判定：无标记 → 跳过；冷却中 → 跳过；过冷却 → 复核。
    #[test]
    fn verify_plan_picks_skip_reasons() {
        let now = 1_700_000_000_000i64;
        assert_eq!(
            verify_plan(&json!({"id": "clean"}), now),
            VerifyPlan::SkipNoMarker
        );
        assert_eq!(
            verify_plan(
                &json!({"id": "cooling", "needs_relogin": true, "needs_relogin_checked_at": now - 1_000}),
                now
            ),
            VerifyPlan::SkipCooldown
        );
        assert_eq!(
            verify_plan(
                &json!({"id": "ready", "needs_relogin": true, "needs_relogin_checked_at": now - 60_000}),
                now
            ),
            VerifyPlan::Proceed
        );
        // 历史标记（无 checked_at）→ 立即复核。
        assert_eq!(
            verify_plan(&json!({"id": "legacy", "needs_relogin": true}), now),
            VerifyPlan::Proceed
        );
    }

    /// 触发闸门：会内联刷新的账号**不再** spawn 复核（否则同账号并发两次刷新）。
    #[test]
    fn verify_is_not_spawned_when_inline_refresh_runs() {
        // stale=true 且带标记 → 内联刷新自己就是复核，不 spawn。
        assert!(!should_spawn_verify(true, true, true));
        // stale=true 但没有 RT → 内联也不会刷（无 RT 分支只写标记），同样不 spawn。
        assert!(!should_spawn_verify(true, false, true));
        // 不 stale：token 还早，靠旁路复核自愈。
        assert!(should_spawn_verify(false, true, true));
        // 不 stale 但没 RT：刷新必然失败，白读账号库。
        assert!(!should_spawn_verify(false, false, true));
        // 正常账号零额外请求。
        assert!(!should_spawn_verify(false, true, false));
    }

    /// 标记字段**不得**进入「本地字段继承白名单」。
    ///
    /// `account::LOCAL_ACCOUNT_FIELDS` 的语义是「`incoming` 缺该键时用旧值补回」
    /// （`inherit_local_fields`）。而续期成功清除标记靠的是**删键**，一旦把
    /// `needs_relogin` 之类加进白名单，删掉的键会被旧值原样补回 —— 标记复活，
    /// 上游 issue #190 会静默回归，且没有任何编译或运行期报错。这条用例把它变成红灯。
    #[test]
    fn marker_fields_are_not_local_account_fields() {
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(
                !crate::modules::account::LOCAL_ACCOUNT_FIELDS.contains(key),
                "`{key}` 不得进入 LOCAL_ACCOUNT_FIELDS：刷新成功靠删键清标记，\
                 白名单会把旧标记复活（issue #190 回归）"
            );
        }
        // 反证：白名单确实只装「展示类」字段，标记类字段一律不在此列。
        assert!(
            !crate::modules::account::LOCAL_ACCOUNT_FIELDS.is_empty(),
            "白名单为空说明已被清空，本用例失去意义，需同步调整"
        );
    }

    // -----------------------------------------------------------------------
    // 冷却 / 退避（R5-08 B）
    // -----------------------------------------------------------------------

    /// 60s 内不重复复核；过 60s 才允许；没有标记的账号永远不触发。
    #[test]
    fn verify_respects_sixty_second_cooldown() {
        let now = 1_700_000_000_000i64;
        let marked = |checked_at: Option<i64>| {
            let mut account = json!({"id": "acc-cd", "needs_relogin": true});
            if let Some(value) = checked_at {
                account["needs_relogin_checked_at"] = json!(value);
            }
            account
        };

        // 刚复核过（30s 前）→ 冷却中，不打接口。
        assert!(!should_verify_relogin(&marked(Some(now - 30_000)), now));
        // 边界：正好 60s → 允许。
        assert!(should_verify_relogin(&marked(Some(now - 60_000)), now));
        // 61s 前 → 允许。
        assert!(should_verify_relogin(&marked(Some(now - 61_000)), now));
        // 历史标记没有 checked_at → 视为从未复核，立即复核。
        assert!(should_verify_relogin(&marked(None), now));
        // 没有标记 → 永不触发（正常账号零额外请求）。
        assert!(!should_verify_relogin(
            &json!({"id": "acc-clean", "needs_relogin_checked_at": 0}),
            now
        ));
    }

    /// 退避阶梯：60s → 5min → 30min → … → 6h 封顶。
    #[test]
    fn verify_backoff_ladder_grows_and_caps() {
        let cooldown = |fail_count: i64| {
            verify_cooldown_ms(&json!({
                "needs_relogin": true,
                "needs_relogin_fail_count": fail_count,
            }))
        };
        assert_eq!(cooldown(0), 60_000, "无计数（历史标记）按首次冷却");
        assert_eq!(cooldown(1), 60_000);
        assert_eq!(cooldown(2), 300_000);
        assert_eq!(cooldown(3), 1_800_000);
        assert_eq!(cooldown(4), 3_600_000);
        assert_eq!(cooldown(99), 21_600_000, "上限 6h");
    }

    /// 退避生效在 `should_verify_relogin` 上：连续失败后 1 分钟内不再复核。
    #[test]
    fn cooldown_follows_backoff_after_repeated_failures() {
        let now = 1_700_000_000_000i64;
        let account = json!({
            "id": "acc-backoff",
            "needs_relogin": true,
            "needs_relogin_fail_count": 2,
            "needs_relogin_checked_at": now - 60_000,
        });
        // 第 2 次失败后的冷却窗口是 5min：过 60s 仍不复核。
        assert!(!should_verify_relogin(&account, now));
        assert!(should_verify_relogin(&account, now + 240_000));
    }

    // -----------------------------------------------------------------------
    // 并发单飞（R5-08 B）
    // -----------------------------------------------------------------------

    /// 两个并发复核只有一个拿到位（= 只打一次刷新接口）。
    #[test]
    fn verify_slot_is_single_flight() {
        let account_id = format!("acc-single-flight-{}", uuid::Uuid::new_v4());
        let barrier = Arc::new(Barrier::new(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let account_id = account_id.clone();
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let slot = try_acquire_verify_slot(&account_id);
                let _ = tx.send(slot.is_some());
                // 两个线程都拿到结果之后再释放，保证第二条一定撞上在飞集合。
                std::thread::sleep(std::time::Duration::from_millis(80));
                drop(slot);
            }));
        }
        drop(tx);
        let acquired: Vec<bool> = rx.iter().collect();
        for handle in handles {
            let _ = handle.join();
        }
        assert_eq!(
            acquired.iter().filter(|value| **value).count(),
            1,
            "同账号并发复核只允许一次拿到位：{acquired:?}"
        );
        // 释放后可以再次复核（不会永久卡死）。
        let slot = try_acquire_verify_slot(&account_id);
        assert!(slot.is_some(), "释放后应当能重新占用");
    }

    /// 令牌 Drop 即释放：异常路径（提前 return）不会把账号永久锁在「在飞」。
    #[test]
    fn verify_slot_released_on_drop() {
        let account_id = format!("acc-slot-drop-{}", uuid::Uuid::new_v4());
        {
            let slot = try_acquire_verify_slot(&account_id);
            assert!(slot.is_some());
            assert!(
                try_acquire_verify_slot(&account_id).is_none(),
                "占位期间第二次获取必须失败"
            );
        }
        assert!(
            try_acquire_verify_slot(&account_id).is_some(),
            "Drop 后必须释放"
        );
    }

    // -----------------------------------------------------------------------
    // 保留原有断言
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_url_follows_account_variant() {
        // 旧账号（无档位字段）→ 国内版，与改造前逐字一致。
        assert_eq!(
            refresh_url(&json!({"uid": "u-1"})),
            format!(
                "{}/v2/plugin/auth/token/refresh",
                crate::modules::variant::WbVariant::Cn.api_endpoint()
            )
        );
        assert_eq!(
            refresh_url(&json!({"uid": "u-1", "access_token": "t"})),
            "https://www.codebuddy.cn/v2/plugin/auth/token/refresh"
        );
        // 国际版账号 → 国际版域名。
        assert_eq!(
            refresh_url(&json!({"uid": "u-2", "variant": "ai"})),
            format!(
                "{}/v2/plugin/auth/token/refresh",
                crate::modules::variant::WbVariant::Ai.api_endpoint()
            )
        );
        assert_ne!(
            refresh_url(&json!({"variant": "ai"})),
            refresh_url(&json!({"variant": "cn"}))
        );
    }

    /// AC11：刷新请求头必须带 `X-Auth-Refresh-Source: plugin`，两个档位都覆盖。
    ///
    /// 断言为 `Some("plugin")`：头被删掉时为 `None`，用例立即失败，不会被写成恒真。
    #[test]
    fn refresh_headers_always_declare_plugin_refresh_source() {
        let cases = [
            // 国内版：旧账号（无档位字段）与显式档位各一例。
            json!({"uid": "u-cn", "access_token": "at", "refresh_token": "rt"}),
            json!({"uid": "u-cn", "access_token": "at", "variant": "cn"}),
            // 国际版：D1 的失败现场。
            json!({"uid": "u-ai", "access_token": "at", "variant": "ai"}),
        ];
        for account in cases {
            let headers = refresh_headers(&account, "rt-value");
            assert_eq!(
                headers.get("X-Auth-Refresh-Source").map(String::as_str),
                Some("plugin"),
                "刷新请求头缺少 X-Auth-Refresh-Source: plugin（{account}）"
            );
            // 刷新会话头不能被这次改动挤掉。
            assert_eq!(
                headers.get("X-Refresh-Token").map(String::as_str),
                Some("rt-value")
            );
            // 刷新不带 platform 查询参数/头：官方刷新同样不带（research §2 已推翻旧假设）。
            assert!(!headers.contains_key("platform"));
        }
    }
}
