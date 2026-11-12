//! The capture chain: what the user selected, in which window, and whether SuperEditr may write it.
//!
//! 1. **Refuse first**: a window of an app running as administrator, a password manager, a password
//!    field, or text an input method is still composing.
//! 2. **UI Automation**: the focused editor's selection, read again for about a second while a
//!    Chromium or Electron app answers with its window instead of the field.
//! 3. **A guarded copy**, on a shortcut press only: the clipboard is put aside, Ctrl+C sent (Ctrl+Insert
//!    in a terminal, where Ctrl+C interrupts), the copy read, and the clipboard given back. Its text
//!    is copy-only. An app that copies after SuperEditr stopped waiting has its copy undone
//!    ([`Clipboard::watch_late_copies`]).
//!
//! A terminal's text, read either way, has the spaces that pad each line to the window's width
//! removed: they are the terminal's layout, not the user's text.
//!
//! Every capture is classed before anything is written: [`Class::KnownSafe`] fields can be applied
//! to, everything else is copy-only.

use super::apps::{self, Profile};
use super::clipboard::{Clipboard, Limits, Raw, Restore};
use super::uia::{Field, Fields, PxRect, ReadError};
use super::window::WindowRef;
use super::{Capture, Source, trace};
use crate::platform::{PlatformError, Rect};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long reading the focused field may take in all, retries included. A warm read takes a few
/// milliseconds; a Chromium app answers with its window for the first 25–370 ms. One call may take
/// this long too: an app that hasn't answered by then isn't responding.
const READ_BUDGET: Duration = Duration::from_millis(if cfg!(test) { 150 } else { 1000 });
/// The budget of the first read in a Chromium or Firefox process while it keeps answering without
/// its field: it builds its accessibility tree on the first request. A freshly started Chrome on a
/// test machine without a GPU took over a second.
const FIRST_READ_BUDGET: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 3000 });
/// A read isn't started with less time than this left: it would only time out, and an app that is
/// answering would be reported as not responding.
const LEAST_CALL: Duration = Duration::from_millis(if cfg!(test) { 10 } else { 100 });
/// How long an app that isn't Chromium or Firefox gets to show a field: those answer at once or
/// not at all (LibreOffice shows only its window).
const OTHER_BUDGET: Duration = Duration::from_millis(if cfg!(test) { 30 } else { 200 });
/// Between reads, while an app wakes its accessibility up.
const RETRY: Duration = Duration::from_millis(if cfg!(test) { 2 } else { 25 });
/// How long a guarded copy waits for the app to copy, per attempt (PowerToys polls 5 × 30 ms).
const COPY_WAIT: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 150 });
/// From this many lines, a selection read through UI Automation is checked against a copy: Monaco
/// shows accessibility only part of a long selection.
const CHECKED_LINES: usize = 100;

/// The window and process side of the desktop.
pub trait Desktop: Send + Sync {
    fn foreground(&self) -> Option<WindowRef>;
    fn image_name(&self, pid: u32) -> Option<String>;
    fn window_class(&self, window: WindowRef) -> String;
    fn elevated(&self, pid: u32) -> bool;
    fn alive(&self, window: WindowRef) -> bool;
    /// Asks Windows to bring `window` to the front; true when it took the request.
    fn bring_to_front(&self, window: WindowRef) -> bool;
    /// Waits briefly for the user to let go of the shortcut's keys, then releases any still down.
    fn release_modifiers(&self);
    fn send_ctrl(&self, key: Key) -> bool;
    fn caret(&self, window: WindowRef) -> Option<PxRect>;
    fn cursor(&self) -> Option<(i32, i32)>;
    /// Starts watching for the user pressing a copy key themselves (Ctrl+C, Ctrl+X, Ctrl+Insert),
    /// not counting the keys SuperEditr sends. The watch ends when what it returns is dropped.
    fn watch_copy_keys(&self) -> InputWatch;
}

