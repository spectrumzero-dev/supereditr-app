//! One review, from capture to replacement.
//!
//! Manual reviews start from the review hotkey and take the keyboard at once: the panel gets its
//! keys from the keyboard tap while the app keeps focus (see `review_keys`). Automatic reviews
//! start when the selection watcher sees a new selection in an editable field; their overlay
//! takes only plain esc, which closes it; any other typing, or clicking elsewhere, dismisses it too.

use crate::app_toggle;
use crate::last_review::Finished;
use crate::platform::{self, Capture, PlatformError, ReplaceOutcome, SelectionGesture, WatchEvent};
use crate::state::{AppState, Dismissal, Session};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use supereditr_core::prompts::Preset;
use supereditr_core::provider::{ProviderKind, ProviderProfile};
use supereditr_core::{Config, Event, Job, sensitive};
use tauri::{AppHandle, Emitter, Manager};

mod shell;
#[cfg(test)]
mod tests_handlers;

use shell::{Host, Live, Shell};

const INITIAL_HEIGHT: f64 = 210.0;

/// The review's event handlers, over its state, its UI (`Shell`) and the platform (`Host`). The app
/// runs them on [`Live`] (see [`controller`]); tests on fakes that record each call.
pub(crate) struct Controller<'a, S, H> {
    state: &'a AppState,
    shell: &'a S,
    host: &'a H,
}

/// Runs `f` with the app's controller.
fn controller<R>(app: &AppHandle, f: impl FnOnce(&Controller<'_, Live<'_>, Live<'_>>) -> R) -> R {
    let live = Live(app);
    f(&Controller { state: app.state::<AppState>().inner(), shell: &live, host: &live })
}

#[derive(Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
struct PresetInfo {
    id: String,
    name: String,
}

#[derive(Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
struct SessionPayload {
    id: u64,
    text: String,
    app_name: String,
    can_apply: bool,
    /// For a copy-only capture, how to make the app replaceable, when SuperEditr knows.
    #[cfg_attr(test, ts(optional = nullable))]
    copy_only_hint: Option<String>,
    auto: bool,
    preset_id: String,
    presets: Vec<PresetInfo>,
    show_timing: bool,
    always_show_tips: bool,
    provider: Option<String>,
}

#[derive(Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
struct EnginePayload {
    id: u64,
    preset_id: String,
    /// Which run of the session the event is from (see `Session::run`), so the panel can drop
    /// events from a run it has replaced.
    run: u64,
    event: Event,
    /// The result of an earlier review of the same selection, shown again: no AI ran just now.
    remembered: bool,
}

#[derive(Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub id: u64,
    #[cfg_attr(test, ts(type = r#""notice" | "applied" | "error""#))]
    pub kind: &'static str,
    pub message: String,
    /// A suggestion the notice offers to copy, when it couldn't be confirmed in the app.
    #[cfg_attr(test, ts(optional = nullable))]
    pub copy: Option<String>,
}

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
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

/// Decides, before any text is read, whether SuperEditr may act in the focused app. Returns a
/// message to show when the user explicitly asked (shortcut) and the app is excluded.
enum Target {
    Allowed,
    Ignored,
    /// The message to show.
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
        Some((_, bundle_id, name)) if config.is_excluded(&bundle_id) => {
            Target::Excluded(excluded_message(&config, &bundle_id, &name))
        }
        _ => Target::Allowed,
    }
}

fn excluded_message(config: &Config, bundle_id: &str, name: &str) -> String {
    if app_toggle::offered(config, bundle_id) {
        let tray = if cfg!(windows) { "notification area" } else { "menu bar" };
        format!(
            "SuperEditr is turned off in {name}. Turn it on from the pen icon in the {tray}, or in Settings → Privacy."
        )
    } else {
        format!("SuperEditr is turned off in {name}. Change this in Settings → Privacy.")
    }
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
    if controller(app, |c| c.keys_to_automatic_review()) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move { start_manual(&app, None).await });
}

pub async fn start_manual(app: &AppHandle, preset_id: Option<String>) {
    let _busy = crate::updater::busy();
    match check_target(app) {
        Target::Allowed => {}
        // In SuperEditr's own window, the shortcut reviews Settings' "Try it now" box, if focused.
        Target::Ignored => {
            let _ = app.emit_to("settings", "settings:review-shortcut", ());
            return;
        }
        Target::Excluded(message) => return show_error(app, message),
        Target::NotTrusted => return show_error(app, NOT_TRUSTED.into()),
    }
    warm_connection(app);
    let select_sentence = state(app).config().select_sentence;
    match capture(app, true, select_sentence).await {
        Ok(captured) => {
            let _ = begin(app, captured, false, preset_id);
        }
        Err(error) => show_capture_error(app, error),
    }
}

/// Says why nothing could be read. While an input method is composing, the panel doesn't take
/// focus: that would commit the composition into the user's text.
fn show_capture_error(app: &AppHandle, error: PlatformError) {
    let keys = !matches!(error, PlatformError::Composing);
    controller(app, |c| c.show_notice_with(error.to_string(), None, keys));
}

/// Shows a standalone message (no selection to work on) near the pointer.
pub fn show_error(app: &AppHandle, message: String) {
    controller(app, |c| c.show_error(message));
}

