//! What a review does outside its own state, behind three narrow traits, so its event handlers
//! (`Controller` in `review`) run in tests against fakes that record each call.
//!
//! - [`Shell`]: the review's own UI: the panel's events, showing, hiding and placing it, and its
//!   shortcuts.
//! - [`Host`]: the few things a review asks of, or does to, the platform and the app it reviews,
//!   and the clock.
//! - [`Engine`]: the proofreading engine, as a review starts it and hears from it.
//!
//! [`Live`] is the app's implementation of all three: each method is the call the handlers made
//! directly before, on the `AppHandle`, `overlay`, `hotkeys`, `follow`, `platform` or the engine.

use super::Asked;
use crate::follow::Following;
use crate::platform::{self, Capture, PlatformError, Rect, ReplaceOutcome};
use crate::state::{AppState, Task};
use serde::Serialize;
use std::future::Future;
use std::time::{Duration, Instant};
use supereditr_core::provider::{ProviderKind, ProviderProfile};
use supereditr_core::{Event, Job};
use tauri::{AppHandle, Emitter, Manager};

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
pub(crate) trait Host: Engine + Sync {
    /// SuperEditr's own process.
    fn own_pid(&self) -> i32;
    /// Another app or Space came to the front: a selection under way belongs to the one left.
    fn selection_app_switched(&self);
    /// The panel had keyboard focus (Windows only; false elsewhere).
    fn panel_focused(&self) -> bool;
    /// The selection watcher saw a press or key that would close the panel. True when it was a
    /// press on the reviewed window, which then holds the close (`follow::hold_for_click`).
    fn hold_for_click(&self) -> bool;
    /// A selection gesture ended. True when it was the end of a drag of the reviewed window, which
    /// selected nothing (`follow::gesture_ended`, which takes the review lock and may close the
    /// panel).
    fn gesture_ended(&self) -> bool;
    /// Starts following the window review `id`'s text is in (`follow::start`).
    fn follow(&self, id: u64, capture: &Capture) -> Option<Following>;
    /// The panel's keys come from the app numbered `pid` only; 0 for none (macOS only; does nothing
    /// elsewhere).
    fn set_review_app(&self, pid: i32);
    /// SuperEditr's own Settings window has focus, asked of the window itself.
    fn settings_focused(&self) -> bool;
    /// SuperEditr may read other apps' text: Accessibility is granted. Never asks for it.
    fn accessibility_trusted(&self) -> bool;
    /// The app in front: its process, bundle ID (or executable) and name.
    fn focused_app(&self) -> Option<(i32, String, String)>;
    /// Reads the selection off the calling thread. `select_sentence` is for the shortcuts only.
    fn capture(
        &self,
        allow_clipboard: bool,
        select_sentence: bool,
    ) -> impl Future<Output = Result<Capture, PlatformError>> + Send;
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
    /// Replaces the selection with `text` as it is, off the calling thread (quick fix). Err when the
    /// worker failed.
    fn replace(
        &self,
        captured: Capture,
        text: String,
        proofread: bool,
    ) -> impl Future<Output = Result<ReplaceOutcome, String>> + Send;
    fn copy_to_clipboard(&self, text: &str);
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
    /// Writes a line to the diagnostic log (`platform::trace`).
    fn trace(&self, message: std::fmt::Arguments);
}

/// The proofreading engine.
pub(crate) trait Engine: Sync {
    /// Opens a connection to the active AI ahead of a review, on a task of its own.
    fn warm_connection(&self);
    /// Starts run `asked` of a review on a task of its own: the engine works through `job` and
    /// hands each event to `Controller::engine_event`, with `asked`. Returns the task.
    fn start_run(&self, job: Job, asked: Asked) -> Task;
    /// Runs `job` to the end, for quick fix, and returns the events that end it: its result or its
    /// failure. Previews are dropped as they arrive; each holds a copy of the text.
    fn run_to_end(&self, job: Job) -> impl Future<Output = Vec<Event>> + Send;
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