/// Whether the user has pressed a copy key since the watch started.
pub type InputWatch = Box<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    C,
    V,
    Insert,
}

pub struct Services<'a, R: Raw> {
    pub desktop: &'a dyn Desktop,
    pub fields: &'a dyn Fields,
    pub clipboard: &'a Clipboard<R>,
}

/// Why a field is copy-only or refused, for capture.log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Terminal,
    NoPaste,
    ReadOnly,
    Recipients,
    Ranges,
    Password,
    Composing,
}

/// What SuperEditr may do with a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Read through UI Automation, writable, not a password, nothing composing, in an app whose
    /// paste was tested: a fix can be applied.
    KnownSafe,
    /// Readable, but a fix goes on the clipboard.
    CopyOnly(Why),
    /// Not read or written at all.
    Refused(Why),
}

/// Classes a field read through UI Automation.
pub fn classify(profile: &Profile, field: &Field) -> Class {
    if field.password || profile.password_manager {
        return Class::Refused(Why::Password);
    }
    if field.composition {
        return Class::Refused(Why::Composing);
    }
    if profile.terminal || apps::terminal_element(&field.class) {
        return Class::CopyOnly(Why::Terminal);
    }
    if field.ranges != 1 {
        return Class::CopyOnly(Why::Ranges);
    }
    if field.read_only != Some(false) {
        return Class::CopyOnly(Why::ReadOnly);
    }
    if apps::recipient_field(field.control_type, &field.class, &field.name) {
        return Class::CopyOnly(Why::Recipients);
    }
    if !profile.pastes {
        return Class::CopyOnly(Why::NoPaste);
    }
    Class::KnownSafe
}

pub fn capture<R: Raw>(services: &Services<'_, R>, allow_copy: bool) -> Result<Capture, PlatformError> {
    let how = if allow_copy { "shortcut" } else { "automatic" };
    let started = Instant::now();
    let window = services.desktop.foreground().ok_or_else(|| {
        trace(format_args!("{how}: no window in front"));
        PlatformError::Other("SuperEditr can’t tell which window is in front.".into())
    })?;
    let image = services.desktop.image_name(window.pid).unwrap_or_default();
    let result = read(services, window, &image, allow_copy);
    match &result {
        Ok(capture) => trace(format_args!(
            "{how} {image}: read {} characters via {}{} in {:?}; {}",
            capture.text.chars().count(),
            match capture.source {
                Source::Uia => "UI Automation",
                Source::Copy => "a guarded copy",
                Source::Playground => "Settings",
            },
            capture.field.as_ref().map_or_else(String::new, |f| format!(
                " ({} {} element of process {}, type {})",
                f.framework, f.class, f.pid, f.control_type
            )),
            started.elapsed(),
            capture.class.map_or_else(|| "copy-only".to_string(), |class| format!("{class:?}")),
        )),
        Err(error) => trace(format_args!("{how} {image}: {error} ({:?})", started.elapsed())),
    }
    result
}