fn show_outcome(app: &AppHandle, outcome: ReplaceOutcome, suggestion: String) -> Option<String> {
    controller(app, |c| c.show_outcome(outcome, suggestion))
}

/// Whether a replacement's outcome puts the suggestion on the clipboard: only when nothing was
/// written. After a write that may have landed, the clipboard stays as the platform left it: a
/// paste that can't be confirmed leaves the fix there, so a late paste inserts it.
fn copies_suggestion(outcome: &ReplaceOutcome) -> bool {
    matches!(outcome, ReplaceOutcome::CopyOnly(_))
}

/// Opens the panel for a selection. Returns why it didn't, for an automatic suggestion's log line.
fn begin(app: &AppHandle, captured: Capture, auto: bool, preset_id: Option<String>) -> Result<(), AutoOutcome> {
    let (id, preset_id) = controller(app, |c| c.begin(captured, auto, preset_id))?;
    run(app, id, 0, preset_id, None, false);
    Ok(())
}

impl<S: Shell, H: Host> Controller<'_, S, H> {
    /// The review shortcut, with an automatic review showing: the panel takes the keys, and the
    /// shortcut starts no new review. False when there's no automatic review to give them to.
    fn keys_to_automatic_review(&self) -> bool {
        let mut review = self.state.review.lock().unwrap();
        let Some(id) = review.take_keyboard() else { return false };
        // Under the lock, so a dismissal can't put its hide ahead of this show and leave an empty
        // panel up.
        self.shell.show(true);
        self.shell.emit("overlay", "overlay:focus", id);
        true
    }

    /// Shows a standalone message (no selection to work on) near the pointer.
    fn show_error(&self, message: String) {
        self.show_notice(message, None);
    }

    /// Says what became of a replacement that didn't apply: a copy-only one puts the suggestion on
    /// the clipboard; an uncertain one, or one that couldn't put the clipboard aside, leaves the
    /// clipboard alone and offers the suggestion to copy.
    fn show_outcome(&self, outcome: ReplaceOutcome, suggestion: String) -> Option<String> {
        if copies_suggestion(&outcome) {
            self.host.copy_to_clipboard(&suggestion);
        }
        match outcome {
            ReplaceOutcome::Applied => None,
            ReplaceOutcome::CopyOnly(error) => {
                self.show_error(error.to_string());
                Some(error.to_string())
            }
            ReplaceOutcome::Uncertain(error) | ReplaceOutcome::ClipboardKept(error) => {
                self.show_notice(error.to_string(), Some(suggestion));
                Some(error.to_string())
            }
        }
    }

    fn show_notice(&self, message: String, copy: Option<String>) {
        self.show_notice_with(message, copy, true);
    }

    fn show_notice_with(&self, message: String, copy: Option<String>, keys: bool) {
        let id = self.state.next_session_id();
        let replaced = self.state.review.lock().unwrap().show_notice();
        drop(replaced);
        self.shell.emit("overlay", "overlay:status", StatusPayload { id, kind: "notice", message, copy });
        self.shell.position(None, 126.0);
        // Does nothing outside macOS.
        self.host.set_review_app(0);
        self.shell.show(keys);
        // The notice closes with the panel's close key too, where that's a registered shortcut.
        #[cfg(windows)]
        self.shell.set_shortcuts(true);
    }

    /// Opens the panel for a selection. Returns the review's number and style, for its first run,
    /// or why it didn't open, for an automatic suggestion's log line.
    fn begin(&self, captured: Capture, auto: bool, preset_id: Option<String>) -> Result<(u64, String), AutoOutcome> {
        let state = self.state;
        let config = state.config();
        if captured.pid == self.host.own_pid() && !captured.playground {
            return Err(AutoOutcome::OwnWindow);
        }
        // Never open an empty review: say what to do instead (only when asked, by shortcut).
        if captured.text.trim().is_empty() {
            if !auto {
                self.show_error(PlatformError::NoSelection.to_string());
            }
            return Err(AutoOutcome::ReadFailed(PlatformError::NoSelection));
        }
        if config.is_excluded(&captured.bundle_id) {
            if !auto {
                self.show_error(excluded_message(&config, &captured.bundle_id, &captured.app_name));
            }
            return Err(AutoOutcome::Excluded);
        }
        // A selection holding a secret never opens a review. The engine refuses it too; stopping
        // here keeps the panel from showing the secret, and keeps automatic suggestions silent.
        if let Some(found) = sensitive::find(&captured.text) {
            if !auto {
                self.show_error(found.message());
            }
            return Err(AutoOutcome::Secret);
        }
        // Reopened on a selection reviewed a moment ago, the panel opens on the style shown last,
        // and `run` shows that result again.
        let preset_id = preset_id
            .or_else(|| {
                state.last_review.last_style(&captured, Instant::now()).filter(|id| config.preset(id).is_some())
            })
            .unwrap_or_else(|| config.default_preset().id.clone());
        let id = state.next_session_id();
        let anchor = captured.bounds;
        // The panel's keys are taken from this app only, not from a launcher opened over it. Does
        // nothing outside macOS.
        self.host.set_review_app(captured.pid);
        // Before the session lock: watching the window reads it through the platform, which may
        // wait on the app.
        let following = self.host.follow(id, &captured);
        let payload = SessionPayload {
            id,
            text: captured.text.clone(),
            app_name: captured.app_name.clone(),
            can_apply: captured.editable,
            copy_only_hint: captured.copy_only_hint(),
            auto,
            preset_id: preset_id.clone(),
            presets: config.presets.iter().map(|p| PresetInfo { id: p.id.clone(), name: p.name.clone() }).collect(),
            show_timing: config.show_timing,
            always_show_tips: config.always_show_tips,
            provider: state.active_profile().map(|p| p.model),
        };
        let session = Session {
            id,
            capture: captured,
            keyboard: !auto,
            anchor,
            height: INITIAL_HEIGHT,
            task: None,
            run: 0,
            proofread: false,
            following,
        };
        // Dropped once the lock is released: that stops watching its window, through the platform.
        let replaced = state.review.lock().unwrap().begin(session);
        drop(replaced);
        self.shell.emit("overlay", "overlay:session", payload);
        self.shell.position(anchor, INITIAL_HEIGHT);
        self.shell.show(!auto);
        self.shell.set_shortcuts(true);
        Ok((id, preset_id))
    }
}

