//! 更新状态快照与阶段流转。

use super::*;

/// 更新阶段。前端与托盘菜单都只读这一份状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    /// 尚未检查 / 未开始。
    Idle,
    /// 正在检查更新源。
    Checking,
    /// 已是最新版本。
    UpToDate,
    /// 有新版且可下载。
    Available,
    /// 正在后台下载更新包。
    Downloading,
    /// 包已下载完成，等待用户重启（安装发生在重启时）。
    ReadyToRestart,
    /// 检查 / 下载 / 安装失败，可按文案重试。
    Error,
}

/// 更新状态快照：命令返回值、`update-state` 事件负载、托盘菜单文案的唯一来源。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSnapshot {
    pub phase: UpdatePhase,
    /// 目标版本号（无 `v` 前缀）；检查失败或已是最新时为 None。
    pub latest: Option<String>,
    /// 下载进度（0-100）；总量未知（无 `Content-Length`）时为 None。
    pub percent: Option<u8>,
    /// 错误 / 提示文案。
    pub message: Option<String>,
    /// 最近一次检查完成时刻（秒）。
    pub checked_at: Option<i64>,
    /// 当前占用更新互斥的任务类型（`download` / `install`），空闲为 None。
    ///
    /// 检查不再占用互斥（single-flight），因此这里不会出现 `check`；
    /// 字段用于前端说明「后台正在下载/安装，已 Xs」，而不是弹无信息的 BUSY 错误。
    pub busy_kind: Option<String>,
    /// 互斥被占用的起始时刻（秒）。
    pub busy_since: Option<i64>,
    /// 当前版本无法自动验证更新包签名（密钥轮换 / 旧客户端）：
    /// 主按钮改为「打开 Releases 手动下载」，重试降为次要（第三轮第 1 条 P0-4）。
    pub manual_install_required: bool,
}

impl UpdateSnapshot {
    pub(crate) fn idle() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            latest: None,
            percent: None,
            message: None,
            checked_at: None,
            busy_kind: None,
            busy_since: None,
            manual_install_required: false,
        }
    }
}

pub(crate) static SNAPSHOT: OnceLock<Mutex<UpdateSnapshot>> = OnceLock::new();

/// 「下载 / 安装」互斥：这两类任务会写同一个更新包槽位，必须串行。
///
/// 检查**不**占用这把锁：检查是幂等的，并发检查走 single-flight 合并
/// （第三轮第 2 条：后台检查进行中点下载/重启不应收到 BUSY）。
pub(crate) static BUSY: AtomicBool = AtomicBool::new(false);

/// 互斥占用起始时刻（秒）；0 表示空闲。看门狗据此强制释放陈旧锁。
pub(crate) static BUSY_SINCE: AtomicI64 = AtomicI64::new(0);

/// 互斥占用者类型（[`BusyKind`] 的 u8 编码）；0 表示空闲。
pub(crate) static BUSY_KIND: AtomicU8 = AtomicU8::new(0);

/// 托盘菜单重建档位（每 10% 一档）；`u8::MAX` 表示当前无下载进度。
pub(crate) static MENU_PROGRESS_BUCKET: AtomicU8 = AtomicU8::new(u8::MAX);

/// 本轮下载是否已打过「进度开始上报」日志（排障用，一次下载只打一行）。
pub(crate) static PROGRESS_REPORTED: AtomicBool = AtomicBool::new(false);

/// 陈旧互斥的强制释放阈值：超过 3 分钟视为挂死（下载 read_timeout 60s，正常不会触发）。
pub(crate) const BUSY_STALE_SECS: i64 = 3 * 60;

/// 占用互斥的任务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BusyKind {
    Download,
    Install,
}

impl BusyKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Install => "install",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Download => 1,
            Self::Install => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Download),
            2 => Some(Self::Install),
            _ => None,
        }
    }
}

/// 当前互斥占用信息：`(类型, 起始秒)`；空闲返回 `(None, None)`。
pub(crate) fn busy_info() -> (Option<&'static str>, Option<i64>) {
    if !BUSY.load(Ordering::Acquire) {
        return (None, None);
    }
    let kind = BusyKind::from_code(BUSY_KIND.load(Ordering::Acquire)).map(BusyKind::as_str);
    let since = BUSY_SINCE.load(Ordering::Acquire);
    (kind, (since > 0).then_some(since))
}

