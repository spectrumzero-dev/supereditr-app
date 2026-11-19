//! The review's event handlers, run against a fake shell and host that record each call, and
//! whether the review lock was held when it was made.

use super::*;
use crate::follow::Following;
use crate::platform::Rect;
use serde_json::{Value, json};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Mutex;

/// SuperEditr's own process, as the fake host reports it.
const OWN: i32 = 100;
/// The app whose text is reviewed.
const REVIEWED: i32 = 4242;
/// Another app.
const OTHER: i32 = 5151;

const BOUNDS: Rect = Rect { x: 40.0, y: 120.0, width: 300.0, height: 20.0 };

/// An `AppState` with its settings in a folder of its own, deleted afterwards.
struct TestState {
    state: AppState,
    dir: PathBuf,
}

impl std::ops::Deref for TestState {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.state
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn test_state() -> TestState {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("supereditr-review-{}-{n}", std::process::id()));
    TestState { state: AppState::new(dir.join("settings.json")), dir }
}

/// What the handlers asked of the shell or the host.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Emit { window: String, event: String, payload: Value },
    Show { keys: bool },
    Hide,
    Position { anchor: Option<Rect>, height: f64 },
    Resize { anchor: Option<Rect>, height: f64 },
    Shortcuts(bool),
    StartTyping(i32),
    StopTyping,
    ForgetFocusReturn,
    SelectionAppSwitched,
    HoldForClick,
    Follow(u64),
    SetReviewApp(i32),
    Write { text: String, proofread: bool, anyway: bool },
    Copy(String),
}

use Call::*;

fn emit(window: &str, event: &str, payload: Value) -> Call {
    Emit { window: window.into(), event: event.into(), payload }
}

fn closes() -> Vec<Call> {
    vec![Hide, Shortcuts(false)]
}

/// Something that happens to the review while its text is being written.
type Meanwhile = Box<dyn Fn(&AppState) + Sync>;

/// The panel and the platform, made up: each call is recorded; the host answers as set.
struct Fake<'a> {
    state: &'a AppState,
    /// Each call, and whether the review lock was held while it was made.
    calls: Mutex<Vec<(Call, bool)>>,
    /// The panel had keyboard focus (as on Windows after a click back into the app).
    panel_focused: bool,
    /// A press on the reviewed window holds the close.
    holds_click: bool,
    /// What writing the text into the app comes to; None by default: nothing to write.
    written: Mutex<Option<Result<Option<ReplaceOutcome>, String>>>,
    /// What else happens while the text is being written.
    while_writing: Option<Meanwhile>,
}

impl<'a> Fake<'a> {
    fn new(state: &'a AppState) -> Self {
        Self {
            state,
            calls: Mutex::default(),
            panel_focused: false,
            holds_click: false,
            written: Mutex::default(),
            while_writing: None,
        }
    }

    fn controller(&self) -> Controller<'_, Self, Self> {
        Controller { state: self.state, shell: self, host: self }
    }

    fn record(&self, call: Call) {
        let held = self.state.review.try_lock().is_err();
        self.calls.lock().unwrap().push((call, held));
    }

    /// The calls made so far, and forgets them.
    fn take(&self) -> Vec<Call> {
        self.take_with_lock().into_iter().map(|(call, _)| call).collect()
    }

    fn take_with_lock(&self) -> Vec<(Call, bool)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl Shell for Fake<'_> {
    fn emit<P: Serialize + Clone>(&self, window: &str, event: &str, payload: P) {
        self.record(emit(window, event, serde_json::to_value(payload).unwrap()));
    }
    fn show(&self, keys: bool) {
        self.record(Show { keys });
    }
    fn hide(&self) {
        self.record(Hide);
    }
    fn position(&self, anchor: Option<Rect>, height: f64) {
        self.record(Position { anchor, height });
    }
    fn resize(&self, anchor: Option<Rect>, height: f64) {
        self.record(Resize { anchor, height });
    }
    fn set_shortcuts(&self, active: bool) {
        self.record(Shortcuts(active));
    }
    fn start_typing(&self, pid: i32) {
        self.record(StartTyping(pid));
    }
    fn stop_typing(&self) {
        self.record(StopTyping);
    }
    fn forget_focus_return(&self) {
        self.record(ForgetFocusReturn);
    }
}

