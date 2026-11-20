//! Writing a fix: an Accessibility write, a checked paste, or edits applied in place, each confirmed
//! by reading the field back.
//!
//! The decisions run against the traits in `seam.rs`, so they can be tested against a pretend Mac
//! (`testing.rs`): the [`Field`] being written, the [`Desktop`] that takes key events, the
//! [`Pasteboard`], and a [`Clock`] for every wait. [`replace`] runs them against the real ones.

use super::apps::focused_app;
use super::ax::{AXUIElementCreateApplication, AXValueCreate, CFRange, Element, kAXValueCFRangeType};
use super::keys::post_key;
use super::seam::{Clock, Desktop, Field, Pasteboard, Services, System};
use super::text::{html_with_code, is_invisible, is_line_break, markdown_with_code};
use super::{Capture, accessibility_trusted, pasteboard, trace};
use crate::platform::{PlatformError, ReplaceMethod, ReplaceOutcome, log_lines};
use core_foundation::base::{CFType, TCFType};
use core_graphics::event::{CGEvent, CGEventFlags};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::ffi::c_void;
use std::time::Duration;
use supereditr_core::policy::{self, Selection};

// --- Replacing ---------------------------------------------------------------------------------

/// Replaces the captured selection with `text`, verifying the result where the app allows it. A
/// `proofread` with a few local changes is applied in place, one edit per change, so formatting
/// around them is never touched; anything else, or an app that doesn't let SuperEditr move the
/// selection, replaces the whole selection.
pub fn replace(capture: &Capture, text: &str, proofread: bool) -> ReplaceOutcome {
    ReplaceOutcome::from(replace_with(
        &System::services(),
        capture,
        capture.element.as_ref().map(|element| element as &dyn Field),
        text,
        proofread,
    ))
}

/// [`replace`], against `services` and `field`, the captured field.
pub(super) fn replace_with<D: Desktop, P: Pasteboard>(
    services: &Services<'_, D, P>,
    capture: &Capture,
    field: Option<&dyn Field>,
    text: &str,
    proofread: bool,
) -> Result<ReplaceMethod, PlatformError> {
    if !services.desktop.trusted() {
        return Err(PlatformError::NotTrusted);
    }
    let Some(field) = field else {
        // Without a field to re-check, a paste could land somewhere else.
        return Err(PlatformError::CopyOnly);
    };
    {
        // Never write into a different field or over a changed selection.
        let still_focused = field.has_focus(capture.pid);
        if !still_focused || !selection_unchanged(capture, field) {
            trace(format_args!(
                "{}: not written: focus moved ({}) or selection changed",
                capture.bundle_id, !still_focused
            ));
            return Err(PlatformError::Stale);
        }
        // In a web field each edit visibly moves the selection, so plain text is replaced in one go,
        // which is quicker; an Accessibility write in a native field takes milliseconds.
        let plan = if capture.web && proofread && !capture.formatted {
            trace(format_args!(
                "{}",
                log_lines::ApplyingWhole { app: &capture.bundle_id, reason: &"the field has no formatting to keep" }
            ));
            None
        } else {
            Some(supereditr_core::edits::plan(&capture.text, text, proofread, &capture.protected()))
        };
        match plan {
            None => {}
            Some(Ok(edits)) if !edits.is_empty() => {
                if let Some(method) = replace_in_place(services, capture, field, text, &edits)? {
                    return Ok(method);
                }
            }
            Some(Ok(_)) => {}
            Some(Err(reason)) => {
                trace(format_args!("{}", log_lines::ApplyingWhole { app: &capture.bundle_id, reason: &reason }))
            }
        }
        if !capture.web && field.can_set_selected_text() {
            let before = field.value();
            if field.set_selected_text(text) {
                if verify_replacement(
                    services.clock,
                    field,
                    capture,
                    before.as_deref(),
                    text,
                    None,
                    Duration::from_millis(150),
                ) {
                    return Ok(ReplaceMethod::Accessibility);
                }
                // The write may have been applied without being verifiable; never paste on top of it.
                if field.value() != before || !selection_unchanged(capture, field) {
                    trace(format_args!(
                        "{}: Accessibility write changed the field but didn't verify",
                        capture.bundle_id
                    ));
                    return Err(PlatformError::Unverified);
                }
            }
        }
    }
    paste(services, capture, field, text)
}

