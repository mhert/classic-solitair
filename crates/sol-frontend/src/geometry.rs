//! Window placement: the size a persisted geometry may be restored at, and
//! what a new geometry report does to the stored one.
//!
//! Both rules are pure functions of their inputs so they can be reasoned
//! about — and tested — without a window manager. What a frontend supplies
//! is the desktop's usable extent and the report itself; what it gets back
//! is a decision.

use sol_session::WindowGeometry;

/// The smallest window the chrome and playfield stay usable at, in logical
/// pixels.
const FLOOR: (u32, u32) = (400, 300);

/// Clamps a persisted window size to a sane floor and a caller-supplied
/// ceiling, both in logical pixels: never smaller than [`FLOOR`], never
/// larger than `max`.
///
/// A `max` narrower than the floor on either axis is itself degenerate —
/// snapping down to it would produce an unusably small window — so the floor
/// wins on that axis instead: better a window too large for the screen than
/// one too small to use.
#[must_use]
pub fn clamp_window_size(width: u32, height: u32, max: (u32, u32)) -> (u32, u32) {
    let clamp_axis = |value: u32, floor: u32, ceiling: u32| value.clamp(floor, ceiling.max(floor));
    (
        clamp_axis(width, FLOOR.0, max.0),
        clamp_axis(height, FLOOR.1, max.1),
    )
}

/// The geometry to store after a window reports itself at `width × height`,
/// at `position`, `maximized` or not — given whatever was stored before.
///
/// A windowed report replaces the stored geometry wholesale; a `None`
/// position clears whatever position was stored, matching platforms that
/// expose no window position. A maximized report only raises the flag: the
/// stored size and position stay put, so a future launch restores the size
/// the window had before it was maximized. With nothing stored yet, a
/// maximized report is all there is, so it is stored as-is.
#[must_use]
pub fn next_window_geometry(
    stored: Option<&WindowGeometry>,
    width: u32,
    height: u32,
    position: Option<(i32, i32)>,
    maximized: bool,
) -> WindowGeometry {
    if maximized && let Some(existing) = stored {
        return WindowGeometry {
            maximized: true,
            ..existing.clone()
        };
    }
    WindowGeometry {
        width,
        height,
        x: position.map(|(x, _)| x),
        y: position.map(|(_, y)| y),
        maximized,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_size_inside_the_bounds_is_left_alone() {
        assert_eq!(clamp_window_size(1000, 700, (1920, 1080)), (1000, 700));
    }

    #[test]
    fn an_oversized_window_snaps_down_to_the_desktop() {
        assert_eq!(clamp_window_size(4000, 3000, (1920, 1080)), (1920, 1080));
    }

    #[test]
    fn an_undersized_window_snaps_up_to_the_floor() {
        assert_eq!(clamp_window_size(10, 10, (1920, 1080)), (400, 300));
    }

    /// A desktop narrower than the floor is degenerate; snapping to it would
    /// produce a window too small to play in, so the floor wins.
    #[test]
    fn a_desktop_smaller_than_the_floor_does_not_shrink_the_window_below_it() {
        assert_eq!(clamp_window_size(800, 600, (200, 150)), (400, 300));
    }

    /// The two axes clamp independently: a desktop short in one direction
    /// only constrains that direction.
    #[test]
    fn the_axes_clamp_independently() {
        assert_eq!(clamp_window_size(4000, 500, (1920, 1080)), (1920, 500));
        assert_eq!(clamp_window_size(500, 4000, (1920, 1080)), (500, 1080));
    }

    fn windowed() -> WindowGeometry {
        WindowGeometry {
            width: 1000,
            height: 700,
            x: Some(20),
            y: Some(30),
            maximized: false,
        }
    }

    #[test]
    fn a_windowed_report_replaces_the_stored_geometry() {
        let next = next_window_geometry(Some(&windowed()), 800, 600, Some((5, 6)), false);
        assert_eq!(
            next,
            WindowGeometry {
                width: 800,
                height: 600,
                x: Some(5),
                y: Some(6),
                maximized: false,
            }
        );
    }

    /// Wayland exposes no window position, so a `None` position must clear a
    /// stored one rather than leave a stale value to restore against.
    #[test]
    fn a_windowed_report_without_a_position_clears_the_stored_one() {
        let next = next_window_geometry(Some(&windowed()), 800, 600, None, false);
        assert_eq!(next.x, None);
        assert_eq!(next.y, None);
    }

    /// The point of the flag-only path: a maximized window's own size is the
    /// screen, and restoring to that on the next launch would lose the size
    /// the user actually chose.
    #[test]
    fn a_maximized_report_keeps_the_stored_size_and_position() {
        let next = next_window_geometry(Some(&windowed()), 1920, 1080, Some((0, 0)), true);
        assert_eq!(
            next,
            WindowGeometry {
                maximized: true,
                ..windowed()
            }
        );
    }

    /// With nothing stored, the maximized report is the only geometry there
    /// is, so it is kept rather than discarded.
    #[test]
    fn a_maximized_report_with_nothing_stored_is_kept_as_is() {
        let next = next_window_geometry(None, 1920, 1080, Some((0, 0)), true);
        assert_eq!(
            next,
            WindowGeometry {
                width: 1920,
                height: 1080,
                x: Some(0),
                y: Some(0),
                maximized: true,
            }
        );
    }
}
