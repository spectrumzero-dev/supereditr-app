//! The replace chain: puts a fix where the selection was, or says why it didn't.
//!
//! 1. **Check**: only a known-safe capture is written, in the same window and process, still not
//!    running as administrator.
//! 2. **Focus handshake**: the window comes back to the front and its focused element compares
//!    equal to the captured one, within about 250 ms, or nothing is written.
//! 3. **Range check**: the selection's ends still equal the captured range (`CompareEndpoints`), with
//!    the same text and the same text around it. Any change is stale: the user moved on, or two
//!    identical sentences could be confused. SuperEditr never searches for the text elsewhere.
//! 4. **Paste**, as one clipboard transaction: snapshot (or keep the clipboard and stop), check
//!    again, write the fix while the clipboard is still the snapshot's, release the user's
//!    modifiers, check once more (the window, the selection, and that the clipboard still holds the
//!    fix) and send Ctrl+V. A copy the user makes meanwhile stays on the clipboard, and nothing is
//!    pasted. When the user chose to replace anyway after a snapshot
//!    couldn't be completed, there is no snapshot: the fix is written over whatever is there, and
//!    stays on the clipboard afterwards. Every other step is the same. In a web page or an
//!    Electron app the fix also goes on as HTML, when HTML can hold it exactly: a rich editor such
//!    as ProseMirror or Lexical pastes plain text with the formatting at the selection's start, so
//!    a selection starting in inline code would turn the whole fix into code.
//! 5. **Verify** through a freshly fetched element, polled for up to a second: the document holds
//!    the fix at the captured offset with the same text around it. A no-break space counts as a
//!    space, and line endings as line endings, however the app writes them.
//! 6. **Restore** the clipboard only after a verified paste with a snapshot, and only while it's still
//!    SuperEditr's. After an unverified paste it isn't restored: a Ctrl+V still queued would paste
//!    the user's old clipboard into the field.

use super::capture::{Class, Key, Services};
use super::clipboard::{self, Incomplete, Limits, Raw, Restore, Snapshot, WriteError};
use super::uia::{CONTEXT, Field, ReadError};
use super::window::WindowRef;
use super::{Capture, Source, trace};
use crate::platform::{PlatformError, ReplaceOutcome};
use std::time::{Duration, Instant};

/// How long the window and its field get to come back after the panel had focus. The focused
/// element arrived 17–61 ms after the window in measurements.
const FOCUS_WAIT: Duration = Duration::from_millis(if cfg!(test) { 40 } else { 250 });
/// How long a paste gets to show up. The slowest measured app took 84 ms.
const PASTE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 60 } else { 1000 });
const POLL: Duration = Duration::from_millis(if cfg!(test) { 1 } else { 15 });
/// How long a field that answers without its text gets to answer again.
const NO_TEXT_WAIT: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 200 });
/// One UI Automation call's limit within the chain.
const CALL: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 1500 });

/// What becomes of the user's clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clip {
    /// Put aside before the paste and given back after it; nothing is pasted if it can't be.
    Restore,
    /// The user chose to replace anyway: the fix is written over the clipboard and stays there.
    Leave,
}

pub fn replace<R: Raw>(services: &Services<'_, R>, capture: &Capture, text: &str, clip: Clip) -> ReplaceOutcome {
    let started = Instant::now();
    let outcome = attempt(services, capture, text, clip);
    let said = match &outcome {
        ReplaceOutcome::Applied => "applied".to_string(),
        ReplaceOutcome::CopyOnly(why) => format!("copy-only ({why})"),
        ReplaceOutcome::Uncertain(why) => format!("uncertain ({why})"),
        ReplaceOutcome::ClipboardKept(why) => format!("clipboard kept ({why})"),
    };
    let anyway = if clip == Clip::Leave { " anyway, without putting the clipboard aside" } else { "" };
    trace(format_args!("replace {}{anyway}: {said} in {:?}", capture.bundle_id, started.elapsed()));
    outcome
}