impl Host for Fake<'_> {
    fn own_pid(&self) -> i32 {
        OWN
    }
    fn selection_app_switched(&self) {
        self.record(SelectionAppSwitched);
    }
    fn panel_focused(&self) -> bool {
        self.panel_focused
    }
    fn hold_for_click(&self) -> bool {
        self.record(HoldForClick);
        self.holds_click
    }
    fn follow(&self, id: u64, _capture: &Capture) -> Option<Following> {
        self.record(Follow(id));
        None
    }
    fn set_review_app(&self, pid: i32) {
        self.record(SetReviewApp(pid));
    }
    fn write(
        &self,
        _captured: Capture,
        text: String,
        proofread: bool,
        anyway: bool,
    ) -> impl Future<Output = Result<Option<ReplaceOutcome>, String>> + Send {
        self.record(Write { text, proofread, anyway });
        if let Some(meanwhile) = &self.while_writing {
            meanwhile(self.state);
        }
        std::future::ready(self.written.lock().unwrap().take().unwrap_or(Ok(None)))
    }
    fn copy_to_clipboard(&self, text: &str) {
        self.record(Copy(text.into()));
    }
}

/// A selection in another app (`REVIEWED`), or with `playground` in Settings' "Try it now" box.
fn capture(text: &str, playground: bool) -> Capture {
    let mut capture = platform::playground_capture(text.into(), BOUNDS);
    capture.playground = playground;
    // Set here: on Linux the platform finds a playground's place on screen through the desktop.
    capture.bounds = Some(BOUNDS);
    if !playground {
        capture.pid = REVIEWED;
        capture.app_name = "QuillPoint Notes".into();
    }
    capture
}

fn session(id: u64, keyboard: bool, captured: Capture) -> Session {
    Session {
        id,
        capture: captured,
        keyboard,
        anchor: Some(BOUNDS),
        height: INITIAL_HEIGHT,
        task: None,
        run: 0,
        proofread: false,
        following: None,
    }
}

const TEXT: &str = "Brynlee sent the KPI updaet.";
const FIXED: &str = "Brynlee sent the KPI update.";

/// Opens review `id` of a selection in another app; `keyboard` when opened with the shortcut.
fn open(state: &AppState, id: u64, keyboard: bool) {
    drop(state.review.lock().unwrap().begin(session(id, keyboard, capture(TEXT, false))));
}

fn showing(state: &AppState) -> Option<u64> {
    state.review.lock().unwrap().session().map(|s| s.id)
}

fn notice_shown(id: u64, message: &str, copy: Option<&str>, keys: bool) -> Vec<Call> {
    let mut calls = vec![
        emit("overlay", "overlay:status", json!({ "id": id, "kind": "notice", "message": message, "copy": copy })),
        Position { anchor: None, height: 126.0 },
        SetReviewApp(0),
        Show { keys },
    ];
    if cfg!(windows) {
        calls.push(Shortcuts(true));
    }
    calls
}

// Typing or clicking in another app.

#[test]
fn typing_in_another_app_closes_the_panel() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, false);
    fake.controller().on_interaction();
    let calls = fake.take_with_lock();
    assert_eq!(calls.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(), [vec![HoldForClick], closes()].concat());
    assert!(calls.iter().all(|(_, held)| !held), "the review lock is free for each: {calls:?}");
    assert_eq!(showing(&state), None);
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1, "a review about to start is forgotten");
}

