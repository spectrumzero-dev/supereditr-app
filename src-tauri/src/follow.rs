//! The review panel follows the window it belongs to. When the user moves that window, the panel
//! moves by the same amount, still placed by `overlay`'s rules (on screen, clear of the selection);
//! when the window is minimised, hidden or closed, the panel closes as it does on Esc.
//!
//! This part is the same on every platform. A platform reports what happened to the window as a
//! [`WindowChange`], through [`window_changed`]: macOS watches another app's window through
//! Accessibility (`platform::watch_window`), and Settings' "Try it now" box, whose window is
//! SuperEditr's own, reports through the window's own events ([`settings_event`]).
//!
//! A click elsewhere closes the panel, but dragging the window by its title bar starts with a
//! click on that window. So a click on the reviewed window holds the close for [`HOLD`] (and for as
//! long as the button stays down): if the window moves meanwhile, it's a drag and the panel stays
//! and follows; if not, the panel closes as before ([`Hold`]).

use crate::overlay;
use crate::platform::{self, Capture, Rect, WindowChange};
use crate::state::AppState;
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

/// How long a click on the reviewed window holds off closing the panel, waiting for the window to
/// move. A drag moves the window as soon as the pointer travels a few points, and the app reports
/// the move through Accessibility within a frame or two, even when busy, so a quarter of a second
/// is enough for a quick flick of the title bar. It also stays short enough that a plain click in
/// the window's text still closes the panel at once to the eye. A slower start is covered anyway:
/// the hold lasts while the button is down.
pub const HOLD: Duration = Duration::from_millis(250);
/// While the button stays down after [`HOLD`], how often to look again.
const HOLD_RECHECK: Duration = Duration::from_millis(50);

/// Whether a click on the reviewed window turned out to be a drag of that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hold {
    /// No click is being held.
    #[default]
    Idle,
    /// A click on the window; the panel closes unless the window moves. `click` numbers it, so a
    /// timer for an earlier click can't close the panel for a later one.
    Waiting { click: u64 },
    /// The window moved after the click: the user is dragging it, and the panel stays.
    Dragging,
}

/// What to do when a held click's time is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Close,
    /// The button is still down: look again shortly.
    Wait,
    /// The window moved, or this click is no longer the one held: the panel stays.
    Keep,
}

impl Hold {
    /// The user pressed the mouse button on the window: hold the close for click number `click`.
    pub fn click(&mut self, click: u64) {
        *self = Self::Waiting { click };
    }

    /// The window moved. After a click, that makes the click a drag.
    pub fn moved(&mut self) {
        if matches!(self, Self::Waiting { .. }) {
            *self = Self::Dragging;
        }
    }

    /// Time is up for click number `click`; `button_down` is whether the mouse button still is.
    pub fn deadline(&mut self, click: u64, button_down: bool) -> Verdict {
        match *self {
            Self::Waiting { click: held } if held == click => {
                if button_down {
                    Verdict::Wait
                } else {
                    *self = Self::Idle;
                    Verdict::Close
                }
            }
            _ => Verdict::Keep,
        }
    }

    /// The button came up after a drag or another selection gesture. A held click that didn't move
    /// the window was the start of a selection, so the panel closes (`Close`), as it would have at
    /// the click; after a drag of the window, the gesture selected nothing (`Keep`).
    pub fn gesture_ended(&mut self) -> Option<Verdict> {
        let verdict = match *self {
            Self::Idle => return None,
            Self::Waiting { .. } => Verdict::Close,
            Self::Dragging => Verdict::Keep,
        };
        *self = Self::Idle;
        Some(verdict)
    }
}

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
    hold: Hold,
    _watch: Option<platform::WindowWatch>,
}

/// Starts following the window a review's text is in. `id` is the review's. None when the
/// platform can't watch that window; the panel then stays where it opened.
pub fn start(app: &AppHandle, id: u64, capture: &Capture) -> Option<Following> {
    if capture.playground {
        let frame = own_frame(&app.get_webview_window("settings")?)?;
        return Some(Following { follow: Follow::new(frame), hold: Hold::Idle, _watch: None });
    }
    let handle = app.clone();
    let (watch, frame) = platform::watch_window(capture, move |change| window_changed(&handle, id, change))?;
    Some(Following { follow: Follow::new(frame), hold: Hold::Idle, _watch: Some(watch) })
}

/// The selection watcher saw a click or key that would close the panel. Returns true when it was a
/// click on the reviewed window, which then holds the close instead (see [`Hold`]); false when the
/// panel should close now.
pub fn hold_for_click(app: &AppHandle) -> bool {
    static CLICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let state = app.state::<AppState>();
    let (id, pid, window) = {
        let session = state.session.lock().unwrap();
        let Some(current) = session.as_ref() else { return false };
        let Some(following) = current.following.as_ref().filter(|f| f._watch.is_some()) else { return false };
        (current.id, current.capture.pid, following.follow.window)
    };
    if !platform::pressed_on_window(pid, window) {
        return false;
    }
    // Follow the window from the press, so the panel moves with the drag's first frame.
    if let Some(watch) = state.session.lock().unwrap().as_ref().and_then(|s| s.following.as_ref()?._watch.as_ref()) {
        watch.track();
    }
    let click = CLICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    {
        let mut session = state.session.lock().unwrap();
        let Some(following) = session.as_mut().filter(|s| s.id == id).and_then(|s| s.following.as_mut()) else {
            return false;
        };
        following.hold.click(click);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(HOLD);
        loop {
            let verdict = {
                let state = app.state::<AppState>();
                let mut session = state.session.lock().unwrap();
                match session.as_mut().filter(|s| s.id == id).and_then(|s| s.following.as_mut()) {
                    Some(following) => following.hold.deadline(click, platform::mouse_button_down()),
                    None => Verdict::Keep,
                }
            };
            match verdict {
                Verdict::Wait => std::thread::sleep(HOLD_RECHECK),
                Verdict::Close => return crate::review::dismiss(&app, Some(id)),
                Verdict::Keep => return,
            }
        }
    });
    true
}

