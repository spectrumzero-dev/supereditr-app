//! The capture chain: what the user selected, and in which window.
//!
//! 1. AT-SPI: the focused field's selection, role and editable state. Only this can make a capture
//!    editable, so only text read here is ever replaced.
//! 2. A guarded copy, for a shortcut press only: the user's clipboard is put aside, the copy key is
//!    sent to the window in front, the copy is read, and the clipboard is given back. The result is
//!    copy-only.
//! 3. Nothing else. The primary selection is never read: it can't be tied to the current selection.
//!
//! An automatic review (no shortcut) never copies, so nothing is read from a field AT-SPI hasn't
//! identified, and nothing at all is read in a password manager.

use super::apps::{self, Profile, Write};
use super::atspi::{Field, Fields, ReadError};
use super::clipboard::{DataControl, Limits, Transaction};
use super::desktop::{Desktop, Shortcut, WindowRef};
use super::{Capture, Source, trace};
use crate::platform::{PlatformError, Rect};
use std::time::{Duration, Instant};

/// How long a guarded copy waits for the app to copy. Chromium copies in about 15 ms and Obsidian
/// in about 45; no copy by then means nothing was selected.
const COPY_WAIT: Duration = Duration::from_millis(if cfg!(test) { 40 } else { 400 });

/// How long reading the focused field may take in all, so a frozen app is given up on in time for
/// the guarded copy and an answer within about two seconds.
const FIELD_LIMIT: Duration = Duration::from_millis(if cfg!(test) { 200 } else { 1500 });

/// The services a chain uses.
pub struct Services<'a> {
    pub desktop: &'a dyn Desktop,
    pub fields: &'a dyn Fields,
    pub clipboard: &'a dyn DataControl,
}

pub async fn capture(
    services: &Services<'_>,
    allow_copy: bool,
    select_sentence: bool,
) -> Result<Capture, PlatformError> {
    let how = if allow_copy { "shortcut" } else { "automatic" };
    let started = Instant::now();
    let window = services.desktop.front_window().map_err(|error| {
        trace(format_args!("{how}: no window in front ({error})"));
        PlatformError::Other("SuperEditr can’t tell which window is in front.".into())
    })?;
    let profile = apps::profile(&window.app_id, &window.tags);
    let result = read(services, &window, profile, allow_copy, select_sentence).await;
    match &result {
        Ok(capture) => trace(format_args!(
            "{how} {}: read {} characters via {} in {:?}; {}",
            window.app_id,
            capture.text.chars().count(),
            match capture.source {
                Source::Atspi => "AT-SPI",
                Source::Copy => "a guarded copy",
                Source::Playground => "Settings",
            },
            started.elapsed(),
            if capture.editable { "pasting allowed" } else { "copy-only" },
        )),
        Err(error) => trace(format_args!("{how} {}: {error} ({:?})", window.app_id, started.elapsed())),
    }
    result
}

async fn read(
    services: &Services<'_>,
    window: &WindowRef,
    profile: Profile,
    allow_copy: bool,
    select_sentence: bool,
) -> Result<Capture, PlatformError> {
    if profile.password_manager {
        return Err(PlatformError::SecureField);
    }
    let mut not_answering = false;
    let field = match tokio::time::timeout(FIELD_LIMIT, services.fields.focused(window.pid)).await {
        Ok(Ok(field)) => field,
        Err(_) => {
            trace(format_args!("{}: AT-SPI didn’t read the focused field in {FIELD_LIMIT:?}", window.app_id));
            not_answering = true;
            None
        }
        Ok(Err(error)) => {
            trace(format_args!("{}: AT-SPI couldn’t read the focused field ({error})", window.app_id));
            not_answering = matches!(error, ReadError::Timeout);
            None
        }
    };
    let field = match field {
        Some(field) if field.password => return Err(PlatformError::SecureField),
        Some(field) if !field.has_selection() && select_sentence && field.editable && profile.write == Write::Paste => {
            match services.fields.select_sentence(&field).await {
                Ok(Some(selected)) => Some(selected),
                _ => Some(field),
            }
        }
        other => other,
    };
    match field {
        Some(field) if field.has_selection() && !field.partial() => {
            let anchor = anchor(window, &field, services.desktop.cursor().ok());
            return Ok(Capture::read(window.clone(), field, profile, anchor));
        }
        // A field that exposes its text, with nothing selected in it.
        Some(field) if !field.partial() => return Err(PlatformError::NoSelection),
        _ => {}
    }
    // Fail closed: without a shortcut press, a field AT-SPI can't read isn't read at all.
    if !allow_copy {
        return Err(PlatformError::NoSelection);
    }
    match guarded_copy(services, window, profile).await {
        // A frozen app neither answers AT-SPI nor copies: that's no sign nothing is selected.
        Err(PlatformError::NoSelection) if not_answering => Err(PlatformError::Other(format!(
            "{} isn’t responding, so SuperEditr couldn’t read the selection.",
            apps::display_name(&window.app_id)
        ))),
        result => result,
    }
}

