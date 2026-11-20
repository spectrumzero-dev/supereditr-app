//! A review's runs, the review and quick fix shortcuts, and automatic reviews, against the fake in
//! `tests_handlers`. Engine events are sent by hand, one at a time, to the run the fake engine
//! started.

use super::tests_handlers::*;
use super::*;
use crate::state::QUIET;
use serde_json::json;
use std::time::Instant;
use supereditr_core::Outcome;

/// How long the selection watcher asks an automatic review to wait, here.
const SETTLE: Duration = Duration::from_millis(250);

/// What the AI answered for `original`: `revised`.
fn outcome(original: &str, revised: &str) -> Outcome {
    Outcome {
        view: supereditr_core::diff::diff(original, revised),
        text: revised.into(),
        source: "claude-sonnet-5".into(),
        first_token_ms: Some(900),
        total_ms: 2400,
        changed_tokens: Vec::new(),
        grade_before: None,
        grade_after: None,
        backup_for: None,
        kept: Vec::new(),
        kept_terms: Vec::new(),
        learnable: Vec::new(),
    }
}

fn done() -> Event {
    Event::Done(outcome(TEXT, FIXED))
}

fn proofread() -> String {
    "proofread".into()
}

/// The panel's engine event for run `run` of review `id`.
fn engine(id: u64, run: u64, event: &Event, remembered: bool) -> Call {
    let event = serde_json::to_value(event).unwrap();
    emit(
        "overlay",
        "overlay:engine",
        json!({ "id": id, "presetId": "proofread", "run": run, "event": event, "remembered": remembered }),
    )
}

fn started(id: u64, run: u64) -> Call {
    StartRun { id, run, text: TEXT.into(), preset_id: proofread(), steer: None }
}

/// Sends `event` from the `index`th run the engine started.
fn send(fake: &Fake, index: usize, event: Event) {
    let runs = fake.runs.lock().unwrap();
    fake.controller().engine_event(&runs[index], event);
}

/// Opens a review of `TEXT` with the shortcut, which starts its first run. Returns its number and
/// forgets the calls.
fn opened(fake: &Fake) -> u64 {
    fake.controller().open(capture(TEXT, false), false, None).unwrap();
    fake.take();
    showing(fake.state).unwrap()
}

fn traces(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|call| match call {
            Trace(line) => Some(line.clone()),
            _ => None,
        })
        .collect()
}

fn select(fake: &Fake, captured: Result<Capture, PlatformError>) {
    fake.captures.lock().unwrap().push_back(captured);
}

/// Turns automatic suggestions on, as they are by default on macOS.
fn turned_on(state: &AppState) {
    state.config.write().unwrap().auto_popup = true;
}

/// A selection made with `gesture`, from the watcher's event to the end of the automatic review.
fn select_with(fake: &Fake, gesture: SelectionGesture) -> Vec<Call> {
    if let Some(generation) = fake.controller().maybe_selection(gesture) {
        block_on(fake.controller().automatic_review(SETTLE, gesture, generation));
    }
    fake.take()
}

// A review's runs.

#[test]
fn opening_a_review_starts_its_first_run_under_the_lock() {
    let state = test_state();
    let fake = Fake::new(&state);
    fake.controller().open(capture(TEXT, false), false, None).unwrap();
    let id = showing(&state).unwrap();
    let calls = fake.take_with_lock();
    assert_eq!(calls.last(), Some(&(started(id, 0), true)), "a dismissal can't come between: {calls:?}");
    assert!(calls[..calls.len() - 1].iter().all(|(call, _)| !matches!(call, StartRun { .. })));
}

#[test]
fn each_engine_event_reaches_the_panel_with_its_review_and_run() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    let local = Event::Local { view: supereditr_core::diff::diff(TEXT, FIXED), ms: 4 };
    send(&fake, 0, local.clone());
    send(&fake, 0, done());
    assert_eq!(fake.take_with_lock(), [(engine(id, 0, &local, false), false), (engine(id, 0, &done(), false), false)]);
}