/// A selection gesture ended (the button came up after a drag, a double-click, a Shift-click).
/// Returns true when it was the end of a drag of the reviewed window, which selected nothing: the
/// panel stays, and no new suggestion starts. After a click on the window that didn't move it, the
/// panel closes now, as it would have at the click, and the gesture goes on to be reviewed.
pub fn gesture_ended(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let (id, verdict) = {
        let mut session = state.session.lock().unwrap();
        let Some(current) = session.as_mut() else { return false };
        let Some(verdict) = current.following.as_mut().and_then(|f| f.hold.gesture_ended()) else { return false };
        (current.id, verdict)
    };
    match verdict {
        Verdict::Close => {
            crate::review::dismiss(app, Some(id));
            false
        }
        _ => {
            // Clicking the window gave it keyboard focus; an instruction being typed gets it back.
            overlay::resume_typing(app);
            true
        }
    }
}

/// The window of review `id` moved or went. While it moves, the panel moves by as much; once it
/// stops, the panel is placed again by the full rules; going closes the panel. Nothing happens
/// once that review has ended.
pub fn window_changed(app: &AppHandle, id: u64, change: WindowChange) {
    let (frame, moving) = match change {
        WindowChange::Gone => return crate::review::dismiss(app, Some(id)),
        WindowChange::Moving(frame) => (frame, true),
        WindowChange::Moved(frame) => (frame, false),
    };
    let state = app.state::<AppState>();
    let (anchor, height, offset) = {
        let mut session = state.session.lock().unwrap();
        let Some(current) = session.as_mut().filter(|s| s.id == id) else { return };
        let Some(anchor) = current.anchor.or_else(overlay::pointer_anchor) else { return };
        let Some(following) = current.following.as_mut() else { return };
        let offset = following.follow.offset(frame);
        if offset != (0.0, 0.0) {
            following.hold.moved();
        }
        let moved = following.follow.follow(anchor, frame);
        current.anchor = Some(moved);
        (moved, current.height, offset)
    };
    if moving {
        // Once per display frame: only move the panel. Its full placement waits for the end.
        if offset != (0.0, 0.0) {
            overlay::nudge(app, offset.0, offset.1);
        }
    } else {
        overlay::resize(app, Some(anchor), height.clamp(90.0, 670.0));
    }
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
    fn a_click_that_moves_the_window_keeps_the_panel() {
        let mut hold = Hold::default();
        hold.click(1);
        hold.moved();
        assert_eq!(hold.deadline(1, false), Verdict::Keep);
        assert_eq!(hold.gesture_ended(), Some(Verdict::Keep), "the drag selected nothing");
        assert_eq!(hold, Hold::Idle);
    }

    #[test]
    fn a_click_that_doesnt_move_the_window_closes_the_panel() {
        let mut hold = Hold::default();
        hold.click(1);
        assert_eq!(hold.deadline(1, false), Verdict::Close);
        hold.click(2);
        assert_eq!(hold.gesture_ended(), Some(Verdict::Close), "a selection made in the window's text");
    }

    #[test]
    fn a_held_button_waits_for_a_move() {
        let mut hold = Hold::default();
        hold.click(1);
        assert_eq!(hold.deadline(1, true), Verdict::Wait);
        assert_eq!(hold.deadline(1, true), Verdict::Wait);
        hold.moved();
        assert_eq!(hold.deadline(1, false), Verdict::Keep);
    }

    #[test]
    fn an_earlier_clicks_timer_leaves_a_later_click_alone() {
        let mut hold = Hold::default();
        hold.click(1);
        hold.click(2);
        assert_eq!(hold.deadline(1, false), Verdict::Keep);
        assert_eq!(hold.deadline(2, false), Verdict::Close);
    }

    #[test]
    fn a_move_without_a_click_changes_nothing() {
        let mut hold = Hold::default();
        hold.moved();
        assert_eq!(hold, Hold::Idle);
        assert_eq!(hold.gesture_ended(), None);
    }

    #[test]
    fn resizing_moves_the_anchor_only_with_the_top_left_corner() {
        let anchor = rect(50.0, 50.0, 10.0, 20.0);
        let mut follow = Follow::new(rect(0.0, 0.0, 500.0, 400.0));
        assert_eq!(follow.follow(anchor, rect(0.0, 0.0, 700.0, 300.0)), anchor, "the bottom-right corner moved");
        assert_eq!(follow.follow(anchor, rect(-30.0, 0.0, 730.0, 300.0)), rect(20.0, 50.0, 10.0, 20.0));
    }
}