/// 更新任务互斥 guard：只覆盖「下载 / 安装」，drop 即释放。
pub(crate) struct BusyGuard {
    kind: BusyKind,
}

impl BusyGuard {
    /// 抢占更新任务；已有任务在跑时返回 None（调用方按各自契约拒绝）。
    ///
    /// 看门狗：持锁超过 [`BUSY_STALE_SECS`] 时强制释放并写 error.log。
    /// 没有这个兜底，一次挂死的下载会让所有后续检查/下载永久 BUSY，只能重启进程。
    pub(crate) fn acquire(kind: BusyKind) -> Option<Self> {
        if BUSY.load(Ordering::Acquire) {
            let since = BUSY_SINCE.load(Ordering::Acquire);
            let stale = since > 0 && now_secs() - since > BUSY_STALE_SECS;
            if stale {
                let holder = BusyKind::from_code(BUSY_KIND.load(Ordering::Acquire))
                    .map(BusyKind::as_str)
                    .unwrap_or("unknown");
                error_log::record(
                    "backend",
                    "更新互斥超时已强制释放",
                    &format!(
                        "holder={holder} since={since} now={} threshold={BUSY_STALE_SECS}s",
                        now_secs()
                    ),
                );
                BUSY.store(false, Ordering::Release);
                BUSY_SINCE.store(0, Ordering::Release);
                BUSY_KIND.store(0, Ordering::Release);
            }
        }
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| {
                BUSY_SINCE.store(now_secs(), Ordering::Release);
                BUSY_KIND.store(kind.code(), Ordering::Release);
                Self { kind }
            })
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // 只清自己这一类：看门狗可能已把锁释放给别的任务。
        if BusyKind::from_code(BUSY_KIND.load(Ordering::Acquire)) == Some(self.kind) {
            BUSY_SINCE.store(0, Ordering::Release);
            BUSY_KIND.store(0, Ordering::Release);
            BUSY.store(false, Ordering::Release);
        }
    }
}

pub(crate) fn snapshot_cell() -> &'static Mutex<UpdateSnapshot> {
    SNAPSHOT.get_or_init(|| Mutex::new(UpdateSnapshot::idle()))
}

pub(crate) fn with_snapshot<T>(edit: impl FnOnce(&mut UpdateSnapshot) -> T) -> T {
    let mut guard = snapshot_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    edit(&mut guard)
}

/// 读当前快照（`update_state` 命令与托盘菜单共用）。
///
/// `busyKind` / `busySince` 在读取时从互斥原子量填充，避免每个写快照的路径都要
/// 记得同步它们（写路径只关心 phase / percent / message）。
pub fn snapshot() -> UpdateSnapshot {
    let mut snapshot = with_snapshot(|snapshot| snapshot.clone());
    let (kind, since) = busy_info();
    snapshot.busy_kind = kind.map(str::to_string);
    snapshot.busy_since = since;
    snapshot
}

/// 进入检查阶段。
///
/// 下载中 / 已就绪不被后台检查改写：否则托盘菜单会从「正在下载 / 重启以完成升级」
/// 闪回「正在检查…」，用户正准备点的入口会消失。
pub(crate) fn begin_check() -> UpdateSnapshot {
    with_snapshot(|snapshot| {
        if matches!(
            snapshot.phase,
            UpdatePhase::Downloading | UpdatePhase::ReadyToRestart
        ) {
            return snapshot.clone();
        }
        snapshot.phase = UpdatePhase::Checking;
        snapshot.percent = None;
        snapshot.message = None;
        snapshot.manual_install_required = false;
        snapshot.clone()
    })
}

