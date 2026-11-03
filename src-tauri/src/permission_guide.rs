//! A small card that walks the user through granting Accessibility access: it
//! opens System Settings → Privacy & Security → Accessibility and floats beside that window with a
//! SuperEditr tile to drag into the list. It follows the window, hides while another app is in
//! front, and closes itself once macOS trusts SuperEditr.

use crate::platform::{self, SettingsWindow};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// The card itself. The window is larger by `SHADOW` on every side, so the card's shadow fades out
/// instead of stopping at the window's edge; `Guide.svelte` uses the same margin.
const CARD_WIDTH: f64 = 368.0;
const CARD_HEIGHT: f64 = 140.0;
const SHADOW: f64 = 28.0;
/// Space between the card and System Settings' window, and from the window's corner.
const GAP: f64 = 12.0;
const INSET: f64 = 18.0;
/// Give up if System Settings stays closed this long, or the user never finishes.
const CLOSED_GRACE: Duration = Duration::from_secs(4);
const MAX_DURATION: Duration = Duration::from_secs(15 * 60);

static RUNNING: AtomicBool = AtomicBool::new(false);
static DISMISSED: AtomicBool = AtomicBool::new(false);
/// When the user last dropped SuperEditr into the list. macOS then asks for Touch ID or a
/// password, so the card stays out of the way for a while.
static DROPPED: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
const AFTER_DROP: Duration = Duration::from_secs(30);

fn recently_dropped() -> bool {
    DROPPED.lock().unwrap().is_some_and(|at| at.elapsed() < AFTER_DROP)
}

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(GuidePanel {
        config: {
            can_become_key_window: false,
            can_become_main_window: false,
            // A floating panel is forced to the floating window level, above the Touch ID prompt
            // for adding an app; this one stays at normal level just above System Settings.
            is_floating_panel: false,
            hides_on_deactivate: false
        }
    })
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("guide")
}

/// Turns the guide window into a non-activating panel, so dragging from it never brings
/// SuperEditr's own windows in front of System Settings.
pub fn init(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::{CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};
        if let Some(window) = window(app)
            && let Ok(panel) = window.to_panel::<GuidePanel>()
        {
            // Normal level, kept directly above System Settings' window (see `stack`), so system
            // dialogs such as the Touch ID prompt for adding an app always appear over the card.
            panel.set_level(PanelLevel::Normal.value());
            let _ = panel.set_style_mask(StyleMask::empty().borderless().nonactivating_panel().value());
            panel.set_collection_behavior(
                CollectionBehavior::new().can_join_all_spaces().full_screen_auxiliary().transient().value(),
            );
            panel.set_opaque(false);
        }
    }
}

fn set_visible(app: &AppHandle, visible: bool) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = handle.get_webview_panel("guide") {
                if visible {
                    panel.order_front_regardless();
                } else {
                    panel.hide();
                }
                return;
            }
        }
        if let Some(window) = window(&handle) {
            let _ = if visible { window.show() } else { window.hide() };
        }
    });
}

/// Shows the guide even when SuperEditr is already trusted, to check it on a Mac that has granted
/// access: `SUPEREDITR_PREVIEW_GUIDE=1 /Applications/SuperEditr.app/Contents/MacOS/supereditr`.
fn preview() -> bool {
    std::env::var_os("SUPEREDITR_PREVIEW_GUIDE").is_some()
}

fn trusted() -> bool {
    !preview() && platform::accessibility_trusted(false)
}

/// Where the card sits relative to System Settings' window. The card's arrow and wording point
/// from there to the list.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum Side {
    Right,
    Left,
    Below,
    /// Over the top of the window, its title and description, when the screen has no room beside it.
    Inside,
}

