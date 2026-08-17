//! Window placement: where the window opens, how a persisted geometry
//! survives a DPI change, and when a placement change counts as settled.
//!
//! Split from `ui.rs`, which owns the widget tree and its event wiring.
//! Everything here is geometry — nwg-logical against physical pixels,
//! client space against outer rect, a monitor's bounds — and none of it
//! needs a widget to reason about, which is why the round-trip properties
//! below are testable against hidden windows.

use std::time::{Duration, Instant};

use native_windows_gui as nwg;
use winapi::shared::windef::{HWND, RECT};
use winapi::um::winuser::{
    GetSystemMetrics, GetWindowRect, MONITOR_DEFAULTTONULL, MonitorFromRect, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos,
};

use sol_frontend::geometry::clamp_window_size;
use sol_session::WindowGeometry;

/// How quiet a keyboard-driven placement change (Win+arrow snap,
/// maximize/restore via keyboard) must be before it counts as settled
/// and is captured: those changes raise no `WM_EXITSIZEMOVE` to mark
/// their end the way an interactive drag does.
pub(crate) const GEOMETRY_SETTLE_MS: u64 = 500;

/// How often the settle timer checks that quiet period. It has to be a
/// fraction of it because the check is a poll, not a one-shot alarm:
/// `nwg::AnimationTimer` restarts do not push a running timer's next
/// tick back, so the deadline lives in [`Ui::placement_changed_at`] and
/// the tick simply returns until it passes. A capture therefore lands
/// between `GEOMETRY_SETTLE_MS` and `GEOMETRY_SETTLE_MS + this` after
/// the last change; each intervening tick costs one `Instant`
/// comparison.
pub(crate) const GEOMETRY_SETTLE_POLL_MS: u64 = 50;

/// The window size and position to build with when there is no
/// persisted geometry to restore: the 2×-scaled design client plus room
/// for the status bar (logical px), centered — today's exact default.
pub(crate) const DEFAULT_WINDOW_SIZE: (i32, i32) = (1170, 792);

/// The placement [`build_ui`] restores, in nwg-logical pixels and in the
/// window's **outer** rect — frame, caption and menu bar included.
///
/// Outer rather than nwg's client space on purpose. nwg's size pair is
/// client-based (`WindowBuilder::size` is inflated through
/// `AdjustWindowRectEx` with no menu allowance, `Window::size` reads
/// `GetClientRect`), while the menu bar is attached *after* the builder
/// has sized the window and takes its strip out of the client area that
/// was just requested. Persisting that client size would hand back a
/// window one menu-bar height shorter on every save/restore cycle, and
/// the loss would compound across launches. The outer rect has no such
/// asymmetry: [`apply_outer_placement`] sets exactly what
/// [`outer_window_rect`] reads back, menu bar or not — and it is the
/// space `x`/`y` were already in, so the whole placement is one unit
/// system.
pub(crate) struct StartupPlacement {
    /// Outer size, already clamped to the floor and the virtual screen.
    pub(crate) size: (i32, i32),
    /// Outer top-left. `Some` only when persisted geometry carried both
    /// coordinates and the resulting rect still lands on a live monitor;
    /// `None` means "center instead", exactly today's behavior with no
    /// geometry.
    pub(crate) position: Option<(i32, i32)>,
}

/// Derives the main window's startup placement from persisted geometry,
/// or `None` when there is none — the window then keeps today's exact
/// default (the builder's own [`DEFAULT_WINDOW_SIZE`], centered).
///
/// With persisted geometry: the stored size clamped between a 400×300
/// floor and the virtual screen (a saved size can outlive the display it
/// was saved on shrinking or disappearing); the stored position applies
/// only when both coordinates were recorded and the resulting window
/// rect still lands on a live monitor (a position alone can outlive the
/// monitor it was saved on being unplugged) — otherwise the window
/// centers instead.
pub(crate) fn startup_placement(geometry: Option<&WindowGeometry>) -> Option<StartupPlacement> {
    let geometry = geometry?;
    let (width, height) = clamp_window_size(geometry.width, geometry.height, virtual_screen_size());
    let saturate = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
    let size = (saturate(width), saturate(height));
    let position = geometry
        .x
        .zip(geometry.y)
        .filter(|&(x, y)| window_rect_on_a_live_monitor(x, y, size));
    Some(StartupPlacement { size, position })
}

