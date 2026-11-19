//! Writing a fix: an Accessibility write, a checked paste, or edits applied in place, each confirmed
//! by reading the field back.

use super::apps::{focused_app, is_chromium_based};
use super::ax::{AXUIElementCreateApplication, AXValueCreate, CFRange, Element, kAXValueCFRangeType};
use super::capture::{press_menu_item, selection_unchanged};
use super::keys::{Key, post_command_key, post_key};
use super::text::{code_in, html_with_code, is_invisible, is_line_break, markdown_with_code};
use super::{Capture, accessibility_trusted, pasteboard, trace};
use crate::platform::{PlatformError, ReplaceMethod};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventFlags};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::ffi::c_void;
use std::time::{Duration, Instant};

/// Replaces the captured selection with `text`, verifying the result where the app allows it. A
/// `proofread` with a few local changes is applied in place, one edit per change, so formatting
/// around them is never touched; anything else, or an app that doesn't let SuperEditr move the
/// selection, replaces the whole selection.
pub fn replace(capture: &Capture, text: &str, proofread: bool) -> Result<ReplaceMethod, PlatformError> {
    if !accessibility_trusted(false) {
        return Err(PlatformError::NotTrusted);
    }
    let Some(element) = &capture.element else {
        // Without a field to re-check, a paste could land somewhere else.
        return Err(PlatformError::CopyOnly);
    };
    {
        // Never write into a different field or over a changed selection.
        let still_focused = field_focused(capture, element);
        if !still_focused || !selection_unchanged(capture) {
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
                "{}: applying the whole selection: the field has no formatting to keep",
                capture.bundle_id
            ));
            None
        } else {
            Some(supereditr_core::edits::plan(&capture.text, text, proofread, &capture.protected()))
        };
        match plan {
            None => {}
            Some(Ok(edits)) if !edits.is_empty() => {
                if let Some(method) = replace_in_place(capture, element, text, &edits)? {
                    return Ok(method);
                }
            }
            Some(Ok(_)) => {}
            Some(Err(reason)) => trace(format_args!("{}: applying the whole selection: {reason}", capture.bundle_id)),
        }
        if !capture.web && element.settable("AXSelectedText") {
            let before = element.string("AXValue");
            let value = CFString::new(text);
            if element.set("AXSelectedText", value.as_CFTypeRef()) {
                if verify_replacement(element, capture, before.as_deref(), text, None, Duration::from_millis(150)) {
                    return Ok(ReplaceMethod::Accessibility);
                }
                // The write may have been applied without being verifiable; never paste on top of it.
                if element.string("AXValue") != before || !selection_unchanged(capture) {
                    trace(format_args!(
                        "{}: Accessibility write changed the field but didn't verify",
                        capture.bundle_id
                    ));
                    return Err(PlatformError::Unverified);
                }
            }
        }
    }
    paste(capture, element, text)
}