fn read<R: Raw>(
    services: &Services<'_, R>,
    window: WindowRef,
    image: &str,
    allow_copy: bool,
) -> Result<Capture, PlatformError> {
    let name = apps::display_name(image);
    if services.desktop.elevated(window.pid) {
        // SuperEditr can't read such a program's name either.
        let app = if image.is_empty() { "This app".to_string() } else { name };
        return Err(PlatformError::Other(format!(
            "{app} is running as administrator, so SuperEditr can’t read or change its text."
        )));
    }
    let class = services.desktop.window_class(window);
    let profile = apps::profile(image, &class);
    if profile.password_manager {
        return Err(PlatformError::SecureField);
    }
    let mut not_answering = false;
    let mut terminal = profile.terminal;
    let field = match read_field(services.fields, profile, window.pid) {
        Ok(field) => Some(field),
        Err(ReadError::Password) => return Err(PlatformError::SecureField),
        Err(ReadError::Call(error)) => {
            trace(format_args!("{image}: UI Automation didn’t answer ({error:?})"));
            not_answering = true;
            None
        }
        Err(error) => {
            trace(format_args!("{image}: no readable field ({error:?})"));
            None
        }
    };
    // A read can take a few seconds. If another window came to the front meanwhile, the field read
    // may be that window's, which this app's profile and exclusions don't cover.
    if services.desktop.foreground() != Some(window) {
        trace(format_args!("{image}: another window came to the front during the read"));
        return Err(PlatformError::Stale);
    }
    // The user may have switched away and back while the read ran, and the field read be the other
    // app's. The field's own window must sit inside the captured one, and a field whose window
    // can't be found doesn't count. Its process needn't be the window's: a WebView2 page, as in the
    // new Outlook, runs in a child process.
    if let Some(field) = &field
        && field.root_window.is_none_or(|root| root != window.hwnd)
    {
        trace(format_args!("{image}: the field read is in another window"));
        return Err(PlatformError::Stale);
    }
    let capture = |field: Field, class: Class| {
        let bounds = anchor(services.desktop, window, field.bounds.or(field.element_bounds));
        let mut capture = Capture::read(window, image, field, class, bounds);
        if class == Class::CopyOnly(Why::Terminal) {
            capture.text = terminal_text(&capture.text);
        }
        capture
    };
    if let Some(field) = field {
        terminal |= apps::terminal_element(&field.class);
        let class = classify(&profile, &field);
        match class {
            Class::Refused(Why::Password) => return Err(PlatformError::SecureField),
            Class::Refused(_) => return Err(PlatformError::Composing),
            _ => {}
        }
        let blank = match class {
            Class::CopyOnly(Why::Terminal) => terminal_text(&String::from_utf16_lossy(&field.text)).is_empty(),
            _ => field.text.is_empty(),
        };
        if !blank {
            let lines = String::from_utf16_lossy(&field.text).lines().count();
            if class == Class::KnownSafe && (profile.code_editor || lines >= CHECKED_LINES) {
                // Checked against a copy: a paste over a selection shown only in part would delete
                // the rest.
                if !allow_copy {
                    return Err(PlatformError::NoSelection);
                }
                let copied = guarded_copy(services, window, image, terminal)?;
                if words(&copied.text) != words(&String::from_utf16_lossy(&field.text)) {
                    trace(format_args!("{image}: UI Automation shows only part of the selection"));
                    return Ok(copied);
                }
            }
            return Ok(capture(field, class));
        }
        // A readable field with nothing selected. A code editor can hide its text from
        // accessibility (VS Code does unless `editor.accessibilitySupport` is on), and a terminal
        // panel's selection isn't in its field, so those get a copy.
        if !profile.code_editor && !terminal {
            return Err(PlatformError::NoSelection);
        }
    }
    // Fail closed: without a shortcut press, nothing UI Automation can't read is read at all.
    if !allow_copy {
        return Err(PlatformError::NoSelection);
    }
    match guarded_copy(services, window, image, terminal) {
        Err(PlatformError::NoSelection) if not_answering => {
            Err(PlatformError::Other(format!("{name} isn’t responding, so SuperEditr couldn’t read the selection.")))
        }
        result => result,
    }
}

