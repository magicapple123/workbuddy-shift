//! Rail placement: keep the window fully inside a monitor work area.
//!
//! One visibility rule serves three entry points: the startup restore
//! (`place`), the post-drag watchdog (`plan_at` + `glide`) and the settings
//! reset (`reset_plan` + `glide`).
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{LogicalPosition, Monitor, WebviewWindow};

mod geometry;
mod persist;

pub use geometry::*;
pub use persist::*;

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };
    const RAIL: (f64, f64) = (368.0, 600.0);
    const RIGHT: f64 = 1920.0 - 368.0;
    const BOTTOM: f64 = 1080.0 - 600.0;

    fn window_at(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            width: RAIL.0,
            height: RAIL.1,
        }
    }

    #[test]
    fn a_window_inside_the_work_area_stays_put() {
        assert_eq!(clamp_into_view(window_at(100.0, 100.0), &[AREA]), None);
    }

    #[test]
    fn a_window_past_the_right_edge_is_pulled_back() {
        assert_eq!(
            clamp_into_view(window_at(1800.0, 100.0), &[AREA]),
            Some((RIGHT, 100.0))
        );
    }

    #[test]
    fn a_window_past_the_left_edge_is_pulled_back() {
        assert_eq!(
            clamp_into_view(window_at(-500.0, 100.0), &[AREA]),
            Some((0.0, 100.0))
        );
    }

    #[test]
    fn a_window_above_the_top_edge_is_pulled_down() {
        assert_eq!(
            clamp_into_view(window_at(100.0, -40.0), &[AREA]),
            Some((100.0, 0.0))
        );
    }

    #[test]
    fn a_window_below_the_bottom_edge_is_pulled_up() {
        assert_eq!(
            clamp_into_view(window_at(100.0, 1000.0), &[AREA]),
            Some((100.0, BOTTOM))
        );
    }

    #[test]
    fn a_window_far_outside_returns_into_the_only_work_area() {
        assert_eq!(
            clamp_into_view(window_at(5000.0, 3000.0), &[AREA]),
            Some((RIGHT, BOTTOM))
        );
    }

    #[test]
    fn a_window_on_an_offset_secondary_area_stays_put() {
        let secondary = Rect {
            x: 1920.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
        assert_eq!(
            clamp_into_view(window_at(2100.0, 200.0), &[secondary]),
            None
        );
    }

    #[test]
    fn a_window_in_the_gap_between_two_areas_snaps_to_the_closer_one() {
        let left = AREA;
        let right = Rect {
            x: 2400.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            clamp_into_view(window_at(2032.0, 100.0), &[left, right]),
            Some((2400.0, 100.0))
        );
    }

    #[test]
    fn a_window_covered_by_two_adjacent_areas_stays_put() {
        let left = AREA;
        let right = Rect {
            x: 1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            clamp_into_view(window_at(1700.0, 100.0), &[left, right]),
            None
        );
    }

    #[test]
    fn an_area_smaller_than_the_window_aligns_to_its_start_edge() {
        let tiny = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 300.0,
        };
        assert_eq!(
            clamp_into_view(window_at(500.0, 500.0), &[tiny]),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn the_default_position_hugs_the_top_right_corner_of_the_area() {
        let area = Rect {
            x: 100.0,
            y: 50.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(default_position(area, RAIL.0, RAIL.1), (1640.0, 130.0));
        let (x, y) = default_position(area, RAIL.0, RAIL.1);
        assert!(x + RAIL.0 <= area.x + area.width && y + RAIL.1 <= area.y + area.height);
    }

    #[test]
    fn an_empty_area_list_does_not_move_the_window() {
        assert_eq!(clamp_into_view(window_at(-500.0, -500.0), &[]), None);
    }

    #[test]
    fn non_finite_geometry_does_not_panic_and_is_ignored() {
        let nan = Rect {
            x: f64::NAN,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let infinite = Rect {
            x: 0.0,
            y: 0.0,
            width: f64::INFINITY,
            height: 100.0,
        };
        assert_eq!(clamp_into_view(nan, &[AREA]), None);
        assert_eq!(
            clamp_into_view(window_at(100.0, 100.0), &[nan, infinite]),
            None
        );
        assert_eq!(
            clamp_into_view(window_at(5000.0, 3000.0), &[nan, AREA]),
            Some((RIGHT, BOTTOM))
        );
    }

    #[test]
    fn a_zero_size_area_cannot_steal_the_closest_center() {
        let degenerate = Rect {
            x: 5000.0,
            y: 3000.0,
            width: 0.0,
            height: 0.0,
        };
        assert_eq!(
            clamp_into_view(window_at(5000.0, 3000.0), &[degenerate, AREA]),
            Some((RIGHT, BOTTOM))
        );
    }

    #[test]
    fn coverage_within_the_epsilon_stays_and_a_larger_sliver_moves() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1024.0,
            height: 1.0,
        };
        let within = Rect {
            x: 1.0 / 1024.0,
            y: 0.0,
            width: 1024.0,
            height: 1.0,
        };
        assert_eq!(clamp_into_view(within, &[area]), None);
        let beyond = Rect {
            x: 1.0 / 512.0,
            y: 0.0,
            width: 1024.0,
            height: 1.0,
        };
        assert_eq!(clamp_into_view(beyond, &[area]), Some((0.0, 0.0)));
    }

    #[test]
    fn overlapping_areas_are_counted_twice_so_a_half_covered_window_stays() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let window = Rect {
            x: 50.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        assert_eq!(clamp_into_view(window, &[area]), Some((0.0, 0.0)));
        assert_eq!(clamp_into_view(window, &[area, area]), None);
    }

    #[test]
    fn an_aligned_window_larger_than_the_area_still_reports_that_point() {
        let tiny = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 300.0,
        };
        assert_eq!(
            clamp_into_view(window_at(0.0, 0.0), &[tiny]),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn a_subpixel_correction_is_not_material_and_one_pixel_is() {
        assert!(!correction_is_material(10.0, 10.0, 10.4, 10.0));
        assert!(!correction_is_material(10.0, 20.0, 10.0, 20.0));
        assert!(correction_is_material(10.0, 10.0, 11.0, 10.0));
    }

    #[test]
    fn the_glide_eases_out_from_start_to_target() {
        assert_eq!(glide_point((0.0, 0.0), (200.0, 100.0), 0.0), (0.0, 0.0));
        assert_eq!(glide_point((0.0, 0.0), (200.0, 100.0), 1.0), (200.0, 100.0));
        // Ease-out covers more than half the distance at the time midpoint.
        let (x, y) = glide_point((0.0, 0.0), (200.0, 100.0), 0.5);
        assert!(x > 100.0 && y > 50.0 && x <= 200.0 && y <= 100.0);
        assert_eq!(glide_point((40.0, 50.0), (40.0, 50.0), 0.7), (40.0, 50.0));
    }

    #[test]
    fn a_reset_generation_invalidates_a_prior_correction_snapshot() {
        let seq = AtomicU64::new(0);
        let correction = snapshot(&seq);
        assert!(is_current(&seq, correction));
        let reset = bump(&seq);
        assert!(!is_current(&seq, correction));
        assert!(is_current(&seq, reset));
        let reset_again = bump(&seq);
        assert!(!is_current(&seq, reset));
        assert!(is_current(&seq, reset_again));
    }
}
