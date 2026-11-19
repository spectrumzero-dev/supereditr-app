//! The suggestion overlay window: a borderless, transparent, non-activating panel.
//!
//! On macOS it is an NSPanel that doesn't activate SuperEditr, so the app being edited stays
//! frontmost and keeps its selection. It doesn't take keyboard focus either: during a review the
//! keyboard tap passes the panel its keys (`crate::review_keys`), and the app keeps its key window.
//! The panel takes focus only while the user types an instruction, and then gives it back.
//!
//! On Linux (Wayland) an app can't place its own windows or keep keys from another app's window.
//! The desktop places the panel through a window rule, and for a review started with the shortcut
//! the panel takes keyboard focus, then hands it back to the app when it closes. The desktop sizes
//! the panel too, and can lose a resize or move made while it is putting the panel on screen, so
//! each placement is checked afterwards (`keep_placed`).
//!
//! On Windows the panel is a window that never activates on a click (`WS_EX_NOACTIVATE`), shown,
//! placed and hidden with Win32 calls in physical pixels (`platform::overlay`). A review started
//! with the shortcut gives it keyboard focus, and closing it gives focus back to the app.

use crate::platform::{self, Rect};
use std::sync::atomic::{AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use tauri::LogicalPosition;
use tauri::{AppHandle, LogicalSize, Manager, WebviewWindow};

/// The app to give keyboard focus back to when the user stops typing an instruction; 0 when the
/// panel doesn't have focus.
static TYPING_FOR: AtomicI32 = AtomicI32::new(0);

/// The window, including transparent room around the card for its shadow (Overlay.svelte's `main`
/// padding). The card itself is 496 points wide.
pub const WIDTH: f64 = 548.0;
pub const PAD_SIDE: f64 = 26.0;
pub const PAD_TOP: f64 = 16.0;
pub const PAD_BOTTOM: f64 = 38.0;
pub const GAP: f64 = 8.0;
pub const MARGIN: f64 = 10.0;

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true,
            hides_on_deactivate: false
        }
    })
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("overlay")
}

/// Converts the overlay window to a panel once at start-up.
pub fn init(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::{CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};
        if let Some(window) = window(app)
            && let Ok(panel) = window.to_panel::<OverlayPanel>()
        {
            panel.set_level(PanelLevel::PopUpMenu.value());
            let _ = panel.set_style_mask(StyleMask::empty().borderless().nonactivating_panel().value());
            // It opens on the Space the user is on, and stays there: on a move to another Space it
            // slides away with the text it reviews, and closes once the move ends
            // (`review::on_space_change`). On every Space, it would float over the switch.
            panel.set_collection_behavior(
                CollectionBehavior::new().move_to_active_space().full_screen_auxiliary().transient().value(),
            );
            panel.set_has_shadow(false);
            panel.set_opaque(false);
        }
    }
    // The desktop's rule for the panel matches this title: every SuperEditr window shares one app ID.
    #[cfg(target_os = "linux")]
    if let Some(window) = window(app) {
        let _ = window.set_title(platform::OVERLAY_TITLE);
    }
    #[cfg(windows)]
    if let Some(window) = window(app) {
        platform::overlay::init(&window);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    let _ = app;
}

/// Where the overlay window was last put, in points.
static PLACED: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

/// Where the overlay last opened at the pointer.
#[cfg_attr(windows, allow(dead_code))]
static POINTER_ANCHOR: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

/// Where the overlay last opened at the pointer, for a review with no selection bounds to go by.
pub fn pointer_anchor() -> Option<Rect> {
    #[cfg(windows)]
    return platform::overlay::pointer_anchor();
    #[cfg(not(windows))]
    *POINTER_ANCHOR.lock().unwrap()
}