/// Processes a field was read in. The first read in any other process gets longer too: an app UI
/// Automation hasn't spoken to yet can take a few hundred milliseconds to answer, as a freshly started
/// Notepad did on the test machine, and the review would otherwise fall back to a copy.
static READ_IN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Reads the focused field, again and again within the budget while the app answers with something
/// that isn't a field yet.
fn read_field(fields: &dyn Fields, profile: Profile, pid: u32) -> Result<Field, ReadError> {
    let started = Instant::now();
    let browser = profile.chromium || profile.gecko;
    let first = !READ_IN.lock().unwrap_or_else(|p| p.into_inner()).contains(&pid);
    let (budget, call) = match (browser, first) {
        (true, true) => (FIRST_READ_BUDGET, READ_BUDGET),
        (true, false) | (false, true) => (READ_BUDGET, READ_BUDGET),
        (false, false) => (OTHER_BUDGET, OTHER_BUDGET),
    };
    let mut tries = 0;
    loop {
        tries += 1;
        let left = budget.saturating_sub(started.elapsed());
        let result = fields.focused(left.min(call));
        let again = match &result {
            Ok(field) => field.text.is_empty() && browser && tries < 3,
            Err(ReadError::NoText | ReadError::Gone | ReadError::Failed(_)) => true,
            Err(_) => false,
        };
        if !again || started.elapsed() + RETRY + LEAST_CALL >= budget {
            if tries > 1 {
                trace(format_args!(
                    "read the field after {tries} tries in {:?}{}",
                    started.elapsed(),
                    if first { ", the first read in this process" } else { "" }
                ));
            }
            if result.is_ok() && first {
                let mut read_in = READ_IN.lock().unwrap_or_else(|p| p.into_inner());
                if read_in.len() >= 64 {
                    read_in.remove(0);
                }
                read_in.push(pid);
            }
            return result;
        }
        std::thread::sleep(RETRY);
    }
}

/// A terminal's text without the spaces that pad each line, and without the empty lines below the
/// last one written.
fn terminal_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix("\r\n") {
            Some(body) => (body, "\r\n"),
            None => match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            },
        };
        out.push_str(body.trim_end_matches([' ', '\t', '\u{a0}']));
        out.push_str(end);
    }
    out.truncate(out.trim_end().len());
    out
}

fn guarded_copy<R: Raw>(
    services: &Services<'_, R>,
    window: WindowRef,
    image: &str,
    terminal: bool,
) -> Result<Capture, PlatformError> {
    let name = apps::display_name(image);
    let snapshot = services.clipboard.snapshot(&Limits::default()).map_err(|why| {
        PlatformError::Other(format!("SuperEditr can’t read the selection in {name}: {why}. Copy it, then review it."))
    })?;
    let key = if terminal { Key::Insert } else { Key::C };
    services.desktop.release_modifiers();
    // From the first copy key on: a copy key the user presses themselves means a later copy from the
    // app may be theirs, which the late-copy watch then leaves alone.
    let user_copied = services.desktop.watch_copy_keys();
    let mut copied = None;
    let mut presses = 0;
    let mut moved = false;
    // Two tries, as PowerToys does: an app busy at the first may take the second.
    for _ in 0..2 {
        // The copy key goes to whatever window is in front: only ever the one being reviewed.
        if services.desktop.foreground() != Some(window) {
            moved = true;
            break;
        }
        if !services.desktop.send_ctrl(key) {
            break;
        }
        presses += 1;
        let started = Instant::now();
        while started.elapsed() < COPY_WAIT {
            if services.clipboard.sequence() != snapshot.sequence {
                copied = Some(services.clipboard.sequence());
                break;
            }
            std::thread::sleep(Duration::from_millis(if cfg!(test) { 1 } else { 10 }));
        }
        if copied.is_some() {
            break;
        }
    }
    let Some(copy) = copied else {
        // A busy app can still take the Ctrl+C: its copy would replace the user's clipboard.
        if presses > 0 {
            trace(format_args!("{image}: no copy in time; watching for a late one"));
        }
        services.clipboard.watch_late_copies(snapshot, name.clone(), presses, user_copied);
        if moved {
            trace(format_args!("{image}: another window came to the front before the copy"));
            return Err(PlatformError::Stale);
        }
        return Err(match terminal {
            true => PlatformError::Other(format!("Nothing was selected in {name}.")),
            false => PlatformError::NoSelection,
        });
    };
    // Settle: an app can write several formats in a row.
    std::thread::sleep(Duration::from_millis(if cfg!(test) { 0 } else { 30 }));
    let text = services.clipboard.text();
    let now = services.clipboard.sequence();
    // The user pressed a copy key themselves while SuperEditr waited: the copy may be theirs, so
    // the clipboard stays as it is, and what's on it can't be taken for the selection SuperEditr
    // asked for.
    if user_copied() {
        trace(format_args!("{image}: the user copied during the guarded copy; their clipboard stays"));
        return Err(PlatformError::Stale);
    }
    match services.clipboard.restore(&snapshot, now) {
        // The first Ctrl+C may have been the one copied, with the second's copy still to come.
        Restore::Restored => services.clipboard.watch_late_copies(snapshot, name.clone(), presses - 1, user_copied),
        other => trace(format_args!("{image}: after the copy, the clipboard {other:?} (copied {copy}, now {now})")),
    }
    if services.desktop.foreground() != Some(window) {
        trace(format_args!("{image}: another window came to the front during the copy"));
        return Err(PlatformError::Stale);
    }
    let text = text.unwrap_or_default();
    let text = if terminal { terminal_text(&text) } else { text };
    if text.trim().is_empty() {
        return Err(PlatformError::NoSelection);
    }
    let bounds = anchor(services.desktop, window, None);
    Ok(Capture::copied(window, image, text, bounds))
}

