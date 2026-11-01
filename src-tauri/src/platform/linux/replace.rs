//! The replace chain: puts a correction where the selection was, or says why it didn't.
//!
//! 1. Refocus the window the text came from, and wait for the desktop to confirm it's in front.
//!    Unconfirmed: copy-only.
//! 2. Read the field again through AT-SPI: the same field with the same selection, or it's stale.
//! 3. Paste, as a clipboard transaction: the user's clipboard aside, the correction in (marked
//!    sensitive), Ctrl+V to that window, the user's clipboard back unless something newer was
//!    copied.
//! 4. Read the field back. Unconfirmed after a paste means the paste may have landed: uncertain,
//!    never retried, so the text is never written twice.
//!
//! Corrections are never written through AT-SPI's editable-text calls, which break undo.

use super::capture::Services;
use super::clipboard::{Limits, Restore, Transaction};
use super::desktop::{Desktop, Shortcut, WindowRef};
use super::{Capture, Source, trace};
use crate::platform::{PlatformError, ReplaceOutcome};
use std::time::{Duration, Instant};

/// How long the desktop gets to bring the window back to the front.
const FOCUS_WAIT: Duration = Duration::from_millis(if cfg!(test) { 60 } else { 600 });
/// How long a paste gets to show up in the field.
const PASTE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 60 } else { 1000 });

pub async fn replace(services: &Services<'_>, capture: &Capture, text: &str) -> ReplaceOutcome {
    let started = Instant::now();
    let outcome = attempt(services, capture, text).await;
    let said = match &outcome {
        ReplaceOutcome::Applied => "applied".to_string(),
        ReplaceOutcome::CopyOnly(why) => format!("copy-only ({why})"),
        ReplaceOutcome::Uncertain(why) => format!("uncertain ({why})"),
    };
    trace(format_args!("replace {}: {said} in {:?}", capture.bundle_id, started.elapsed()));
    outcome
}

async fn attempt(services: &Services<'_>, capture: &Capture, text: &str) -> ReplaceOutcome {
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
    let Ok(transaction) = Transaction::begin(services.clipboard, &limits) else { return copy_only() };
    if !transaction.before().skipped.is_empty() {
        trace(format_args!(
            "replace {}: kept {} clipboard types aside, not {:?}",
            capture.bundle_id,
            transaction.before().types().count(),
            transaction.before().skipped
        ));
    }
    if transaction.put_text(text).is_err() {
        transaction.restore();
        return copy_only();
    }
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

/// Whether the field now holds `text` where the selection began.
async fn landed(services: &Services<'_>, field: &super::atspi::Field, text: &str) -> bool {
    let wanted = text.replace("\r\n", "\n");
    let end = field.start + wanted.chars().count() as i32;
    let started = Instant::now();
    loop {
        if let Ok(now) = services.fields.text(&field.id, field.start, end).await
            && now.replace("\r\n", "\n") == wanted
        {
            return true;
        }
        if started.elapsed() >= PASTE_WAIT {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