#[test]
fn a_finished_review_of_the_same_selection_is_shown_again_without_the_ai() {
    let state = test_state();
    let fake = Fake::new(&state);
    opened(&fake);
    send(&fake, 0, done());
    fake.controller().dismiss(None);
    fake.take();

    let id = opened_again(&fake);
    let calls = fake.take_with_lock();
    let n = calls.len();
    assert_eq!(
        calls[n - 2..],
        [
            (Trace("review: the same selection again, showing the last result".into()), false),
            (engine(id, 0, &done(), true), false)
        ]
    );
    assert!(!calls.iter().any(|(call, _)| matches!(call, StartRun { .. })), "no AI runs: {calls:?}");
}

/// Opens a review of `TEXT` again, leaving its calls to the test. Returns its number.
fn opened_again(fake: &Fake) -> u64 {
    fake.controller().open(capture(TEXT, false), false, None).unwrap();
    showing(fake.state).unwrap()
}

#[test]
fn asking_again_runs_the_ai_even_with_a_result_kept() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    send(&fake, 0, done());
    fake.take();
    fake.controller().run(id, 1, proofread(), None, true);
    assert_eq!(fake.take(), [started(id, 1)]);
}

#[test]
fn a_failed_run_says_why_in_the_panel_and_keeps_nothing_to_show_again() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    let failed = Event::Failed { message: "QuillPoint's AI didn't answer.".into(), local_only: false };
    send(&fake, 0, failed.clone());
    assert_eq!(fake.take(), [engine(id, 0, &failed, false)]);
    fake.controller().run(id, 1, proofread(), None, false);
    assert_eq!(fake.take(), [started(id, 1)], "the AI is asked again");
}

#[test]
fn a_result_that_arrives_after_the_settings_were_saved_is_not_kept() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    state.save_config(state.config()).unwrap();
    send(&fake, 0, done());
    fake.take();
    fake.controller().run(id, 1, proofread(), None, false);
    assert_eq!(fake.take(), [started(id, 1)], "it may have been asked with the settings from before");
}

#[test]
fn a_newer_run_cancels_the_one_before() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().run(id, 1, "polish".into(), Some("Keep it short".into()), false);
    assert_eq!(
        fake.take_with_lock(),
        [(
            StartRun { id, run: 1, text: TEXT.into(), preset_id: "polish".into(), steer: Some("Keep it short".into()) },
            true
        )]
    );
    assert_eq!(*fake.cancelled.lock().unwrap(), [(id, 0)]);
    assert!(!state.review.lock().unwrap().session().unwrap().proofread, "a rewrite replaces the whole selection");
}

#[test]
fn a_plain_proofread_run_is_applied_in_place() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().run(id, 1, proofread(), Some("  ".into()), true);
    assert!(state.review.lock().unwrap().session().unwrap().proofread, "a blank instruction is none");
}

#[test]
fn a_run_asked_for_before_a_later_one_starts_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().run(id, 2, proofread(), None, true);
    fake.take();
    fake.controller().run(id, 1, proofread(), None, true);
    assert_eq!(fake.take(), []);
    assert_eq!(*fake.cancelled.lock().unwrap(), [(id, 0)], "run 2 goes on");
}

#[test]
fn a_run_of_a_style_that_no_longer_exists_starts_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().run(id, 1, "deleted-style".into(), None, true);
    assert_eq!(fake.take(), []);
    assert!(fake.cancelled.lock().unwrap().is_empty(), "the first run goes on");
}

#[test]
fn dismissing_a_review_cancels_its_run_and_a_later_run_starts_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().dismiss(Some(id));
    assert_eq!(*fake.cancelled.lock().unwrap(), [(id, 0)]);
    fake.take();
    fake.controller().run(id, 1, proofread(), None, true);
    assert_eq!(fake.take(), []);
}

