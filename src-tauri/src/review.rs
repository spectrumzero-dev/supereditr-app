//! One review, from capture to replacement.
//!
//! Manual reviews start from the review hotkey and take the keyboard at once: the panel gets its
//! keys from the keyboard tap while the app keeps focus (see `review_keys`). Automatic reviews
//! start when the selection watcher sees a new selection in an editable field; their overlay
//! leaves the keys alone and any typing or clicking elsewhere dismisses it.

use crate::overlay;
use crate::platform::{self, Capture, PlatformError, SelectionGesture, WatchEvent};
use crate::state::{AppState, Session};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use supereditr_core::provider::{ProviderKind, ProviderProfile};
use supereditr_core::{Event, Job, sensitive};
use tauri::{AppHandle, Emitter, Manager};

const INITIAL_HEIGHT: f64 = 210.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetInfo {
    id: String,
    name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionPayload {
    id: u64,
    text: String,
    app_name: String,
    can_apply: bool,
    auto: bool,
    preset_id: String,
    presets: Vec<PresetInfo>,
    show_timing: bool,
    provider: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnginePayload {
    id: u64,
    preset_id: String,
    event: Event,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub id: u64,
    pub kind: &'static str,
    pub message: String,
}

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

/// Reads the selection on a worker thread. AX calls can block on a busy app; they must never
/// run on the UI thread.
async fn capture(app: &AppHandle, allow_clipboard: bool) -> Result<Capture, PlatformError> {
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
    tauri::async_runtime::spawn_blocking(move || platform::capture(allow_clipboard))
        .await
        .unwrap_or_else(|e| Err(PlatformError::Other(e.to_string())))
}

/// Decides, before any text is read, whether SuperEditr may act in the focused app. Returns a
/// message to show when the user explicitly asked (shortcut) and the app is excluded.
enum Target {
    Allowed,
    Ignored,
    Excluded(String),
    /// Accessibility isn't granted yet. SuperEditr then never touches other apps' text: macOS
    /// lists an app in Accessibility (switched off) the first time it tries, which would undo the
    /// user removing it.
    NotTrusted,
}

const NOT_TRUSTED: &str = "Allow SuperEditr in System Settings first: open SuperEditr Settings → Get Started.";

fn check_target(app: &AppHandle) -> Target {
    // SuperEditr's own Settings window. Asked directly: right after SuperEditr reopens (returning
    // from System Settings), Accessibility can still report the previous app as focused.
    if app.get_webview_window("settings").and_then(|w| w.is_focused().ok()).unwrap_or(false) {
        return Target::Ignored;
    }
    if !platform::accessibility_trusted(false) {
        return Target::NotTrusted;
    }
    let config = state(app).config();
    match platform::focused_app_identity() {
        Some((pid, _, _)) if pid == platform::own_pid() => Target::Ignored,
        Some((_, bundle_id, name)) if config.is_excluded(&bundle_id) => Target::Excluded(name),
        _ => Target::Allowed,
    }
}

fn excluded_message(name: &str) -> String {
    format!("SuperEditr is turned off in {name}. Change this in Settings → Behavior.")
}

fn warm_connection(app: &AppHandle) {
    let state = state(app);
    let profile = state.active_profile().unwrap_or_else(|| ProviderProfile::new(ProviderKind::AppleIntelligence));
    let providers = state.engine.providers.clone();
    tauri::async_runtime::spawn(async move { providers.warm(&profile).await });
}

/// Review hotkey: give the keys to an automatic overlay that's already showing, or start a new
/// review.
pub fn on_review_hotkey(app: &AppHandle) {
    {
        let state = state(app);
        let mut session = state.session.lock().unwrap();
        if let Some(current) = session.as_mut().filter(|s| !s.keyboard) {
            current.keyboard = true;
            overlay::show(app, true);
            let _ = app.emit_to("overlay", "overlay:focus", current.id);
            return;
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move { start_manual(&app, None).await });
}

pub async fn start_manual(app: &AppHandle, preset_id: Option<String>) {
    match check_target(app) {
        Target::Allowed => {}
        // In SuperEditr's own window, the shortcut reviews Settings' "Try it now" box, if focused.
        Target::Ignored => {
            let _ = app.emit_to("settings", "settings:review-shortcut", ());
            return;
        }
        Target::Excluded(name) => return show_error(app, excluded_message(&name)),
        Target::NotTrusted => return show_error(app, NOT_TRUSTED.into()),
    }
    warm_connection(app);
    match capture(app, true).await {
        Ok(captured) => begin(app, captured, false, preset_id),
        Err(error) => show_error(app, error.to_string()),
    }
}

/// Shows a standalone message (no selection to work on) near the pointer.
fn show_error(app: &AppHandle, message: String) {
    let state = state(app);
    let id = state.next_session_id();
    if let Some(mut old) = state.session.lock().unwrap().take() {
        old.cancel();
    }
    state.notice_visible.store(true, Ordering::Relaxed);
    let _ = app.emit_to("overlay", "overlay:status", StatusPayload { id, kind: "notice", message });
    overlay::position(app, None, 126.0);
    overlay::show(app, true);
}

fn begin(app: &AppHandle, captured: Capture, auto: bool, preset_id: Option<String>) {
    let state = state(app);
    let config = state.config();
    if captured.pid == platform::own_pid() && !captured.playground {
        return;
    }
    // Never open an empty review: say what to do instead (only when asked, by shortcut).
    if captured.text.trim().is_empty() {
        if !auto {
            show_error(app, PlatformError::NoSelection.to_string());
        }
        return;
    }
    if config.is_excluded(&captured.bundle_id) {
        if !auto {
            show_error(app, excluded_message(&captured.app_name));
        }
        return;
    }
    // A selection holding a secret never opens a review. The engine refuses it too; stopping here
    // keeps the panel from showing the secret, and keeps automatic suggestions silent.
    if let Some(found) = sensitive::find(&captured.text) {
        if !auto {
            show_error(app, found.message());
        }
        return;
    }
    let preset_id = preset_id.unwrap_or_else(|| config.default_preset().id.clone());
    let id = state.next_session_id();
    let anchor = captured.bounds;
    let payload = SessionPayload {
        id,
        text: captured.text.clone(),
        app_name: captured.app_name.clone(),
        can_apply: captured.editable,
        auto,
        preset_id: preset_id.clone(),
        presets: config.presets.iter().map(|p| PresetInfo { id: p.id.clone(), name: p.name.clone() }).collect(),
        show_timing: config.show_timing,
        provider: state.active_profile().map(|p| p.model),
    };
    {
        let mut session = state.session.lock().unwrap();
        if let Some(mut old) = session.take() {
            old.cancel();
        }
        state.notice_visible.store(false, Ordering::Relaxed);
        *session = Some(Session { id, capture: captured, keyboard: !auto, anchor, height: INITIAL_HEIGHT, task: None });
    }
    let _ = app.emit_to("overlay", "overlay:session", payload);
    overlay::position(app, anchor, INITIAL_HEIGHT);
    overlay::show(app, !auto);
    crate::hotkeys::set_overlay_shortcuts(app, true);
    run(app, id, preset_id, None);
}

/// Starts (or restarts) the model run for a session with a preset and optional steering.
pub fn run(app: &AppHandle, id: u64, preset_id: String, steer: Option<String>) {
    let state = state(app);
    let config = state.config();
    let Some(preset) = config.preset(&preset_id).cloned() else { return };
    let profile = state.active_profile();
    let mut session = state.session.lock().unwrap();
    let Some(current) = session.as_mut().filter(|s| s.id == id) else { return };
    current.cancel();
    let job = Job {
        text: current.capture.text.clone(),
        preset,
        steer,
        profile,
        style_notes: config.style_notes.clone(),
        dialect: config.dialect.clone(),
        words: config.words.clone(),
        local_checks: true,
        builtin_ai: true,
    };
    let engine = state.engine.clone();
    let handle = app.clone();
    current.task = Some(tauri::async_runtime::spawn(async move {
        engine
            .run(job, &mut |event| {
                let _ = handle.emit_to(
                    "overlay",
                    "overlay:engine",
                    EnginePayload { id, preset_id: preset_id.clone(), event },
                );
            })
            .await;
    }));
}

/// Writes the accepted text back into the source app.
pub async fn apply(app: &AppHandle, id: u64, text: String) -> Result<String, String> {
    let state = state(app);
    let captured = {
        let mut session = state.session.lock().unwrap();
        let current = session.as_mut().filter(|s| s.id == id).ok_or("This review has ended.")?;
        current.cancel();
        current.capture.clone()
    };
    overlay::hide(app);
    crate::hotkeys::set_overlay_shortcuts(app, false);
    if captured.playground {
        state.session.lock().unwrap().take();
        let _ = app.emit_to("settings", "settings:playground-apply", text);
        return Ok("Applied.".into());
    }
    *state.quiet_until.lock().unwrap() = Instant::now() + Duration::from_millis(800);
    tokio::time::sleep(Duration::from_millis(40)).await;
    let result = tauri::async_runtime::spawn_blocking(move || {
        if captured.text == text {
            return Ok(None);
        }
        platform::replace(&captured, &text).map(Some).inspect_err(|_| {
            platform::copy_to_clipboard(&text);
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    state.session.lock().unwrap().take();
    match result {
        Ok(None) => Ok("No changes needed.".into()),
        Ok(Some(_)) => Ok("Replaced.".into()),
        Err(error) => {
            show_error(app, error.to_string());
            Err(error.to_string())
        }
    }
}

/// Settings' "Try it now" box asked for a review (⌘E E, or its Review button), as if its text were
/// selected in another app.
pub fn review_playground(app: &AppHandle, text: String, bounds: platform::Rect) {
    begin(app, platform::playground_capture(text, bounds), false, None);
}

pub fn dismiss(app: &AppHandle, id: Option<u64>) {
    let state = state(app);
    let mut session = state.session.lock().unwrap();
    if id.is_some() && session.as_ref().map(|s| s.id) != id {
        return;
    }
    if let Some(mut current) = session.take() {
        current.cancel();
    }
    drop(session);
    state.notice_visible.store(false, Ordering::Relaxed);
    overlay::hide(app);
    crate::hotkeys::set_overlay_shortcuts(app, false);
}

/// The user starts (`true`) or stops typing an instruction in the panel, which takes keyboard focus
/// meanwhile.
pub fn typing(app: &AppHandle, typing: bool) {
    if !typing {
        return overlay::stop_typing(app);
    }
    let pid = state(app).session.lock().unwrap().as_ref().map(|s| s.capture.pid);
    if let Some(pid) = pid {
        overlay::start_typing(app, pid);
    }
}

pub fn resize(app: &AppHandle, height: f64) {
    let state = state(app);
    let anchor = {
        let mut session = state.session.lock().unwrap();
        match session.as_mut() {
            Some(current) => {
                current.height = height;
                current.anchor
            }
            None => None,
        }
    };
    overlay::position(app, anchor, height.clamp(90.0, 670.0));
}

/// Quick fix: proofread the selection and replace it directly, with no overlay.
pub async fn quick_fix(app: &AppHandle) {
    match check_target(app) {
        Target::Allowed => {}
        Target::Ignored => return,
        Target::Excluded(name) => return show_error(app, excluded_message(&name)),
        Target::NotTrusted => return show_error(app, NOT_TRUSTED.into()),
    }
    warm_connection(app);
    let state = state(app);
    let captured = match capture(app, true).await {
        Ok(c) => c,
        Err(error) => return show_error(app, error.to_string()),
    };
    let config = state.config();
    if captured.pid == platform::own_pid() || config.is_excluded(&captured.bundle_id) {
        return;
    }
    if !captured.editable {
        return show_error(
            app,
            "Quick fix works in text you’re editing. Use your review shortcut to review this text instead.".into(),
        );
    }
    let job = Job {
        text: captured.text.clone(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: state.active_profile(),
        style_notes: config.style_notes.clone(),
        dialect: config.dialect.clone(),
        words: config.words.clone(),
        local_checks: true,
        builtin_ai: true,
    };
    let mut outcome = None;
    let mut failure = None;
    state
        .engine
        .run(job, &mut |event| match event {
            Event::Done(done) => outcome = Some(done),
            Event::Failed { message, .. } => failure = Some(message),
            _ => {}
        })
        .await;
    let Some(outcome) = outcome else {
        return show_error(app, failure.unwrap_or_else(|| "Couldn’t check that text.".into()));
    };
    // Like the panel, leave out changes that would drop the author's own terms.
    let revised = outcome.kept_text();
    if revised == captured.text {
        return;
    }
    *state.quiet_until.lock().unwrap() = Instant::now() + Duration::from_millis(800);
    let text = revised.clone();
    let result = tauri::async_runtime::spawn_blocking(move || platform::replace(&captured, &text)).await;
    if let Ok(Err(error)) = result {
        platform::copy_to_clipboard(&revised);
        show_error(app, error.to_string());
    }
}

/// Another app came to the front (⌘Tab, the Dock): the panel belongs to the app whose text it
/// reviews, so it closes. ⌘Tab never reaches the selection watcher, which only sees the user's
/// typing and clicks.
pub fn on_app_switch(app: &AppHandle, pid: i32) {
    let state = state(app);
    if pid == platform::own_pid() {
        return;
    }
    // A review about to start, or a Shift selection under way, belongs to the previous app.
    state.watch_generation.fetch_add(1, Ordering::Relaxed);
    platform::selection_app_switched();
    let reviewing = state.session.lock().unwrap().as_ref().map(|s| s.capture.pid);
    let notice = state.notice_visible.load(Ordering::Relaxed);
    if reviewing.is_some_and(|source| source != pid) || (reviewing.is_none() && notice) {
        dismiss(app, None);
    }
}

/// Reacts to activity in other apps: dismiss transient overlays, start automatic reviews.
pub fn on_watch_event(app: &AppHandle, event: WatchEvent) {
    let state = state(app);
    match event {
        WatchEvent::Interaction => {
            // A review about to start is for a selection the user is changing.
            state.watch_generation.fetch_add(1, Ordering::Relaxed);
            // Keys the panel takes never reach other apps, so any typing or clicking seen here
            // means the user has moved on.
            let showing = state.session.lock().unwrap().is_some() || state.notice_visible.load(Ordering::Relaxed);
            if showing && Instant::now() >= *state.quiet_until.lock().unwrap() {
                dismiss(app, None);
            }
        }
        WatchEvent::Wait => {
            state.watch_generation.fetch_add(1, Ordering::Relaxed);
        }
        WatchEvent::MaybeSelection { settle, gesture } => {
            let config = state.config();
            if !config.auto_popup {
                return trace_auto(gesture, None, &AutoOutcome::TurnedOff);
            }
            if state.paused.load(Ordering::Relaxed) {
                return trace_auto(gesture, None, &AutoOutcome::Paused);
            }
            if Instant::now() < *state.quiet_until.lock().unwrap() {
                return trace_auto(gesture, None, &AutoOutcome::JustApplied);
            }
            if state.session.lock().unwrap().as_ref().is_some_and(|s| s.keyboard) {
                return trace_auto(gesture, None, &AutoOutcome::ReviewOpen);
            }
            let generation = state.watch_generation.fetch_add(1, Ordering::Relaxed) + 1;
            let app = app.clone();
            warm_connection(&app);
            tauri::async_runtime::spawn(async move {
                // Wait for the selection to settle; any newer event cancels this one.
                tokio::time::sleep(settle).await;
                let state = app.state::<AppState>();
                if state.watch_generation.load(Ordering::Relaxed) != generation {
                    return trace_auto(gesture, None, &AutoOutcome::Superseded);
                }
                match check_target(&app) {
                    Target::Allowed => {}
                    Target::Ignored => return trace_auto(gesture, None, &AutoOutcome::OwnWindow),
                    Target::Excluded(_) => return trace_auto(gesture, None, &AutoOutcome::Excluded),
                    Target::NotTrusted => return trace_auto(gesture, None, &AutoOutcome::NotTrusted),
                }
                let captured = match capture(&app, false).await {
                    Ok(captured) => captured,
                    Err(error) => return trace_auto(gesture, None, &AutoOutcome::ReadFailed(error)),
                };
                let app_id = Some(captured.bundle_id.as_str());
                let min_chars = state.config().auto_min_chars;
                if let Some(outcome) = check_selection(&captured.text, captured.editable, min_chars) {
                    return trace_auto(gesture, app_id, &outcome);
                }
                if state.session.lock().unwrap().as_ref().is_some_and(|s| s.capture.text == captured.text) {
                    return trace_auto(gesture, app_id, &AutoOutcome::AlreadyShowing);
                }
                if state.watch_generation.load(Ordering::Relaxed) != generation {
                    return trace_auto(gesture, app_id, &AutoOutcome::Superseded);
                }
                trace_auto(gesture, app_id, &AutoOutcome::Shown);
                begin(&app, captured, true, None);
            });
        }
    }
}

/// What became of one automatic suggestion attempt, for capture.log. Never holds selected text.
#[derive(Debug)]
enum AutoOutcome {
    Shown,
    TurnedOff,
    Paused,
    JustApplied,
    ReviewOpen,
    Superseded,
    OwnWindow,
    Excluded,
    NotTrusted,
    ReadFailed(PlatformError),
    NotEditable,
    TooShort { chars: usize, min: usize },
    NoLetter,
    AlreadyShowing,
}

impl std::fmt::Display for AutoOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shown => write!(f, "shown"),
            Self::TurnedOff => write!(f, "skipped: automatic suggestions are turned off"),
            Self::Paused => write!(f, "skipped: automatic suggestions are paused"),
            Self::JustApplied => write!(f, "skipped: a fix was just applied"),
            Self::ReviewOpen => write!(f, "skipped: a review opened with the shortcut is showing"),
            Self::Superseded => write!(f, "skipped: a newer selection or keystroke replaced this one"),
            Self::OwnWindow => write!(f, "skipped: SuperEditr's own window is in front"),
            Self::Excluded => write!(f, "skipped: the app is excluded in Settings"),
            Self::NotTrusted => write!(f, "skipped: Accessibility permission isn't granted"),
            Self::ReadFailed(error) => write!(f, "skipped: the selection couldn't be read ({error})"),
            Self::NotEditable => write!(f, "skipped: the field isn't editable"),
            Self::TooShort { chars, min } => write!(f, "skipped: {chars} characters, under the minimum of {min}"),
            Self::NoLetter => write!(f, "skipped: the selection has no letter"),
            Self::AlreadyShowing => write!(f, "skipped: the same text is already showing"),
        }
    }
}

/// Applies the rules an automatic suggestion's selection must pass; `None` when it may open.
fn check_selection(text: &str, editable: bool, min_chars: usize) -> Option<AutoOutcome> {
    let trimmed = text.trim();
    let chars = trimmed.chars().count();
    if !editable {
        Some(AutoOutcome::NotEditable)
    } else if chars < min_chars {
        Some(AutoOutcome::TooShort { chars, min: min_chars })
    } else if !trimmed.chars().any(char::is_alphabetic) {
        Some(AutoOutcome::NoLetter)
    } else {
        None
    }
}

fn trace_auto(gesture: SelectionGesture, app_id: Option<&str>, outcome: &AutoOutcome) {
    match app_id {
        Some(app_id) => platform::trace(format_args!("automatic {app_id}: after {gesture}, {outcome}")),
        None => platform::trace(format_args!("automatic: after {gesture}, {outcome}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(text: &str, editable: bool, min_chars: usize) -> Option<String> {
        check_selection(text, editable, min_chars).map(|outcome| outcome.to_string())
    }

    #[test]
    fn a_long_enough_editable_selection_with_a_letter_may_open() {
        assert_eq!(reason("Rhyan sent the KPI report.", true, 12), None);
    }

    #[test]
    fn each_rejected_selection_names_its_rule() {
        assert_eq!(reason("Rhyan sent the KPI report.", false, 12).unwrap(), "skipped: the field isn't editable");
        assert_eq!(reason("  Glenmoor  ", true, 12).unwrap(), "skipped: 8 characters, under the minimum of 12");
        assert_eq!(reason("1234 5678 9012", true, 12).unwrap(), "skipped: the selection has no letter");
    }

    #[test]
    fn a_rejection_never_includes_the_selected_text() {
        let line = reason("Brynlee", true, 12).unwrap();
        assert!(!line.contains("Brynlee"), "{line}");
    }
}