/// What a review of `captured` with `preset` and `steer` asks the engine: the panel's runs and
/// quick fix both build their job here, so a setting reaches every review the same way.
fn job_for(
    config: &Config,
    profile: Option<ProviderProfile>,
    captured: &Capture,
    preset: Preset,
    steer: Option<String>,
) -> Job {
    Job {
        text: captured.text.clone(),
        preset,
        steer,
        profile,
        style_notes: config.style_notes.clone(),
        dialect: config.dialect.clone(),
        words: config.words.clone(),
        code: captured.protected(),
        local_checks: true,
        builtin_ai: true,
    }
}

/// Starts (or restarts) the model run for a session with a preset and optional steering. When the
/// same selection already has a result for the style, asked the same way, it is shown again as if
/// it had just finished, unless `fresh` asks the AI again (R).
///
/// `run` numbers the run within the session: 0 for the first, which `begin` starts, and the panel
/// numbers each later one before asking for it, so it drops a replaced run's late events however
/// they interleave with the request. A request numbered below the session's latest run is stale and
/// starts nothing.
pub fn run(app: &AppHandle, id: u64, run: u64, preset_id: String, steer: Option<String>, fresh: bool) {
    let state = state(app);
    // Read before the settings and key: a save from here on forgets, so this run's result isn't
    // kept when it may have been asked with settings from before the save.
    let generation = state.last_review.generation();
    let config = state.config();
    let Some(preset) = config.preset(&preset_id).cloned() else { return };
    let profile = state.active_profile();
    let proofread = is_proofread(&preset_id, steer.as_deref());
    let mut review = state.review.lock().unwrap();
    let Some(current) = review.start_run(id, run, proofread) else { return };
    let job = job_for(&config, profile, &current.capture, preset, steer);
    if !fresh && let Some(outcome) = state.last_review.recall(&current.capture, &job, Instant::now()) {
        drop(review);
        platform::trace(format_args!("review: the same selection again, showing the last result"));
        let _ = app.emit_to(
            "overlay",
            "overlay:engine",
            EnginePayload { id, preset_id, run, event: Event::Done(outcome), remembered: true },
        );
        return;
    }
    // Kept with the result, to tell whether a later review is of the same selection, asked the same way.
    let asked = (current.capture.clone(), job.clone());
    let engine = state.engine.clone();
    let handle = app.clone();
    current.task = Some(tauri::async_runtime::spawn(async move {
        engine
            .run(job, &mut |event| {
                if let Event::Done(outcome) = &event {
                    let (capture, job) = asked.clone();
                    let review = Finished::new(capture, job, outcome.clone(), Instant::now());
                    handle.state::<AppState>().last_review.remember(generation, review);
                }
                let _ = handle.emit_to(
                    "overlay",
                    "overlay:engine",
                    EnginePayload { id, preset_id: preset_id.clone(), run, event, remembered: false },
                );
            })
            .await;
    }));
}

/// Whether a run is a plain proofread: the Proofread style with no instruction. Only a proofread is
/// applied in place; any other style rewrites, and replaces the whole selection.
fn is_proofread(preset_id: &str, steer: Option<&str>) -> bool {
    preset_id == "proofread" && steer.is_none_or(|s| s.trim().is_empty())
}

/// Writes the accepted text back into the source app. With `anyway`, the user chose to replace
/// without their clipboard put aside, after a replacement that couldn't (Windows).
pub async fn apply(app: &AppHandle, id: u64, text: String, anyway: bool) -> Result<String, String> {
    let live = Live(app);
    let controller = Controller { state: app.state::<AppState>().inner(), shell: &live, host: &live };
    controller.apply(id, text, anyway).await
}

/// Settings' "Try it now" box asked for a review (⌘E E, or its Review button), as if its text were
/// selected in another app.
pub fn review_playground(app: &AppHandle, text: String, bounds: platform::Rect) {
    let _ = begin(app, platform::playground_capture(text, bounds), false, None);
}