#[test]
fn an_event_from_a_replaced_review_never_reaches_the_panel() {
    let state = test_state();
    let fake = Fake::new(&state);
    let first = opened(&fake);
    let second = opened(&fake);
    assert_eq!(*fake.cancelled.lock().unwrap(), [(first, 0)]);
    send(&fake, 0, done());
    assert_eq!(fake.take(), []);
    assert_eq!(showing(&state), Some(second));
}

#[test]
fn an_event_from_a_dismissed_review_never_reaches_the_panel() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().dismiss(Some(id));
    fake.take();
    // The cancelled run's task was already handing the engine's event on.
    send(&fake, 0, done());
    assert_eq!(fake.take(), []);
}

#[test]
fn an_event_from_a_replaced_run_never_reaches_the_panel() {
    let state = test_state();
    let fake = Fake::new(&state);
    let id = opened(&fake);
    fake.controller().run(id, 1, proofread(), None, true);
    fake.take();
    send(&fake, 0, done());
    assert_eq!(fake.take(), []);
    send(&fake, 1, done());
    assert_eq!(fake.take(), [engine(id, 1, &done(), false)]);
}

// The review shortcut.

#[test]
fn the_shortcut_reads_the_selection_and_opens_a_review_of_it() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Ok(capture(TEXT, false)));
    block_on(fake.controller().start_manual(Some("polish".into())));
    let calls = fake.take();
    assert_eq!(
        calls[..3],
        [WarmConnection, ReadSelection { allow_clipboard: true, select_sentence: true }, SetReviewApp(REVIEWED)]
    );
    let id = showing(&state).unwrap();
    assert_eq!(
        calls.last(),
        Some(&StartRun { id, run: 0, text: TEXT.into(), preset_id: "polish".into(), steer: None })
    );
    assert!(state.review.lock().unwrap().keyboard_review_open());
}

#[test]
fn the_shortcut_in_supereditrs_settings_asks_try_it_now_to_review() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.settings_focused = true;
    block_on(fake.controller().start_manual(None));
    assert_eq!(fake.take(), [emit("settings", "settings:review-shortcut", json!(null))], "nothing is read");
}

#[test]
fn the_shortcut_in_an_excluded_app_says_how_to_turn_it_on_and_reads_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    state.config.write().unwrap().set_excluded(REVIEWED_APP, true);
    let message = excluded_message(&state.config(), REVIEWED_APP, "QuillPoint Notes");
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().start_manual(None));
    assert_eq!(fake.take(), notice_shown(id, &message, None, true));
}

#[cfg(not(any(target_os = "linux", windows)))]
const PASSWORD_MANAGER: &str = "com.1password.1password";
#[cfg(windows)]
const PASSWORD_MANAGER: &str = "1Password.exe";
#[cfg(target_os = "linux")]
const PASSWORD_MANAGER: &str = "1Password";

#[test]
fn the_shortcut_in_a_password_manager_taken_off_the_excluded_apps_still_reads_nothing() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.front = Some((REVIEWED, PASSWORD_MANAGER.into(), "1Password".into()));
    state.config.write().unwrap().set_excluded(PASSWORD_MANAGER, false);
    let message = excluded_message(&state.config(), PASSWORD_MANAGER, "1Password");
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().start_manual(None));
    assert_eq!(fake.take(), notice_shown(id, &message, None, true), "no connection warmed, nothing read");
}

#[test]
fn a_selection_in_a_password_manager_taken_off_the_excluded_apps_opens_no_review() {
    let state = test_state();
    let fake = Fake::new(&state);
    state.config.write().unwrap().set_excluded(PASSWORD_MANAGER, false);
    let mut captured = capture(TEXT, false);
    captured.bundle_id = PASSWORD_MANAGER.into();
    captured.app_name = "1Password".into();
    let message = excluded_message(&state.config(), PASSWORD_MANAGER, "1Password");
    let id = state.next_id.load(Ordering::Relaxed);
    assert!(matches!(fake.controller().open(captured, false, None), Err(AutoOutcome::Excluded)));
    assert_eq!(fake.take(), notice_shown(id, &message, None, true));
}