/// Places the card beside System Settings' window (`window` and `screen` in screen points, top-left
/// origin) so it never covers the list, where SuperEditr is usually near the bottom. Beside the
/// window it lines up with the list's end; with no room on any side it covers the window's top.
fn spot(window: &platform::Rect, screen: &platform::Rect) -> (platform::Rect, Side) {
    let (right, bottom) = (window.x + window.width, window.y + window.height);
    let (screen_right, screen_bottom) = (screen.x + screen.width, screen.y + screen.height);
    let y = (bottom - INSET - CARD_HEIGHT).min(screen_bottom - CARD_HEIGHT).max(screen.y);
    let card = |x: f64, y: f64| platform::Rect { x, y, width: CARD_WIDTH, height: CARD_HEIGHT };
    if right + GAP + CARD_WIDTH <= screen_right {
        (card(right + GAP, y), Side::Right)
    } else if window.x - GAP - CARD_WIDTH >= screen.x {
        (card(window.x - GAP - CARD_WIDTH, y), Side::Left)
    } else if bottom + GAP + CARD_HEIGHT <= screen_bottom {
        let x = (right - INSET - CARD_WIDTH).min(screen_right - CARD_WIDTH).max(screen.x);
        (card(x, bottom + GAP), Side::Below)
    } else {
        (card(right - INSET - CARD_WIDTH, window.y + INSET), Side::Inside)
    }
}

/// The usable area (without the menu bar and Dock) of the display showing most of `frame`, in
/// points. Falls back to `frame` itself, which puts the card inside the window.
fn screen_for(app: &AppHandle, frame: &platform::Rect) -> platform::Rect {
    let (cx, cy) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            let area = m.work_area();
            platform::Rect {
                x: area.position.x as f64 / scale,
                y: area.position.y as f64 / scale,
                width: area.size.width as f64 / scale,
                height: area.size.height as f64 / scale,
            }
        })
        .find(|s| cx >= s.x && cx < s.x + s.width && cy >= s.y && cy < s.y + s.height)
        .unwrap_or(*frame)
}

/// Puts the guide beside System Settings' window (see `spot`) and tells the card which way to
/// point. Set directly on the NSWindow, in points, so it lines up exactly with the window-list
/// frame whatever Tauri's logical units are.
fn place(app: &AppHandle, frame: &platform::Rect) {
    let (card, side) = spot(frame, &screen_for(app, frame));
    let _ = app.emit_to("guide", "guide:side", side);
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let (x, top) = (card.x - SHADOW, card.y - SHADOW);
        let (width, height) = (CARD_WIDTH + 2.0 * SHADOW, CARD_HEIGHT + 2.0 * SHADOW);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(ptr) = window(&handle).and_then(|w| w.ns_window().ok()) else { return };
            // AppKit's origin is the primary display's bottom-left corner.
            let primary = core_graphics::display::CGDisplay::main().bounds().size.height;
            let rect = NSRect::new(NSPoint::new(x, primary - top - height), NSSize::new(width, height));
            let ns_window = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
            ns_window.setFrame_display(rect, true);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = card;
}

/// Brings SuperEditr in front of System Settings once access is granted. macOS may refuse an app
/// that activates itself while another app is in use; opening the app through Launch Services, as
/// the Dock does, is honored.
fn bring_forward() {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = platform::own_bundle() {
        let _ = std::process::Command::new("/usr/bin/open").arg(bundle).spawn();
    }
}

/// Orders the card just above System Settings' window. Clicking System Settings raises its window
/// over the card; this puts the card back on the next tick.
fn stack(app: &AppHandle, settings_window: i64) {
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(ptr) = window(&handle).and_then(|w| w.ns_window().ok()) else { return };
            let ns_window = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
            ns_window.orderWindow_relativeTo(objc2_app_kit::NSWindowOrderingMode::Above, settings_window as isize);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, settings_window);
}