/// `value * scale`, rounded the way nwg's own DPI conversion rounds.
/// Both directions round here and nowhere else, so a value pushed
/// through [`to_physical`] and read back through [`to_logical`] comes
/// out unchanged.
pub(crate) fn round_scaled(value: i32, scale: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)] // screen coordinates stay far within i32's range
    {
        (f64::from(value) * scale).round() as i32
    }
}

/// nwg-logical pixels — the unit the persisted geometry and every nwg
/// control geometry use — into the physical screen pixels the raw Win32
/// placement calls speak.
pub(crate) fn to_physical(logical: i32) -> i32 {
    round_scaled(logical, nwg::scale_factor())
}

/// Physical screen pixels back into nwg-logical ones.
pub(crate) fn to_logical(physical: i32) -> i32 {
    round_scaled(physical, 1.0 / nwg::scale_factor())
}

/// The virtual screen's size (spanning every monitor), converted from
/// `GetSystemMetrics`' physical pixels into the nwg-logical unit the
/// persisted geometry uses.
pub(crate) fn virtual_screen_size() -> (u32, u32) {
    // SAFETY: reads global display configuration; no pointers involved.
    #[allow(unsafe_code)]
    let (width, height) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let logical = |physical: i32| u32::try_from(to_logical(physical)).unwrap_or(0);
    (logical(width), logical(height))
}

/// Whether a window whose outer rect is `size` at logical position
/// `(x, y)` would land on a live monitor. Persisted geometry can outlive
/// the monitor it was saved on; `MonitorFromRect` with
/// `MONITOR_DEFAULTTONULL` reports that as a null handle instead of
/// silently picking a fallback monitor, which is exactly the signal
/// needed here. Position and size are both outer-rect logical values
/// (see [`StartupPlacement`]), so the tested rect is the one the window
/// will actually occupy.
pub(crate) fn window_rect_on_a_live_monitor(x: i32, y: i32, size: (i32, i32)) -> bool {
    let (width, height) = size;
    let rect = RECT {
        left: to_physical(x),
        top: to_physical(y),
        right: to_physical(x.saturating_add(width)),
        bottom: to_physical(y.saturating_add(height)),
    };
    // SAFETY: `rect` is a fully-initialized local RECT; MonitorFromRect
    // only reads through the pointer.
    #[allow(unsafe_code)]
    let monitor = unsafe { MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONULL) };
    !monitor.is_null()
}

/// The window's outer rect — frame, caption and menu bar included — in
/// nwg-logical pixels: `(x, y, width, height)`. The exact inverse of
/// [`apply_outer_placement`]; see [`StartupPlacement`] for why placement
/// is persisted in this space and not in nwg's client-space
/// `Window::size`.
pub(crate) fn outer_window_rect(hwnd: HWND) -> (i32, i32, u32, u32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: live top-level window handle from our own control; the
    // out-pointer is to a stack RECT.
    #[allow(unsafe_code)]
    unsafe {
        GetWindowRect(hwnd, &raw mut rect);
    }
    let extent =
        |from: i32, to: i32| u32::try_from(to_logical(to.saturating_sub(from))).unwrap_or(0);
    (
        to_logical(rect.left),
        to_logical(rect.top),
        extent(rect.left, rect.right),
        extent(rect.top, rect.bottom),
    )
}

/// Forces the window's outer rect to `placement`. `SetWindowPos` takes
/// the outer rect verbatim — no `AdjustWindowRectEx` guesswork and no
/// menu-bar allowance to get wrong — which is what makes this the exact
/// inverse of [`outer_window_rect`]. A `None` position leaves the window
/// wherever the builder centered it; z-order, activation and visibility
/// stay untouched (the window is deliberately still hidden here).
pub(crate) fn apply_outer_placement(hwnd: HWND, placement: &StartupPlacement) {
    let (width, height) = (to_physical(placement.size.0), to_physical(placement.size.1));
    let (x, y, keep_position) = match placement.position {
        Some((x, y)) => (to_physical(x), to_physical(y), 0),
        None => (0, 0, SWP_NOMOVE),
    };
    // SAFETY: live top-level window handle from our own control; the
    // null insert-after is inert under SWP_NOZORDER.
    #[allow(unsafe_code)]
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE | keep_position,
        );
    }
}