async fn guarded_copy(services: &Services<'_>, window: &WindowRef, profile: Profile) -> Result<Capture, PlatformError> {
    let cant_read = || {
        PlatformError::Other(format!(
            "SuperEditr can’t read the selection in {}. Copy it, and SuperEditr can review it from Settings.",
            apps::display_name(&window.app_id)
        ))
    };
    if !services.desktop.capabilities(window).await.send_keys {
        return Err(cant_read());
    }
    let limits = Limits::default();
    let transaction = Transaction::begin(services.clipboard, &limits).map_err(|_| cant_read())?;
    transaction.put_marker().map_err(|_| cant_read())?;
    // Terminals take Ctrl+C as an interrupt; Ctrl+Insert copies there, as Omarchy's own copy key does.
    let key = Shortcut::ctrl(if profile.terminal { "Insert" } else { "C" });
    super::wait_for_shortcut_release();
    if services.desktop.send_shortcut(window, key).await.is_err() {
        transaction.restore();
        return Err(cant_read());
    }
    if !transaction.wait_for_copy(COPY_WAIT) {
        transaction.restore();
        return Err(PlatformError::NoSelection);
    }
    let text = transaction.read_text(&limits);
    transaction.restore_after_copy();
    match text {
        Ok(text) if !text.trim().is_empty() => {
            let anchor = services.desktop.cursor().ok().filter(|&point| inside(window, point)).map(point_rect);
            Ok(Capture::copied(window.clone(), text, anchor))
        }
        Ok(_) => Err(PlatformError::NoSelection),
        Err(_) => Err(cant_read()),
    }
}

/// Where the panel goes: the selection, when the app reports a position inside its window,
/// otherwise the pointer when it's over the window, otherwise near the window's top-left corner.
/// Positions from a Wayland app are relative to its window (Firefox reports none at all); only an
/// XWayland app's screen coordinates are global.
fn anchor(window: &WindowRef, field: &Field, cursor: Option<(f64, f64)>) -> Option<Rect> {
    let offset = |b: Rect| window.rect.map(|r| Rect { x: r.x + b.x, y: r.y + b.y, ..b });
    let selection = match (field.bounds, field.window_bounds) {
        (Some(screen), _) if window.xwayland => Some(screen),
        (Some(relative), _) | (None, Some(relative)) => offset(relative),
        (None, None) => None,
    };
    if let Some(bounds) = selection.filter(|b| inside(window, (b.x, b.y))) {
        return Some(bounds);
    }
    if let Some(point) = cursor.filter(|&point| inside(window, point)) {
        return Some(point_rect(point));
    }
    window.rect.map(|r| Rect { x: r.x + 24.0, y: r.y + 48.0, width: 1.0, height: 20.0 })
}

fn inside(window: &WindowRef, (x, y): (f64, f64)) -> bool {
    window.rect.is_some_and(|r| x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height)
}