/// Opens the Accessibility pane and starts the guide, unless SuperEditr is already trusted.
pub fn start(app: &AppHandle) {
    platform::open_accessibility_settings();
    if trusted() || platform::own_bundle().is_none() {
        return;
    }
    DISMISSED.store(false, Ordering::Relaxed);
    *DROPPED.lock().unwrap() = None;
    if RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut last_open = Instant::now();
        let mut shown = false;
        let mut last_frame = None;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if trusted() {
                set_visible(&app, false);
                let _ = app.emit_to("settings", "settings:accessibility-granted", ());
                crate::show_settings(&app, None);
                bring_forward();
                break;
            }
            if DISMISSED.load(Ordering::Relaxed) || started.elapsed() > MAX_DURATION {
                break;
            }
            match platform::system_settings_window() {
                SettingsWindow::Prompt => {
                    last_open = Instant::now();
                    if shown {
                        set_visible(&app, false);
                        shown = false;
                    }
                }
                SettingsWindow::Front(..) if recently_dropped() => {
                    last_open = Instant::now();
                    if shown {
                        set_visible(&app, false);
                        shown = false;
                    }
                }
                SettingsWindow::Front(frame, number) => {
                    last_open = Instant::now();
                    // Showing the panel restores its configured frame, so place it after.
                    if !shown {
                        set_visible(&app, true);
                        shown = true;
                        last_frame = None;
                    }
                    if last_frame.as_ref() != Some(&frame) {
                        place(&app, &frame);
                        last_frame = Some(frame);
                    }
                    stack(&app, number);
                }
                state => {
                    if shown {
                        set_visible(&app, false);
                        shown = false;
                    }
                    if matches!(state, SettingsWindow::Behind) {
                        last_open = Instant::now();
                    } else if last_open.elapsed() > CLOSED_GRACE {
                        break;
                    }
                }
            }
        }
        set_visible(&app, false);
        RUNNING.store(false, Ordering::Relaxed);
    });
}

pub fn dismiss(app: &AppHandle) {
    DISMISSED.store(true, Ordering::Relaxed);
    set_visible(app, false);
}

/// Starts dragging SuperEditr.app from the guide. Must run on the main thread while the mouse
/// button is down, which a synchronous command called from `mousedown` satisfies.
pub fn drag_app(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let bundle = platform::own_bundle().ok_or("SuperEditr isn’t running from an app bundle.")?;
        let icon = include_bytes!("../icons/64x64.png").to_vec();
        drag::start_drag(
            window,
            drag::DragItem::Files(vec![bundle]),
            drag::Image::Raw(icon),
            |result, _| {
                if matches!(result, drag::DragResult::Dropped) {
                    *DROPPED.lock().unwrap() = Some(Instant::now());
                }
            },
            Default::default(),
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err("Not supported on this platform.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::Rect;

    const SCREEN: Rect = Rect { x: 0.0, y: 25.0, width: 1470.0, height: 890.0 };

    fn settings(x: f64, width: f64) -> Rect {
        Rect { x, y: 60.0, width, height: 760.0 }
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    #[test]
    fn sits_beside_the_window_where_the_screen_has_room() {
        let window = settings(300.0, 715.0);
        let (card, side) = spot(&window, &SCREEN);
        assert_eq!(side, Side::Right);
        assert!(!overlaps(&card, &window), "{card:?}");
        assert_eq!(card.y + card.height, window.y + window.height - INSET, "lines up with the list's end");

        let (card, side) = spot(&settings(740.0, 715.0), &SCREEN);
        assert_eq!(side, Side::Left);
        assert!(!overlaps(&card, &settings(740.0, 715.0)), "{card:?}");
    }

    #[test]
    fn goes_below_a_short_wide_window_and_inside_a_full_screen_one() {
        let wide = Rect { x: 0.0, y: 25.0, width: 1470.0, height: 500.0 };
        let (card, side) = spot(&wide, &SCREEN);
        assert_eq!(side, Side::Below);
        assert!(!overlaps(&card, &wide) && card.y + card.height <= SCREEN.y + SCREEN.height, "{card:?}");

        let (card, side) = spot(&SCREEN, &SCREEN);
        assert_eq!(side, Side::Inside);
        assert_eq!(card.y, SCREEN.y + INSET, "covers the title, not the list");
    }

    #[test]
    fn stays_on_screen_beside_a_window_that_runs_off_the_bottom() {
        let low = Rect { x: 300.0, y: 500.0, width: 715.0, height: 760.0 };
        let (card, side) = spot(&low, &SCREEN);
        assert_eq!(side, Side::Right);
        assert_eq!(card.y + card.height, SCREEN.y + SCREEN.height);
    }
}