/// True while the captured app is in front with focus in `element` or inside it.
fn field_focused(capture: &Capture, element: &Element) -> bool {
    focused_app()
        .filter(|(_, pid)| *pid == capture.pid)
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
    element: &Element,
    capture: &Capture,
    before: Option<&str>,
    text: &str,
    pasted: Option<&str>,
    wait: Duration,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(value) = element.string("AXValue").filter(|value| Some(value.as_str()) != before) {
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
            if landed || found(&value) || element.web_text().is_some_and(|full| found(&full)) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn paste(capture: &Capture, element: &Element, text: &str) -> Result<ReplaceMethod, PlatformError> {
    let before = element.string("AXValue");
    let snapshot = pasteboard::Snapshot::take();
    // Code the app formatted is pasted as code again: a plain-text paste would turn it into prose.
    let code = supereditr_core::diff::carry_ranges(&capture.text, text, &capture.code);
    let html = (!code.is_empty()).then(|| html_with_code(text, &code));
    let plain = if code.is_empty() { text.to_string() } else { markdown_with_code(text, &code) };
    let ours = pasteboard::write_transient_string(&plain, html.as_deref());
    let window = key_window_state(capture.pid);
    let started = Instant::now();
    // The app's own Paste menu item first. The app can be left without a key window (after the
    // panel had focus for an instruction, or the user clicked it); browsers and Electron apps then
    // drop a posted ⌘V, but a menu item acts on the app's main window.
    let app = Element::from_create(unsafe { AXUIElementCreateApplication(capture.pid) });
    let press = || {
        if app.as_ref().and_then(|app| press_menu_item(app, "v")) == Some(true) {
            "the app's Paste menu item"
        } else {
            post_command_key(capture.pid, Key::V);
            "⌘V to the app's process"
        }
    };
    // Chrome 156 drops a paste when its asynchronous clipboard-change notice arrives after the
    // paste started (crbug.com/568453825); the restore after a previous paste makes that likely,
    // and waiting before the paste didn't help in testing. A dropped paste never lands late, and
    // the paste after one always landed, so paste again, but only while the field and the
    // selection are untouched: a slow paste that did land replaces the selection, so it is never
    // pasted twice. Other apps get one paste and the full wait.
    let windows: &[u64] = if is_chromium_based(&capture.bundle_id, capture.pid) { &[250, 500, 600] } else { &[600] };
    let mut how = "";
    let mut attempts = 0;
    let mut landed = false;
    for (i, wait) in windows.iter().enumerate() {
        // Retry only while focus is still in the field, and the field, the selection and the
        // clipboard (still our text) are as left.
        if i > 0
            && (!field_focused(capture, element)
                || element.string("AXValue") != before
                || !selection_unchanged(capture)
                || pasteboard::change_count() != ours)
        {
            break;
        }
        how = press();
        attempts += 1;
        landed =
            verify_replacement(element, capture, before.as_deref(), text, Some(&plain), Duration::from_millis(*wait));
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
        started.elapsed(),
        if element.string("AXValue") == before { "unchanged" } else { "changed" },
    ));
    if html.is_some() {
        // Whether the editor kept the code as code: the walk logs what the field now has.
        trace(format_args!(
            "{}: pasted {} of {} code spans as HTML",
            capture.bundle_id,
            code.len(),
            capture.code.len()
        ));
        code_in(element);
    }
    if !landed {
        // The paste may still land late. Leave the suggestion on the clipboard rather than
        // restoring, so a late paste can't insert the user's old clipboard instead.
        return Err(PlatformError::Unverified);
    }
    // Give the app a moment to finish reading the pasteboard. A newer copy from another app always
    // wins over the restore.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(120));
        snapshot.restore_if_unchanged_since(ours);
    });
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
fn type_text(pid: i32, text: &str) -> bool {
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
fn wait_until(wait: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
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
fn replace_in_place(
    capture: &Capture,
    element: &Element,
    text: &str,
    edits: &[supereditr_core::edits::Edit],
) -> Result<Option<ReplaceMethod>, PlatformError> {
    let started = Instant::now();
    let fallback = |reason: &str| {
        trace(format_args!("{}: applying the whole selection: {reason}", capture.bundle_id));
        Ok(None)
    };
    let stopped = |applied: usize, reason: &str| {
        trace(format_args!(
            "{}: applied in place partly: {applied} of {} edits, then {reason}",
            capture.bundle_id,
            edits.len()
        ));
        // With nothing applied, the first edit may still have landed unseen.
        Err(if applied == 0 {
            PlatformError::Unverified
        } else {
            PlatformError::Partial { applied, total: edits.len() }
        })
    };
    // The selection as the app counts positions in it, still as captured (`replace` checked).
    let (Some(range), Some(selected), Some(before)) =
        (element.range("AXSelectedTextRange"), element.string("AXSelectedText"), element.string("AXValue"))
    else {
        return fallback("the field doesn't report its text or selection");
    };
    let (location, length) = (range.location.max(0) as usize, range.length.max(0) as usize);
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
        let deadline = Instant::now() + wait;
        loop {
            if !set_selection(element, start, text.encode_utf16().count()) {
                return false;
            }
            if wait_until(Duration::from_millis(100), || element.string("AXSelectedText").as_deref() == Some(text)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    };
    // Selects the text an edit checks; for an insertion, then puts the caret in place.
    let select = |place: &Place, wait: Duration| {
        select_text(place.check, &place.text, wait)
            && (place.start != place.end || set_selection(element, place.start, 0))
    };
    let restore = || {
        set_selection(element, location, length)
            && wait_until(Duration::from_millis(200), || selection_unchanged(capture))
    };
    if !select(&places[0], Duration::from_millis(200)) {
        if !restore() {
            trace(format_args!("{}: in place: the selection couldn't be put back", capture.bundle_id));
            return Err(PlatformError::Stale);
        }
        return fallback("the app doesn't let SuperEditr move the selection");
    }
    let native = !capture.web && element.settable("AXSelectedText");
    for (i, (edit, place)) in edits.iter().zip(&places).enumerate() {
        if i > 0 && !wait_until(AFTER_EDIT, || field_focused(capture, element)) {
            return stopped(i, "focus left the field");
        }
        if i > 0 && !select(place, AFTER_EDIT) {
            return stopped(i, "the text to fix no longer matched");
        }
        let value = element.string("AXValue");
        let wrote = if native {
            element.set("AXSelectedText", CFString::new(&edit.new).as_CFTypeRef())
        } else {
            type_text(capture.pid, &edit.new)
        };
        // Landed once the field changes and what was written reads back where it should be.
        let landed = wrote
            && wait_until(AFTER_EDIT, || element.string("AXValue") != value)
            && (edit.new.is_empty() || select_text(place.start, &edit.new, AFTER_EDIT));
        if !landed {
            // Nothing written yet: the whole replace can still go ahead, once the selection is back.
            if i == 0 && element.string("AXValue").as_deref() == Some(before.as_str()) && restore() {
                return fallback("the first change didn't land");
            }
            return stopped(i, "a change didn't land");
        }
    }
    // The caret after the fixed text, where replacing the whole selection leaves it.
    let grown: isize =
        edits.iter().map(|e| e.new.encode_utf16().count() as isize - e.old.encode_utf16().count() as isize).sum();
    set_selection(element, (location + length).saturating_add_signed(grown), 0);
    let verified = verify_replacement(element, capture, Some(&before), text, None, AFTER_EDIT);
    let code = if capture.code.is_empty() {
        String::new()
    } else {
        let visible = |s: &str| s.chars().filter(|c| !is_invisible(*c)).collect::<String>();
        let pieces: Vec<String> = code_in(element).code.iter().map(|p| visible(&p.text)).collect();
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
        "{}: applied in place: {} edits in {} ms by {}; {}{code}",
        capture.bundle_id,
        edits.len(),
        started.elapsed().as_millis(),
        if native { "Accessibility writes" } else { "typing" },
        if verified { "verified" } else { "not verified" },
    ));
    if !verified {
        return Err(PlatformError::Unverified);
    }
    Ok(Some(if native { ReplaceMethod::Accessibility } else { ReplaceMethod::Typing }))
}

/// Whether `pid` is the front app, whether SuperEditr's panel still has focus, and whether the
/// app's window is main and focused, for the log.
fn key_window_state(pid: i32) -> String {
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
}