fn point_rect((x, y): (f64, f64)) -> Rect {
    Rect { x: x - 24.0, y: y - 10.0, width: 1.0, height: 20.0 }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn a_selection_read_through_atspi_can_be_replaced() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going"));
        let capture = world.capture(true, false).unwrap();
        assert_eq!(capture.source, Source::Atspi);
        assert!(capture.editable);
        assert_eq!(capture.text, "Their going");
        assert!(world.keys_sent().is_empty(), "AT-SPI needs no keys");
    }

    #[test]
    fn only_an_atspi_capture_is_editable() {
        // Read through AT-SPI, but in an app profiled copy-only.
        let world = World::new("com.mitchellh.ghostty").with_field(field("ls -la"));
        let capture = world.capture(true, false).unwrap();
        assert_eq!(capture.source, Source::Atspi);
        assert!(!capture.editable, "terminals are copy-only");

        // A field that isn't editable, such as a web page's text.
        let mut text = field("Kyndal's OKR draft");
        text.editable = false;
        assert!(!World::new("chromium").with_field(text).capture(true, false).unwrap().editable);

        // A guarded copy, in an app that would take pastes if AT-SPI could read it.
        let world = World::new("chromium").copying("Their going");
        let capture = world.capture(true, false).unwrap();
        assert_eq!(capture.source, Source::Copy);
        assert!(!capture.editable);
    }

    #[test]
    fn without_atspi_a_shortcut_copies_and_gives_the_clipboard_back() {
        let world = World::new("chromium").copying("Their going too the store");
        let capture = world.capture(true, false).unwrap();
        assert_eq!(capture.text, "Their going too the store");
        assert_eq!(world.keys_sent(), ["CTRL C"]);
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
        assert!(world.clipboard.types().contains(&"text/html".to_string()), "every type comes back");
    }

    #[test]
    fn a_terminal_copies_with_ctrl_insert() {
        let world = World::new("foot").copying("make deploy");
        assert!(!world.capture(true, false).unwrap().editable);
        assert_eq!(world.keys_sent(), ["CTRL Insert"]);
    }

    #[test]
    fn nothing_selected_means_no_capture_and_the_clipboard_comes_back() {
        let world = World::new("foot");
        assert!(matches!(world.capture(true, false), Err(PlatformError::NoSelection)));
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
    }

    #[test]
    fn a_frozen_app_is_said_to_be_frozen_rather_than_to_have_nothing_selected() {
        let world = World::new("org.gnome.TextEditor").with_field(field("Their going")).frozen();
        match world.capture(true, false) {
            Err(PlatformError::Other(message)) => assert!(message.contains("isn’t responding"), "{message}"),
            other => panic!("expected a message about the app, got {other:?}"),
        }
        assert_eq!(world.clipboard.get("text/plain;charset=utf-8").unwrap(), b"Rhyan's own copy");
    }

    #[test]
    fn an_automatic_review_never_copies_from_a_field_atspi_cant_read() {
        let world = World::new("chromium").copying("Their going");
        assert!(matches!(world.capture(false, false), Err(PlatformError::NoSelection)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_offers(), 0, "the clipboard isn't touched");
    }

    #[test]
    fn password_fields_and_managers_are_never_read() {
        let mut secret = field("");
        secret.password = true;
        let world = World::new("chromium").with_field(secret).copying("hunter2");
        assert!(matches!(world.capture(true, false), Err(PlatformError::SecureField)));
        assert!(world.keys_sent().is_empty(), "no copy either");

        let world = World::new("org.keepassxc.KeePassXC").copying("hunter2");
        assert!(matches!(world.capture(true, false), Err(PlatformError::SecureField)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn embedded_objects_fall_back_to_the_guarded_copy() {
        let world = World::new("obsidian").with_field(field("\u{FFFC}\u{FFFC}")).copying("Their going");
        let capture = world.capture(true, false).unwrap();
        assert_eq!((capture.source, capture.text.as_str()), (Source::Copy, "Their going"));
    }

    #[test]
    fn with_nothing_selected_the_sentence_before_the_caret_is_selected() {
        let world =
            || World::new("org.gnome.TextEditor").with_field(field("")).with_sentence(field("Brynlee sent teh notes."));
        assert_eq!(world().capture(true, true).unwrap().text, "Brynlee sent teh notes.");
        let unchanged = world();
        assert!(matches!(unchanged.capture(true, false), Err(PlatformError::NoSelection)));
        assert!(unchanged.keys_sent().is_empty(), "a readable field with nothing selected isn't copied");
    }

    #[test]
    fn a_desktop_without_window_information_reads_nothing() {
        let world = World::new("chromium").copying("Their going").on_fallback();
        assert!(matches!(world.capture(true, false), Err(PlatformError::Other(_))));
    }

    #[test]
    fn the_panel_goes_to_the_selection_only_when_its_position_is_believable() {
        // The window is at (12, 38).
        let mut window = window("org.gnome.TextEditor");
        let mut text = field("Their going");
        // GTK 4 on Wayland: only window coordinates.
        text.window_bounds = Some(Rect { x: 12.0, y: 60.0, width: 9.0, height: 18.0 });
        assert_eq!(anchor(&window, &text, Some((900.0, 700.0))).map(|r| (r.x, r.y)), Some((24.0, 98.0)));
        // Chromium on Wayland: "screen" coordinates relative to the window, ahead of its window
        // coordinates, which are relative to the page.
        text.bounds = Some(Rect { x: 33.0, y: 168.0, width: 465.0, height: 22.0 });
        text.window_bounds = Some(Rect { x: 33.0, y: 81.0, width: 465.0, height: 22.0 });
        assert_eq!(anchor(&window, &text, Some((900.0, 700.0))).map(|r| (r.x, r.y)), Some((45.0, 206.0)));
        text.window_bounds = None;
        // An XWayland app's screen coordinates are global.
        window.xwayland = true;
        assert_eq!(anchor(&window, &text, None).map(|r| (r.x, r.y)), Some((33.0, 168.0)));
        // Firefox: no position at all, so the pointer, when it's over the window.
        window.xwayland = false;
        text.bounds = None;
        assert_eq!(anchor(&window, &text, Some((420.0, 260.0))).map(|r| (r.x, r.y)), Some((396.0, 250.0)));
        // The pointer is outside the window too: near the window's corner.
        assert_eq!(anchor(&window, &text, Some((4000.0, 10.0))).map(|r| (r.x, r.y)), Some((36.0, 86.0)));
    }
}
