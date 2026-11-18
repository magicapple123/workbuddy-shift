//! 工作区几何：可见性夹取与默认落点。

use super::*;

/// Slack for the summed-area comparison. It absorbs floating-point error in the
/// intersection sum, not a physical-pixel round trip (see `MOVE_DEADBAND`).
pub(crate) const COVERAGE_EPSILON: f64 = 1e-3;

/// Default placement: 12 px from the work area's right edge, 80 px from its top.
pub(crate) const RIGHT_MARGIN: f64 = 12.0;

pub(crate) const TOP_MARGIN: f64 = 80.0;

/// A rectangle in logical points, the coordinate space the saved position file
/// and `set_position` both use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub(crate) fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }
}

/// `None` when the window is fully covered by the union of the work areas;
/// otherwise the corrected top-left corner.
///
/// Overlapping areas (mirrored displays) are counted twice, which only makes
/// the check more lenient: a window that is visible either way stays put.
pub fn clamp_into_view(window: Rect, areas: &[Rect]) -> Option<(f64, f64)> {
    // Non-finite or empty input has no safe target. `None` means "do not move".
    if !finite_rect(window) {
        return None;
    }
    let areas: Vec<Rect> = areas.iter().copied().filter(usable_area).collect();
    if areas.is_empty() {
        return None;
    }
    let covered: f64 = areas
        .iter()
        .map(|area| intersection(window, *area).area())
        .sum();
    if covered >= window.area() - COVERAGE_EPSILON {
        return None;
    }
    let target = areas
        .iter()
        .copied()
        .max_by(|a, b| {
            intersection(window, *a)
                .area()
                .total_cmp(&intersection(window, *b).area())
        })
        .unwrap();
    // No overlap at all: fall back to the work area with the closest center.
    let target = if intersection(window, target).area() > 0.0 {
        target
    } else {
        areas
            .iter()
            .copied()
            .min_by(|a, b| distance2(window, *a).total_cmp(&distance2(window, *b)))
            .unwrap()
    };
    Some((
        axis(window.x, target.x, target.width, window.width),
        axis(window.y, target.y, target.height, window.height),
    ))
}

/// Top-right default: `RIGHT_MARGIN` from the right edge, `TOP_MARGIN` from the
/// top. The caller clamps the result, so this stays a plain candidate; the
/// height only matters to that clamp, never to the corner itself.
pub fn default_position(area: Rect, width: f64, _height: f64) -> (f64, f64) {
    (
        area.x + area.width - width - RIGHT_MARGIN,
        area.y + TOP_MARGIN,
    )
}

pub(crate) fn work_areas(window: &WebviewWindow) -> Result<Vec<Rect>, String> {
    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    let areas: Vec<Rect> = monitors.iter().filter_map(work_area).collect();
    if areas.is_empty() {
        return Err("无法获取显示器工作区".into());
    }
    Ok(areas)
}

pub(crate) fn primary_work_area(window: &WebviewWindow) -> Result<Rect, String> {
    let monitor = window
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("无法获取主显示器工作区")?;
    work_area(&monitor).ok_or("无法获取主显示器工作区".into())
}

/// Work area in logical points, converted with the monitor's own scale factor.
/// `None` when the scale would panic inside `to_logical` or the area is unusable.
pub(crate) fn work_area(monitor: &Monitor) -> Option<Rect> {
    let scale = monitor.scale_factor();
    if !(scale.is_sign_positive() && scale.is_normal()) {
        return None;
    }
    let position = monitor.work_area().position.to_logical::<f64>(scale);
    let size = monitor.work_area().size.to_logical::<f64>(scale);
    let rect = Rect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    usable_area(&rect).then_some(rect)
}

pub(crate) fn intersection(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

pub(crate) fn finite_rect(rect: Rect) -> bool {
    rect.x.is_finite() && rect.y.is_finite() && rect.width.is_finite() && rect.height.is_finite()
}

pub(crate) fn usable_area(rect: &Rect) -> bool {
    finite_rect(*rect) && rect.width > 0.0 && rect.height > 0.0
}

/// Clamps one axis into the area; an area narrower than the window degenerates
/// to its start edge. Comparisons, not `f64::clamp`, so a non-finite value
/// cannot assert on the main thread.
pub(crate) fn axis(value: f64, start: f64, extent: f64, window_extent: f64) -> f64 {
    if !(value.is_finite() && start.is_finite() && extent.is_finite() && window_extent.is_finite())
    {
        return if start.is_finite() { start } else { 0.0 };
    }
    if window_extent >= extent {
        return start;
    }
    let end = start + extent - window_extent;
    if !end.is_finite() || end < start {
        return start;
    }
    if value < start {
        start
    } else if value > end {
        end
    } else {
        value
    }
}

pub(crate) fn distance2(window: Rect, area: Rect) -> f64 {
    let dx = (window.x + window.width / 2.0) - (area.x + area.width / 2.0);
    let dy = (window.y + window.height / 2.0) - (area.y + area.height / 2.0);
    dx * dx + dy * dy
}