#[test]
fn the_shortcut_without_accessibility_says_where_to_allow_it() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.trusted = false;
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().start_manual(None));
    assert_eq!(fake.take(), notice_shown(id, NOT_TRUSTED, None, true));
}

#[test]
fn a_selection_that_cant_be_read_while_composing_leaves_the_keys_with_the_app() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Err(PlatformError::Composing));
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().start_manual(None));
    let message = PlatformError::Composing.to_string();
    let expected = [
        vec![WarmConnection, ReadSelection { allow_clipboard: true, select_sentence: true }],
        notice_shown(id, &message, None, false),
    ];
    assert_eq!(fake.take(), expected.concat());
}

// Quick fix.

#[test]
fn quick_fix_writes_the_fix_in_place_and_starts_the_quiet_period() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Ok(capture(TEXT, false)));
    *fake.events.lock().unwrap() = vec![done()];
    block_on(fake.controller().quick_fix());
    assert_eq!(
        fake.take(),
        [
            WarmConnection,
            ReadSelection { allow_clipboard: true, select_sentence: true },
            RunToEnd(TEXT.into()),
            Replace { text: FIXED.into(), proofread: true }
        ]
    );
    assert!(state.review.lock().unwrap().is_quiet(Instant::now()));
}

#[test]
fn quick_fix_with_nothing_to_fix_writes_nothing() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Ok(capture(TEXT, false)));
    *fake.events.lock().unwrap() = vec![Event::Done(outcome(TEXT, TEXT))];
    block_on(fake.controller().quick_fix());
    assert_eq!(fake.take().last(), Some(&RunToEnd(TEXT.into())));
    assert!(!state.review.lock().unwrap().is_quiet(Instant::now()));
}

#[test]
fn quick_fix_says_why_the_engine_failed() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Ok(capture(TEXT, false)));
    let message = "QuillPoint's AI didn't answer.";
    *fake.events.lock().unwrap() = vec![Event::Failed { message: message.into(), local_only: false }];
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().quick_fix());
    assert_eq!(fake.take()[3..], notice_shown(id, message, None, true));
}

#[test]
fn quick_fix_in_text_that_cant_be_edited_points_to_the_review_shortcut() {
    let state = test_state();
    let fake = Fake::new(&state);
    let mut read_only = capture(TEXT, false);
    read_only.editable = false;
    select(&fake, Ok(read_only));
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().quick_fix());
    let message = "Quick fix works in text you’re editing. Use your review shortcut to review this text instead.";
    assert_eq!(fake.take()[2..], notice_shown(id, message, None, true), "the engine never runs");
}

#[test]
fn quick_fix_in_supereditrs_own_window_does_nothing() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    fake.front = Some((OWN, String::new(), "SuperEditr".into()));
    block_on(fake.controller().quick_fix());
    assert_eq!(fake.take(), []);
}

#[test]
fn quick_fix_that_couldnt_write_copies_the_fix_and_says_so() {
    let state = test_state();
    let fake = Fake::new(&state);
    select(&fake, Ok(capture(TEXT, false)));
    *fake.events.lock().unwrap() = vec![done()];
    *fake.replaced.lock().unwrap() = Some(Ok(ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)));
    let id = state.next_id.load(Ordering::Relaxed);
    block_on(fake.controller().quick_fix());
    let message = PlatformError::CopyOnly.to_string();
    assert_eq!(fake.take()[4..], [vec![Copy(FIXED.into())], notice_shown(id, &message, None, true)].concat());
}

// Automatic reviews.