#[test]
fn typing_in_another_app_closes_a_notice() {
    let state = test_state();
    let fake = Fake::new(&state);
    drop(state.review.lock().unwrap().show_notice());
    fake.controller().on_interaction();
    assert_eq!(fake.take(), [vec![HoldForClick], closes()].concat());
    assert!(!state.review.lock().unwrap().showing());
}

#[test]
fn typing_right_after_a_fix_leaves_the_panel() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, false);
    state.review.lock().unwrap().start_quiet(Instant::now());
    fake.controller().on_interaction();
    assert_eq!(fake.take(), [], "the fix's own selection changes close nothing");
    assert_eq!(showing(&state), Some(7));
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);
}

#[test]
fn a_press_on_the_reviewed_window_holds_the_close() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.holds_click = true;
    open(&state, 7, false);
    fake.controller().on_interaction();
    assert_eq!(fake.take(), [HoldForClick], "it may be the start of a drag");
    assert_eq!(showing(&state), Some(7));
}

#[test]
fn typing_with_nothing_showing_closes_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().on_interaction();
    assert_eq!(fake.take(), []);
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);
}

// Another app or Space comes to the front.

#[test]
fn switching_to_another_app_closes_the_review_and_leaves_focus_there() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, true);
    fake.controller().on_app_switch(OTHER);
    assert_eq!(fake.take(), [vec![SelectionAppSwitched, ForgetFocusReturn], closes()].concat());
    assert_eq!(showing(&state), None);
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);
}

#[test]
fn supereditr_coming_to_the_front_changes_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, true);
    fake.controller().on_app_switch(OWN);
    assert_eq!(fake.take(), []);
    assert_eq!(showing(&state), Some(7));
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 0);
}

#[test]
fn the_reviewed_app_coming_back_closes_the_review_only_on_linux() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, true);
    fake.controller().on_app_switch(REVIEWED);
    if cfg!(target_os = "linux") {
        // The panel has focus during a review there: the user has moved on. Focus stays where it is.
        assert_eq!(fake.take(), [vec![SelectionAppSwitched, HoldForClick], closes()].concat());
        assert_eq!(showing(&state), None);
    } else {
        assert_eq!(fake.take(), [SelectionAppSwitched]);
        assert_eq!(showing(&state), Some(7));
    }
}

#[test]
fn the_reviewed_app_coming_back_while_the_panel_has_focus_closes_the_review() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.panel_focused = true;
    open(&state, 7, true);
    fake.controller().on_app_switch(REVIEWED);
    assert_eq!(fake.take(), [vec![SelectionAppSwitched, HoldForClick], closes()].concat());
    assert_eq!(showing(&state), None);
}

#[test]
fn a_press_that_brings_the_reviewed_app_back_holds_the_close() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.panel_focused = true;
    fake.holds_click = true;
    open(&state, 7, true);
    fake.controller().on_app_switch(REVIEWED);
    assert_eq!(fake.take(), [SelectionAppSwitched, HoldForClick]);
    assert_eq!(showing(&state), Some(7));
}

#[test]
fn any_app_coming_to_the_front_closes_a_notice() {
    let state = test_state();
    let fake = Fake::new(&state);
    drop(state.review.lock().unwrap().show_notice());
    fake.controller().on_app_switch(REVIEWED);
    assert_eq!(fake.take(), [vec![SelectionAppSwitched, ForgetFocusReturn], closes()].concat());
    assert!(!state.review.lock().unwrap().showing());
}

#[test]
fn an_app_switch_with_nothing_showing_only_forgets_the_selection_under_way() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().on_app_switch(OTHER);
    assert_eq!(fake.take(), [SelectionAppSwitched]);
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_space_change_closes_the_review_and_leaves_focus_where_the_user_went() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, false);
    fake.controller().on_space_change();
    assert_eq!(fake.take(), [vec![SelectionAppSwitched, ForgetFocusReturn], closes()].concat());
    assert_eq!(showing(&state), None);
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);

    fake.controller().on_space_change();
    assert_eq!(fake.take(), [SelectionAppSwitched], "nothing left to close");
}