pub fn dismiss(app: &AppHandle, id: Option<u64>) {
    controller(app, |c| c.dismiss(id));
}

/// The user starts (`true`) or stops typing an instruction in the panel, which takes keyboard focus
/// meanwhile.
pub fn typing(app: &AppHandle, typing: bool) {
    controller(app, |c| c.typing(typing));
}

pub fn resize(app: &AppHandle, height: f64) {
    controller(app, |c| c.resize(height));
}

impl<S: Shell, H: Host> Controller<'_, S, H> {
    /// Writes the accepted text back into the source app (see [`apply`]).
    async fn apply(&self, id: u64, text: String, anyway: bool) -> Result<String, String> {
        let state = self.state;
        let keep_formatting = state.config().keep_formatting;
        let applying = {
            let mut review = state.review.lock().unwrap();
            // The panel closes for good; the window may change while the text is written.
            let applying = review.apply(id).ok_or("This review has ended.")?;
            // The text is about to change; a review of it is never shown again.
            state.last_review.forget();
            applying
        };
        // Stops watching the window. On Linux there is no watch, so nothing to drop.
        #[allow(clippy::drop_non_drop)]
        drop(applying.following);
        let (captured, proofread) = (applying.capture, applying.proofread && keep_formatting);
        self.shell.hide();
        self.shell.set_shortcuts(false);
        if captured.playground {
            // Only this review: another may have begun since.
            let ended = state.review.lock().unwrap().end(id);
            drop(ended);
            self.shell.emit("settings", "settings:playground-apply", text);
            return Ok("Applied.".into());
        }
        state.review.lock().unwrap().start_quiet(Instant::now());
        let suggestion = text.clone();
        let result = self.host.write(captured, text, proofread, anyway).await?;
        if let Some(ReplaceOutcome::ClipboardKept(PlatformError::ClipboardUnrestorable(why))) = &result
            && self.offer_replace_anyway(id)
        {
            return Err(why.clone());
        }
        // Only this review: one that began while the text was written stays, with its panel.
        let ended = state.review.lock().unwrap().end(id);
        drop(ended);
        match result {
            None => Ok("No changes needed.".into()),
            Some(outcome) => match self.show_outcome(outcome, suggestion) {
                None => Ok("Replaced.".into()),
                Some(error) => Err(error),
            },
        }
    }

    /// Nothing was pasted because the user's clipboard couldn't be put aside to give back
    /// afterwards. The panel comes back with the review and says so in one line; the apply key
    /// again replaces anyway, and Ctrl+C and Esc work as before. False when the review ended
    /// meanwhile.
    fn offer_replace_anyway(&self, id: u64) -> bool {
        let state = self.state;
        let captured = state.review.lock().unwrap().current(id).map(|s| s.capture.clone());
        let Some(captured) = captured else { return false };
        // Before the review lock, as in `begin`: watching the window reads it through the platform.
        let mut following = self.host.follow(id, &captured);
        // Either way, what's left in `following` is dropped after the lock.
        if !state.review.lock().unwrap().reopen(id, &mut following) {
            return false;
        }
        #[allow(clippy::drop_non_drop)]
        drop(following);
        self.shell.emit("overlay", "overlay:replace-anyway", id);
        self.shell.show(true);
        self.shell.set_shortcuts(true);
        true
    }

    /// Closes the panel: review `id`, or with None whatever is showing. Nothing happens when `id`
    /// is no longer the review showing.
    fn dismiss(&self, id: Option<u64>) {
        let dismissal = self.state.review.lock().unwrap().dismiss(id);
        let Dismissal::Close(ended) = dismissal else { return };
        drop(ended);
        self.shell.hide();
        self.shell.set_shortcuts(false);
    }

    fn typing(&self, typing: bool) {
        if !typing {
            return self.shell.stop_typing();
        }
        let pid = self.state.review.lock().unwrap().reviewed_pid();
        if let Some(pid) = pid {
            self.shell.start_typing(pid);
        }
    }

    fn resize(&self, height: f64) {
        let anchor = self.state.review.lock().unwrap().resize(height);
        self.shell.resize(anchor, height.clamp(90.0, 670.0));
    }
}