/// 写入检查结果（`ok=false` 或 `hasUpdate=false` 分别落到 Error / UpToDate）。
pub(crate) fn apply_check_result(result: &Value) -> UpdateSnapshot {
    with_snapshot(|snapshot| {
        if matches!(
            snapshot.phase,
            UpdatePhase::Downloading | UpdatePhase::ReadyToRestart
        ) {
            return snapshot.clone();
        }
        snapshot.percent = None;
        snapshot.manual_install_required = false;
        if result.get("ok").and_then(Value::as_bool) != Some(true) {
            // 已有明确目标版本时（Available 被后台复查带进 Checking、或下载失败后
            // 再检查），失败不得清掉升级入口：否则 30 分钟后台检查一旦网络失败，
            // 托盘会从「升级到 vX」变成「检查更新失败」。
            if snapshot.latest.is_some()
                && matches!(
                    snapshot.phase,
                    UpdatePhase::Checking | UpdatePhase::Available
                )
            {
                snapshot.phase = UpdatePhase::Available;
                snapshot.message = None;
                return snapshot.clone();
            }
            snapshot.phase = UpdatePhase::Error;
            snapshot.latest = None;
            snapshot.message = result
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| result.get("error").and_then(Value::as_str))
                .map(str::to_string);
            return snapshot.clone();
        }
        let latest = result
            .get("latest")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let has_update =
            result.get("hasUpdate").and_then(Value::as_bool) == Some(true) && !latest.is_empty();
        snapshot.phase = if has_update {
            UpdatePhase::Available
        } else {
            UpdatePhase::UpToDate
        };
        snapshot.latest = has_update.then_some(latest);
        snapshot.message = None;
        snapshot.checked_at = result
            .get("checkedAt")
            .and_then(Value::as_i64)
            .or_else(|| Some(now_secs()));
        snapshot.clone()
    })
}

/// 进入下载阶段（进度百分比由首个 chunk 回调填入，总量未知时保持 None）。
pub(crate) fn begin_download(latest: &str) -> UpdateSnapshot {
    MENU_PROGRESS_BUCKET.store(0, Ordering::Release);
    PROGRESS_REPORTED.store(false, Ordering::Release);
    with_snapshot(|snapshot| {
        snapshot.phase = UpdatePhase::Downloading;
        snapshot.latest = Some(latest.to_string());
        snapshot.percent = None;
        snapshot.message = None;
        snapshot.clone()
    })
}

/// 下载完成：包已存入 `PACKAGE`，菜单转为「重启以完成升级」。
pub(crate) fn mark_ready_to_restart(latest: &str) -> UpdateSnapshot {
    MENU_PROGRESS_BUCKET.store(u8::MAX, Ordering::Release);
    with_snapshot(|snapshot| {
        snapshot.phase = UpdatePhase::ReadyToRestart;
        snapshot.latest = Some(latest.to_string());
        snapshot.percent = Some(100);
        snapshot.message = None;
        snapshot.clone()
    })
}

/// 失败：置 Error（保留 `latest`，托盘据此区分「检查失败」与「下载失败」两种重试文案）。
pub(crate) fn fail(message: String) -> UpdateSnapshot {
    MENU_PROGRESS_BUCKET.store(u8::MAX, Ordering::Release);
    with_snapshot(|snapshot| {
        snapshot.phase = UpdatePhase::Error;
        snapshot.percent = None;
        snapshot.message = Some(message);
        snapshot.manual_install_required = false;
        snapshot.clone()
    })
}

/// 签名校验失败：置 Error 并要求手动安装一次（不引导反复重试）。
pub(crate) fn fail_signature(message: String) -> UpdateSnapshot {
    MENU_PROGRESS_BUCKET.store(u8::MAX, Ordering::Release);
    with_snapshot(|snapshot| {
        snapshot.phase = UpdatePhase::Error;
        snapshot.percent = None;
        snapshot.message = Some(message);
        snapshot.manual_install_required = true;
        snapshot.clone()
    })
}

/// 下载进度换算：总量未知或为 0 → None；超出总量按 100 截断。
pub(crate) fn percent_of(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    let percent = downloaded.saturating_mul(100) / total;
    Some(percent.min(100) as u8)
}

/// 记录一次进度回调：整数百分比变化才写快照并返回新值（未变化返回 None）。
pub(crate) fn advance_progress(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let percent = percent_of(downloaded, total)?;
    with_snapshot(|snapshot| {
        if snapshot.percent == Some(percent) {
            return None;
        }
        snapshot.percent = Some(percent);
        Some(percent)
    })
}

/// 菜单重建判定：跨 10% 档时才重建（每 1% 重建会让打开的菜单闪掉）。
pub(crate) fn should_rebuild_menu(percent: u8) -> bool {
    let bucket = percent / 10;
    MENU_PROGRESS_BUCKET.swap(bucket, Ordering::AcqRel) != bucket
}