#[test]
fn a_settled_selection_that_passes_every_rule_opens_an_automatic_review() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    select(&fake, Ok(capture(TEXT, false)));
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        calls[..4],
        [GestureEnded, WarmConnection, Sleep(SETTLE), ReadSelection { allow_clipboard: false, select_sentence: false }]
    );
    let id = showing(&state).unwrap();
    assert_eq!(
        calls[calls.len() - 2..],
        [started(id, 0), Trace(format!("automatic {REVIEWED_APP}: after drag, shown"))]
    );
    assert!(!state.review.lock().unwrap().keyboard_review_open(), "the app keeps the keys");
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 1);
}

#[test]
fn a_rule_that_stops_an_automatic_review_is_logged_before_any_wait() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    state.paused.store(true, Ordering::Relaxed);
    let calls = select_with(&fake, SelectionGesture::DoubleClick);
    assert_eq!(
        calls,
        [GestureEnded, Trace("automatic: after double-click, skipped: automatic suggestions are paused".into())]
    );
    assert_eq!(state.watch_generation.load(Ordering::Relaxed), 0, "a review about to start goes on");
}

#[test]
fn a_drag_of_the_reviewed_window_starts_no_automatic_review() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    turned_on(&state);
    fake.window_dragged = true;
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(traces(&calls), ["automatic: after drag, skipped: the reviewed window was dragged"]);
}

#[test]
fn an_automatic_review_waits_out_the_quiet_period_after_a_fix() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    state.review.lock().unwrap().start_quiet(fake.controller().host.now());
    assert_eq!(fake.controller().maybe_selection(SelectionGesture::Shift), None);
    assert_eq!(traces(&fake.take()), ["automatic: after Shift selection, skipped: a fix was just applied"]);
    *fake.ahead.lock().unwrap() = QUIET;
    assert_eq!(fake.controller().maybe_selection(SelectionGesture::Shift), Some(1));
}

#[test]
fn a_newer_event_while_the_selection_settles_cancels_the_automatic_review() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    turned_on(&state);
    fake.while_sleeping = Some(Box::new(|state| {
        state.watch_generation.fetch_add(1, Ordering::Relaxed);
    }));
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        calls[2..],
        [
            Sleep(SETTLE),
            Trace("automatic: after drag, skipped: a newer selection or keystroke replaced this one".into())
        ],
        "nothing is read"
    );
}

#[test]
fn the_selection_watcher_asking_to_wait_cancels_an_automatic_review_about_to_start() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    let generation = fake.controller().maybe_selection(SelectionGesture::Drag).unwrap();
    fake.controller().on_wait();
    block_on(fake.controller().automatic_review(SETTLE, SelectionGesture::Drag, generation));
    assert_eq!(
        traces(&fake.take()),
        ["automatic: after drag, skipped: a newer selection or keystroke replaced this one"]
    );
}

#[test]
fn an_automatic_review_reads_nothing_in_an_excluded_app_and_shows_no_notice() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    state.config.write().unwrap().set_excluded(REVIEWED_APP, true);
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        calls[2..],
        [Sleep(SETTLE), Trace("automatic: after drag, skipped: the app is excluded in Settings".into())]
    );
}

#[test]
fn an_automatic_review_reads_nothing_in_supereditrs_own_window_or_without_accessibility() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    turned_on(&state);
    fake.front = Some((OWN, String::new(), "SuperEditr".into()));
    let own = select_with(&fake, SelectionGesture::Drag);
    fake.trusted = false;
    let untrusted = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        [traces(&own), traces(&untrusted)].concat(),
        [
            "automatic: after drag, skipped: SuperEditr's own window is in front",
            "automatic: after drag, skipped: Accessibility permission isn't granted"
        ]
    );
    assert!(![own, untrusted].concat().iter().any(|call| matches!(call, ReadSelection { .. })));
}

