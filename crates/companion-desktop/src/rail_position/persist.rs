//! 悬浮栏位置恢复、校正与动画（glide）。

use super::*;

/// `set_position` emits `Moved`. A fractional-DPI round trip is under one
/// logical pixel; writing that again would schedule another pass forever.
pub(crate) const MOVE_DEADBAND: f64 = 1.0;

/// Corrective moves are animated: about 200 ms of ease-out, one frame per 16 ms.
pub(crate) const GLIDE_MS: u64 = 200;

pub(crate) const FRAME_MS: u64 = 16;

/// Reset bumps this so an in-flight correction exits; a newer reset preempts
/// an older one. Corrections snapshot it *before* queueing `plan_at`, otherwise
/// a later `glide` would start from a stale off-screen `from`.
pub(crate) static ANIMATION_SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) fn snapshot(seq: &AtomicU64) -> u64 {
    seq.load(Ordering::SeqCst)
}

pub(crate) fn bump(seq: &AtomicU64) -> u64 {
    seq.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn is_current(seq: &AtomicU64, token: u64) -> bool {
    seq.load(Ordering::SeqCst) == token
}

/// Caller's snapshot of the animation generation, taken before queueing `plan`.
pub fn animation_generation() -> u64 {
    snapshot(&ANIMATION_SEQ)
}

/// Startup restore: `None` uses the default position, and the candidate always
/// passes through the clamp before it is applied.
pub fn place(window: &WebviewWindow, saved: Option<(f64, f64)>) -> Result<(), String> {
    let size = window_size(window)?;
    let areas = work_areas(window)?;
    let candidate = match saved {
        Some((x, y)) if x.is_finite() && y.is_finite() => (x, y),
        _ => default_position(primary_work_area(window)?, size.0, size.1),
    };
    move_to(window, candidate, size, &areas)
}

/// A planned corrective move: where the window is and where it must go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub(crate) seq: u64,
}

/// Main-thread read for the watchdog: `None` when the window is already fully
/// visible, when the gap is under the move deadband, or when `generation` is
/// no longer current (a reset started after the caller sampled the counter).
pub fn plan_at(window: &WebviewWindow, generation: u64) -> Result<Option<Move>, String> {
    if !is_current(&ANIMATION_SEQ, generation) {
        return Ok(None);
    }
    let rect = current_rect(window)?;
    match clamp_into_view(rect, &work_areas(window)?) {
        None => Ok(None),
        Some((x, y)) if !correction_is_material(rect.x, rect.y, x, y) => Ok(None),
        Some((x, y)) => Ok(Some(Move {
            from: (rect.x, rect.y),
            to: (x, y),
            seq: generation,
        })),
    }
}

/// Main-thread read for the reset command: the primary work area's top-right
/// corner, clamped as a fallback. Always returns a target. Bumps the
/// generation so any in-flight or already-planned correction exits.
pub fn reset_plan(window: &WebviewWindow) -> Result<Move, String> {
    let rect = current_rect(window)?;
    let size = (rect.width, rect.height);
    let candidate = default_position(primary_work_area(window)?, size.0, size.1);
    let target = Rect {
        x: candidate.0,
        y: candidate.1,
        width: size.0,
        height: size.1,
    };
    let (x, y) = clamp_into_view(target, &work_areas(window)?).unwrap_or(candidate);
    Ok(Move {
        from: (rect.x, rect.y),
        to: (x, y),
        seq: bump(&ANIMATION_SEQ),
    })
}

/// Animates `mv` at roughly 60 fps. `set_position` dispatches to the main
/// thread asynchronously on every backend, so this may run on any thread.
pub fn glide(window: &WebviewWindow, mv: Move) {
    if mv.from == mv.to {
        return;
    }
    let steps = (GLIDE_MS / FRAME_MS).max(1);
    for step in 1..=steps {
        if !is_current(&ANIMATION_SEQ, mv.seq) {
            return;
        }
        let t = step as f64 / steps as f64;
        let (x, y) = glide_point(mv.from, mv.to, t);
        if window.set_position(LogicalPosition::new(x, y)).is_err() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(FRAME_MS));
    }
}

/// Ease-out cubic: fast at first, settling on the target.
pub(crate) fn glide_point(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    let eased = 1.0 - (1.0 - t).powi(3);
    (
        from.0 + (to.0 - from.0) * eased,
        from.1 + (to.1 - from.1) * eased,
    )
}

pub(crate) fn current_rect(window: &WebviewWindow) -> Result<Rect, String> {
    let scale = window_scale(window)?;
    let position = window
        .outer_position()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    let size = window_size(window)?;
    Ok(Rect {
        x: position.x,
        y: position.y,
        width: size.0,
        height: size.1,
    })
}

pub(crate) fn move_to(
    window: &WebviewWindow,
    candidate: (f64, f64),
    size: (f64, f64),
    areas: &[Rect],
) -> Result<(), String> {
    let rect = Rect {
        x: candidate.0,
        y: candidate.1,
        width: size.0,
        height: size.1,
    };
    let (x, y) = clamp_into_view(rect, areas).unwrap_or(candidate);
    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

pub(crate) fn window_scale(window: &WebviewWindow) -> Result<f64, String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    // `PhysicalPosition::to_logical` asserts on a non-normal scale and would
    // take down the main thread from the correction worker.
    if scale.is_sign_positive() && scale.is_normal() {
        Ok(scale)
    } else {
        Err("无法读取窗口缩放".into())
    }
}

pub(crate) fn window_size(window: &WebviewWindow) -> Result<(f64, f64), String> {
    let scale = window_scale(window)?;
    let size = window
        .outer_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    if size.width.is_finite() && size.height.is_finite() && size.width > 1.0 && size.height > 1.0 {
        Ok((size.width, size.height))
    } else {
        Err("无法读取悬浮窗尺寸".into())
    }
}

/// True when the corrected origin is at least one logical pixel away.
pub(crate) fn correction_is_material(
    current_x: f64,
    current_y: f64,
    next_x: f64,
    next_y: f64,
) -> bool {
    (next_x - current_x).abs() >= MOVE_DEADBAND || (next_y - current_y).abs() >= MOVE_DEADBAND
}