/// Brings the window back and waits until its captured field has focus again.
pub fn hand_back<R: Raw>(services: &Services<'_, R>, window: WindowRef, field: &Field) -> bool {
    let desktop = services.desktop;
    let in_front = || desktop.foreground().is_some_and(|front| front == window);
    if !in_front() && !desktop.bring_to_front(window) {
        return false;
    }
    let started = Instant::now();
    loop {
        if in_front() && services.fields.focused_is(field.token, CALL) {
            return true;
        }
        if started.elapsed() >= FOCUS_WAIT {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Whether the window is still in front and the selection still exactly the captured one. The
/// window is checked again after the selection: reading it takes a few milliseconds, in which the
/// user can switch to another window.
fn unchanged<R: Raw>(services: &Services<'_, R>, window: WindowRef, field: &Field) -> Result<(), PlatformError> {
    let in_front = || services.desktop.foreground() == Some(window);
    if !in_front() {
        return Err(PlatformError::FocusNotReturned);
    }
    same_selection(services, field)?;
    if !in_front() {
        return Err(PlatformError::FocusNotReturned);
    }
    Ok(())
}

/// Chromium can answer without its field's text for a moment after its window comes back to the
/// front, as it does for a capture's first reads, so a read that finds no text is tried again briefly.
fn same_selection<R: Raw>(services: &Services<'_, R>, field: &Field) -> Result<(), PlatformError> {
    let started = Instant::now();
    let mut read = services.fields.recheck(field.token, CALL);
    while matches!(read, Err(ReadError::NoText)) && started.elapsed() < NO_TEXT_WAIT {
        std::thread::sleep(POLL);
        read = services.fields.recheck(field.token, CALL);
    }
    match read {
        Ok(now)
            if now.same_element
                && now.ranges == 1
                && now.same_endpoints
                && now.text == field.text
                && now.before == field.before
                && now.after == field.after =>
        {
            Ok(())
        }
        Ok(_) | Err(ReadError::Gone) => Err(PlatformError::Stale),
        Err(_) => Err(PlatformError::CopyOnly),
    }
}

fn attempt<R: Raw>(services: &Services<'_, R>, capture: &Capture, text: &str, clip: Clip) -> ReplaceOutcome {
    let copy_only = |error| ReplaceOutcome::CopyOnly(error);
    let (Some(window), Some(field)) = (capture.window, capture.field.as_ref()) else {
        return copy_only(PlatformError::CopyOnly);
    };
    if capture.source != Source::Uia || !capture.editable || capture.class != Some(Class::KnownSafe) {
        return copy_only(PlatformError::CopyOnly);
    }
    if !services.desktop.alive(window) || services.desktop.elevated(window.pid) {
        return copy_only(PlatformError::CopyOnly);
    }
    if !hand_back(services, window, field) {
        return copy_only(PlatformError::FocusNotReturned);
    }
    // Coming back can move the caret or the selection.
    if let Err(error) = unchanged(services, window, field) {
        return copy_only(error);
    }
    let snapshot = match clip {
        Clip::Leave => None,
        Clip::Restore => match services.clipboard.snapshot(&Limits::default()) {
            Ok(snapshot) => Some(snapshot),
            Err(why) => return ReplaceOutcome::ClipboardKept(not_restorable(&why)),
        },
    };
    // Reading a slow clipboard can take a while, and the user may have moved on meanwhile.
    if let Err(error) = unchanged(services, window, field) {
        return copy_only(error);
    }
    let pasted = windows_line_endings(text);
    let html = if web_field(field) { clipboard::html_paragraphs(text) } else { None };
    if web_field(field) {
        trace(format_args!(
            "replace: the fix goes on the clipboard as text{}",
            if html.is_some() { " and HTML" } else { " only: HTML would change its spaces" }
        ));
    }
    let written = match &snapshot {
        Some(snapshot) => services.clipboard.write_text(snapshot.sequence, &pasted, html.as_deref()),
        None => services.clipboard.write_text_anyway(&pasted, html.as_deref()),
    };
    let ours = match written {
        Ok(ours) => ours,
        Err(WriteError::Changed) => {
            // The user copied something since the snapshot: theirs stays, and nothing is pasted.
            return newer_copy();
        }
        Err(error) => {
            if snapshot.is_some() {
                give_back(services, snapshot, services.clipboard.sequence());
                return copy_only(PlatformError::ReplaceFailed);
            }
            // Nothing was written, and a copy would fail the same way: the panel offers the fix.
            let why = match error {
                WriteError::Held => Incomplete::Held(services.clipboard.holder()).to_string(),
                _ => "Your clipboard couldn’t be written".to_string(),
            };
            return ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(format!(
                "{why}, so SuperEditr didn’t paste. Copy the suggestion from here."
            )));
        }
    };
    // The user may still hold the apply key's modifiers, and waiting for them takes time in which
    // they can switch windows or copy, so the last checks come after it, right before the paste.
    services.desktop.release_modifiers();
    if let Err(error) = unchanged(services, window, field) {
        give_back(services, snapshot, ours);
        return copy_only(error);
    }
    if services.clipboard.sequence() != ours {
        // A copy since SuperEditr wrote the clipboard is what Ctrl+V would paste. It's the user's:
        // it stays, and nothing is pasted.
        return newer_copy();
    }
    if !services.desktop.send_ctrl(Key::V) {
        give_back(services, snapshot, ours);
        return copy_only(PlatformError::ReplaceFailed);
    }
    if landed(services, field, text) {
        give_back(services, snapshot, ours);
        ReplaceOutcome::Applied
    } else {
        // The suggestion stays on the clipboard: restoring could let a late Ctrl+V paste the old one.
        ReplaceOutcome::Uncertain(PlatformError::Unverified)
    }
}

/// Nothing was pasted because the clipboard couldn't be put aside. The message is the one shown
/// where the panel can't offer to replace anyway (a quick fix); the panel shows its own.
fn not_restorable(why: &Incomplete) -> PlatformError {
    PlatformError::ClipboardUnrestorable(format!("{why}, so SuperEditr didn’t paste. Copy the suggestion from here."))
}

/// Nothing was pasted because the user copied something while the fix was being written. Their copy
/// stays on the clipboard, so the suggestion isn't put there: the panel offers it to copy.
fn newer_copy() -> ReplaceOutcome {
    ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(
        "You copied something just now, so SuperEditr didn’t paste and left your copy on the clipboard. Copy the suggestion from here."
            .into(),
    ))
}

/// A field in a web page or an Electron app, whose editor may paste HTML.
fn web_field(field: &Field) -> bool {
    field.framework.eq_ignore_ascii_case("Chrome")
}

fn give_back<R: Raw>(services: &Services<'_, R>, snapshot: Option<Snapshot>, ours: u32) {
    let Some(snapshot) = snapshot else { return };
    match services.clipboard.restore(&snapshot, ours) {
        Restore::Restored => {}
        other => trace(format_args!("replace: the clipboard {other:?}")),
    }
}

/// CF_UNICODETEXT's line endings.
fn windows_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Text as compared after a paste: line endings as `\n`, and a no-break space as a space.
fn normal(text: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            0x0D => {
                out.push(0x0A);
                if text.get(i + 1) == Some(&0x0A) {
                    i += 1;
                }
            }
            0xA0 => out.push(0x20),
            unit => out.push(unit),
        }
        i += 1;
    }
    out
}

