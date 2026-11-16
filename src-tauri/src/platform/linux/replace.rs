//! The replace chain: puts a correction where the selection was, or says why it didn't.
//!
//! 1. Refocus the window the text came from, and wait for the desktop to confirm it's in front.
//!    Unconfirmed: copy-only.
//! 2. Read the field again through AT-SPI: the same field with the same selection, or it's stale.
//! 3. Paste, as a clipboard transaction: the user's clipboard aside, the correction in (marked
//!    sensitive), Ctrl+V to that window, the user's clipboard back unless something newer was
//!    copied. A proofread of formatted text in a web editor is pasted change by change, each over
//!    only the words it fixes, so nothing around them is rewritten; anything else is one paste over
//!    the whole selection, with its formatting as HTML.
//! 4. Read the field back. Unconfirmed after a paste means the paste may have landed: uncertain,
//!    never retried, so the text is never written twice.
//!
//! Corrections are never written through AT-SPI's editable-text calls, which break undo.

use super::atspi::Moved;
use super::capture::Services;
use super::clipboard::{ClipboardError, Limits, Restore, Transaction};
use super::desktop::{Desktop, Shortcut, WindowRef};
use super::{Capture, Source, trace};
use crate::platform::{PlatformError, ReplaceOutcome};
use std::time::{Duration, Instant};
use supereditr_core::rich::Step;

/// How long the desktop gets to bring the window back to the front.
const FOCUS_WAIT: Duration = Duration::from_millis(if cfg!(test) { 60 } else { 600 });
/// How long the app gets to hear of the new clipboard before Ctrl+V. Wayland tells the focused app
/// about a new clipboard with an event of its own: a Ctrl+V straight after it can paste the
/// clipboard before (in the smoke tests, about one paste in 75 pasted nothing that way).
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 60 });
/// How long an editor gets to notice that its selection moved, before a paste over it. One editor
/// (Slate) handles selection changes at most once in 100 ms, and pastes at its old caret until it
/// has.
const SELECTION_SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 200 });
/// How long a paste gets to show up in the field.
const PASTE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 60 } else { 1000 });

pub async fn replace(services: &Services<'_>, capture: &Capture, text: &str, proofread: bool) -> ReplaceOutcome {
    let started = Instant::now();
    let outcome = attempt(services, capture, text, proofread).await;
    let said = match &outcome {
        ReplaceOutcome::Applied => "applied".to_string(),
        ReplaceOutcome::CopyOnly(why) => format!("copy-only ({why})"),
        ReplaceOutcome::Uncertain(why) => format!("uncertain ({why})"),
        ReplaceOutcome::ClipboardKept(why) => format!("clipboard kept ({why})"),
    };
    trace(format_args!("replace {}: {said} in {:?}", capture.bundle_id, started.elapsed()));
    outcome
}

async fn attempt(services: &Services<'_>, capture: &Capture, text: &str, proofread: bool) -> ReplaceOutcome {
    let copy_only = || ReplaceOutcome::CopyOnly(PlatformError::CopyOnly);
    let (Some(window), Some(field)) = (&capture.window, &capture.field) else { return copy_only() };
    if capture.source != Source::Atspi || !capture.editable {
        return copy_only();
    }
    let capabilities = services.desktop.capabilities(window).await;
    if !capabilities.send_keys || !capabilities.focus {
        return copy_only();
    }
    if !bring_back(services.desktop, window).await {
        trace(format_args!("replace {}: the window didn’t come back to the front", capture.bundle_id));
        return copy_only();
    }
    match services.fields.focused(window.pid).await {
        Ok(Some(now))
            if now.id == field.id
                && (now.start, now.end) == (field.start, field.end)
                && now.selected == field.selected => {}
        Ok(_) => return ReplaceOutcome::CopyOnly(PlatformError::Stale),
        Err(_) => return copy_only(),
    }
    let limits = Limits::default();
    let transaction = match Transaction::begin(services.clipboard, &limits) {
        Ok(transaction) => transaction,
        Err(error @ ClipboardError::CantKeep(_)) => {
            return ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(format!(
                "{error}. Copy the suggestion from here."
            )));
        }
        Err(_) => return copy_only(),
    };
    // Formatting the app reported goes back with the text: a web editor makes plain paragraphs of
    // plain text, so a heading, a list, a code block, bold or a link would be lost. An editor that
    // pastes plain text ignores the HTML, and a field without formatting gets none.
    let formatting = field.rich.as_ref().filter(|rich| field.embedded && rich.formatted(&capture.text));
    let html = formatting.and_then(|rich| supereditr_core::rich::html(&capture.text, rich, text));
    let steps = formatting.and_then(|_| steps(capture, field, text, proofread));
    if let Some(steps) = &steps {
        match in_place(services, capture, window, field, steps, &transaction).await {
            InPlace::Untouched => {}
            InPlace::Stopped(outcome) => {
                transaction.restore();
                return outcome;
            }
            InPlace::Done => {
                let landed = landed(services, field, text).await;
                match transaction.restore() {
                    Restore::Restored => {}
                    other => trace(format_args!("paste {}: clipboard {other:?}", capture.bundle_id)),
                }
                return match landed {
                    true => ReplaceOutcome::Applied,
                    false => ReplaceOutcome::Uncertain(PlatformError::Unverified),
                };
            }
        }
    }
    if formatting.is_some() {
        trace(format_args!(
            "paste {}: the formatting goes as {}",
            capture.bundle_id,
            if html.is_some() { "HTML" } else { "plain text: HTML can’t hold its spaces" }
        ));
    }
    if transaction.put_text(text, html.as_deref()).is_err() {
        transaction.restore();
        return copy_only();
    }
    super::wait_for_shortcut_release();
    tokio::time::sleep(CLIPBOARD_SETTLE).await;
    if services.desktop.send_shortcut(window, Shortcut::ctrl("V")).await.is_err() {
        transaction.restore();
        return ReplaceOutcome::CopyOnly(PlatformError::ReplaceFailed);
    }
    let landed = landed(services, field, text).await;
    match transaction.restore() {
        Restore::Restored => {}
        other => trace(format_args!("replace {}: clipboard {other:?}", capture.bundle_id)),
    }
    if landed { ReplaceOutcome::Applied } else { ReplaceOutcome::Uncertain(PlatformError::Unverified) }
}