/// Where the panel goes, in physical pixels: the selection when its rectangle is believable, else
/// the caret, else the pointer.
fn anchor(desktop: &dyn Desktop, window: WindowRef, selection: Option<PxRect>) -> Option<Rect> {
    let believable = |r: &PxRect| !r.is_empty() && r.right - r.left < 20_000 && r.bottom - r.top < 4_000;
    let rect = selection.filter(believable).or_else(|| desktop.caret(window).filter(believable));
    if let Some(r) = rect {
        return Some(Rect {
            x: f64::from(r.left),
            y: f64::from(r.top),
            width: f64::from(r.right - r.left),
            height: f64::from(r.bottom - r.top),
        });
    }
    desktop.cursor().map(|(x, y)| Rect { x: f64::from(x) - 24.0, y: f64::from(y) - 10.0, width: 1.0, height: 20.0 })
}

/// `text`'s words, one space apart, for comparing a selection with its copy.
pub fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn fields_are_classed_before_anything_is_written() {
        let notepad = apps::profile("notepad.exe", "Notepad");
        let safe = field("Their going");
        assert_eq!(classify(&notepad, &safe), Class::KnownSafe);

        let mut password = safe.clone();
        password.password = true;
        assert_eq!(classify(&notepad, &password), Class::Refused(Why::Password));

        let mut composing = safe.clone();
        composing.composition = true;
        assert_eq!(classify(&notepad, &composing), Class::Refused(Why::Composing));

        for read_only in [Some(true), None] {
            let mut field = safe.clone();
            field.read_only = read_only;
            assert_eq!(classify(&notepad, &field), Class::CopyOnly(Why::ReadOnly), "{read_only:?}");
        }

        let mut two = safe.clone();
        two.ranges = 2;
        assert_eq!(classify(&notepad, &two), Class::CopyOnly(Why::Ranges));

        let terminal = apps::profile("WindowsTerminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS");
        assert_eq!(classify(&terminal, &safe), Class::CopyOnly(Why::Terminal));
        let mut panel = safe.clone();
        panel.class = "xterm-helper-textarea".into();
        assert_eq!(classify(&apps::profile("Code.exe", ""), &panel), Class::CopyOnly(Why::Terminal));

        let mut to = safe.clone();
        (to.control_type, to.class, to.name) = (50026, "EditorClass".into(), "To".into());
        assert_eq!(classify(&apps::profile("olk.exe", ""), &to), Class::CopyOnly(Why::Recipients));

        assert_eq!(classify(&apps::profile("soffice.bin", ""), &safe), Class::CopyOnly(Why::NoPaste));
        assert_eq!(classify(&apps::profile("WINWORD.EXE", ""), &safe), Class::KnownSafe, "an app nobody listed");
        assert_eq!(classify(&apps::profile("KeePassXC.exe", ""), &field("hunter2")), Class::Refused(Why::Password));
    }

    #[test]
    fn a_selection_read_through_ui_automation_can_be_applied() {
        let world = World::new("notepad.exe").with_field(field("Their going"));
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.source, Source::Uia);
        assert!(capture.editable);
        assert_eq!(capture.text, "Their going");
        assert!(world.keys_sent().is_empty(), "UI Automation needs no keys");
    }

    #[test]
    fn a_field_read_while_another_window_came_to_the_front_isnt_used() {
        let world = World::new("notepad.exe").with_field(field("Their going"));
        world.other_window_in_front_during_read();
        assert!(matches!(world.capture(true), Err(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn no_copy_key_goes_to_a_window_that_came_to_the_front() {
        let world = World::new("soffice.bin").copying("Their going");
        world.other_window_in_front_during_snapshot();
        assert!(matches!(world.capture(true), Err(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty(), "no Ctrl+C to the other window");
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn a_field_in_another_window_read_while_the_user_switched_away_and_back_is_stale() {
        let mut other = field("Their going");
        other.root_window = Some(300);
        let world = World::new("notepad.exe").with_field(other);
        assert!(matches!(world.capture(true), Err(PlatformError::Stale)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_field_whose_window_cant_be_found_is_stale() {
        let mut lost = field("Their going");
        lost.root_window = None;
        let world = World::new("notepad.exe").with_field(lost);
        assert!(matches!(world.capture(true), Err(PlatformError::Stale)));
    }

    #[test]
    fn a_users_own_copy_during_the_wait_is_never_overwritten() {
        // The user presses Ctrl+C themselves while SuperEditr waits, and a copy lands then.
        let world = World::new("soffice.bin").copying("Their going");
        world.user_presses_copy_during_the_wait();
        assert!(matches!(world.capture(true), Err(PlatformError::Stale)));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(world.clipboard_text().as_deref(), Some("Their going"), "the copy stays");
    }

    #[test]
    fn a_field_in_a_child_process_of_the_window_is_read() {
        // A WebView2 page runs in a process of its own, inside the app's window.
        let mut hosted = field("Their going");
        hosted.pid = 9090;
        let world = World::new("olk.exe").with_field(hosted);
        assert_eq!(world.capture(true).unwrap().text, "Their going");
    }

    #[test]
    fn the_users_own_copy_key_during_the_wait_ends_the_late_copy_watch() {
        // While SuperEditr waits for the app, the user presses Ctrl+C themselves: the app's copy,
        // when it comes, is theirs.
        let world = World::new("soffice.bin").copying_late("Their going", Duration::from_millis(60));
        world.user_presses_copy_during_the_wait();
        assert!(matches!(world.capture(true), Err(PlatformError::NoSelection)));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(world.clipboard_text().as_deref(), Some("Their going"));
    }

    #[test]
    fn closing_the_notice_typing_or_the_mouse_dont_end_the_late_copy_watch() {
        let world = World::new("soffice.bin").copying_late("Their going", Duration::from_millis(60));
        assert!(matches!(world.capture(true), Err(PlatformError::NoSelection)));
        // Esc on the notice, typing, the mouse: none is a copy.
        world.user_input();
        std::thread::sleep(Duration::from_millis(150));
        assert!(wait_for(|| world.clipboard_text().as_deref() == Some("Rhyan's own copy")));
    }

    #[test]
    fn the_first_read_in_an_app_waits_longer_than_later_ones() {
        let world = World::new("notepad.exe").in_process(4201).with_document("Their going.", "Their");
        world.capture(true).unwrap();
        world.capture(true).unwrap();
        let limits = world.read_limits();
        assert_eq!(limits.len(), 2, "{limits:?}");
        assert!(limits[0] > OTHER_BUDGET && limits[0] <= READ_BUDGET, "the first read: {limits:?}");
        assert!(limits[1] <= OTHER_BUDGET, "a later read: {limits:?}");
    }

    #[test]
    fn chromium_is_read_again_until_the_field_answers() {
        let world = World::new("chrome.exe").with_field(field("Their going")).answering_late(3);
        let capture = world.capture(true).unwrap();
        assert!(capture.editable);
        assert_eq!(world.reads(), 4);
    }

    #[test]
    fn password_fields_and_managers_are_never_read() {
        let mut secret = field("•••");
        secret.password = true;
        let world = World::new("chrome.exe").with_field(secret).copying("hunter2");
        assert!(matches!(world.capture(true), Err(PlatformError::SecureField)));
        assert!(world.keys_sent().is_empty(), "no copy either");

        let world = World::new("KeePassXC.exe").copying("hunter2");
        assert!(matches!(world.capture(true), Err(PlatformError::SecureField)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn an_app_running_as_administrator_is_refused_without_keys() {
        let world = World::new("notepad.exe").with_field(field("Their going")).elevated();
        match world.capture(true) {
            Err(PlatformError::Other(message)) => assert!(message.contains("administrator"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn text_still_being_composed_is_refused() {
        let mut composing = field("");
        composing.composition = true;
        let world = World::new("notepad.exe").with_field(composing);
        assert!(matches!(world.capture(true), Err(PlatformError::Composing)));
        assert!(world.keys_sent().is_empty(), "no copy: it would commit the composition");
    }

    #[test]
    fn terminals_are_copy_only() {
        let world = World::new("WindowsTerminal.exe").with_field(field("make deploy"));
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.source, Source::Uia);
        assert!(!capture.editable);
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_terminal_panel_with_its_selection_elsewhere_is_copied_with_ctrl_insert() {
        let mut panel = field("");
        panel.class = "xterm-helper-textarea".into();
        let world = World::new("Code.exe").with_field(panel).copying("npm run build");
        let capture = world.capture(true).unwrap();
        assert_eq!((capture.source, capture.text.as_str(), capture.editable), (Source::Copy, "npm run build", false));
        assert_eq!(world.keys_sent(), ["Ctrl+Insert"], "Ctrl+C would interrupt the panel's command");
    }

    #[test]
    fn without_ui_automation_a_shortcut_copies_and_gives_the_clipboard_back() {
        let world = World::new("soffice.bin").copying("Their going too the store");
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.text, "Their going too the store");
        assert_eq!(capture.source, Source::Copy);
        assert!(!capture.editable);
        assert_eq!(world.keys_sent(), ["Ctrl+C"]);
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
        assert!(world.clipboard_has_html(), "every format comes back");
    }

    fn wait_for(what: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn a_copy_that_comes_after_the_wait_is_undone_and_a_later_users_copy_kept() {
        let world = World::new("soffice.bin").copying_late("Their going", Duration::from_millis(60));
        assert!(matches!(world.capture(true), Err(PlatformError::NoSelection)));
        assert_eq!(world.keys_sent(), ["Ctrl+C", "Ctrl+C"]);
        // Both Ctrl+C presses copy, late; each copy is undone.
        std::thread::sleep(Duration::from_millis(120));
        assert!(wait_for(|| world.clipboard_text().as_deref() == Some("Rhyan's own copy")));
        assert!(world.clipboard_has_html(), "every format comes back");
        // The user then copies in the same app: that copy is theirs.
        world.clipboard.raw().copy_from(&apps::display_name("soffice.bin"), "Kyndal's own copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(world.clipboard_text().as_deref(), Some("Kyndal's own copy"));
    }

    #[test]
    fn a_users_own_copy_after_a_given_up_copy_is_never_overwritten() {
        let world = World::new("soffice.bin");
        assert!(matches!(world.capture(true), Err(PlatformError::NoSelection)));
        world.clipboard.raw().copy_from("Notepad", "Brynlee's copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(world.clipboard_text().as_deref(), Some("Brynlee's copy"));
    }

    #[test]
    fn a_second_copy_key_that_copies_after_the_first_is_undone_too() {
        // The first Ctrl+C copies just after the first wait ends: the second's copy comes later.
        let world = World::new("soffice.bin").copying_late("Their going too the store", Duration::from_millis(30));
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.text, "Their going too the store");
        std::thread::sleep(Duration::from_millis(80));
        assert!(wait_for(|| world.clipboard_text().as_deref() == Some("Rhyan's own copy")));
    }

    #[test]
    fn a_terminals_padding_isnt_part_of_its_text() {
        assert_eq!(
            terminal_text("Their going to love this.   \r\n      \r\nC:\\Users\\Rhyan>    \r\n   \r\n  \r\n"),
            "Their going to love this.\r\n\r\nC:\\Users\\Rhyan>"
        );
        assert_eq!(terminal_text("  indented stays\n"), "  indented stays");
        let world = World::new("WindowsTerminal.exe").with_field(field("make deploy      \r\n     \r\n    "));
        let capture = world.capture(true).unwrap();
        assert_eq!(capture.text, "make deploy");
        assert!(!capture.editable);
        let world = World::new("WindowsTerminal.exe").with_field(field("        \r\n    ")).copying("    ");
        assert!(world.capture(true).is_err(), "only padding is nothing selected");
        let world = World::new("notepad.exe").with_field(field("Their going   "));
        assert_eq!(world.capture(true).unwrap().text, "Their going   ", "only a terminal's text is trimmed");
    }

    #[test]
    fn an_automatic_review_never_copies() {
        let world = World::new("soffice.bin").copying("Their going");
        assert!(matches!(world.capture(false), Err(PlatformError::NoSelection)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn nothing_selected_is_no_capture_and_the_clipboard_comes_back() {
        let world = World::new("soffice.bin");
        assert!(matches!(world.capture(true), Err(PlatformError::NoSelection)));
        assert_eq!(world.keys_sent(), ["Ctrl+C", "Ctrl+C"], "tried twice");
        assert_eq!(world.clipboard_text().as_deref(), Some("Rhyan's own copy"));
    }

    #[test]
    fn a_code_editor_showing_part_of_the_selection_is_copy_only_with_all_of_it() {
        let world = World::new("Code.exe").with_field(field("Line 1\n…")).copying("Line 1\nLine 2\nLine 3");
        let capture = world.capture(true).unwrap();
        assert!(!capture.editable, "a paste would delete what accessibility didn't show");
        assert_eq!(capture.text, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn a_code_editor_showing_all_of_the_selection_can_be_applied() {
        let world = World::new("Code.exe").with_field(field("Their going")).copying("Their going");
        assert!(world.capture(true).unwrap().editable);
    }

    #[test]
    fn a_clipboard_that_cant_be_put_aside_stops_a_copy_before_any_key() {
        let world = World::new("soffice.bin").copying("Their going");
        world.clipboard.raw().held.store(true, std::sync::atomic::Ordering::SeqCst);
        match world.capture(true) {
            Err(PlatformError::Other(message)) => assert!(message.contains("Kyndal’s Notes"), "{message}"),
            other => panic!("expected the clipboard message, got {other:?}"),
        }
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn the_panel_goes_to_the_selection_only_when_its_rectangle_is_believable() {
        let world = World::new("notepad.exe");
        let window = world.window();
        let at = |r: Option<PxRect>| anchor(&world, window, r).map(|r| (r.x, r.y));
        assert_eq!(at(Some(PxRect { left: -1500, top: 200, right: -1400, bottom: 220 })), Some((-1500.0, 200.0)));
        assert_eq!(at(Some(PxRect { left: 10, top: 10, right: 10, bottom: 30 })), Some((300.0, 400.0)), "the caret");
        world.no_caret();
        assert_eq!(at(None), Some((876.0, 490.0)), "the pointer");
    }
}