/// Places the overlay just below the selection, or above it when there's no room below.
pub fn position(app: &AppHandle, anchor: Option<Rect>, height: f64) {
    #[cfg(windows)]
    if let Some(window) = window(app) {
        return on_main(app, move |_| platform::overlay::place(&window, anchor, height, false));
    }
    let anchor = anchor.unwrap_or_else(|| {
        let (x, y) = platform::mouse_location();
        let at_pointer = Rect { x: x - 24.0, y: y - 10.0, width: 1.0, height: 20.0 };
        *POINTER_ANCHOR.lock().unwrap() = Some(at_pointer);
        at_pointer
    });
    place(app, anchor, height);
}

/// Places the overlay again at a new height. With no selection to go by, it stays where it opened
/// instead of following the pointer: a desktop may move the pointer onto a window that takes focus.
pub fn resize(app: &AppHandle, anchor: Option<Rect>, height: f64) {
    #[cfg(windows)]
    if let Some(window) = window(app) {
        return on_main(app, move |_| platform::overlay::place(&window, anchor, height, true));
    }
    // Read on its own line: a lock taken in a match's scrutinee is held until the match ends, and
    // `position` below takes it again, which would hang the main thread.
    let at_pointer = *POINTER_ANCHOR.lock().unwrap();
    match anchor.or(at_pointer) {
        Some(anchor) => place(app, anchor, height),
        None => position(app, None, height),
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn place(app: &AppHandle, anchor: Rect, height: f64) {
    let Some(window) = window(app) else { return };
    let (x, y) = target(app, anchor, height);
    *PLACED.lock().unwrap() = Some(Rect { x, y, width: WIDTH, height });
    let _ = window.set_size(LogicalSize::new(WIDTH, height));
    #[cfg(target_os = "linux")]
    {
        let wanted = Rect { x, y, width: WIDTH, height };
        // Counted before anything is sent, and under the lock a correction takes, so a correction
        // of an earlier placement either finishes first or finds itself superseded and sends nothing.
        let placement = {
            let _one_at_a_time = platform::lock(&PLACING);
            let placement = PLACEMENT.fetch_add(1, Ordering::Relaxed) + 1;
            platform::place_overlay(wanted, window.is_visible().unwrap_or(false));
            placement
        };
        keep_placed(wanted, placement);
    }
    #[cfg(target_os = "macos")]
    let _ = window.set_position(LogicalPosition::new(x, y));
}

/// Counts the panel's placements and each time it is hidden, so a check of an earlier placement
/// stops.
#[cfg(target_os = "linux")]
static PLACEMENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Held while a placement or a correction of one is sent to the compositor, so the two don't
/// interleave.
#[cfg(target_os = "linux")]
static PLACING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether the panel's window isn't where it was put, or is smaller than it was asked to be, which
/// cuts the card off. A larger window is left alone: a window has a smallest size of its own, and
/// the room past the card is transparent.
#[cfg(any(target_os = "linux", test))]
fn drifted(wanted: Rect, actual: Rect) -> bool {
    let short = actual.width + 1.0 < wanted.width || actual.height + 1.0 < wanted.height;
    let moved = (actual.x - wanted.x).abs() > 1.0 || (actual.y - wanted.y).abs() > 1.0;
    short || moved
}

/// Checks a few times that the panel is where it was just put, at the size it was given, and puts
/// it there again when it isn't.
///
/// On Hyprland the panel's size and place are the compositor's to give: a rule gives them when the
/// panel opens, and a resize and a move change them while it shows. The answer often arrives while
/// Hyprland is still putting the panel on screen. A resize or move sent before the panel is mapped
/// is lost, and a rule replaced in those moments may or may not be the one the panel opens by.
/// Hyprland also resizes a window around its centre whenever the window's own limits on its size
/// change, which moves it. Nothing corrects any of that later.
#[cfg(target_os = "linux")]
fn keep_placed(wanted: Rect, placement: u64) {
    use std::time::Duration;
    /// Milliseconds between checks. They start again after a correction.
    const WAITS: [u64; 6] = [40, 80, 150, 300, 600, 1200];
    const MOST_CORRECTIONS: u32 = 6;
    tauri::async_runtime::spawn(async move {
        let (mut corrections, mut next) = (0, 0);
        while let Some(wait) = WAITS.get(next) {
            next += 1;
            tokio::time::sleep(Duration::from_millis(*wait)).await;
            if PLACEMENT.load(Ordering::Relaxed) != placement {
                return;
            }
            // Not on screen yet, or on a desktop that can't say where its windows are. Asked off the
            // async runtime's workers, as the placing below is: both wait on the compositor.
            let Ok(Some(actual)) = tauri::async_runtime::spawn_blocking(platform::overlay_rect).await else { continue };
            if !drifted(wanted, actual) {
                continue;
            }
            let again = corrections < MOST_CORRECTIONS;
            platform::trace(format_args!(
                "panel is at {:.0},{:.0}, {:.0}×{:.0}, not where it was put: {}",
                actual.x,
                actual.y,
                actual.width,
                actual.height,
                if again { "placing it again" } else { "leaving it there" }
            ));
            if !again {
                return;
            }
            corrections += 1;
            // Checked again where it is sent: the panel may have been placed anew, or hidden, while
            // the compositor answered or this waited its turn.
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let _one_at_a_time = platform::lock(&PLACING);
                if PLACEMENT.load(Ordering::Relaxed) == placement {
                    platform::place_overlay(wanted, true);
                }
            })
            .await;
            next = 0;
        }
    });
}