// Dismissing.

#[test]
fn dismissing_a_review_that_is_no_longer_current_does_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, true);
    open(&state, 8, true);
    fake.controller().dismiss(Some(7));
    assert_eq!(fake.take(), []);
    assert_eq!(showing(&state), Some(8));
}

#[test]
fn dismissing_hides_the_panel_and_turns_its_shortcuts_off_after_the_lock() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 8, true);
    fake.controller().dismiss(Some(8));
    assert_eq!(fake.take_with_lock(), [(Hide, false), (Shortcuts(false), false)]);
    assert_eq!(showing(&state), None);
    fake.controller().dismiss(None);
    assert_eq!(fake.take(), closes(), "closing nothing still hides the panel");
}

// Notices and new reviews.

#[test]
fn a_notice_replaces_the_review() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, false);
    let id = state.next_id.load(Ordering::Relaxed);
    fake.controller().show_error("Select some text first.".into());
    assert_eq!(fake.take(), notice_shown(id, "Select some text first.", None, true));
    assert_eq!(showing(&state), None);
    assert!(state.review.lock().unwrap().notice_visible());
}

#[test]
fn a_notice_while_an_input_method_is_composing_leaves_the_keys_with_the_app() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = state.next_id.load(Ordering::Relaxed);
    fake.controller().show_notice_with("Finish typing first.".into(), None, false);
    assert_eq!(fake.take(), notice_shown(id, "Finish typing first.", None, false));
}

#[test]
fn a_new_review_hides_the_notice_and_opens_the_panel_with_the_keys() {
    let state = test_state();
    let fake = Fake::new(&state);
    drop(state.review.lock().unwrap().show_notice());
    let (id, preset_id) = fake.controller().begin(capture(TEXT, false), false, None).unwrap();
    assert_eq!(preset_id, state.config().default_preset().id);
    let calls = fake.take();
    // The panel's keys come from the reviewed app only, and its window is watched, before the panel
    // gets the review.
    assert_eq!(calls[..2], [SetReviewApp(REVIEWED), Follow(id)]);
    let Emit { window, event, payload } = &calls[2] else { panic!("{calls:?}") };
    assert_eq!((window.as_str(), event.as_str()), ("overlay", "overlay:session"));
    assert_eq!(payload["id"], json!(id));
    assert_eq!(payload["text"], json!(TEXT));
    assert_eq!(payload["auto"], json!(false));
    assert_eq!(
        calls[3..],
        [Position { anchor: Some(BOUNDS), height: INITIAL_HEIGHT }, Show { keys: true }, Shortcuts(true)]
    );
    let review = state.review.lock().unwrap();
    assert!(!review.notice_visible());
    assert_eq!(review.session().map(|s| s.id), Some(id));
    assert!(review.keyboard_review_open());
}

#[test]
fn an_automatic_review_opens_the_panel_without_the_keys() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().begin(capture(TEXT, false), true, None).unwrap();
    let calls = fake.take();
    assert!(calls.contains(&Show { keys: false }), "{calls:?}");
    assert!(!state.review.lock().unwrap().keyboard_review_open());
}

#[test]
fn an_empty_selection_says_so_only_when_asked() {
    let state = test_state();
    let fake = Fake::new(&state);
    let result = fake.controller().begin(capture("  ", false), true, None);
    assert!(matches!(result, Err(AutoOutcome::ReadFailed(PlatformError::NoSelection))));
    assert_eq!(fake.take(), [], "an automatic suggestion stays silent");

    let id = state.next_id.load(Ordering::Relaxed);
    assert!(fake.controller().begin(capture("  ", false), false, None).is_err());
    assert_eq!(fake.take(), notice_shown(id, &PlatformError::NoSelection.to_string(), None, true));
    assert_eq!(showing(&state), None);
}