/// The writes that apply `text` in place, when it is a proofread of a few local changes and each
/// lies in text the read placed. None says to replace the whole selection.
fn steps(capture: &Capture, field: &super::atspi::Field, text: &str, proofread: bool) -> Option<Vec<Step>> {
    let whole = |why: &dyn std::fmt::Display| {
        trace(format_args!("paste {}: the whole selection: {why}", capture.bundle_id));
        None
    };
    let edits = match supereditr_core::edits::plan(&capture.text, text, proofread, &capture.code) {
        Ok(edits) if edits.is_empty() => return None,
        Ok(edits) => edits,
        Err(why) => return whole(&why),
    };
    let root = field.rich.as_ref()?;
    match supereditr_core::rich::steps(&capture.text, root, &field.runs, &edits) {
        Some(steps) if steps.iter().all(|step| step.object < field.objects.len()) => Some(steps),
        _ => whole(&"a change isn’t inside one element’s text"),
    }
}

/// How applying a proofread in place ended.
enum InPlace {
    /// Every change is written.
    Done,
    /// Nothing was written and the selection is as captured: the whole selection can be replaced.
    Untouched,
    Stopped(ReplaceOutcome),
}

/// Applies a proofread as `steps`, last first: each selects the words it changes in the element
/// that holds them, reads them back, pastes over them and reads the result back. Once a step is
/// written there is no going back: a later failure stops and reports a partial fix.
async fn in_place(
    services: &Services<'_>,
    capture: &Capture,
    window: &WindowRef,
    field: &super::atspi::Field,
    steps: &[Step],
    transaction: &Transaction<'_>,
) -> InPlace {
    let started = Instant::now();
    let untouched = |why: &str| {
        trace(format_args!("paste {}: the whole selection: {why}", capture.bundle_id));
        InPlace::Untouched
    };
    let range = |step: &Step| (step.start as i32, step.end as i32);
    // Every place is checked before anything is written.
    for step in steps {
        let (start, end) = range(step);
        let there = services.fields.text(&field.objects[step.object], start, end).await;
        if there.ok().as_deref() != Some(step.old.as_str()) {
            return untouched("the field’s text doesn’t line up with the changes");
        }
    }
    for (i, step) in steps.iter().enumerate() {
        let stopped = |why: &str| {
            trace(format_args!(
                "paste {}: applied in place partly: {i} of {} edits, then {why}",
                capture.bundle_id,
                steps.len()
            ));
            // With nothing applied, the first edit may still have landed unseen.
            InPlace::Stopped(ReplaceOutcome::Uncertain(match i {
                0 => PlatformError::Unverified,
                applied => PlatformError::Partial { applied, total: steps.len() },
            }))
        };
        let id = &field.objects[step.object];
        let (start, end) = range(step);
        let in_front = services.desktop.front_window().is_ok_and(|front| front.handle == window.handle);
        let moved = match in_front {
            true => services.fields.select(id, start, end).await.unwrap_or(Moved::Unconfirmed),
            false => Moved::Refused,
        };
        if moved != Moved::Yes {
            if i > 0 {
                return stopped("the words to fix couldn’t be selected");
            }
            // Nothing is written yet. An app that refused still has the selection as captured, and
            // all of it can be replaced. One that didn't answer in time may move its selection
            // yet, so nothing more is written.
            let captured =
                |now: &super::atspi::Field| now.id == field.id && (now.start, now.end) == (field.start, field.end);
            let kept = moved == Moved::Refused
                && services.fields.focused(window.pid).await.ok().flatten().is_some_and(|now| captured(&now));
            return match kept {
                true => untouched("the app doesn’t let SuperEditr move the selection"),
                false => {
                    trace(format_args!("paste {}: nothing written: the selection may have moved", capture.bundle_id));
                    InPlace::Stopped(ReplaceOutcome::CopyOnly(PlatformError::Stale))
                }
            };
        }
        // An editor can take a moment to notice its selection moved (one waits up to 100 ms), and
        // until then pastes where it had it. So the selection gets time to settle, and must still
        // be on the words right before the paste.
        if i == 0 {
            super::wait_for_shortcut_release();
        }
        if transaction.put_text(&step.new, None).is_err() {
            return stopped("the clipboard didn’t take the fix");
        }
        tokio::time::sleep(SELECTION_SETTLE).await;
        if !services.fields.selected(id, start, end).await {
            // The first paste never goes over anything but the words; later ones can't be undone
            // by replacing everything, so they stop too.
            return match i {
                0 => {
                    trace(format_args!("paste {}: nothing written: the selection moved again", capture.bundle_id));
                    InPlace::Stopped(ReplaceOutcome::CopyOnly(PlatformError::Stale))
                }
                _ => stopped("the selection moved off the words to fix"),
            };
        }
        if services.desktop.send_shortcut(window, Shortcut::ctrl("V")).await.is_err() {
            return stopped("the paste couldn’t be sent");
        }
        let written = start + step.new.chars().count() as i32;
        let pasted = Instant::now();
        loop {
            if services.fields.text(id, start, written).await.is_ok_and(|now| now == step.new) {
                break;
            }
            if pasted.elapsed() >= PASTE_WAIT {
                return stopped("a change didn’t land");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // The caret after the fixed text, where replacing the whole selection leaves it.
    if let Some((object, end)) = supereditr_core::rich::end_after(&field.runs, steps)
        && let Some(id) = field.objects.get(object)
    {
        let _ = services.fields.select(id, end as i32, end as i32).await;
    }
    trace(format_args!(
        "paste {}: applied in place: {} edits in {} ms",
        capture.bundle_id,
        steps.len(),
        started.elapsed().as_millis()
    ));
    InPlace::Done
}

/// Focuses `window` if it isn't in front, and waits until the desktop says it is.
async fn bring_back(desktop: &dyn Desktop, window: &WindowRef) -> bool {
    let in_front = || desktop.front_window().is_ok_and(|front| front.handle == window.handle);
    if in_front() {
        return true;
    }
    if desktop.focus(window).await.is_err() {
        return false;
    }
    let started = Instant::now();
    while started.elapsed() < FOCUS_WAIT {
        if in_front() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    false
}

/// Whether the field now holds `text` where the selection began. In a field of embedded objects
/// (a web editor's paragraphs), offsets move when a paste adds or removes paragraphs, and an editor
/// may turn an empty line into none: there, the field's words must hold the text's words.
async fn landed(services: &Services<'_>, field: &super::atspi::Field, text: &str) -> bool {
    let wanted = text.replace("\r\n", "\n");
    let end = field.start + wanted.chars().count() as i32;
    let started = Instant::now();
    loop {
        let found = if field.embedded {
            services.fields.whole_text(field).await.is_ok_and(|now| words(&now).contains(&words(&wanted)))
        } else {
            services.fields.text(&field.id, field.start, end).await.is_ok_and(|now| now.replace("\r\n", "\n") == wanted)
        };
        if found {
            return true;
        }
        if started.elapsed() >= PASTE_WAIT {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `text`'s words, one space apart, without the zero-width characters editors put around code.
pub(super) fn words(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !matches!(c, '\u{200B}' | '\u{2060}' | '\u{FEFF}')).collect();
    visible.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn captured(world: &World) -> Capture {
        world.capture(true, false).unwrap()
    }

    #[test]
    fn a_paste_that_lands_is_applied_and_the_clipboard_comes_back() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).pasting();
        let capture = captured(&world);
        assert!(matches!(world.replace(&capture, "They’re going"), ReplaceOutcome::Applied));
        assert_eq!(world.keys_sent(), ["CTRL V"]);
        assert_eq!(world.field_text(), "They’re going too the store tomorow.");
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
        assert!(world.clipboard.types().contains(&"text/html".to_string()));
    }

    #[test]
    fn the_window_is_brought_back_before_anything_is_written() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).pasting();
        let capture = captured(&world);
        world.put_panel_in_front();
        assert!(matches!(world.replace(&capture, "They’re going"), ReplaceOutcome::Applied));
        assert_eq!(world.focus_requests(), 1);
    }

    #[test]
    fn a_window_that_doesnt_come_back_is_copy_only_and_nothing_is_written() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).pasting().refusing_focus();
        let capture = captured(&world);
        world.put_panel_in_front();
        assert!(matches!(world.replace(&capture, "They’re going"), ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_offers(), 0);
    }

    #[test]
    fn a_clipboard_too_big_to_put_aside_is_kept_and_nothing_is_pasted() {
        use super::super::clipboard::DataControl;
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).pasting();
        let capture = captured(&world);
        let image = vec![7u8; (16 << 20) + 1];
        world.clipboard.offer(vec![("image/png".into(), image.clone())]).unwrap();
        world.clear_keys();
        let outcome = world.replace(&capture, "They’re going");
        assert!(matches!(outcome, ReplaceOutcome::ClipboardKept(PlatformError::ClipboardNotKept(_))), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard.get("image/png").unwrap(), image);
    }

    #[test]
    fn a_changed_selection_is_stale() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).pasting();
        let capture = captured(&world);
        world.select(0, 5);
        assert!(matches!(world.replace(&capture, "They’re going"), ReplaceOutcome::CopyOnly(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_paste_that_cant_be_confirmed_is_uncertain_and_not_tried_again() {
        // Ctrl+V reaches the window, but the field never shows the new text.
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going"));
        let capture = captured(&world);
        let outcome = world.replace(&capture, "They’re going");
        assert!(matches!(outcome, ReplaceOutcome::Uncertain(PlatformError::Unverified)));
        assert_eq!(world.keys_sent(), ["CTRL V"], "one paste, never a second");
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
    }

    #[test]
    fn a_paste_into_a_web_editors_paragraphs_is_checked_by_its_words() {
        let mut paragraphs = field("Their going\n\nSee you soon.");
        paragraphs.embedded = true;
        let world = World::new("chromium").with_field(paragraphs).pasting();
        let capture = captured(&world);
        assert!(matches!(world.replace(&capture, "They’re going\n\nSee you soon."), ReplaceOutcome::Applied));
    }

    #[test]
    fn formatting_in_a_web_editor_is_pasted_as_html_too() {
        use supereditr_core::rich::{Object, Piece, assemble};
        let block = |tag: &str, run: &str| {
            Piece::Object(Object {
                tag: tag.into(),
                block: true,
                pieces: vec![Piece::Text(run.into())],
                ..Object::default()
            })
        };
        let read =
            assemble(&[block("h2", "Their going"), block("pre", "npm ci\nnpm test"), block("p", "See you soon.")]);
        let mut editor = field(&read.text);
        editor.embedded = true;
        editor.code = read.root.code();
        editor.rich = Some(read.root);
        let world = World::new("chromium").with_field(editor).pasting();
        let capture = captured(&world);
        let fixed = "They’re going\nnpm ci\nnpm test\nSee you soon.";
        assert!(matches!(world.replace(&capture, fixed), ReplaceOutcome::Applied));
        assert_eq!(
            world.pasted_html().as_deref(),
            Some(
                "<meta charset=\"utf-8\"><h2>They’re going</h2><pre><code>npm ci\nnpm test</code></pre><p>See you soon.</p>"
            )
        );
        assert!(world.field_text().starts_with(fixed), "the plain text is the fix as it is");
        assert_eq!(world.clipboard.get("text/html").unwrap(), b"<b>Rhyan's</b> own copy", "the user's HTML is back");
    }

    /// A heading and a code block in a web editor, as read: the fake app holds all of its text in
    /// one element, so each run is where it is in the text.
    fn formatted(heading: &str, code: &str) -> super::super::atspi::Field {
        use supereditr_core::rich::{Object, Piece, assemble};
        let block = |tag: &str, run: &str, offset: usize| {
            let pieces = vec![Piece::Run { text: run.into(), object: 0, offset }];
            Piece::Object(Object { tag: tag.into(), block: true, pieces, ..Object::default() })
        };
        let read = assemble(&[block("h2", heading, 0), block("pre", code, heading.chars().count() + 1)]);
        let mut editor = field(&read.text);
        editor.embedded = true;
        editor.code = read.root.code();
        editor.rich = Some(read.root);
        editor.runs = read.runs;
        editor.objects = vec![editor.id.clone()];
        editor
    }

    #[test]
    fn a_proofread_of_formatted_text_is_pasted_change_by_change() {
        let world = World::new("chromium").with_field(formatted("Their going realy soon", "npm ci")).pasting();
        let capture = captured(&world);
        let fixed = "They’re going really soon\nnpm ci";
        assert!(matches!(world.proofread(&capture, fixed), ReplaceOutcome::Applied));
        assert_eq!(world.keys_sent(), ["CTRL V", "CTRL V"], "one paste for each change");
        assert_eq!(world.pasted_html(), None, "only the changed words are pasted, as plain text");
        assert!(world.field_text().starts_with(fixed), "{}", world.field_text());
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
    }

    #[test]
    fn a_rewrite_or_an_app_that_keeps_its_selection_gets_the_whole_selection() {
        let fixed = "They’re going really soon\nnpm ci";
        let world = World::new("chromium").with_field(formatted("Their going realy soon", "npm ci")).pasting();
        let capture = captured(&world);
        assert!(matches!(world.replace(&capture, fixed), ReplaceOutcome::Applied));
        assert_eq!(world.keys_sent(), ["CTRL V"]);
        assert!(world.pasted_html().is_some_and(|html| html.contains("<h2>They’re going really soon</h2>")));

        let world = World::new("chromium")
            .with_field(formatted("Their going realy soon", "npm ci"))
            .pasting()
            .answering_a_selection(Moved::Refused);
        let capture = captured(&world);
        assert!(matches!(world.proofread(&capture, fixed), ReplaceOutcome::Applied));
        assert_eq!(world.keys_sent(), ["CTRL V"], "one paste of everything");
        assert!(world.field_text().starts_with(fixed), "{}", world.field_text());
    }

    #[test]
    fn a_selection_that_may_still_move_is_never_pasted_over() {
        // The app took the request to select the first words but hasn't shown it in time: the
        // selection may move at any moment, so neither the words nor the whole text is pasted.
        let world = World::new("chromium")
            .with_field(formatted("Their going realy soon", "npm ci"))
            .pasting()
            .answering_a_selection(Moved::Unconfirmed);
        let capture = captured(&world);
        let outcome = world.proofread(&capture, "They’re going really soon\nnpm ci");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::Stale)), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
        assert!(world.field_text().starts_with("Their going realy soon"));
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
    }

    #[test]
    fn a_selection_that_moves_off_the_words_isnt_pasted_over() {
        let world = World::new("chromium")
            .with_field(formatted("Their going realy soon", "npm ci"))
            .pasting()
            .moving_its_selection_back();
        let capture = captured(&world);
        let outcome = world.proofread(&capture, "They’re going really soon\nnpm ci");
        assert!(matches!(outcome, ReplaceOutcome::CopyOnly(PlatformError::Stale)), "{outcome:?}");
        assert!(world.keys_sent().is_empty());
        assert!(world.field_text().starts_with("Their going realy soon"));
    }

    #[test]
    fn a_field_without_formatting_gets_plain_text_only() {
        use supereditr_core::rich::{Object, Piece, assemble};
        let paragraph = |run: &str| {
            Piece::Object(Object {
                tag: "p".into(),
                block: true,
                pieces: vec![Piece::Text(run.into())],
                ..Object::default()
            })
        };
        let read = assemble(&[paragraph("Their going"), paragraph("\n"), paragraph("See you soon.")]);
        let mut paragraphs = field(&read.text);
        paragraphs.embedded = true;
        paragraphs.rich = Some(read.root);
        let world = World::new("chromium").with_field(paragraphs).pasting();
        let capture = captured(&world);
        assert!(matches!(world.replace(&capture, "They’re going\n\nSee you soon."), ReplaceOutcome::Applied));
        assert_eq!(world.pasted_html(), None);
    }

    #[test]
    fn words_ignore_line_breaks_and_the_spaces_around_code() {
        assert_eq!(words("\u{200B}teh fix\u{200B} first.\n\nSee  you"), "teh fix first. See you");
    }

    #[test]
    fn copy_only_captures_are_never_pasted() {
        let world = World::new("foot").copying("make deploy");
        let capture = captured(&world);
        world.clear_keys();
        assert!(matches!(
            world.replace(&capture, "make deploy-all"),
            ReplaceOutcome::CopyOnly(PlatformError::CopyOnly)
        ));
        assert!(world.keys_sent().is_empty());
    }
}