/// True while the captured selection is still exactly what the user selected, in the same place.
/// Accessibility reads the selected text alone, without the text around it, so the same words
/// selected somewhere else in the field would read the same: where the app reported a range when
/// the selection was read, the field must report that same range now. Both ranges come from the
/// same element through the same attribute, so they count positions alike, even in a web field
/// whose positions don't match the text SuperEditr read. An app that reported no range is checked
/// by its text alone.
fn selection_unchanged(capture: &Capture, field: &dyn Field) -> bool {
    let same_place = capture.range.is_none_or(|range| field.selected_range() == Some(range));
    field.selection().is_some_and(|now| {
        policy::selection_unchanged(same_place, Selection::text(capture.text.as_str()), Selection::text(now.as_str()))
    })
}

/// True while the app `pid` is in front with focus in `element` or inside it.
pub(super) fn field_focused(pid: i32, element: &Element) -> bool {
    focused_app()
        .filter(|(_, front)| *front == pid)
        .and_then(|(app, _)| app.element("AXFocusedUIElement"))
        .is_some_and(|focus| is_same_or_descendant(&focus, element))
}

fn is_same_or_descendant(candidate: &Element, ancestor: &Element) -> bool {
    let mut current = candidate.clone();
    for _ in 0..12 {
        if current.same_as(ancestor) {
            return true;
        }
        match current.element("AXParent") {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

/// Checks that the field now contains `text` where the selection was.
/// Confirms by reading the field back that it now contains `text`: at the captured position when
/// the range is known, anywhere in a changed value otherwise. A field whose text can't be read is
/// never treated as verified.
/// `pasted`, when given, is what was put on the clipboard if it differs from `text`: an editor that
/// keeps the Markdown for code as written shows it with its backticks.
fn verify_replacement(
    clock: &dyn Clock,
    field: &dyn Field,
    capture: &Capture,
    before: Option<&str>,
    text: &str,
    pasted: Option<&str>,
    wait: Duration,
) -> bool {
    let deadline = clock.now() + wait;
    loop {
        if let Some(value) = field.value().filter(|value| Some(value.as_str()) != before) {
            let landed = match capture.range {
                Some((location, _)) => {
                    let units: Vec<u16> = value.encode_utf16().collect();
                    let inserted: Vec<u16> = text.encode_utf16().collect();
                    let start = location.max(0) as usize;
                    units.get(start..start + inserted.len()) == Some(inserted.as_slice())
                }
                None => value.contains(text),
            };
            // Chromium's value drops empty paragraphs; its rebuilt text keeps them. An editor may
            // pad pasted code with invisible characters of its own.
            let visible = |s: &str| s.chars().filter(|c| !is_invisible(*c)).collect::<String>();
            let found = |full: &str| {
                std::iter::once(text).chain(pasted).any(|t| full.contains(t) || visible(full).contains(&visible(t)))
            };
            if landed || found(&value) || field.web_text().is_some_and(|full| found(&full)) {
                return true;
            }
        }
        if clock.now() >= deadline {
            return false;
        }
        clock.sleep(Duration::from_millis(10));
    }
}

fn paste<D: Desktop, P: Pasteboard>(
    services: &Services<'_, D, P>,
    capture: &Capture,
    field: &dyn Field,
    text: &str,
) -> Result<ReplaceMethod, PlatformError> {
    let (desktop, pasteboard, clock) = (services.desktop, services.pasteboard, services.clock);
    let before = field.value();
    let snapshot = pasteboard.snapshot();
    // Code the app formatted is pasted as code again: a plain-text paste would turn it into prose.
    let code = supereditr_core::diff::carry_ranges(&capture.text, text, &capture.code);
    let html = (!code.is_empty()).then(|| html_with_code(text, &code));
    let plain = if code.is_empty() { text.to_string() } else { markdown_with_code(text, &code) };
    let ours = pasteboard.write_transient(&plain, html.as_deref());
    let window = desktop.window_state(capture.pid);
    let started = clock.now();
    // Chrome 156 drops a paste when its asynchronous clipboard-change notice arrives after the
    // paste started (crbug.com/568453825); the restore after a previous paste makes that likely,
    // and waiting before the paste didn't help in testing. A dropped paste never lands late, and
    // the paste after one always landed, so paste again, but only while the field and the
    // selection are untouched: a slow paste that did land replaces the selection, so it is never
    // pasted twice. Other apps get one paste and the full wait.
    let windows: &[u64] =
        if desktop.chromium_based(&capture.bundle_id, capture.pid) { &[250, 500, 600] } else { &[600] };
    let mut how = "";
    let mut attempts = 0;
    let mut landed = false;
    for (i, wait) in windows.iter().enumerate() {
        // Retry only while focus is still in the field, and the field, the selection and the
        // clipboard (still our text) are as left.
        if i > 0
            && (!field.has_focus(capture.pid)
                || field.value() != before
                || !selection_unchanged(capture, field)
                || pasteboard.change_count() != ours)
        {
            break;
        }
        how = desktop.paste(capture.pid);
        attempts += 1;
        landed = verify_replacement(
            clock,
            field,
            capture,
            before.as_deref(),
            text,
            Some(&plain),
            Duration::from_millis(*wait),
        );
        if landed {
            break;
        }
    }
    trace(format_args!(
        "{}: pasted {} characters with {how} ({attempts} {}), {window}; {} after {:?}, field value {}",
        capture.bundle_id,
        text.chars().count(),
        if attempts == 1 { "attempt" } else { "attempts" },
        if landed { "verified" } else { "not verified" },
        clock.now().duration_since(started),
        if field.value() == before { "unchanged" } else { "changed" },
    ));
    if html.is_some() {
        // Whether the editor kept the code as code: the walk logs what the field now has.
        trace(format_args!(
            "{}: pasted {} of {} code spans as HTML",
            capture.bundle_id,
            code.len(),
            capture.code.len()
        ));
        field.formatting();
    }
    if !landed {
        // The paste may still land late. Leave the suggestion on the clipboard rather than
        // restoring, so a late paste can't insert the user's old clipboard instead.
        return Err(PlatformError::Unverified);
    }
    pasteboard.restore_soon(snapshot, ours);
    Ok(ReplaceMethod::Paste)
}

// --- Applying in place ------------------------------------------------------------------------

/// Where the edit at character `start` of `captured` (the selection as SuperEditr read it) starts in
/// `selected` (the selection as the app counts positions in it), in UTF-16 units. The two can differ
/// in line breaks (Chromium counts none between blocks) and in the zero-width characters editors pad
/// formatting with, so both are skipped when counting. None when `start` is past the text or sits on
/// a skipped character.
fn field_offset(captured: &str, selected: &str, start: usize) -> Option<usize> {
    let skipped = |c: char| is_line_break(c) || is_invisible(c);
    let before = captured.chars().take(start).filter(|c| !skipped(*c)).count();
    if captured.chars().nth(start).is_none_or(skipped) {
        return None;
    }
    let mut seen = 0;
    let mut offset = 0;
    for c in selected.chars() {
        if !skipped(c) {
            if seen == before {
                return Some(offset);
            }
            seen += 1;
        }
        offset += c.len_utf16();
    }
    None
}

/// Where an edit goes in the field: UTF-16 units `start..end` to replace (equal for an insertion),
/// and `text`, at `check`, to read back first: the words replaced, or an insertion's anchor.
struct Place {
    check: usize,
    text: String,
    start: usize,
    end: usize,
}

/// Finds `edit` in the selection, `selected` as the app reports it, starting at `location`, and
/// confirms the selection holds what the edit expects there. Positions count as the app counts its
/// selection, which isn't always its `AXValue`: Chromium's leave out the line breaks between blocks.
/// An insertion goes on the same side of any invisible padding beside its anchor as in the text
/// that was read.
fn place(edit: &supereditr_core::edits::Edit, captured: &str, selected: &str, location: usize) -> Option<Place> {
    let units: Vec<u16> = selected.encode_utf16().collect();
    let (check_from, text) = match &edit.anchor {
        Some(anchor) => (anchor.start, &anchor.text),
        None => (edit.start, &edit.old),
    };
    let offset = field_offset(captured, selected, check_from)?;
    let len = text.encode_utf16().count();
    if units.get(offset..offset + len)? != text.encode_utf16().collect::<Vec<_>>().as_slice() {
        return None;
    }
    let check = location + offset;
    let invisible = |u: u16| char::from_u32(u32::from(u)).is_some_and(is_invisible);
    let start = match &edit.anchor {
        None => return Some(Place { check, text: text.clone(), start: check, end: check + len }),
        Some(anchor) if !anchor.after => {
            let mut at = offset + len;
            while anchor.padded && units.get(at).is_some_and(|&u| invisible(u)) {
                at += 1;
            }
            location + at
        }
        Some(anchor) => {
            let mut at = offset;
            while anchor.padded && at > 0 && invisible(units[at - 1]) {
                at -= 1;
            }
            location + at
        }
    };
    Some(Place { check, text: text.clone(), start, end: start })
}

/// Sets the field's selection to UTF-16 units `start..start + length`, without waiting.
pub(super) fn set_selection(element: &Element, start: usize, length: usize) -> bool {
    let range = CFRange { location: start as isize, length: length as isize };
    let value = unsafe { AXValueCreate(kAXValueCFRangeType, &range as *const CFRange as *const c_void) };
    if value.is_null() {
        return false;
    }
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    element.set("AXSelectedTextRange", value.as_CFTypeRef())
}

/// Types `text` into the app `pid`, replacing its selection: key events carrying the text, 20 UTF-16
/// units at a time (the most one event holds), with no clipboard round trip. Empty text deletes the
/// selection with the Delete key.
pub(super) fn type_text(pid: i32, text: &str) -> bool {
    const DELETE: u16 = 51;
    if text.is_empty() {
        return post_key(pid, DELETE, CGEventFlags::empty());
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return false };
    for chunk in units.chunks(20) {
        for down in [true, false] {
            let Ok(event) = CGEvent::new_keyboard_event(source.clone(), 0, down) else { return false };
            event.set_flags(CGEventFlags::empty());
            event.set_string_from_utf16_unchecked(chunk);
            if !accessibility_trusted(false) {
                return false;
            }
            event.post_to_pid(pid);
        }
    }
    true
}

/// Polls `done` until it holds or `wait` runs out.
fn wait_until(clock: &dyn Clock, wait: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = clock.now() + wait;
    loop {
        if done() {
            return true;
        }
        if clock.now() >= deadline {
            return false;
        }
        clock.sleep(Duration::from_millis(10));
    }
}

/// Applies a proofread as `edits` (last first), each writing only one change: an Accessibility
/// write in a native field, typing in a web one. Before each write the text there is selected and
/// read back; after each, what was written is selected and read back; after the last, the field
/// must hold `text`.
///
/// `Ok(None)` when nothing was written and the selection is back as captured (the app doesn't let
/// SuperEditr move its selection, or the field doesn't line up with the edits): the caller then
/// replaces the whole selection. Once an edit is written there is no going back: a later failure
/// stops and reports a partial fix.
fn replace_in_place<D: Desktop, P: Pasteboard>(
    services: &Services<'_, D, P>,
    capture: &Capture,
    field: &dyn Field,
    text: &str,
    edits: &[supereditr_core::edits::Edit],
) -> Result<Option<ReplaceMethod>, PlatformError> {
    let clock = services.clock;
    let started = clock.now();
    let fallback = |reason: &str| {
        trace(format_args!("{}", log_lines::ApplyingWhole { app: &capture.bundle_id, reason: &reason }));
        Ok(None)
    };
    let stopped = |applied: usize, reason: &str| {
        trace(format_args!(
            "{}",
            log_lines::AppliedPartly { app: &capture.bundle_id, applied, total: edits.len(), reason }
        ));
        // With nothing applied, the first edit may still have landed unseen.
        Err(if applied == 0 {
            PlatformError::Unverified
        } else {
            PlatformError::Partial { applied, total: edits.len() }
        })
    };
    // The selection as the app counts positions in it, still as captured (`replace` checked).
    let (Some(range), Some(selected), Some(before)) = (field.selected_range(), field.selected(), field.value()) else {
        return fallback("the field doesn't report its text or selection");
    };
    let (location, length) = (range.0.max(0) as usize, range.1.max(0) as usize);
    // Where each edit is, checked against the selection before anything moves.
    let Some(places) =
        edits.iter().map(|edit| place(edit, &capture.text, &selected, location)).collect::<Option<Vec<_>>>()
    else {
        return fallback("the field's text doesn't line up with the changes");
    };
    // After an edit, Chrome's accessibility tree lags behind its text, more on a busy Mac: the field's
    // value updates late, and a selection placed from the stale tree lands on other text. Each step
    // after an edit waits up to this long. Before any edit a short wait is enough, so a field that
    // doesn't move its selection falls back to the whole replace without delay.
    const AFTER_EDIT: Duration = Duration::from_secs(1);
    // Selects `text` at `start` and confirms it by reading the selection back, setting it again
    // until it reads back or `wait` runs out.
    let select_text = |start: usize, text: &str, wait: Duration| {
        let deadline = clock.now() + wait;
        loop {
            if !field.select(start, text.encode_utf16().count()) {
                return false;
            }
            if wait_until(clock, Duration::from_millis(100), || field.selected().as_deref() == Some(text)) {
                return true;
            }
            if clock.now() >= deadline {
                return false;
            }
        }
    };
    // Selects the text an edit checks; for an insertion, then puts the caret in place.
    let select = |place: &Place, wait: Duration| {
        select_text(place.check, &place.text, wait) && (place.start != place.end || field.select(place.start, 0))
    };
    // A field that refused to move its selection still has it as captured: nothing to put back.
    let restore = || {
        (field.selected_range() == Some(range) && selection_unchanged(capture, field))
            || field.select(location, length)
                && wait_until(clock, Duration::from_millis(200), || selection_unchanged(capture, field))
    };
    if !select(&places[0], Duration::from_millis(200)) {
        if !restore() {
            trace(format_args!("{}: in place: the selection couldn't be put back", capture.bundle_id));
            return Err(PlatformError::Stale);
        }
        return fallback("the app doesn't let SuperEditr move the selection");
    }
    let native = !capture.web && field.can_set_selected_text();
    for (i, (edit, place)) in edits.iter().zip(&places).enumerate() {
        if i > 0 && !wait_until(clock, AFTER_EDIT, || field.has_focus(capture.pid)) {
            return stopped(i, "focus left the field");
        }
        if i > 0 && !select(place, AFTER_EDIT) {
            return stopped(i, "the text to fix no longer matched");
        }
        let value = field.value();
        let wrote = if native {
            field.set_selected_text(&edit.new)
        } else {
            services.desktop.type_text(capture.pid, &edit.new)
        };
        // Landed once the field changes and what was written reads back where it should be.
        let landed = wrote
            && wait_until(clock, AFTER_EDIT, || field.value() != value)
            && (edit.new.is_empty() || select_text(place.start, &edit.new, AFTER_EDIT));
        if !landed {
            // Nothing written yet: the whole replace can still go ahead, once the selection is back.
            if i == 0 && field.value().as_deref() == Some(before.as_str()) && restore() {
                return fallback("the first change didn't land");
            }
            return stopped(i, "a change didn't land");
        }
    }
    // The caret after the fixed text, where replacing the whole selection leaves it.
    let grown: isize =
        edits.iter().map(|e| e.new.encode_utf16().count() as isize - e.old.encode_utf16().count() as isize).sum();
    field.select((location + length).saturating_add_signed(grown), 0);
    let verified = verify_replacement(clock, field, capture, Some(&before), text, None, AFTER_EDIT);
    let code = if capture.code.is_empty() {
        String::new()
    } else {
        let visible = |s: &str| s.chars().filter(|c| !is_invisible(*c)).collect::<String>();
        let pieces: Vec<String> = field.formatting().code.iter().map(|p| visible(&p.text)).collect();
        let kept = capture
            .code
            .iter()
            .filter(|&&(s, e)| {
                let code = visible(&capture.text.chars().skip(s).take(e - s).collect::<String>());
                pieces.iter().any(|p| p.contains(code.trim()))
            })
            .count();
        format!("; {kept} of {} code spans still code", capture.code.len())
    };
    trace(format_args!(
        "{}",
        log_lines::AppliedInPlace {
            app: &capture.bundle_id,
            edits: edits.len(),
            ms: clock.now().duration_since(started).as_millis(),
            native,
            verified,
            code: &code,
        }
    ));
    if !verified {
        return Err(PlatformError::Unverified);
    }
    Ok(Some(if native { ReplaceMethod::Accessibility } else { ReplaceMethod::Typing }))
}

/// Whether `pid` is the front app, whether SuperEditr's panel still has focus, and whether the
/// app's window is main and focused, for the log.
pub(super) fn key_window_state(pid: i32) -> String {
    let front = focused_app().is_some_and(|(_, front)| front == pid);
    let own = Element::from_create(unsafe { AXUIElementCreateApplication(std::process::id() as i32) })
        .is_some_and(|me| me.element("AXFocusedWindow").is_some());
    let window = Element::from_create(unsafe { AXUIElementCreateApplication(pid) })
        .and_then(|app| app.element("AXFocusedWindow"));
    let flag = |name| window.as_ref().and_then(|w| w.flag(name)).map_or("unknown".to_string(), |v| v.to_string());
    format!(
        "front {front}, SuperEditr window focused {own}, window main {} focused {}",
        flag("AXMain"),
        flag("AXFocused")
    )
}

pub fn copy_to_clipboard(text: &str) {
    pasteboard::write_string(text);
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn finds_an_edit_in_the_field_text() {
        // The same text: the edit is where it was read.
        assert_eq!(field_offset("Their going, realy", "Their going, realy", 13), Some(13));
        // Chromium's text for a rich field leaves an empty paragraph out.
        assert_eq!(field_offset("One.\n\nteh end", "One.\nteh end", 6), Some(5));
        // An editor pads code with zero-width spaces that the read text may not have.
        assert_eq!(field_offset("run make realy", "run \u{200B}make\u{200B} realy", 9), Some(11));
        // Offsets are UTF-16 units: an emoji takes two.
        assert_eq!(field_offset("😀 teh", "😀 teh", 2), Some(3));
        // Past the end, or on a line break, is nowhere.
        assert_eq!(field_offset("ab", "ab", 2), None);
        assert_eq!(field_offset("a\nb", "a\nb", 1), None);
    }

    #[test]
    fn places_edits_as_chromium_counts_its_selection() {
        use supereditr_core::edits::plan;
        // Read with its line breaks; Chromium's selection has none between blocks, and its code chip
        // is padded with zero-width spaces.
        let read = "Hi\n\nthis is \u{200B}code\u{200B}";
        let selected = "Hithis is \u{200B}code\u{200B}";
        let edits = plan(read, "Hi\n\nThis is \u{200B}code\u{200B}.", true, &[(13, 17)]).unwrap();
        let insertion = place(&edits[0], read, selected, 5).unwrap();
        assert_eq!((insertion.check, insertion.text.as_str(), insertion.start, insertion.end), (16, "code", 21, 21));
        let replacement = place(&edits[1], read, selected, 5).unwrap();
        assert_eq!((replacement.check, replacement.start, replacement.end), (7, 7, 11));
        // A selection that doesn't hold what was read gives no place.
        assert!(place(&edits[1], read, "Hithat is code", 5).is_none());
    }

    /// A native text field in QuillPoint, with a sentence's two typos selected.
    fn notes() -> World {
        World::new("com.example.QuillPoint").with_text("Rhyan said their going too the store.", "their going too")
    }

    #[test]
    fn an_accessibility_write_that_reads_back_is_applied() {
        let world = notes();
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.writes(), ["they’re going to"]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
        assert!(world.keys_sent().is_empty(), "nothing is pasted");
        assert!(world.put_on_clipboard().is_empty(), "the clipboard isn't touched");
    }

    #[test]
    fn a_proofread_in_a_native_field_is_written_change_by_change() {
        let world = notes();
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
        let writes = world.writes();
        assert_eq!(writes.len(), 2, "one write per change, never the whole selection: {writes:?}");
        assert!(writes.iter().all(|w| !w.contains("going")), "{writes:?}");
        assert!(world.keys_sent().is_empty());
        // The caret ends after the fixed text, where replacing the whole selection leaves it.
        assert_eq!(
            world.field().selected_range(),
            Some(("Rhyan said they’re going to".encode_utf16().count() as isize, 0))
        );
    }

    #[test]
    fn a_proofread_of_formatted_web_text_is_typed_change_by_change() {
        let world = World::new("com.example.QuillPoint")
            .with_text("Brynlee sent the the KPI notes realy late.", "Brynlee sent the the KPI notes realy late.")
            .formatted();
        let capture = world.capture();
        let fixed = "Brynlee sent the KPI notes really late.";
        assert!(matches!(world.proofread(&capture, fixed), Ok(ReplaceMethod::Typing)));
        assert_eq!(world.field_text(), fixed);
        assert!(world.writes().is_empty(), "a web field is never written through Accessibility");
        let keys = world.keys_sent();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys.iter().all(|k| k.starts_with("typed")), "typed, not pasted: {keys:?}");
        assert!(world.put_on_clipboard().is_empty());
    }

    #[test]
    fn a_proofread_of_plain_web_text_is_pasted_whole() {
        let world = notes().web();
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Ok(ReplaceMethod::Paste)));
        assert_eq!(world.keys_sent(), ["⌘V"]);
        assert_eq!(world.put_on_clipboard(), [("they’re going to".to_string(), None)]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
    }

    #[test]
    fn an_app_that_keeps_its_selection_gets_the_whole_selection_without_a_long_wait() {
        let world = notes().keeping_its_selection();
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.writes(), ["they’re going to"], "one write of the whole selection");
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
        assert!(world.waited() < Duration::from_millis(500), "{:?}", world.waited());
    }

    #[test]
    fn an_app_that_refuses_to_move_its_selection_gets_the_whole_selection() {
        let world = notes().refusing_selection_changes();
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.writes(), ["they’re going to"], "one write of the whole selection");
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
    }

    #[test]
    fn an_edit_the_app_silently_ignores_falls_back_to_a_paste() {
        // The app takes every Accessibility write and applies none: in place, then whole.
        let world = notes().writes_are(Write::Ignored);
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Ok(ReplaceMethod::Paste)));
        assert_eq!(world.writes().len(), 2, "the first change in place, then the whole selection");
        assert_eq!(world.keys_sent(), ["⌘V"]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy", "the user's clipboard comes back");
    }

    #[test]
    fn an_accessibility_write_that_changed_the_field_unconfirmed_is_never_pasted_over() {
        let world = notes().writes_are(Write::Garbled);
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert!(world.keys_sent().is_empty(), "a paste would write the fix twice");
        assert!(world.put_on_clipboard().is_empty());
    }

    #[test]
    fn a_first_change_in_place_that_lands_wrong_is_never_written_over() {
        let world = notes().writes_are(Write::Garbled);
        let capture = world.capture();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert_eq!(world.writes().len(), 1, "nothing after the change that landed wrong");
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_later_change_that_cant_go_ahead_is_a_partial_fix() {
        let world = notes();
        let capture = world.capture();
        world.after_write(World::focus_elsewhere);
        let outcome = world.proofread(&capture, "they’re going to");
        assert!(matches!(outcome, Err(PlatformError::Partial { applied: 1, total: 2 })), "{outcome:?}");
        assert_eq!(world.writes().len(), 1);
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_field_whose_selected_text_cant_be_set_is_pasted_into() {
        let world = notes().not_settable();
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Ok(ReplaceMethod::Paste)));
        assert!(world.writes().is_empty());
        assert_eq!(world.keys_sent(), ["⌘V"]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
    }

    #[test]
    fn a_paste_that_lands_is_applied_and_the_clipboard_comes_back() {
        let world = notes().web();
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Ok(ReplaceMethod::Paste)));
        assert_eq!(world.keys_sent(), ["⌘V"]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
        assert_eq!(world.clipboard_html().as_deref(), Some("<b>Rhyan's own copy</b>"), "every representation");
    }

    #[test]
    fn a_paste_that_cant_be_confirmed_leaves_the_fix_on_the_clipboard() {
        let world = notes().web().pastes(Pastes::Never);
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["⌘V"], "one paste, never a second");
        assert_eq!(
            world.clipboard_text(),
            "they’re going to",
            "a late paste must paste the suggestion, not the user's old clipboard"
        );
    }

    #[test]
    fn chromium_gets_a_dropped_paste_again_while_the_field_is_untouched() {
        let world = notes().web().chromium().pastes(Pastes::AfterDropping(1));
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Ok(ReplaceMethod::Paste)));
        assert_eq!(world.keys_sent(), ["⌘V", "⌘V"]);
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.", "pasted once");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");

        // Three tries at most, then the fix stays on the clipboard.
        let world = notes().web().chromium().pastes(Pastes::Never);
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["⌘V", "⌘V", "⌘V"]);
        assert_eq!(world.clipboard_text(), "they’re going to");
    }

    #[test]
    fn a_copy_made_during_an_unconfirmed_paste_stops_a_second_one() {
        let world = notes().web().chromium().pastes(Pastes::Never);
        let capture = world.capture();
        world.after_paste(|world| world.user_copies("Kyndal's newer copy"));
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["⌘V"], "a second paste would paste the user's copy");
        assert_eq!(world.clipboard_text(), "Kyndal's newer copy", "the user's copy stays");
    }

    #[test]
    fn focus_leaving_during_an_unconfirmed_paste_stops_a_second_one() {
        let world = notes().web().chromium().pastes(Pastes::Never);
        let capture = world.capture();
        world.after_paste(World::focus_elsewhere);
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["⌘V"]);
    }

    #[test]
    fn a_changed_selection_is_stale_and_nothing_is_written() {
        let world = notes();
        let capture = world.capture();
        world.select_now(0, 5);
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Stale)));
        assert!(world.writes().is_empty());
        assert!(world.keys_sent().is_empty());
        assert!(world.put_on_clipboard().is_empty());
    }

    /// A field where "teh" is written twice, with the first one selected, and where the second is.
    fn twice() -> (World, usize) {
        let text = "Kyndal sent teh notes and teh KPI table.";
        let second = text[..text.rfind("teh").unwrap()].encode_utf16().count();
        (World::new("com.example.QuillPoint").with_text(text, "teh"), second)
    }

    #[test]
    fn the_same_words_selected_somewhere_else_are_stale_and_nothing_is_written() {
        let (world, second) = twice();
        let capture = world.capture();
        world.select_now(second, 3);
        assert!(matches!(world.replace(&capture, "the"), Err(PlatformError::Stale)));
        assert!(world.writes().is_empty());
        assert_eq!(world.field_text(), "Kyndal sent teh notes and teh KPI table.");

        // A web field, which would be pasted into.
        let (world, second) = twice();
        let world = world.web().chromium();
        let capture = world.capture();
        world.select_now(second, 3);
        assert!(matches!(world.proofread(&capture, "the"), Err(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
        assert!(world.put_on_clipboard().is_empty());
        assert_eq!(world.field_text(), "Kyndal sent teh notes and teh KPI table.");
    }

    #[test]
    fn a_selection_made_again_in_the_same_place_is_written() {
        let (world, _) = twice();
        let capture = world.capture();
        world.select_now(0, 0);
        world.select_now(12, 3);
        assert!(matches!(world.replace(&capture, "the"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.field_text(), "Kyndal sent the notes and teh KPI table.");
    }

    #[test]
    fn an_app_that_reports_no_range_is_checked_by_its_text() {
        let world = notes();
        let mut capture = world.capture();
        capture.range = None;
        assert!(matches!(world.replace(&capture, "they’re going to"), Ok(ReplaceMethod::Accessibility)));
        assert_eq!(world.field_text(), "Rhyan said they’re going to the store.");

        let world = notes();
        let mut capture = world.capture();
        capture.range = None;
        world.select_now(0, 5);
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::Stale)));
        assert!(world.writes().is_empty());
    }

    #[test]
    fn a_selection_moved_to_the_same_words_during_an_unconfirmed_paste_stops_a_second_one() {
        let (world, second) = twice();
        let world = world.web().chromium().pastes(Pastes::Never);
        let capture = world.capture();
        world.after_paste(move |world| world.select_now(second, 3));
        assert!(matches!(world.replace(&capture, "the"), Err(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["⌘V"], "a second paste would land on the other words");
    }

    #[test]
    fn focus_in_another_field_is_stale_and_nothing_is_written() {
        let world = notes();
        let capture = world.capture();
        world.focus_elsewhere();
        assert!(matches!(world.proofread(&capture, "they’re going to"), Err(PlatformError::Stale)));
        assert!(world.writes().is_empty());
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn copy_only_captures_are_never_written() {
        let world = notes();
        let capture = world.capture();
        assert!(matches!(world.replace_without_a_field(&capture, "they’re going to"), Err(PlatformError::CopyOnly)));
        assert!(world.writes().is_empty());
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn without_accessibility_permission_nothing_is_written() {
        let world = notes().untrusted();
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "they’re going to"), Err(PlatformError::NotTrusted)));
        assert!(world.writes().is_empty());
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn code_the_app_formatted_is_pasted_as_code_again() {
        let world = World::new("com.example.QuillPoint")
            .with_text("Run make deploy realy soon.", "Run make deploy realy soon.")
            .web();
        let mut capture = world.capture();
        capture.code = vec![(4, 15)];
        let fixed = "Run make deploy really soon.";
        assert!(matches!(world.replace(&capture, fixed), Ok(ReplaceMethod::Paste)));
        let put = world.put_on_clipboard();
        assert_eq!(put.len(), 1);
        assert_eq!(put[0].0, "Run `make deploy` really soon.", "the plain text marks the code as Markdown");
        let html = put[0].1.as_deref().expect("an HTML version");
        assert!(html.contains("<code>make deploy</code>"), "{html}");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn a_paste_is_confirmed_by_chromiums_rebuilt_text_when_its_value_drops_empty_paragraphs() {
        let text = "Teh plan.\n\nShip it.";
        let world = World::new("com.google.Chrome").with_text(text, text).web().chromium().dropping_empty_paragraphs();
        let capture = world.capture();
        assert!(matches!(world.replace(&capture, "The plan.\n\nShip it."), Ok(ReplaceMethod::Paste)));
        assert_eq!(world.keys_sent(), ["⌘V"]);
    }
}