/// Where the overlay window goes for a selection at `anchor`: the top-left corner, in points.
#[cfg_attr(windows, allow(dead_code))]
fn target(app: &AppHandle, anchor: Rect, height: f64) -> (f64, f64) {
    let (sx, sy, sw, sh) = screen_for(app, anchor.x + 1.0, anchor.y + 1.0);
    // Placement is worked out for the card; the window extends past it by the shadow padding.
    let card_width = WIDTH - 2.0 * PAD_SIDE;
    let card_height = height - PAD_TOP - PAD_BOTTOM;
    let card_x = anchor.x.clamp(sx + MARGIN, (sx + sw - card_width - MARGIN).max(sx + MARGIN));
    let below = anchor.y + anchor.height + GAP;
    let card_y = if below + card_height <= sy + sh - MARGIN || anchor.y - GAP - card_height < sy + MARGIN {
        below.min(sy + sh - card_height - MARGIN).max(sy + MARGIN)
    } else {
        // Kept on screen even if the height it was given is out of date.
        (anchor.y - GAP - card_height).max(sy + MARGIN)
    };
    (card_x - PAD_SIDE, card_y - PAD_TOP)
}

/// Places the overlay for a selection at `anchor` by the usual rules, gliding there from where it
/// is rather than jumping: for following a window that has moved. A glide already headed there is
/// left to finish. Where there's no glide, or the height changed too, it is placed at once.
pub fn glide(app: &AppHandle, anchor: Rect, height: f64) {
    let Some(window) = window(app) else { return };
    // Windows places the panel in physical pixels, as it gets the anchor.
    #[cfg(windows)]
    on_main(app, move |_| platform::overlay::glide(&window, anchor, height));
    #[cfg(not(windows))]
    glide_in_points(app, window, anchor, height);
}

#[cfg(not(windows))]
fn glide_in_points(app: &AppHandle, window: WebviewWindow, anchor: Rect, height: f64) {
    let (x, y) = target(app, anchor, height);
    let to = Rect { x, y, width: WIDTH, height };
    let placed = PLACED.lock().unwrap().replace(to);
    if placed == Some(to) {
        return;
    }
    #[cfg(target_os = "macos")]
    if placed.is_some_and(|p| p.height == height) {
        return on_main(app, move |_| {
            if !window.ns_window().is_ok_and(|ns_window| platform::glide_own_window(ns_window, x, y)) {
                let _ = window.set_position(LogicalPosition::new(x, y));
            }
        });
    }
    let _ = window;
    place(app, anchor, height);
}