#[test]
fn a_selection_in_supereditrs_own_window_opens_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    let mut own = capture(TEXT, false);
    own.pid = OWN;
    assert!(matches!(fake.controller().begin(own, false, None), Err(AutoOutcome::OwnWindow)));
    assert_eq!(fake.take(), []);
}

// The review shortcut.

#[test]
fn the_shortcut_gives_the_keys_to_an_automatic_review_under_the_lock() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, false);
    assert!(fake.controller().keys_to_automatic_review(), "no new review starts");
    // Under the lock, so a dismissal can't put its hide ahead of this show.
    assert_eq!(
        fake.take_with_lock(),
        [(Show { keys: true }, true), (emit("overlay", "overlay:focus", json!(7)), true)]
    );
    assert!(state.review.lock().unwrap().keyboard_review_open());
}

#[test]
fn the_shortcut_starts_a_new_review_over_one_that_has_the_keys_or_none() {
    let state = test_state();
    let fake = Fake::new(&state);
    assert!(!fake.controller().keys_to_automatic_review(), "nothing is showing");
    open(&state, 7, true);
    assert!(!fake.controller().keys_to_automatic_review());
    assert_eq!(fake.take(), []);
}

// Applying.

fn block_on<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

fn written() -> Call {
    Write { text: FIXED.into(), proofread: false, anyway: false }
}

#[test]
fn applying_in_try_it_now_hands_the_text_to_settings() {
    let state = test_state();
    let fake = Fake::new(&state);
    drop(state.review.lock().unwrap().begin(session(7, true, capture(TEXT, true))));
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap(), "Applied.");
    assert_eq!(fake.take(), [closes(), vec![emit("settings", "settings:playground-apply", json!(FIXED))]].concat());
    assert_eq!(showing(&state), None);
    assert!(!state.review.lock().unwrap().is_quiet(Instant::now()), "nothing was written into another app");
}

#[test]
fn applying_a_review_that_has_ended_does_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap_err(), "This review has ended.");
    assert_eq!(fake.take(), []);
}

#[test]
fn a_replacement_ends_the_review_and_starts_the_quiet_period() {
    let state = test_state();
    let fake = Fake::new(&state);
    *fake.written.lock().unwrap() = Some(Ok(Some(ReplaceOutcome::Applied)));
    open(&state, 7, true);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap(), "Replaced.");
    assert_eq!(fake.take(), [closes(), vec![written()]].concat());
    assert_eq!(showing(&state), None);
    assert!(state.review.lock().unwrap().is_quiet(Instant::now()));
}

#[test]
fn applying_unchanged_text_needs_no_changes() {
    let state = test_state();
    let fake = Fake::new(&state);
    open(&state, 7, true);
    assert_eq!(block_on(fake.controller().apply(7, TEXT.into(), false)).unwrap(), "No changes needed.");
    assert_eq!(showing(&state), None);
}

#[test]
fn a_replacement_that_couldnt_put_the_clipboard_aside_offers_to_replace_anyway() {
    let state = test_state();
    let fake = Fake::new(&state);
    let why = "Kyndal's clipboard holds a file that can't be put back.";
    let unrestorable = ReplaceOutcome::ClipboardKept(PlatformError::ClipboardUnrestorable(why.into()));
    *fake.written.lock().unwrap() = Some(Ok(Some(unrestorable)));
    open(&state, 7, false);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap_err(), why);
    assert_eq!(
        fake.take(),
        [
            closes(),
            vec![
                written(),
                Follow(7),
                emit("overlay", "overlay:replace-anyway", json!(7)),
                Show { keys: true },
                Shortcuts(true)
            ]
        ]
        .concat()
    );
    let review = state.review.lock().unwrap();
    assert_eq!(review.session().map(|s| s.id), Some(7), "the review comes back");
    assert!(review.keyboard_review_open(), "with the keys");
}