/// Whether a placement change recorded at `changed_at` has been quiet
/// long enough to capture. A `now` before `changed_at` (a clock that
/// stepped backwards) reads as "not yet" rather than panicking.
pub(crate) fn placement_settled(changed_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(changed_at) >= Duration::from_millis(GEOMETRY_SETTLE_MS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A hidden, resizable window shaped like the main one. Windows are
    /// never shown here: `SetWindowPos`/`GetWindowRect` work on hidden
    /// windows, so the placement round trip is measurable without ever
    /// putting anything on screen.
    fn hidden_main_window(title: &str) -> nwg::Window {
        nwg::init().unwrap();
        let mut window = nwg::Window::default();
        nwg::Window::builder()
            .flags(nwg::WindowFlags::MAIN_WINDOW | nwg::WindowFlags::RESIZABLE)
            .title(title)
            .size((800, 600))
            .build(&mut window)
            .unwrap();
        window
    }

    fn placement_of(rect: (i32, i32, u32, u32)) -> StartupPlacement {
        let (x, y, width, height) = rect;
        StartupPlacement {
            size: (
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
            ),
            position: Some((x, y)),
        }
    }

    /// The property the whole outer-rect choice exists for: applying a
    /// placement and capturing it back is the identity, so repeated
    /// launch cycles cannot drift.
    #[test]
    fn applying_and_capturing_a_placement_round_trips_without_drift() {
        let window = hidden_main_window("placement round trip");
        let mut menu = nwg::Menu::default();
        nwg::Menu::builder()
            .text("&Game")
            .parent(&window)
            .build(&mut menu)
            .unwrap();
        let hwnd = window.handle.hwnd().unwrap();

        apply_outer_placement(
            hwnd,
            &StartupPlacement {
                size: (620, 480),
                position: Some((40, 30)),
            },
        );
        let captured = outer_window_rect(hwnd);
        assert_eq!(captured, (40, 30, 620, 480));

        // Three more save/restore cycles: not a pixel moves.
        for cycle in 0..3 {
            apply_outer_placement(hwnd, &placement_of(outer_window_rect(hwnd)));
            assert_eq!(
                outer_window_rect(hwnd),
                captured,
                "drifted on cycle {cycle}"
            );
        }
    }

    /// Why that identity needs the outer rect: the menu bar is attached
    /// after the window is sized and takes its strip out of the client
    /// area, so a client-space capture would hand back a shorter window
    /// every cycle. The outer rect is untouched by the same attach.
    #[test]
    fn attaching_a_menu_bar_leaves_the_outer_rect_alone() {
        let window = hidden_main_window("menu bar vs outer rect");
        let hwnd = window.handle.hwnd().unwrap();
        apply_outer_placement(
            hwnd,
            &StartupPlacement {
                size: (620, 480),
                position: Some((60, 50)),
            },
        );
        let outer = outer_window_rect(hwnd);
        let client = window.size();

        let mut menu = nwg::Menu::default();
        nwg::Menu::builder()
            .text("&Game")
            .parent(&window)
            .build(&mut menu)
            .unwrap();

        assert_eq!(outer_window_rect(hwnd), outer);
        assert!(
            window.size().1 < client.1,
            "expected the menu bar to shrink the client area, {client:?} -> {:?}",
            window.size()
        );
    }

    /// The other half of the round trip: geometry is persisted in
    /// logical pixels, so the DPI conversion has to survive being
    /// applied and read back at every scale a display can report.
    #[test]
    fn logical_to_physical_and_back_is_the_identity_at_every_scale() {
        for scale in [1.0_f64, 1.25, 1.5, 1.75, 2.0, 3.0] {
            for logical in [400, 401, 792, 1170, 1171, 1920, 2559] {
                let physical = round_scaled(logical, scale);
                assert_eq!(
                    round_scaled(physical, 1.0 / scale),
                    logical,
                    "{logical} logical px drifted at scale {scale}"
                );
            }
        }
    }

    #[test]
    fn no_persisted_geometry_leaves_the_builder_default_alone() {
        assert!(startup_placement(None).is_none());
    }

    #[test]
    fn a_placement_change_settles_only_after_the_quiet_period() {
        let changed_at = Instant::now();
        assert!(!placement_settled(
            changed_at,
            changed_at + Duration::from_millis(GEOMETRY_SETTLE_MS - 1)
        ));
        assert!(placement_settled(
            changed_at,
            changed_at + Duration::from_millis(GEOMETRY_SETTLE_MS)
        ));
        assert!(placement_settled(
            changed_at,
            changed_at + Duration::from_secs(5)
        ));
    }

    #[test]
    fn a_placement_change_is_never_settled_before_it_happened() {
        let now = Instant::now();
        assert!(!placement_settled(now + Duration::from_secs(1), now));
    }
}