/// The overlay's window has started moving again: a glide under way stops where it is, to go on
/// from there once the window settles.
pub fn stop_glide(app: &AppHandle) {
    // Where the glide stops isn't known here: no target matches it, so the next glide goes ahead.
    if let Some(placed) = PLACED.lock().unwrap().as_mut() {
        placed.x = f64::NAN;
    }
    #[cfg(target_os = "macos")]
    if let Some(window) = window(app) {
        on_main(app, move |_| {
            if let Ok(ns_window) = window.ns_window() {
                platform::stop_own_window_glide(ns_window);
            }
        });
    }
    #[cfg(windows)]
    platform::overlay::stop_glide();
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// On Windows, after the user dragged the reviewed window: clicking its title bar took the keyboard
/// from the panel, so a panel that had it gets it back, and its keys work again.
#[cfg(windows)]
pub fn take_focus_back(app: &AppHandle) {
    if let Some(window) = window(app) {
        // Off the main thread, which the glide starting now needs for each of its steps.
        std::thread::spawn(move || platform::overlay::refocus(&window));
    }
}

/// Logical bounds of the display containing a point (falls back to the primary display).
#[cfg_attr(windows, allow(dead_code))]
fn screen_for(app: &AppHandle, x: f64, y: f64) -> (f64, f64, f64, f64) {
    let monitors = app.available_monitors().unwrap_or_default();
    let logical = |m: &tauri::Monitor| {
        let scale = m.scale_factor();
        let p = m.position();
        let s = m.size();
        (p.x as f64 / scale, p.y as f64 / scale, s.width as f64 / scale, s.height as f64 / scale)
    };
    monitors
        .iter()
        .map(logical)
        .find(|(mx, my, mw, mh)| x >= *mx && x < mx + mw && y >= *my && y < my + mh)
        .or_else(|| monitors.first().map(logical))
        .unwrap_or((0.0, 0.0, 1440.0, 900.0))
}

/// Runs `f` on the main thread, where AppKit and GTK require window changes. Reviews run on async
/// worker threads, and ordering the panel from one of them aborts the app on macOS.
fn on_main(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_main_np() -> i32;
        }
        if unsafe { pthread_main_np() } == 0 {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || f(&handle));
            return;
        }
    }
    #[cfg(any(target_os = "linux", windows))]
    {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || f(&handle));
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    f(app);
}

/// Shows the panel without taking keyboard focus. With `keys`, the panel gets the review keys
/// (↩, esc, tab…) instead of the app. Where the keyboard tap can't run, it takes focus instead.
/// Without `keys`, on macOS, the panel still gets esc alone, so closing it leaves the app and its
/// selection as they were.
pub fn show(app: &AppHandle, keys: bool) {
    let take_focus = keys && !platform::take_review_keys(true);
    #[cfg(target_os = "macos")]
    if !keys {
        platform::take_escape_key();
    }
    on_main(app, move |app| {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = app.get_webview_panel("overlay") {
                if take_focus {
                    panel.show_and_make_key();
                } else {
                    panel.order_front_regardless();
                }
                return;
            }
        }
        #[cfg(windows)]
        if let Some(window) = window(app) {
            platform::overlay::show(&window, take_focus);
            return;
        }
        if let Some(window) = window(app) {
            let _ = window.show();
            #[cfg(target_os = "linux")]
            if take_focus {
                platform::focus_overlay();
            }
            #[cfg(not(target_os = "linux"))]
            if take_focus {
                let _ = window.set_focus();
            }
        }
    });
}

/// Gives the panel keyboard focus so the user can type an instruction. `pid` is the app under
/// review, which gets focus back from `stop_typing`.
pub fn start_typing(app: &AppHandle, pid: i32) {
    TYPING_FOR.store(pid, Ordering::Relaxed);
    platform::take_review_keys(false);
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        platform::focus_overlay();
    }
    #[cfg(not(target_os = "linux"))]
    on_main(app, |app| {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = app.get_webview_panel("overlay") {
                panel.show_and_make_key();
                return;
            }
        }
        #[cfg(windows)]
        if let Some(window) = window(app) {
            platform::overlay::show(&window, true);
            return;
        }
        if let Some(window) = window(app) {
            let _ = window.set_focus();
        }
    });
}

