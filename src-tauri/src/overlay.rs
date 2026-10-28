//! The suggestion overlay window: a borderless, transparent, non-activating panel.
//!
//! On macOS it is an NSPanel that can take keyboard focus without activating SuperEditr, so the
//! app being edited stays frontmost, keeps its selection, and gets focus back on dismiss.

use crate::platform::{self, Rect};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewWindow};

/// The window, including transparent room around the card for its shadow (Overlay.svelte's `main`
/// padding). The card itself is 496 points wide.
pub const WIDTH: f64 = 548.0;
const PAD_SIDE: f64 = 26.0;
const PAD_TOP: f64 = 16.0;
const PAD_BOTTOM: f64 = 38.0;
const GAP: f64 = 8.0;
const MARGIN: f64 = 10.0;

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
            panel.set_collection_behavior(
                CollectionBehavior::new().can_join_all_spaces().full_screen_auxiliary().transient().value(),
            );
            panel.set_has_shadow(false);
            panel.set_opaque(false);
        }
    }
}

/// Places the overlay just below the selection, or above it when there's no room below.
pub fn position(app: &AppHandle, anchor: Option<Rect>, height: f64) {
    let Some(window) = window(app) else { return };
    let anchor = anchor.unwrap_or_else(|| {
        let (x, y) = platform::mouse_location();
        Rect { x: x - 24.0, y: y - 10.0, width: 1.0, height: 20.0 }
    });
    let (sx, sy, sw, sh) = screen_for(app, anchor.x + 1.0, anchor.y + 1.0);
    // Placement is worked out for the card; the window extends past it by the shadow padding.
    let card_width = WIDTH - 2.0 * PAD_SIDE;
    let card_height = height - PAD_TOP - PAD_BOTTOM;
    let card_x = anchor.x.clamp(sx + MARGIN, (sx + sw - card_width - MARGIN).max(sx + MARGIN));
    let below = anchor.y + anchor.height + GAP;
    let card_y = if below + card_height <= sy + sh - MARGIN || anchor.y - GAP - card_height < sy + MARGIN {
        below.min(sy + sh - card_height - MARGIN).max(sy + MARGIN)
    } else {
        anchor.y - GAP - card_height
    };
    let (x, y) = (card_x - PAD_SIDE, card_y - PAD_TOP);
    let _ = window.set_size(LogicalSize::new(WIDTH, height));
    let _ = window.set_position(LogicalPosition::new(x, y));
}

/// Logical bounds of the display containing a point (falls back to the primary display).
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

/// Runs `f` on the main thread, where AppKit requires window changes. Reviews run on async worker
/// threads, and ordering the panel from one of them aborts the app.
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
    f(app);
}

pub fn show(app: &AppHandle, take_focus: bool) {
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
        if let Some(window) = window(app) {
            let _ = window.show();
            if take_focus {
                let _ = window.set_focus();
            }
        }
    });
}

pub fn focus(app: &AppHandle) {
    show(app, true);
}

pub fn hide(app: &AppHandle) {
    on_main(app, |app| {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = app.get_webview_panel("overlay") {
                panel.resign_key_window();
                panel.hide();
                return;
            }
        }
        if let Some(window) = window(app) {
            let _ = window.hide();
        }
    });
}