/// Quick fix: proofread the selection and replace it directly, with no overlay.
pub async fn quick_fix(app: &AppHandle) {
    let _busy = crate::updater::busy();
    match check_target(app) {
        Target::Allowed => {}
        Target::Ignored => return,
        Target::Excluded(message) => return show_error(app, message),
        Target::NotTrusted => return show_error(app, NOT_TRUSTED.into()),
    }
    warm_connection(app);
    let state = state(app);
    let captured = match capture(app, true, state.config().select_sentence).await {
        Ok(c) => c,
        Err(error) => return show_capture_error(app, error),
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
    let job = job_for(&config, state.active_profile(), &captured, config.default_preset().clone(), None);
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
    state.review.lock().unwrap().start_quiet(Instant::now());
    let text = revised.clone();
    let proofread = config.keep_formatting && is_proofread(&config.default_preset().id, None);
    let result = tauri::async_runtime::spawn_blocking(move || platform::replace(&captured, &text, proofread)).await;
    match result {
        Ok(result) => {
            show_outcome(app, result.into(), revised);
        }
        Err(error) => show_error(app, error.to_string()),
    }
}

/// Another app came to the front (⌘Tab, the Dock): the panel belongs to the app whose text it
/// reviews, so it closes. ⌘Tab never reaches the selection watcher, which only sees the user's
/// typing and clicks.
pub fn on_app_switch(app: &AppHandle, pid: i32) {
    controller(app, |c| c.on_app_switch(pid));
}

/// Whether the window that came to the front belongs to another app than the reviewed one, so
/// closing the panel mustn't take focus back from it.
fn left_for_another_window(reviewing: Option<i32>, front: i32) -> bool {
    reviewing != Some(front)
}

/// The user moved to another Space (a workspace, on Linux). The panel shows on every one, but what
/// it reviews stays on the one the user left, so it would float there attached to nothing: close
/// it, and forget a review about to start or a selection under way.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn on_space_change(app: &AppHandle) {
    controller(app, |c| c.on_space_change());
}

impl<S: Shell, H: Host> Controller<'_, S, H> {
    /// App `pid` came to the front (see [`on_app_switch`]).
    fn on_app_switch(&self, pid: i32) {
        let state = self.state;
        if pid == self.host.own_pid() {
            return;
        }
        // A review about to start, or a Shift selection under way, belongs to the previous app.
        state.watch_generation.fetch_add(1, Ordering::Relaxed);
        self.host.selection_app_switched();
        let (reviewing, notice) = {
            let review = state.review.lock().unwrap();
            (review.reviewed_pid(), review.notice_visible())
        };
        // On Linux the panel has keyboard focus during a review, so any other window coming to the
        // front, the reviewed app's included, means the user has moved on. So on Windows while the
        // panel has focus: the user clicked back into the app.
        let focused = cfg!(target_os = "linux") || self.host.panel_focused();
        let moved_on = reviewing.is_some_and(|source| focused || source != pid);
        if moved_on || (reviewing.is_none() && notice) {
            // A click on the reviewed window may be the start of dragging it by its title bar.
            if reviewing == Some(pid) && self.host.hold_for_click() {
                return;
            }
            // Focus stays with the window the user chose. On Hyprland this is also how a move to
            // another workspace often arrives, before the workspace event itself. (Windows keeps no
            // focus to forget.)
            if left_for_another_window(reviewing, pid) {
                self.shell.forget_focus_return();
            }
            self.dismiss(None);
        }
    }

    /// The user moved to another Space (see [`on_space_change`]).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn on_space_change(&self) {
        let state = self.state;
        state.watch_generation.fetch_add(1, Ordering::Relaxed);
        self.host.selection_app_switched();
        let showing = state.review.lock().unwrap().showing();
        if showing {
            self.shell.forget_focus_return();
            self.dismiss(None);
        }
    }

    /// The selection watcher saw typing or a click in another app.
    fn on_interaction(&self) {
        let state = self.state;
        // A review about to start is for a selection the user is changing.
        state.watch_generation.fetch_add(1, Ordering::Relaxed);
        // Keys the panel takes never reach other apps, so any typing or clicking seen here means the
        // user has moved on.
        // In its own statement: `hold_for_click` takes the review lock.
        let closes = state.review.lock().unwrap().closes_on_interaction(Instant::now());
        // A click on the reviewed window may be the start of dragging it by its title bar.
        if closes && !self.host.hold_for_click() {
            self.dismiss(None);
        }
    }
}

#[cfg(windows)]
fn platform_panel_focused() -> bool {
    platform::overlay::had_focus()
}

#[cfg(not(windows))]
fn platform_panel_focused() -> bool {
    false
}