#[test]
fn after_select_all_a_selection_not_there_yet_is_read_again_briefly() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    select(&fake, Err(PlatformError::NoSelection));
    select(&fake, Err(PlatformError::NoSelection));
    select(&fake, Ok(capture(TEXT, false)));
    let calls = select_with(&fake, SelectionGesture::SelectAll);
    let read = ReadSelection { allow_clipboard: false, select_sentence: false };
    let retry = |attempt: u32| Trace(format!("automatic: after ⌘A, no selection yet, reading again ({attempt} of 3)"));
    assert_eq!(
        calls[2..9],
        [
            Sleep(SETTLE),
            read.clone(),
            retry(1),
            Sleep(SELECT_ALL_RETRY_DELAY),
            read.clone(),
            retry(2),
            Sleep(SELECT_ALL_RETRY_DELAY)
        ]
    );
    assert_eq!(calls[9], read);
    assert_eq!(calls.last(), Some(&Trace(format!("automatic {REVIEWED_APP}: after ⌘A, shown"))));
}

#[test]
fn after_select_all_an_automatic_review_gives_up_after_three_more_reads() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    let calls = select_with(&fake, SelectionGesture::SelectAll);
    assert_eq!(calls.iter().filter(|call| matches!(call, ReadSelection { .. })).count(), 4);
    assert_eq!(
        traces(&calls).last().unwrap(),
        &format!("automatic: after ⌘A, skipped: the selection couldn't be read ({})", PlatformError::NoSelection)
    );
}

#[test]
fn a_drag_that_selected_nothing_is_read_only_once() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(calls.iter().filter(|call| matches!(call, ReadSelection { .. })).count(), 1);
    assert_eq!(
        traces(&calls),
        [format!("automatic: after drag, skipped: the selection couldn't be read ({})", PlatformError::NoSelection)]
    );
}

#[test]
fn a_selection_under_the_minimum_is_skipped_and_logged_with_its_app() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    select(&fake, Ok(capture("  Glenmoor  ", false)));
    let calls = select_with(&fake, SelectionGesture::DoubleClick);
    assert_eq!(
        traces(&calls),
        [format!("automatic {REVIEWED_APP}: after double-click, skipped: 8 characters, under the minimum of 12")]
    );
    assert_eq!(showing(&state), None);
}

#[test]
fn the_text_already_showing_opens_no_second_automatic_review() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    open(&state, 7, false);
    select(&fake, Ok(capture(TEXT, false)));
    let generation = fake.controller().maybe_selection(SelectionGesture::Drag).unwrap();
    block_on(fake.controller().automatic_review(SETTLE, SelectionGesture::Drag, generation));
    // Logged once the lock is released.
    let calls = fake.take_with_lock();
    assert_eq!(
        calls.last(),
        Some(&(
            Trace(format!("automatic {REVIEWED_APP}: after drag, skipped: the same text is already showing")),
            false
        ))
    );
    assert_eq!(showing(&state), Some(7));
}

#[test]
fn a_keystroke_while_the_selection_is_read_cancels_the_automatic_review() {
    let state = test_state();
    let mut fake = Fake::new(&state);
    turned_on(&state);
    select(&fake, Ok(capture(TEXT, false)));
    fake.while_reading = Some(Box::new(|state| {
        state.watch_generation.fetch_add(1, Ordering::Relaxed);
    }));
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        traces(&calls),
        [format!("automatic {REVIEWED_APP}: after drag, skipped: a newer selection or keystroke replaced this one")]
    );
    assert_eq!(showing(&state), None);
}

#[test]
fn an_automatic_review_of_a_secret_stays_silent() {
    let state = test_state();
    let fake = Fake::new(&state);
    turned_on(&state);
    // Assembled here, so no complete secret-shaped line appears in the source.
    let secret = format!("Kyndal's key: {}PRIVATE KEY BLOCK-----", "-----BEGIN PGP ");
    select(&fake, Ok(capture(&secret, false)));
    let calls = select_with(&fake, SelectionGesture::Drag);
    assert_eq!(
        calls[4..],
        [Trace(format!("automatic {REVIEWED_APP}: after drag, skipped: the selection looks like it holds a secret"))],
        "no notice, no panel"
    );
}