/// Whether `document` holds `text` where the captured selection began, with the captured text on
/// either side. Offsets are UTF-16 units, as UI Automation counts them.
pub fn holds(document: &[u16], field: &Field, text: &str) -> bool {
    let start = field.start.min(document.len());
    let at = normal(&document[..start]).len();
    let doc = normal(document);
    let before = normal(&field.before);
    let wanted: Vec<u16> = text.encode_utf16().collect();
    let expected = [before.as_slice(), &normal(&wanted), &normal(&field.after)].concat();
    // The captured context is at most CONTEXT units, so it starts that far back at most.
    let Some(from) = at.checked_sub(before.len()) else { return false };
    debug_assert!(before.len() <= CONTEXT + 1);
    doc.get(from..).is_some_and(|rest| rest.starts_with(&expected))
}

fn landed<R: Raw>(services: &Services<'_, R>, field: &Field, text: &str) -> bool {
    let started = Instant::now();
    loop {
        if let Ok(document) = services.fields.document(field.token, CALL)
            && holds(&document, field, text)
        {
            trace(format_args!("replace: verified in {:?}", started.elapsed()));
            return true;
        }
        if started.elapsed() >= PASTE_WAIT {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_paste_that_lands_is_applied_and_the_clipboard_comes_back() {
        let world = World::new("notepad.exe").with_document("Rhyan said their going too the store.", "their going");
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::Applied));
        assert_eq!(world.keys_sent(), ["Ctrl+V"]);
        assert_eq!(world.document(), "Rhyan said they’re going too the store.");
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
        assert!(world.clipboard_has_html());
        assert_eq!(world.focus_requests(), 1);
    }

    #[test]
    fn a_web_field_gets_the_fix_as_html_too_and_a_native_one_doesnt() {
        let mut web = field("Rhyan said their going.");
        web.framework = "Chrome".into();
        let world = World::new("chrome.exe").with_field(web).select(11, 22).pasting_late();
        let capture = world.capture(true).unwrap();
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::Uncertain(_)));
        let html = String::from_utf8(world.clipboard_html().expect("an HTML version")).unwrap();
        assert!(html.contains("<!--StartFragment-->they’re going<!--EndFragment-->"), "{html}");

        let world = World::new("notepad.exe").with_document("Rhyan said their going.", "their going").pasting_late();
        let capture = world.capture(true).unwrap();
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::Uncertain(_)));
        assert_eq!(world.clipboard_html(), None, "a native field gets plain text only");
    }

    #[test]
    fn a_web_field_paste_with_html_lands_and_the_clipboard_comes_back() {
        let mut web = field("Rhyan said their going.");
        web.framework = "Chrome".into();
        let world = World::new("chrome.exe").with_field(web).select(11, 22);
        let capture = world.capture(true).unwrap();
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::Applied));
        assert_eq!(world.document(), "Rhyan said they’re going.");
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn offsets_count_utf16_units_past_emoji() {
        let world = World::new("chrome.exe").with_document("👋🏽 Brynlee: teh notes 🎉 are in.", "teh");
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.field.as_ref().unwrap().start, "👋🏽 Brynlee: ".encode_utf16().count());
        assert!(matches!(world.replace(&capture, "the"), ReplaceOutcome::Applied));
        assert_eq!(world.document(), "👋🏽 Brynlee: the notes 🎉 are in.");
    }

    #[test]
    fn a_window_that_doesnt_come_back_is_copy_only_and_nothing_is_touched() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their").refusing_focus();
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        let outcome = world.replace(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);
    }

    #[test]
    fn a_focus_return_that_moves_the_selection_is_stale() {
        let world = World::new("notepad.exe").with_document("Their going there.", "Their");
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        world.on_focus_select(0, 5 + 1);
        assert!(matches!(world.replace(&capture, "They’re"), ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);
    }

    #[test]
    fn the_second_of_two_identical_sentences_is_told_apart() {
        let text = "Kyndal sent teh report. Kyndal sent teh report.";
        let second = text.rfind("teh").unwrap();
        let world = World::new("notepad.exe").with_document(text, "").select(second, second + 3);
        let capture = world.capture(true).unwrap();
        // The user selects the same word in the first sentence instead.
        let first = text.find("teh").unwrap();
        world.select_now(first, first + 3);
        assert!(matches!(world.replace(&capture, "the"), ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert_eq!(world.document(), text);
        // Back on the second: only the second changes.
        world.select_now(second, second + 3);
        assert!(matches!(world.replace(&capture, "the"), ReplaceOutcome::Applied));
        assert_eq!(world.document(), "Kyndal sent teh report. Kyndal sent the report.");
    }

    #[test]
    fn a_selection_changed_during_a_slow_snapshot_is_stale_and_nothing_is_pasted() {
        let world = World::new("notepad.exe").with_document("Their going there.", "Their");
        let capture = world.capture(true).unwrap();
        world.during_snapshot_select(6, 11);
        assert!(matches!(world.replace(&capture, "They’re"), ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn an_incomplete_snapshot_keeps_the_clipboard_and_pastes_nothing() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.clipboard_too_big();
        let outcome = world.replace(&capture, "They’re");
        assert!(
            matches!(outcome, ReplaceOutcome::ClipboardKept(PlatformError::ClipboardUnrestorable(_))),
            "the panel can offer to replace anyway: {outcome:?}"
        );
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);
    }

    #[test]
    fn a_stalled_clipboard_owner_is_given_up_on_and_the_clipboard_left_free() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.clipboard_owner_stalls(Duration::from_secs(3));
        let started = Instant::now();
        let outcome = world.replace(&capture, "They’re");
        assert!(started.elapsed() < Duration::from_secs(2), "the snapshot's deadline holds: {:?}", started.elapsed());
        assert!(
            matches!(outcome, ReplaceOutcome::ClipboardKept(PlatformError::ClipboardUnrestorable(_))),
            "{outcome:?}"
        );
        assert!(world.keys_sent().is_empty());
        assert!(world.clipboard_is_free(), "another app can copy while the owner is still stalled");
    }

    #[test]
    fn replacing_anyway_pastes_without_a_snapshot_and_leaves_the_fix_on_the_clipboard() {
        let world = World::new("notepad.exe").with_document("Rhyan said their going.", "their going");
        let capture = world.capture(true).unwrap();
        world.clipboard_owner_stalls(Duration::from_secs(3));
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::ClipboardKept(_)));
        let reads = world.clipboard_reads();
        world.put_panel_in_front();
        let outcome = world.replace_anyway(&capture, "they’re going");
        assert!(matches!(outcome, ReplaceOutcome::Applied), "{outcome:?}");
        assert_eq!(world.clipboard_reads(), reads, "the clipboard isn't read again");
        assert_eq!(world.keys_sent(), ["Ctrl+V"]);
        assert_eq!(world.document(), "Rhyan said they’re going.");
        assert_eq!(world.clipboard_text().as_deref(), Some("they’re going"), "the fix stays on the clipboard");
        assert_eq!(world.focus_requests(), 1, "the focus handshake still runs");
    }

    #[test]
    fn replacing_anyway_still_checks_focus_the_selection_and_the_paste() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their").refusing_focus();
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        let outcome = world.replace_anyway(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)), "{outcome:?}");
        assert_eq!(world.clipboard_writes(), 0);

        let world = World::new("notepad.exe").with_document("Their going there.", "Their");
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        world.on_focus_select(0, 6);
        assert!(matches!(world.replace_anyway(&capture, "They’re"), ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);

        let world = World::new("notepad.exe").with_document("Their going.", "Their").pasting_late();
        let capture = world.capture(true).unwrap();
        let outcome = world.replace_anyway(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::Uncertain(PlatformError::Unverified)), "{outcome:?}");
        assert_eq!(world.keys_sent(), ["Ctrl+V"], "one paste, never a second");
    }

    #[test]
    fn replacing_anyway_while_another_app_holds_the_clipboard_pastes_nothing() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.clipboard.raw().held.store(true, std::sync::atomic::Ordering::SeqCst);
        let outcome = world.replace_anyway(&capture, "They’re");
        match outcome {
            ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(why)) => {
                assert!(why.starts_with("Kyndal’s Notes is holding the clipboard"), "{why}")
            }
            other => panic!("expected the clipboard kept, got {other:?}"),
        }
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_newer_copy_before_the_write_is_kept_and_nothing_is_pasted() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.user_copies_during_snapshot("Brynlee's newer copy");
        let outcome = world.replace(&capture, "They’re");
        assert!(
            matches!(&outcome, ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(why)) if why.starts_with("You copied something")),
            "the suggestion mustn't go on the clipboard over the user's copy: {outcome:?}"
        );
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_text().as_deref(), Some("Brynlee's newer copy"));
    }

    #[test]
    fn a_copy_made_after_the_fix_went_on_the_clipboard_is_kept_and_nothing_is_pasted() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.while_modifiers_held(|world| world.clipboard.raw().copy_from_app("Brynlee's newer copy"));
        let outcome = world.replace(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(_))), "{outcome:?}");
        assert!(world.keys_sent().is_empty(), "Ctrl+V would paste the user's copy");
        assert_eq!(world.document(), "Their going.");
        assert_eq!(world.clipboard_text().as_deref(), Some("Brynlee's newer copy"));
    }

    #[test]
    fn switching_windows_while_the_modifiers_are_held_pastes_nothing_and_gives_the_clipboard_back() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.while_modifiers_held(World::other_window_in_front);
        let outcome = world.replace(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)), "{outcome:?}");
        assert!(world.keys_sent().is_empty(), "Ctrl+V would go to the other window");
        assert_eq!(world.document(), "Their going.");
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn another_window_in_front_after_the_focus_returned_pastes_nothing() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their");
        let capture = world.capture(true).unwrap();
        world.other_window_in_front_during_snapshot();
        let outcome = world.replace(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::FocusNotReturned)), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_writes(), 0);
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn an_uncertain_paste_that_lands_late_pastes_the_suggestion_once() {
        let world = World::new("notepad.exe").with_document("Rhyan said their going.", "their going").pasting_late();
        let capture = world.capture(true).unwrap();
        let outcome = world.replace(&capture, "they’re going");
        assert!(matches!(outcome, ReplaceOutcome::Uncertain(_)), "{outcome:?}");
        // The app gets to the Ctrl+V after SuperEditr stopped waiting: it pastes what's on the
        // clipboard then, which is still the suggestion, never the user's old clipboard.
        world.paste_now();
        assert_eq!(world.document(), "Rhyan said they’re going.");
        assert_eq!(world.keys_sent(), ["Ctrl+V"], "never tried again");
        assert_eq!(world.clipboard_text().as_deref(), Some("they’re going"));
    }

    #[test]
    fn a_paste_that_cant_be_confirmed_is_uncertain_and_the_clipboard_isnt_restored() {
        let world = World::new("notepad.exe").with_document("Their going.", "Their").pasting_late();
        let capture = world.capture(true).unwrap();
        let outcome = world.replace(&capture, "They’re");
        assert!(matches!(outcome, ReplaceOutcome::Uncertain(PlatformError::Unverified)), "{outcome:?}");
        assert_eq!(world.keys_sent(), ["Ctrl+V"], "one paste, never a second");
        assert_eq!(
            world.clipboard_text().as_deref(),
            Some("They’re"),
            "a late Ctrl+V must paste the suggestion, not the user's old clipboard"
        );
    }

    #[test]
    fn a_field_that_answers_without_its_text_for_a_moment_is_read_again() {
        let world = World::new("chrome.exe").with_document("Rhyan said their going.", "their going");
        let capture = world.capture(true).unwrap();
        world.put_panel_in_front();
        world.rechecks_without_text(2);
        assert!(matches!(world.replace(&capture, "they’re going"), ReplaceOutcome::Applied));
        assert_eq!(world.document(), "Rhyan said they’re going.");

        // One that keeps answering without it is copy-only, with nothing pasted.
        let world = World::new("chrome.exe").with_document("Rhyan said their going.", "their going");
        let capture = world.capture(true).unwrap();
        world.rechecks_without_text(usize::MAX);
        let outcome = world.replace(&capture, "they’re going");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn copy_only_captures_are_never_pasted() {
        let world = World::new("soffice.bin").copying("Their going");
        let capture = world.capture(true).unwrap();
        world.clear_keys();
        assert!(matches!(world.replace(&capture, "They’re going"), ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn verification_reads_a_no_break_space_as_a_space_and_any_line_ending() {
        let mut field = field("teh");
        field.start = 6;
        field.before = utf16("Rhyan ");
        field.after = utf16(" notes");
        assert!(holds(&utf16("Rhyan the\u{A0}notes"), &field, "the"));
        assert!(!holds(&utf16("Rhyan teh notes"), &field, "the"));
        field.after = utf16("\r\nnext");
        assert!(holds(&utf16("Rhyan line one\nline two\nnext"), &field, "line one\r\nline two"));
        assert!(holds(&utf16("Rhyan line one\rline two\rnext"), &field, "line one\nline two"));
    }
}
