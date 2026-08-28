//! Cursor hit-testing for the overlay window.
//!
//! A transparent, decoration-less window is still a solid target as far as
//! Windows is concerned: every click inside its bounds is swallowed, whether or
//! not anything is drawn there. For a subtitle overlay that spans the bottom of
//! the screen and is empty most of the time, that is the wrong default — the
//! window spends most of its life blocking the app underneath it.
//!
//! CSS `pointer-events: none` does not help. It governs which element inside
//! the page receives an event, not whether the OS delivers one to the window at
//! all; the click is already consumed by the time the page sees it.
//!
//! The only lever is `set_ignore_cursor_events`, which is all-or-nothing per
//! window. So we drive it from the cursor position: the frontend publishes the
//! rectangles it wants to stay clickable (`set_hit_regions`), and this thread
//! polls the global cursor and flips the flag as it crosses them.
//!
//! Polling rather than reacting to events, because the window receives no mouse
//! events at all while it is ignoring the cursor — there is no event that could
//! tell us to turn interaction back on.

use tauri::{AppHandle, Manager};

use crate::state::{self, HitRect};
use crate::types::ClickThrough;

/// How often the cursor is sampled. Fast enough that moving onto the control
/// bar feels instant, slow enough to be invisible in a CPU profile.
const POLL_MS: u64 = 50;

/// Start the hit-test thread. Runs for the lifetime of the app.
pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("hit-test".into())
        .spawn(move || {
            let mut applied: Option<bool> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                let Some(want) = desired_ignore(&app) else { continue };
                if applied == Some(want) {
                    continue;
                }
                if set_ignore(&app, want) {
                    applied = Some(want);
                }
            }
        })
        .expect("spawn hit-test thread");
}

/// Apply the current mode immediately, without waiting for the next poll.
/// Called when the mode itself changes so the toggle feels instant.
pub fn apply_now(app: &AppHandle) {
    if let Some(want) = desired_ignore(app) {
        set_ignore(app, want);
    }
}

/// Should the window be ignoring the cursor right now?
/// `None` means "cannot tell" — leave whatever is applied alone.
fn desired_ignore(app: &AppHandle) -> Option<bool> {
    let mode = state::read_state(app, |s| s.click_through)?;
    let want = match mode {
        ClickThrough::On => true,
        ClickThrough::Off => false,
        ClickThrough::Auto => {
            // Never yank the mouse away mid-drag. Dragging a slider or the
            // window itself routinely takes the cursor outside every region,
            // and turning the window transparent to the mouse at that moment
            // drops the drag on the floor.
            if left_button_down() {
                return None;
            }
            !cursor_over_region(app)
        }
    };
    Some(want)
}

fn set_ignore(app: &AppHandle, ignore: bool) -> bool {
    let Some(w) = app.get_webview_window("main") else { return false };
    if w.set_ignore_cursor_events(ignore).is_err() {
        return false;
    }
    state::update_and_emit(app, |s| s.click_through_active = ignore);
    true
}

/// Is the cursor inside any rectangle the frontend published?
///
/// Fails safe: if the window geometry or the cursor cannot be read we report
/// "yes", leaving the window interactive. An overlay that wrongly keeps the
/// mouse is recoverable by the user; one that wrongly passes it through hides
/// its own controls.
fn cursor_over_region(app: &AppHandle) -> bool {
    let Some(regions) = state::read_state(app, |s| s.hit_regions.clone()) else {
        return true;
    };
    if regions.is_empty() {
        return false;
    }
    let Some(w) = app.get_webview_window("main") else { return true };
    // `inner_position` is the client area — the same origin the frontend's
    // getBoundingClientRect() values are relative to.
    let (Ok(origin), Ok(scale)) = (w.inner_position(), w.scale_factor()) else {
        return true;
    };
    let Some((cx, cy)) = cursor_pos() else { return true };

    let (ox, oy) = (f64::from(origin.x), f64::from(origin.y));
    regions.iter().any(|r| inside(r, ox, oy, scale, cx, cy))
}

fn inside(r: &HitRect, ox: f64, oy: f64, scale: f64, cx: f64, cy: f64) -> bool {
    let x0 = ox + r.x * scale;
    let y0 = oy + r.y * scale;
    cx >= x0 && cx < x0 + r.w * scale && cy >= y0 && cy < y0 + r.h * scale
}

#[cfg(target_os = "windows")]
fn cursor_pos() -> Option<(f64, f64)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut p = POINT::default();
    // SAFETY: `p` is a valid, owned POINT; GetCursorPos only writes to it.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((f64::from(p.x), f64::from(p.y)))
}

#[cfg(not(target_os = "windows"))]
fn cursor_pos() -> Option<(f64, f64)> {
    None
}

#[cfg(target_os = "windows")]
fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

    // The high bit means "currently down". The low bit is a since-last-call
    // latch we deliberately ignore.
    // SAFETY: no arguments to validate; the call only reads keyboard state.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) };
    state as u16 & 0x8000 != 0
}

#[cfg(not(target_os = "windows"))]
fn left_button_down() -> bool {
    false
}

/// Store the regions the frontend published, replacing the previous set.
pub fn set_regions(app: &AppHandle, regions: Vec<HitRect>) {
    if let Some(st) = app.try_state::<std::sync::Mutex<crate::state::AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.hit_regions = regions;
        }
    }
    // Deliberately no status emit: this fires on every layout change and
    // carries nothing the UI does not already know.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> HitRect {
        HitRect { x, y, w, h }
    }

    #[test]
    fn inside_maps_css_pixels_through_the_window_origin() {
        // A 100x36 bar at CSS (10, 200) in a window whose client area starts at
        // physical (500, 900) on a 1.5x display covers physical x 515..=664.
        let r = rect(10.0, 200.0, 100.0, 36.0);
        assert!(inside(&r, 500.0, 900.0, 1.5, 515.0, 1201.0));
        assert!(!inside(&r, 500.0, 900.0, 1.5, 514.0, 1201.0));
        assert!(!inside(&r, 500.0, 900.0, 1.5, 665.0, 1201.0));
    }

    #[test]
    fn inside_is_half_open_so_adjacent_rects_do_not_overlap() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        assert!(inside(&r, 0.0, 0.0, 1.0, 0.0, 0.0));
        assert!(!inside(&r, 0.0, 0.0, 1.0, 10.0, 0.0));
    }
}