/// After the user dragged the reviewed window, which took keyboard focus from the panel: an
/// instruction they were typing gets focus back. Nothing happens when they weren't typing; the
/// panel's keys never left it then.
pub fn resume_typing(app: &AppHandle) {
    let pid = TYPING_FOR.load(Ordering::Relaxed);
    if pid != 0 {
        start_typing(app, pid);
    }
}

/// The user is done typing: the app under review gets keyboard focus back and the panel its keys.
/// Nothing happens once the panel has closed.
pub fn stop_typing(app: &AppHandle) {
    if give_focus_back(app) {
        platform::take_review_keys(true);
    }
}

/// Returns whether the panel had focus for typing.
fn give_focus_back(app: &AppHandle) -> bool {
    let pid = TYPING_FOR.swap(0, Ordering::Relaxed);
    if pid == 0 {
        return false;
    }
    if pid == platform::own_pid() {
        // A review of Settings' "Try it now" box.
        if let Some(settings) = app.get_webview_window("settings") {
            let _ = settings.set_focus();
        }
    } else {
        platform::return_focus(pid);
    }
    true
}

/// The user moved to another window, Space or workspace: closing the panel next mustn't focus the
/// app the text came from, which would take focus from where the user went, or take them back to
/// the Space or workspace they left.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn forget_focus_return() {
    TYPING_FOR.store(0, Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    platform::forget_focus_target();
}

pub fn hide(app: &AppHandle) {
    platform::take_review_keys(false);
    // At once, not on the main thread: the app coming back to the front from here on isn't the user
    // moving on.
    #[cfg(windows)]
    let had_focus = platform::overlay::hiding();
    #[cfg(target_os = "linux")]
    let had_focus = platform::overlay_focused();
    #[cfg(target_os = "linux")]
    PLACEMENT.fetch_add(1, Ordering::Relaxed);
    give_focus_back(app);
    on_main(app, move |app| {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = app.get_webview_panel("overlay") {
                panel.resign_key_window();
                panel.hide();
                return;
            }
        }
        #[cfg(windows)]
        if let Some(window) = window(app) {
            platform::overlay::hide(&window, had_focus);
            return;
        }
        if let Some(window) = window(app) {
            let _ = window.hide();
        }
        #[cfg(target_os = "linux")]
        if had_focus {
            platform::overlay_closed();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn forgetting_the_focus_return_leaves_nothing_to_refocus() {
        // The user was typing an instruction for an app with pid 4242, then went elsewhere.
        TYPING_FOR.store(4242, Ordering::Relaxed);
        forget_focus_return();
        assert_eq!(TYPING_FOR.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_panel_cut_short_or_moved_has_drifted() {
        // A tall panel for a long text, put at 590,297: Hyprland was seen leaving it 240 high, and
        // once 146 lower as well, with its foot off an 800-high screen.
        let wanted = Rect { x: 590.0, y: 297.0, width: WIDTH, height: 531.0 };
        assert!(drifted(wanted, Rect { height: 240.0, ..wanted }));
        assert!(drifted(wanted, Rect { y: 443.0, height: 240.0, ..wanted }));
        // Resized around its centre after the move reached the desktop: 160 higher, at its full size.
        assert!(drifted(wanted, Rect { y: 137.0, ..wanted }));
        assert!(drifted(wanted, Rect { x: 600.0, ..wanted }));
    }

    #[test]
    fn a_panel_where_it_was_put_has_not() {
        let wanted = Rect { x: 590.0, y: 297.0, width: WIDTH, height: 531.0 };
        assert!(!drifted(wanted, wanted));
        assert!(!drifted(wanted, Rect { y: 298.0, height: 530.0, ..wanted }), "half a pixel rounds either way");
        // A window keeps to a smallest size of its own.
        let short = Rect { x: 3.0, y: 484.0, width: WIDTH, height: 90.0 };
        assert!(!drifted(short, Rect { height: 120.0, ..short }));
    }
}
