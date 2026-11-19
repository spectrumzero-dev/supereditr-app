//! What a review does outside its own state, behind two narrow traits, so its event handlers
//! (`Controller` in `review`) run in tests against fakes that record each call.
//!
//! - [`Shell`]: the review's own UI: the panel's events, showing, hiding and placing it, and its
//!   shortcuts.
//! - [`Host`]: the few things a review asks of, or does to, the platform and the app it reviews.
//!
//! [`Live`] is the app's implementation of both: each method is the call the handlers made directly
//! before, on the `AppHandle`, `overlay`, `hotkeys`, `follow` or `platform`.

use crate::follow::Following;
use crate::platform::{self, Capture, PlatformError, Rect, ReplaceOutcome};
use serde::Serialize;
use std::future::Future;
use tauri::{AppHandle, Emitter};

/// The review panel and its shortcuts.
pub(crate) trait Shell: Sync {
    /// Sends `event` to the window labelled `window` ("overlay" for the panel, "settings").
    fn emit<P: Serialize + Clone>(&self, window: &str, event: &str, payload: P);
    /// Shows the panel; with `keys`, it takes the review keys (`overlay::show`).
    fn show(&self, keys: bool);
    fn hide(&self);
    /// Places the panel for content `height` tall by `anchor`, or by the pointer.
    fn position(&self, anchor: Option<Rect>, height: f64);
    fn resize(&self, anchor: Option<Rect>, height: f64);
    /// Turns the panel's own shortcuts on or off, where they are registered shortcuts.
    fn set_shortcuts(&self, active: bool);
    /// The panel takes keyboard focus for an instruction; `pid` gets it back afterwards.
    fn start_typing(&self, pid: i32);
    fn stop_typing(&self);
    /// Closing the panel next leaves focus where the user went (`overlay::forget_focus_return`). Does
    /// nothing on Windows.
    fn forget_focus_return(&self);
}

/// The platform, and the app whose text is reviewed.
pub(crate) trait Host: Sync {
    /// SuperEditr's own process.
    fn own_pid(&self) -> i32;
    /// Another app or Space came to the front: a selection under way belongs to the one left.
    fn selection_app_switched(&self);
    /// The panel had keyboard focus (Windows only; false elsewhere).
    fn panel_focused(&self) -> bool;
    /// The selection watcher saw a press or key that would close the panel. True when it was a
    /// press on the reviewed window, which then holds the close (`follow::hold_for_click`).
    fn hold_for_click(&self) -> bool;
    /// Starts following the window review `id`'s text is in (`follow::start`).
    fn follow(&self, id: u64, capture: &Capture) -> Option<Following>;
    /// The panel's keys come from the app numbered `pid` only; 0 for none (macOS only; does nothing
    /// elsewhere).
    fn set_review_app(&self, pid: i32);
    /// Returns focus to the reviewed field and, when it's back there and `text` differs, writes
    /// `text` into it, off the calling thread. None when there was nothing to write; Err when the
    /// worker failed.
    fn write(
        &self,
        captured: Capture,
        text: String,
        proofread: bool,
        anyway: bool,
    ) -> impl Future<Output = Result<Option<ReplaceOutcome>, String>> + Send;
    fn copy_to_clipboard(&self, text: &str);
}

/// The running app.
pub(crate) struct Live<'a>(pub &'a AppHandle);

impl Shell for Live<'_> {
    fn emit<P: Serialize + Clone>(&self, window: &str, event: &str, payload: P) {
        let _ = self.0.emit_to(window, event, payload);
    }

    fn show(&self, keys: bool) {
        crate::overlay::show(self.0, keys);
    }

    fn hide(&self) {
        crate::overlay::hide(self.0);
    }

    fn position(&self, anchor: Option<Rect>, height: f64) {
        crate::overlay::position(self.0, anchor, height);
    }

    fn resize(&self, anchor: Option<Rect>, height: f64) {
        crate::overlay::resize(self.0, anchor, height);
    }

    fn set_shortcuts(&self, active: bool) {
        crate::hotkeys::set_overlay_shortcuts(self.0, active);
    }

    fn start_typing(&self, pid: i32) {
        crate::overlay::start_typing(self.0, pid);
    }

    fn stop_typing(&self) {
        crate::overlay::stop_typing(self.0);
    }

    fn forget_focus_return(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        crate::overlay::forget_focus_return();
    }
}

impl Host for Live<'_> {
    fn own_pid(&self) -> i32 {
        platform::own_pid()
    }

    fn selection_app_switched(&self) {
        platform::selection_app_switched();
    }

    fn panel_focused(&self) -> bool {
        super::platform_panel_focused()
    }

    fn hold_for_click(&self) -> bool {
        crate::follow::hold_for_click(self.0)
    }

    fn follow(&self, id: u64, capture: &Capture) -> Option<Following> {
        crate::follow::start(self.0, id, capture)
    }

    fn set_review_app(&self, pid: i32) {
        #[cfg(target_os = "macos")]
        platform::set_review_app(pid);
        #[cfg(not(target_os = "macos"))]
        let _ = pid;
    }

    fn write(
        &self,
        captured: Capture,
        text: String,
        proofread: bool,
        anyway: bool,
    ) -> impl Future<Output = Result<Option<ReplaceOutcome>, String>> + Send {
        let written = tauri::async_runtime::spawn_blocking(move || {
            // Focus goes back to the app first, whether or not there's anything to write. Unless it's
            // confirmed there, nothing is written: a paste could land somewhere else.
            let returned = platform::return_to_field(&captured);
            (captured.text != text).then(|| {
                if returned {
                    ReplaceOutcome::from(replace_text(&captured, &text, proofread, anyway))
                } else {
                    ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)
                }
            })
        });
        async move { written.await.map_err(|e| e.to_string()) }
    }

    fn copy_to_clipboard(&self, text: &str) {
        platform::copy_to_clipboard(text);
    }
}

/// The platform's replacement. `anyway` is asked for only where the panel offered it (Windows): the
/// clipboard isn't put aside, and the fix stays on it.
fn replace_text(
    captured: &Capture,
    text: &str,
    proofread: bool,
    anyway: bool,
) -> Result<platform::ReplaceMethod, PlatformError> {
    #[cfg(windows)]
    if anyway {
        return platform::replace_anyway(captured, text);
    }
    #[cfg(not(windows))]
    let _ = anyway;
    platform::replace(captured, text, proofread)
}