/// Reacts to activity in other apps: dismiss transient overlays, start automatic reviews.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn on_watch_event(app: &AppHandle, event: WatchEvent) {
    let state = state(app);
    match event {
        WatchEvent::Interaction => controller(app, |c| c.on_interaction()),
        WatchEvent::Wait => {
            state.watch_generation.fetch_add(1, Ordering::Relaxed);
        }
        WatchEvent::MaybeSelection { settle, gesture } => {
            // In this order: ending the gesture may close a panel, which the last check reads. Each
            // lock is read in its own statement, so none is held while another is taken.
            let window_dragged = crate::follow::gesture_ended(app);
            let turned_on = state.config().auto_popup;
            let paused = state.paused.load(Ordering::Relaxed);
            let (just_applied, review_open) = {
                let review = state.review.lock().unwrap();
                (review.is_quiet(Instant::now()), review.keyboard_review_open())
            };
            let gate = AutoGate { window_dragged, turned_on, paused, just_applied, review_open };
            if let Some(outcome) = auto_gate(&gate) {
                return trace_auto(gesture, None, &outcome);
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
                let mut captured = capture(&app, false, false).await;
                // After ⌘A, some apps (Electron ones especially) take a moment to report the new
                // selection through Accessibility. Try again briefly; never fall back to ⌘C.
                if gesture == SelectionGesture::SelectAll {
                    for attempt in 1..=SELECT_ALL_RETRIES {
                        let Err(PlatformError::NoSelection) = captured else { break };
                        trace_auto(gesture, None, &AutoOutcome::Retrying { attempt, of: SELECT_ALL_RETRIES });
                        tokio::time::sleep(SELECT_ALL_RETRY_DELAY).await;
                        if state.watch_generation.load(Ordering::Relaxed) != generation {
                            return trace_auto(gesture, None, &AutoOutcome::Superseded);
                        }
                        captured = capture(&app, false, false).await;
                    }
                }
                let captured = match captured {
                    Ok(captured) => captured,
                    Err(error) => return trace_auto(gesture, None, &AutoOutcome::ReadFailed(error)),
                };
                let app_id = Some(captured.bundle_id.as_str());
                let config = state.config();
                let (min_chars, max_chars) = (config.auto_min_chars, config.auto_max_chars);
                if let Some(outcome) = check_selection(&captured.text, captured.editable, min_chars, max_chars) {
                    return trace_auto(gesture, app_id, &outcome);
                }
                if state.review.lock().unwrap().is_showing_text(&captured.text) {
                    return trace_auto(gesture, app_id, &AutoOutcome::AlreadyShowing);
                }
                if state.watch_generation.load(Ordering::Relaxed) != generation {
                    return trace_auto(gesture, app_id, &AutoOutcome::Superseded);
                }
                let bundle_id = captured.bundle_id.clone();
                let outcome = match begin(&app, captured, true, None) {
                    Ok(()) => AutoOutcome::Shown,
                    Err(refused) => refused,
                };
                trace_auto(gesture, Some(bundle_id.as_str()), &outcome);
            });
        }
    }
}

/// After ⌘A, how many more times an automatic suggestion reads a selection that isn't there yet,
/// and how long it waits before each: about 300 ms in all.
const SELECT_ALL_RETRIES: u32 = 3;
const SELECT_ALL_RETRY_DELAY: Duration = Duration::from_millis(100);

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
    Retrying {
        attempt: u32,
        of: u32,
    },
    NotEditable,
    TooShort {
        chars: usize,
        min: usize,
    },
    /// The selection is more than a few paragraphs, such as a whole document.
    TooLong {
        chars: usize,
        max: usize,
    },
    NoLetter,
    /// The selection is one link, e-mail address, domain or file path, such as a browser's address bar.
    Address,
    AlreadyShowing,
    /// The gesture was a drag of the reviewed window by its title bar.
    WindowDragged,
    /// The selection holds something that looks like a secret (`sensitive::find`).
    Secret,
}

impl std::fmt::Display for AutoOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shown => write!(f, "shown"),
            Self::TurnedOff => write!(f, "skipped: automatic suggestions are turned off"),
            Self::Paused => write!(f, "skipped: automatic suggestions are paused"),
            Self::JustApplied => write!(f, "skipped: a fix was just applied"),
            Self::WindowDragged => write!(f, "skipped: the reviewed window was dragged"),
            Self::ReviewOpen => write!(f, "skipped: a review opened with the shortcut is showing"),
            Self::Superseded => write!(f, "skipped: a newer selection or keystroke replaced this one"),
            Self::OwnWindow => write!(f, "skipped: SuperEditr's own window is in front"),
            Self::Excluded => write!(f, "skipped: the app is excluded in Settings"),
            Self::NotTrusted => write!(f, "skipped: Accessibility permission isn't granted"),
            Self::ReadFailed(error) => write!(f, "skipped: the selection couldn't be read ({error})"),
            Self::Retrying { attempt, of } => write!(f, "no selection yet, reading again ({attempt} of {of})"),
            Self::NotEditable => write!(f, "skipped: the field isn't editable"),
            Self::TooShort { chars, min } => write!(f, "skipped: {chars} characters, under the minimum of {min}"),
            Self::TooLong { chars, max } => write!(f, "skipped: {chars} characters, over the maximum of {max}"),
            Self::NoLetter => write!(f, "skipped: the selection has no letter"),
            Self::Address => write!(f, "skipped: the selection is a single link or address"),
            Self::AlreadyShowing => write!(f, "skipped: the same text is already showing"),
            Self::Secret => write!(f, "skipped: the selection looks like it holds a secret"),
        }
    }
}

/// What an automatic suggestion knows the moment a selection may have been made, before it waits
/// for the selection to settle and reads it.
struct AutoGate {
    /// The gesture was a drag of the reviewed window by its title bar.
    window_dragged: bool,
    /// Automatic suggestions are on in Settings.
    turned_on: bool,
    /// They are paused for now (`AppState::paused`).
    paused: bool,
    /// A fix was written into the app a moment ago, and its own selection changes are still arriving.
    just_applied: bool,
    /// A review opened with the shortcut is showing, which a selection never replaces.
    review_open: bool,
}

/// Whether an automatic suggestion goes on to read the selection; `None` when it does, otherwise
/// the first rule that stops it, for capture.log.
fn auto_gate(gate: &AutoGate) -> Option<AutoOutcome> {
    if gate.window_dragged {
        Some(AutoOutcome::WindowDragged)
    } else if !gate.turned_on {
        Some(AutoOutcome::TurnedOff)
    } else if gate.paused {
        Some(AutoOutcome::Paused)
    } else if gate.just_applied {
        Some(AutoOutcome::JustApplied)
    } else if gate.review_open {
        Some(AutoOutcome::ReviewOpen)
    } else {
        None
    }
}