#[test]
fn a_review_that_ended_while_its_text_was_written_says_so_in_a_notice() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    let why = "Kyndal's clipboard holds a file that can't be put back.";
    let error = PlatformError::ClipboardUnrestorable(why.into());
    let message = error.to_string();
    *fake.written.lock().unwrap() = Some(Ok(Some(ReplaceOutcome::ClipboardKept(error))));
    fake.while_writing = Some(Box::new(|state| drop(state.review.lock().unwrap().dismiss(None))));
    open(&state, 7, true);
    let id = state.next_id.load(Ordering::Relaxed);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap_err(), message);
    assert_eq!(
        fake.take(),
        [closes(), vec![written()], notice_shown(id, &message, Some(FIXED), true)].concat(),
        "nothing to offer again; the notice offers the fix to copy, and leaves the clipboard alone"
    );
}

#[test]
fn a_review_that_begins_while_text_is_written_stays_open() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    *fake.written.lock().unwrap() = Some(Ok(Some(ReplaceOutcome::Applied)));
    fake.while_writing = Some(Box::new(|state| open(state, 8, true)));
    open(&state, 7, true);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap(), "Replaced.");
    assert_eq!(fake.take(), [closes(), vec![written()]].concat(), "the new review's panel stays up");
    assert_eq!(showing(&state), Some(8));
}

#[test]
fn a_failed_write_still_says_so_over_a_review_that_began_meanwhile() {
    // The user must learn that the write failed, so its notice takes the panel, as any notice does.
    let state = test_state();
    let mut fake = Fake::new(&state);
    let message = PlatformError::FocusNotReturned.to_string();
    *fake.written.lock().unwrap() = Some(Ok(Some(ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned))));
    fake.while_writing = Some(Box::new(|state| open(state, 8, true)));
    open(&state, 7, true);
    let id = state.next_id.load(Ordering::Relaxed);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap_err(), message);
    assert_eq!(
        fake.take(),
        [closes(), vec![written(), Copy(FIXED.into())], notice_shown(id, &message, None, true)].concat()
    );
    assert_eq!(showing(&state), None);
    assert!(state.review.lock().unwrap().notice_visible());
}

#[test]
fn a_replacement_that_wrote_nothing_copies_the_fix_and_says_so() {
    let state = test_state();
    let fake = Fake::new(&state);
    let message = PlatformError::FocusNotReturned.to_string();
    *fake.written.lock().unwrap() = Some(Ok(Some(ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned))));
    open(&state, 7, true);
    let id = state.next_id.load(Ordering::Relaxed);
    assert_eq!(block_on(fake.controller().apply(7, FIXED.into(), false)).unwrap_err(), message);
    assert_eq!(
        fake.take(),
        [closes(), vec![written(), Copy(FIXED.into())], notice_shown(id, &message, None, true)].concat()
    );
}

// Typing an instruction, and the panel's size.

#[test]
fn typing_an_instruction_takes_focus_for_the_reviewed_app() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().typing(true);
    assert_eq!(fake.take(), [], "nothing is showing");
    open(&state, 7, true);
    fake.controller().typing(true);
    fake.controller().typing(false);
    assert_eq!(fake.take(), [StartTyping(REVIEWED), StopTyping]);
}

#[test]
fn resizing_keeps_the_panel_by_the_selection_within_its_limits() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().resize(300.0);
    open(&state, 7, true);
    fake.controller().resize(1_000.0);
    fake.controller().resize(10.0);
    assert_eq!(
        fake.take(),
        [
            Resize { anchor: None, height: 300.0 },
            Resize { anchor: Some(BOUNDS), height: 670.0 },
            Resize { anchor: Some(BOUNDS), height: 90.0 }
        ]
    );
    assert_eq!(state.review.lock().unwrap().session().unwrap().height, 10.0, "the content's own height is kept");
}