    fn gesture_ended(&self) -> bool {
        crate::follow::gesture_ended(self.0)
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

    fn settings_focused(&self) -> bool {
        self.0.get_webview_window("settings").and_then(|w| w.is_focused().ok()).unwrap_or(false)
    }

    fn accessibility_trusted(&self) -> bool {
        platform::accessibility_trusted(false)
    }

    fn focused_app(&self) -> Option<(i32, String, String)> {
        platform::focused_app_identity()
    }

    fn capture(
        &self,
        allow_clipboard: bool,
        select_sentence: bool,
    ) -> impl Future<Output = Result<Capture, PlatformError>> + Send {
        capture(self.0, allow_clipboard, select_sentence)
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
                    replace_text(&captured, &text, proofread, anyway)
                } else {
                    ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)
                }
            })
        });
        async move { written.await.map_err(|e| e.to_string()) }
    }

    fn replace(
        &self,
        captured: Capture,
        text: String,
        proofread: bool,
    ) -> impl Future<Output = Result<ReplaceOutcome, String>> + Send {
        let replaced = tauri::async_runtime::spawn_blocking(move || platform::replace(&captured, &text, proofread));
        async move { replaced.await.map_err(|e| e.to_string()) }
    }

    fn copy_to_clipboard(&self, text: &str) {
        platform::copy_to_clipboard(text);
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }

    fn trace(&self, message: std::fmt::Arguments) {
        platform::trace(message);
    }
}

impl Engine for Live<'_> {
    fn warm_connection(&self) {
        let state = self.0.state::<AppState>();
        let profile = state.active_profile().unwrap_or_else(|| ProviderProfile::new(ProviderKind::AppleIntelligence));
        let providers = state.engine.providers.clone();
        tauri::async_runtime::spawn(async move { providers.warm(&profile).await });
    }

    fn start_run(&self, job: Job, asked: Asked) -> Task {
        let engine = self.0.state::<AppState>().engine.clone();
        let handle = self.0.clone();
        let task = tauri::async_runtime::spawn(async move {
            engine.run(job, &mut |event| super::controller(&handle, |c| c.engine_event(&asked, event))).await;
        });
        Task::new(move || task.abort())
    }

    fn run_to_end(&self, job: Job) -> impl Future<Output = Vec<Event>> + Send {
        let engine = self.0.state::<AppState>().engine.clone();
        async move {
            let mut events = Vec::new();
            engine
                .run(job, &mut |event| {
                    if matches!(event, Event::Done(_) | Event::Failed { .. }) {
                        events.push(event);
                    }
                })
                .await;
            events
        }
    }
}

/// Reads the selection on a worker thread. AX calls can block on a busy app; they must never
/// run on the UI thread. `select_sentence` is for the shortcuts only: an automatic suggestion
/// never changes what the user selected.
async fn capture(app: &AppHandle, allow_clipboard: bool, select_sentence: bool) -> Result<Capture, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        // Keyboard-layout lookups must happen on the main thread.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = app.run_on_main_thread(move || {
            platform::refresh_keyboard_layout();
            let _ = tx.send(());
        });
        let _ = tokio::time::timeout(Duration::from_millis(100), rx).await;
    }
    let _ = app;
    tauri::async_runtime::spawn_blocking(move || platform::capture(allow_clipboard, select_sentence))
        .await
        .unwrap_or_else(|e| Err(PlatformError::Other(e.to_string())))
}

/// The platform's replacement. `anyway` is asked for only where the panel offered it (Windows): the
/// clipboard isn't put aside, and the fix stays on it.
fn replace_text(captured: &Capture, text: &str, proofread: bool, anyway: bool) -> ReplaceOutcome {
    #[cfg(windows)]
    if anyway {
        return platform::replace_anyway(captured, text);
    }
    #[cfg(not(windows))]
    let _ = anyway;
    platform::replace(captured, text, proofread)
}
