//! The review panel follows the window it belongs to. When the user moves that window, the panel
//! moves by the same amount, still placed by `overlay`'s rules (on screen, clear of the selection);
//! when the window is minimised, hidden or closed, the panel closes as it does on Esc.
//!
//! This part is the same on every platform. A platform reports what happened to the window as a
//! [`WindowChange`], through [`window_changed`]: macOS watches another app's window through
//! Accessibility (`platform::watch_window`), and Settings' "Try it now" box, whose window is
//! SuperEditr's own, reports through the window's own events ([`settings_event`]).

use crate::overlay;
use crate::platform::{self, Capture, Rect, WindowChange};
use crate::state::AppState;
use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

/// Where the window was when the panel was last placed for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    window: Rect,
}

impl Follow {
    pub fn new(window: Rect) -> Self {
        Self { window }
    }

    /// How far the window's top-left corner has moved from where it was.
    pub fn offset(&self, window: Rect) -> (f64, f64) {
        (window.x - self.window.x, window.y - self.window.y)
    }

    /// Moves `anchor` by as much as the window's top-left corner moved, and remembers where the
    /// window is now. The selection sits at the same place in the window, so it moved as much.
    pub fn follow(&mut self, anchor: Rect, window: Rect) -> Rect {
        let (dx, dy) = self.offset(window);
        self.window = window;
        Rect { x: anchor.x + dx, y: anchor.y + dy, ..anchor }
    }
}

/// A review's link to its window: where the window was, and the platform's watch on it, which
/// stops when this is dropped with the review.
pub struct Following {
    follow: Follow,
    _watch: Option<platform::WindowWatch>,
}

/// Starts following the window a review's text is in. `id` is the review's. None when the
/// platform can't watch that window; the panel then stays where it opened.
pub fn start(app: &AppHandle, id: u64, capture: &Capture) -> Option<Following> {
    if capture.playground {
        let frame = own_frame(&app.get_webview_window("settings")?)?;
        return Some(Following { follow: Follow::new(frame), _watch: None });
    }
    let handle = app.clone();
    let (watch, frame) = platform::watch_window(capture, move |change| window_changed(&handle, id, change))?;
    Some(Following { follow: Follow::new(frame), _watch: Some(watch) })
}

/// The window of review `id` moved or went. Moving places the panel again, shifted by as much as
/// the window moved; going closes the panel. Nothing happens once that review has ended.
pub fn window_changed(app: &AppHandle, id: u64, change: WindowChange) {
    let frame = match change {
        WindowChange::Gone => return crate::review::dismiss(app, Some(id)),
        WindowChange::Moved(frame) => frame,
    };
    let state = app.state::<AppState>();
    let (anchor, height) = {
        let mut session = state.session.lock().unwrap();
        let Some(current) = session.as_mut().filter(|s| s.id == id) else { return };
        let Some(anchor) = current.anchor.or_else(overlay::pointer_anchor) else { return };
        let Some(following) = current.following.as_mut() else { return };
        let moved = following.follow.follow(anchor, frame);
        if moved == anchor {
            return;
        }
        current.anchor = Some(moved);
        (moved, current.height)
    };
    overlay::resize(app, Some(anchor), height.clamp(90.0, 670.0));
}

/// An event of the Settings window, for a review of its "Try it now" box.
pub fn settings_event(app: &AppHandle, window: &WebviewWindow, event: &WindowEvent) {
    let id = {
        let state = app.state::<AppState>();
        let session = state.session.lock().unwrap();
        match session.as_ref().filter(|s| s.capture.playground) {
            Some(current) => current.id,
            None => return,
        }
    };
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true) {
                window_changed(app, id, WindowChange::Gone);
            } else if let Some(frame) = own_frame(window) {
                window_changed(app, id, WindowChange::Moved(frame));
            }
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => window_changed(app, id, WindowChange::Gone),
        _ => {}
    }
}

/// One of SuperEditr's own windows, in points.
fn own_frame(window: &WebviewWindow) -> Option<Rect> {
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.outer_size().ok()?.to_logical::<f64>(scale);
    Some(Rect { x: position.x, y: position.y, width: size.width, height: size.height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn the_anchor_moves_as_far_as_the_window() {
        let mut follow = Follow::new(rect(100.0, 80.0, 800.0, 600.0));
        let anchor = rect(240.0, 300.0, 180.0, 18.0);
        assert_eq!(follow.offset(rect(160.0, 40.0, 800.0, 600.0)), (60.0, -40.0));
        assert_eq!(follow.follow(anchor, rect(160.0, 40.0, 800.0, 600.0)), rect(300.0, 260.0, 180.0, 18.0));
    }

    #[test]
    fn each_move_counts_from_the_last() {
        let mut follow = Follow::new(rect(0.0, 0.0, 500.0, 400.0));
        let first = follow.follow(rect(50.0, 50.0, 10.0, 20.0), rect(10.0, 0.0, 500.0, 400.0));
        let second = follow.follow(first, rect(10.0, 25.0, 500.0, 400.0));
        assert_eq!(second, rect(60.0, 75.0, 10.0, 20.0));
    }

    #[test]
    fn resizing_moves_the_anchor_only_with_the_top_left_corner() {
        let anchor = rect(50.0, 50.0, 10.0, 20.0);
        let mut follow = Follow::new(rect(0.0, 0.0, 500.0, 400.0));
        assert_eq!(follow.follow(anchor, rect(0.0, 0.0, 700.0, 300.0)), anchor, "the bottom-right corner moved");
        assert_eq!(follow.follow(anchor, rect(-30.0, 0.0, 730.0, 300.0)), rect(20.0, 50.0, 10.0, 20.0));
    }
}