/// Applies the rules an automatic suggestion's selection must pass; `None` when it may open.
fn check_selection(text: &str, editable: bool, min_chars: usize, max_chars: usize) -> Option<AutoOutcome> {
    let trimmed = text.trim();
    let chars = trimmed.chars().count();
    if !editable {
        Some(AutoOutcome::NotEditable)
    } else if chars < min_chars {
        Some(AutoOutcome::TooShort { chars, min: min_chars })
    } else if chars > max_chars {
        Some(AutoOutcome::TooLong { chars, max: max_chars })
    } else if !trimmed.chars().any(char::is_alphabetic) {
        Some(AutoOutcome::NoLetter)
    } else if is_address(trimmed) {
        Some(AutoOutcome::Address)
    } else {
        None
    }
}

/// True when `text` is one word that names a place rather than saying something: a link
/// (`https://quillpoint.example/a`, `www.quillpoint.example`), an e-mail address, a domain
/// (`quillpoint.example`) or a file path (`~/notes/draft.md`). Writing that merely contains
/// one of these has spaces and is still reviewed.
fn is_address(text: &str) -> bool {
    if text.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = text.to_lowercase();
    let is_link = lower.contains("://") || lower.starts_with("www.") || lower.starts_with("mailto:");
    let is_email = text.find('@').is_some_and(|at| at > 0 && text[at + 1..].contains('.'));
    let is_path = ["/", "~/", "./", "../"].iter().any(|start| text.starts_with(start))
        || (text.contains('/') && text.contains('.'));
    // A dot between two letters or digits, as in a domain; a word that ends a sentence has none.
    let chars: Vec<char> = text.chars().collect();
    let is_domain = chars.windows(3).any(|w| w[1] == '.' && w[0].is_alphanumeric() && w[2].is_alphanumeric());
    is_link || is_email || is_path || is_domain
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

    #[test]
    fn focus_goes_back_only_when_the_reviewed_app_comes_to_the_front() {
        // A window of another app took focus, as on Hyprland when the new workspace's window is
        // reported before the workspace change: closing the panel leaves focus there.
        assert!(left_for_another_window(Some(100), 200));
        // A notice with no review, then another app.
        assert!(left_for_another_window(None, 200));
        // The user clicked back into the reviewed app.
        assert!(!left_for_another_window(Some(100), 100));
    }

    #[test]
    fn the_excluded_message_points_to_the_menu_where_it_can_turn_the_app_on() {
        let mut config = Config::default();
        config.set_excluded("com.apple.Notes", true);
        let tray = if cfg!(windows) { "notification area" } else { "menu bar" };
        assert_eq!(
            excluded_message(&config, "com.apple.Notes", "Notes"),
            format!(
                "SuperEditr is turned off in Notes. Turn it on from the pen icon in the {tray}, or in Settings → Privacy."
            )
        );
        #[cfg(not(any(target_os = "linux", windows)))]
        let terminal = "com.apple.Terminal";
        #[cfg(windows)]
        let terminal = "WindowsTerminal.exe";
        #[cfg(target_os = "linux")]
        let terminal = "foot";
        assert_eq!(
            excluded_message(&config, terminal, "Terminal"),
            "SuperEditr is turned off in Terminal. Change this in Settings → Privacy.",
            "the menu doesn't offer to turn a terminal on"
        );
    }

    fn reason(text: &str, editable: bool, min_chars: usize) -> Option<String> {
        check_selection(text, editable, min_chars, 2_000).map(|outcome| outcome.to_string())
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
        assert_eq!(
            reason("https://quillpoint.example/", true, 12).unwrap(),
            "skipped: the selection is a single link or address"
        );
    }

    #[test]
    fn a_selection_over_the_maximum_is_skipped() {
        let sentence = "Rhyan sent the KPI report. ";
        let document = sentence.repeat(100);
        let chars = document.trim().chars().count();
        assert_eq!(
            reason(&document, true, 12).unwrap(),
            format!("skipped: {chars} characters, over the maximum of 2000")
        );
        // The limit counts characters, not bytes, after trimming, and a selection exactly at it opens.
        let at_the_limit = format!("  {}  ", "é".repeat(2_000));
        assert_eq!(reason(&at_the_limit, true, 12), None);
        assert!(matches!(
            check_selection(&"é".repeat(2_001), true, 12, 2_000),
            Some(AutoOutcome::TooLong { chars: 2_001, max: 2_000 })
        ));
    }

    #[test]
    fn a_single_link_address_or_path_is_skipped() {
        for text in [
            "https://quillpoint.example/",
            "  http://quillpoint.example/reports?q=kpi  ",
            "www.quillpoint.example",
            "quillpoint.example",
            "mailto:kyndal@quillpoint.example",
            "kyndal@quillpoint.example",
            "~/Documents/draft.md",
            "/Users/rhyan/notes",
            "docs/quarterly-okr.md",
        ] {
            assert!(matches!(check_selection(text, true, 12, 2_000), Some(AutoOutcome::Address)), "{text}");
        }
    }

    #[test]
    fn writing_that_mentions_a_link_is_still_reviewed() {
        for text in [
            "See https://quillpoint.example for the KPI report.",
            "Email kyndal@quillpoint.example about the OKRs.",
            "Recieveables",
            "Done. Thanks!",
            "The input/output split looks rihgt.",
        ] {
            assert!(check_selection(text, true, 12, 2_000).is_none(), "{text}");
        }
    }

    #[test]
    fn only_a_replacement_that_wrote_nothing_takes_the_clipboard() {
        assert!(copies_suggestion(&ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)));
        assert!(copies_suggestion(&ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert!(!copies_suggestion(&ReplaceOutcome::Uncertain(PlatformError::Unverified)), "a paste may have landed");
        let kept = ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept("image/png".into()));
        assert!(!copies_suggestion(&kept), "the user's clipboard couldn't be put aside");
        assert!(!copies_suggestion(&ReplaceOutcome::Applied));
    }

    /// A moment when every rule lets the automatic suggestion go on.
    fn open_gate() -> AutoGate {
        AutoGate { window_dragged: false, turned_on: true, paused: false, just_applied: false, review_open: false }
    }

    fn gate_reason(gate: AutoGate) -> Option<String> {
        auto_gate(&gate).map(|outcome| outcome.to_string())
    }

    #[test]
    fn an_automatic_suggestion_goes_on_when_no_rule_stops_it() {
        assert!(auto_gate(&open_gate()).is_none());
    }

    #[test]
    fn an_automatic_suggestion_stops_after_the_reviewed_window_was_dragged() {
        let gate = AutoGate { window_dragged: true, ..open_gate() };
        assert_eq!(gate_reason(gate).unwrap(), "skipped: the reviewed window was dragged");
    }

    #[test]
    fn an_automatic_suggestion_stops_when_they_are_turned_off() {
        let gate = AutoGate { turned_on: false, ..open_gate() };
        assert_eq!(gate_reason(gate).unwrap(), "skipped: automatic suggestions are turned off");
    }

    #[test]
    fn an_automatic_suggestion_stops_while_they_are_paused() {
        let gate = AutoGate { paused: true, ..open_gate() };
        assert_eq!(gate_reason(gate).unwrap(), "skipped: automatic suggestions are paused");
    }

    #[test]
    fn an_automatic_suggestion_stops_right_after_a_fix_was_applied() {
        let gate = AutoGate { just_applied: true, ..open_gate() };
        assert_eq!(gate_reason(gate).unwrap(), "skipped: a fix was just applied");
    }

    #[test]
    fn an_automatic_suggestion_never_replaces_a_review_opened_with_the_shortcut() {
        let gate = AutoGate { review_open: true, ..open_gate() };
        assert_eq!(gate_reason(gate).unwrap(), "skipped: a review opened with the shortcut is showing");
    }

    #[test]
    fn the_first_rule_that_stops_an_automatic_suggestion_is_the_one_logged() {
        let all =
            AutoGate { window_dragged: true, turned_on: false, paused: true, just_applied: true, review_open: true };
        assert!(matches!(auto_gate(&all), Some(AutoOutcome::WindowDragged)));
        let all_but_dragged = AutoGate { window_dragged: false, ..all };
        assert!(matches!(auto_gate(&all_but_dragged), Some(AutoOutcome::TurnedOff)));
        let only_late_rules = AutoGate { turned_on: true, paused: false, ..all_but_dragged };
        assert!(matches!(auto_gate(&only_late_rules), Some(AutoOutcome::JustApplied)));
    }

    fn quillpoint_config() -> Config {
        Config {
            style_notes: "Write for QuillPoint's KPI readers.".into(),
            dialect: "en-GB".into(),
            words: vec!["Brynlee".into(), "Kyndal".into()],
            ..Config::default()
        }
    }

    fn quillpoint_capture() -> Capture {
        let bounds = platform::Rect { x: 10.0, y: 10.0, width: 300.0, height: 20.0 };
        platform::playground_capture("Brynlee and Kyndal sent the OKR updaet.".into(), bounds)
    }

    #[test]
    fn a_job_carries_the_selection_the_style_and_the_writing_settings() {
        let config = quillpoint_config();
        let captured = quillpoint_capture();
        let profile = Some(ProviderProfile::new(ProviderKind::Anthropic));
        let preset = config.preset("proofread").unwrap().clone();
        let job = job_for(&config, profile.clone(), &captured, preset.clone(), Some("Keep it short".into()));
        assert!(job.text == captured.text);
        assert!(job.preset == preset);
        assert_eq!(job.steer.as_deref(), Some("Keep it short"));
        assert!(job.profile == profile);
        assert_eq!(job.style_notes, config.style_notes);
        assert_eq!(job.dialect, config.dialect);
        assert_eq!(job.words, config.words);
        assert_eq!(job.code, captured.protected());
        assert!(job.local_checks && job.builtin_ai);
    }

    #[test]
    fn a_rejection_never_includes_the_selected_text() {
        let line = reason("Brynlee", true, 12).unwrap();
        assert!(!line.contains("Brynlee"), "{line}");
    }
}
