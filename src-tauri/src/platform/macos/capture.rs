//! Reading the selection: through Accessibility, after selecting the sentence before the caret, or
//! with a guarded copy.
//!
//! The decisions run against the traits in `seam.rs`, so they can be tested against a pretend Mac
//! (`testing.rs`): the [`App`] in front and its [`Field`]s, the [`Desktop`] that finds the app and
//! sends ⌘C, the [`Pasteboard`] a copy lands on, and a [`Clock`] for every wait. [`capture`] runs
//! them against the real ones.

use super::ax::{CFRange, Element};
use super::log::TRACE_ON;
use super::seam::{App, Clock, Desktop, Field, Pasteboard, Services, System};
use super::text::{code_ranges, visible_before};
use super::{Capture, trace};
use crate::platform::{PlatformError, Rect, log_lines};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

type CFStringTokenizerRef = *const c_void;
const kCFStringTokenizerUnitSentence: usize = 1;
const kCFStringTokenizerTokenNone: usize = 0;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringTokenizerCreate(
        alloc: *const c_void,
        string: CFStringRef,
        range: CFRange,
        options: usize,
        locale: *const c_void,
    ) -> CFStringTokenizerRef;
    fn CFStringTokenizerGoToTokenAtIndex(tokenizer: CFStringTokenizerRef, index: isize) -> usize;
    fn CFStringTokenizerGetCurrentTokenRange(tokenizer: CFStringTokenizerRef) -> CFRange;
}

/// What a read found: the capture, and the field it was read from, which [`capture`] puts in it.
/// Copied text has no field.
#[derive(Debug)]
pub(super) struct Read<N> {
    /// The capture, without its element.
    pub(super) capture: Capture,
    pub(super) field: Option<N>,
}

impl Read<Element> {
    fn into_capture(self) -> Capture {
        Capture { element: self.field, ..self.capture }
    }
}

/// The element type of the app a [`Desktop`] finds in front.
type NodeOf<'a, D> = <<D as Desktop>::App<'a> as App>::Node;

/// Why a capture can't be replaced, for capture.log: what kind of element the selection was read
/// from and what kind has the keyboard, so an app SuperEditr can't write to can be told from a field
/// it misreads. Roles and yes/no answers only, never text.
fn why_copy_only<D: Desktop, A: App>(desktop: &D, app: &A, read: &Read<A::Node>, pid: i32) -> String {
    let Some(element) = &read.field else {
        return "copy-only: copied text has no field to write back to".into();
    };
    let kind = |e: &A::Node| {
        let role = e.role().unwrap_or_else(|| "no role".into());
        match e.subrole() {
            Some(subrole) => format!("{role} ({subrole})"),
            None => role,
        }
    };
    let focused = app.focused();
    let mut above = 0;
    let mut current = focused.clone();
    while let Some(e) = current.filter(|e| !e.same_as(element) && above < 8) {
        above += 1;
        current = e.parent();
    }
    let yes = |b: bool| if b { "yes" } else { "no" };
    format!(
        "copy-only: the selection is in {}, {above} above the focused {}; focused value settable: {}; web area: {}; Chromium: {}",
        kind(element),
        focused.as_ref().map(kind).unwrap_or_else(|| "nothing".into()),
        yes(focused.as_ref().is_some_and(|f| f.value_settable())),
        yes(in_web_area(element)),
        yes(desktop.chromium_based(&read.capture.bundle_id, pid)),
    )
}

/// Reads the current selection. With `allow_clipboard`, falls back to a guarded ⌘C when the
/// app doesn't expose its selection through Accessibility. Only a shortcut allows the clipboard;
/// an automatic suggestion never copies, so the log calls the read "shortcut" or "automatic".
/// With `select_sentence` and nothing selected, first selects the sentence before the caret,
/// visibly, and reads that.
pub fn capture(allow_clipboard: bool, select_sentence: bool) -> Result<Capture, PlatformError> {
    capture_with(&System::services(), allow_clipboard, select_sentence).map(Read::into_capture)
}

/// [`capture`], against `services`.
pub(super) fn capture_with<'a, D: Desktop, P: Pasteboard>(
    services: &Services<'a, D, P>,
    allow_clipboard: bool,
    select_sentence: bool,
) -> Result<Read<NodeOf<'a, D>>, PlatformError> {
    let (desktop, clock) = (services.desktop, services.clock);
    if !desktop.trusted() {
        return Err(PlatformError::NotTrusted);
    }
    let source = log_lines::how(allow_clipboard);
    let Some((app, pid)) = desktop.focused_app() else {
        trace(format_args!("{source}: no app in front"));
        return Err(PlatformError::NoSelection);
    };
    let (bundle_id, app_name) = desktop.app_identity(pid);
    let started = clock.now();
    let mut first = capture_ax(desktop, clock, &app, pid, &bundle_id, &app_name);
    if select_sentence && matches!(first, Err(PlatformError::NoSelection)) {
        let caret = select_before_caret(clock, &app);
        trace(format_args!("{bundle_id}: nothing selected; {caret:?}"));
        if caret != CaretSelection::Nothing {
            first = capture_ax(desktop, clock, &app, pid, &bundle_id, &app_name);
        }
    }
    let result = match first {
        Ok(read) => Ok(read),
        Err(PlatformError::SecureField) => Err(PlatformError::SecureField),
        Err(error) => {
            trace(format_args!(
                "{source} {bundle_id}: Accessibility found no selection after {:?}",
                clock.now().duration_since(started)
            ));
            if !allow_clipboard {
                return Err(error);
            }
            capture_clipboard(services, &app, pid, bundle_id.clone(), app_name)
        }
    };
    match &result {
        // Only with the diagnostic log on: describing the read asks the app more questions.
        Ok(read) if TRACE_ON.load(Ordering::Relaxed) => {
            let capture = &read.capture;
            // The field's whole length shows a stale read: a selection made with ⌘A should match it.
            let of_field = read.field.as_ref().and_then(|e| e.character_count());
            let took = clock.now().duration_since(started);
            let access = if capture.editable {
                "replacing allowed".to_string()
            } else {
                why_copy_only(desktop, &app, read, pid)
            };
            trace(format_args!(
                "{}",
                log_lines::Read {
                    how: source,
                    app: &bundle_id,
                    chars: capture.text.chars().count(),
                    of_field,
                    via: if read.field.is_some() { "Accessibility" } else { "⌘C" },
                    element: "",
                    took,
                    access: &access,
                }
            ))
        }
        Ok(_) => {}
        Err(error) => trace(format_args!(
            "{}",
            log_lines::ReadFailed { how: source, app: &bundle_id, error, took: clock.now().duration_since(started) }
        )),
    }
    result
}

/// One read of the selection: the element that owns it, and its text.
fn read_selection<A: App>(app: &A) -> Result<(A::Node, String), PlatformError> {
    // Chromium reports focus on a descendant of the editable root; walk up to the element that
    // owns the selection.
    let mut element = app.focused().ok_or(PlatformError::NoSelection)?;
    for _ in 0..8 {
        if element.subrole().as_deref() == Some("AXSecureTextField") {
            return Err(PlatformError::SecureField);
        }
        if let Some(selected) = element.selection() {
            return Ok((element, selected));
        }
        match element.parent() {
            Some(parent) => element = parent,
            None => break,
        }
    }
    Err(PlatformError::NoSelection)
}

/// Reads until a read finds a selection, `deadline` passes, or `settled` says there is no point
/// waiting. A read past the deadline still happens once.
fn read_until<T>(
    clock: &dyn Clock,
    deadline: Instant,
    settled: impl Fn() -> bool,
    read: impl Fn() -> Result<T, PlatformError>,
) -> Result<T, PlatformError> {
    loop {
        let result = read();
        if !matches!(result, Err(PlatformError::NoSelection)) || clock.now() >= deadline || settled() {
            return result;
        }
        clock.sleep(Duration::from_millis(50));
    }
}

fn capture_ax<D: Desktop, A: App>(
    desktop: &D,
    clock: &dyn Clock,
    app: &A,
    pid: i32,
    bundle_id: &str,
    app_name: &str,
) -> Result<Read<A::Node>, PlatformError> {
    // Electron apps (Claude, Slack, VS Code) can report a focused window before their text fields
    // are exposed, so ask for the tree before the first read rather than only when focus is missing.
    let tree_deadline = if desktop.browser_or_electron(bundle_id, pid) || app.focused().is_none() {
        app.ask_for_tree(pid, bundle_id)
    } else {
        clock.now()
    };
    // While the tree is being built a read finds nothing, and the ⌘C read that would follow can't
    // replace anything, so wait for it. Focus in a text field means it's there and nothing is selected.
    let in_text_field = || {
        let role = app.focused().and_then(|focused| focused.role());
        matches!(role.as_deref(), Some("AXTextArea" | "AXTextField"))
    };
    let (element, text) = read_until(clock, tree_deadline, in_text_field, || read_selection(app))?;
    // A text role alone isn't enough: terminals, consoles, and read-only text views are text areas
    // too. Editable fields let their value be set; web editors report an editable ancestor.
    let web = desktop.browser_or_electron(bundle_id, pid) || in_web_area(&element);
    // A web view inside another app can hold an editable box that doesn't say so: one given a
    // button's role reports no settable value and no editable ancestor, though the user types in it.
    // There the app's Cut menu item answers instead. Browsers and Chromium apps describe their
    // fields well enough, and their menus are left out of it.
    let editable = element.value_settable()
        || element.has_editable_ancestor()
        || (web && !desktop.browser_or_electron(bundle_id, pid) && app.cut_enabled());
    let range = element.selected_range();
    // Only a text field is searched for code: when the selection is read from a whole page, the
    // page can have thousands of elements.
    let field = matches!(element.role().as_deref(), Some("AXTextArea" | "AXTextField"));
    let (code, markers, formatted) = if web && field {
        let selection_start =
            range.and_then(|(location, _)| element.value().and_then(|value| visible_before(&value, location)));
        let found = element.formatting();
        let markers = code_ranges(&text, selection_start, &found.markers);
        (code_ranges(&text, selection_start, &found.code), markers, found.any)
    } else {
        (Vec::new(), Vec::new(), false)
    };
    let plausible = |r: &Rect| r.width > 0.5 && r.height > 0.5 && r.height < 2000.0;
    let bounds = range
        .and_then(|(location, length)| element.bounds_for_range(location, length))
        .filter(plausible)
        .or_else(|| element.marker_bounds().filter(plausible))
        .or_else(|| element.frame());
    Ok(Read {
        capture: Capture {
            pid,
            bundle_id: bundle_id.to_string(),
            app_name: app_name.to_string(),
            text,
            bounds,
            editable,
            web,
            element: None,
            range,
            playground: false,
            code,
            markers,
            formatted,
        },
        field: Some(element),
    })
}

/// What [`select_before_caret`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaretSelection {
    /// Selected the sentence before the caret through Accessibility.
    Sentence,
    /// Left alone: an empty field, a caret at the start, no text field at all, or a field that
    /// hides its text or caret from Accessibility. No key is ever sent to select instead: the same
    /// keys mean different things in different apps (⌥⇧↑ copies a line in some code editors).
    Nothing,
}

/// With nothing selected, selects the text a review of "what I just wrote" should cover, so the
/// user sees it and the usual capture, same-field and unchanged-selection checks apply to it.
fn select_before_caret<A: App>(clock: &dyn Clock, app: &A) -> CaretSelection {
    let mut current = app.focused();
    for _ in 0..8 {
        let Some(element) = current else { break };
        if element.subrole().as_deref() == Some("AXSecureTextField") {
            return CaretSelection::Nothing;
        }
        if let Some((location, _)) = element.selected_range().filter(|&(_, length)| length == 0)
            && let Some(value) = element.value()
        {
            let Some(sentence) = sentence_before(&value, location.max(0) as usize) else {
                return CaretSelection::Nothing;
            };
            if select_range(clock, &element, sentence) {
                return CaretSelection::Sentence;
            }
            break;
        }
        current = element.parent();
    }
    CaretSelection::Nothing
}

/// Sets the field's selection to `range` and waits briefly for the app to report it.
fn select_range(clock: &dyn Clock, element: &impl Field, range: CFRange) -> bool {
    if !element.select(range.location.max(0) as usize, range.length.max(0) as usize) {
        return false;
    }
    let deadline = clock.now() + Duration::from_millis(150);
    loop {
        if element
            .selected_range()
            .is_some_and(|(location, length)| location == range.location && length == range.length)
        {
            return true;
        }
        if clock.now() >= deadline {
            return false;
        }
        clock.sleep(Duration::from_millis(10));
    }
}

/// The sentence that ends at or contains the caret, skipping whitespace just before it, as a range
/// of UTF-16 units in `text` without its surrounding whitespace. Sentences come from Core
/// Foundation's segmentation, which keeps abbreviations such as "e.g." and "Dr." inside a sentence.
fn sentence_before(text: &str, caret: usize) -> Option<CFRange> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let space = |unit: u16| char::from_u32(unit as u32).is_some_and(char::is_whitespace);
    let last = units[..caret.min(units.len())].iter().rposition(|&u| !space(u))?;
    let string = CFString::new(text);
    let whole = CFRange { location: 0, length: units.len() as isize };
    let tokenizer = unsafe {
        CFStringTokenizerCreate(
            std::ptr::null(),
            string.as_concrete_TypeRef(),
            whole,
            kCFStringTokenizerUnitSentence,
            std::ptr::null(),
        )
    };
    if tokenizer.is_null() {
        return None;
    }
    let tokenizer = unsafe { CFType::wrap_under_create_rule(tokenizer) };
    let raw = tokenizer.as_CFTypeRef();
    if unsafe { CFStringTokenizerGoToTokenAtIndex(raw, last as isize) } == kCFStringTokenizerTokenNone {
        return None;
    }
    let token = unsafe { CFStringTokenizerGetCurrentTokenRange(raw) };
    let mut start = token.location.max(0) as usize;
    let mut end = ((token.location + token.length) as usize).min(units.len());
    while start < end && space(units[start]) {
        start += 1;
    }
    while end > start && space(units[end - 1]) {
        end -= 1;
    }
    (end > start).then_some(CFRange { location: start as isize, length: (end - start) as isize })
}

fn in_web_area(element: &(impl Field + Clone)) -> bool {
    let mut current = element.clone();
    for _ in 0..40 {
        if current.role().as_deref() == Some("AXWebArea") {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

/// How SuperEditr asked the app to copy its selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyRequest {
    /// Pressed the app's own Edit ▸ Copy menu item.
    Menu,
    /// The app's Copy item is disabled: nothing is selected.
    MenuDisabled,
    /// Sent ⌘C as a keystroke to the front app.
    Keystroke,
    /// Sent ⌘C to the app's process.
    KeyToApp,
}

/// Presses the app's own Copy menu item (⌘C with no other modifiers). Unlike a
/// synthetic ⌘C, it doesn't depend on how the app treats posted keys or on modifier keys the user
/// is still holding from the shortcut. Electron apps have a native menu bar even when their web
/// content hides from Accessibility. Returns None when the app has no such item.
fn press_copy_menu(app: &impl App) -> Option<CopyRequest> {
    // A slow app can time out the press yet still copy, so it counts as sent either way; sending
    // ⌘C as well could copy twice and land after the clipboard is restored.
    app.press_menu_item("c").map(|pressed| if pressed { CopyRequest::Menu } else { CopyRequest::MenuDisabled })
}

/// Presses the menu item whose shortcut is ⌘ plus `key` alone, such as Edit ▸ Copy or Edit ▸ Paste.
/// Returns Some(false) when the item is disabled and None when the app has no such item.
pub(super) fn press_menu_item(app: &Element, key: &str) -> Option<bool> {
    let item = menu_item(app, key)?;
    if item.flag("AXEnabled") == Some(false) {
        return Some(false);
    }
    item.press();
    Some(true)
}

/// The menu item whose shortcut is ⌘ plus `key` alone, or None when the app has no such item or is
/// too slow to say.
fn menu_item(app: &Element, key: &str) -> Option<Element> {
    // A busy app answers each query slowly; give up rather than make the user wait.
    let deadline = Instant::now() + Duration::from_millis(500);
    let bar = app.element("AXMenuBar")?;
    // Skip the Apple and application menus; Cut, Copy and Paste live in Edit, normally the third.
    for menu_title in bar.children().into_iter().skip(2) {
        for menu in menu_title.children() {
            for item in menu.children() {
                if Instant::now() > deadline {
                    return None;
                }
                // Electron sets lowercase key equivalents; modifiers 0 means ⌘ alone.
                if item.string("AXMenuItemCmdChar").is_some_and(|c| c.eq_ignore_ascii_case(key))
                    && item.number("AXMenuItemCmdModifiers") == Some(0)
                {
                    return Some(item);
                }
            }
        }
    }
    None
}

/// Whether the app's own Cut menu item is enabled. An app enables Cut only for a selection it would
/// let the user remove, so with text selected this says the selection is somewhere text can be
/// edited, whatever the element calls itself.
pub(super) fn cut_enabled(app: &Element) -> bool {
    menu_item(app, "x").and_then(|item| item.flag("AXEnabled")) == Some(true)
}

fn capture_clipboard<D: Desktop, P: Pasteboard, A: App>(
    services: &Services<'_, D, P>,
    app: &A,
    pid: i32,
    bundle_id: String,
    app_name: String,
) -> Result<Read<A::Node>, PlatformError> {
    let (desktop, pasteboard, clock) = (services.desktop, services.pasteboard, services.clock);
    let snapshot = pasteboard.snapshot();
    let before = pasteboard.change_count();
    // Exactly one copy request per read (see press_copy_menu). Without a Copy menu item, browsers
    // and Electron apps in front get ⌘C as a keystroke, because some ignore keys sent to their
    // process; other apps get it directly.
    let request = press_copy_menu(app).unwrap_or_else(|| {
        if desktop.browser_or_electron(&bundle_id, pid) && desktop.focused_app().is_some_and(|(_, front)| front == pid)
        {
            desktop.post_copy(0);
            CopyRequest::Keystroke
        } else {
            desktop.post_copy(pid);
            CopyRequest::KeyToApp
        }
    });
    if request == CopyRequest::MenuDisabled {
        trace(format_args!("{bundle_id}: Copy is disabled in the app's Edit menu"));
        return Err(PlatformError::NoSelection);
    }
    // Electron copies from the page's process, which can take several hundred milliseconds for a
    // long selection in a busy app.
    let deadline = clock.now() + Duration::from_millis(1000);
    while pasteboard.change_count() == before && clock.now() < deadline {
        clock.sleep(Duration::from_millis(5));
    }
    if pasteboard.change_count() == before {
        trace(format_args!("{bundle_id}: no copy after {request:?}"));
        // Nothing to restore. A copy that lands later can't be told apart from one the user makes,
        // so it is left alone rather than risk erasing theirs.
        return Err(PlatformError::NoSelection);
    }
    // An app empties the clipboard and then writes to it. The change count moves once, at the first
    // step, so the text can be a moment behind the change and arrives under the same count.
    let changed = pasteboard.change_count();
    let mut copied = pasteboard.read_string();
    let text_deadline = clock.now() + Duration::from_millis(200);
    while copied.is_none() && pasteboard.change_count() == changed && clock.now() < text_deadline {
        clock.sleep(Duration::from_millis(5));
        copied = pasteboard.read_string();
    }
    if pasteboard.change_count() != changed {
        // Something else wrote to the clipboard meanwhile. What is there now isn't the selection,
        // and it is newer than the snapshot, so it is neither read nor replaced.
        trace(format_args!("{bundle_id}: the clipboard changed again after {request:?}"));
        return Err(PlatformError::NoSelection);
    }
    if copied.is_none() {
        trace(format_args!("{bundle_id}: the clipboard changed but holds no plain text"));
    }
    trace(format_args!("{bundle_id}: copied via {request:?}"));
    pasteboard.restore(snapshot, changed);
    let text = copied.filter(|t| !t.trim().is_empty()).ok_or(PlatformError::NoSelection)?;
    let focused = app.focused();
    let bounds = focused.as_ref().and_then(|focused| focused.frame());
    // Copied text has no field identity to re-check before writing, so it is copy-only.
    Ok(Read {
        capture: Capture {
            pid,
            bundle_id,
            app_name,
            text,
            bounds,
            editable: false,
            web: true,
            element: None,
            range: None,
            playground: false,
            code: Vec::new(),
            markers: Vec::new(),
            formatted: false,
        },
        field: None,
    })
}
#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn a_read_waits_for_the_selection_only_while_it_might_appear() {
        use std::cell::Cell;
        let clock = World::new("com.example.QuillPoint");
        let later = clock.now() + Duration::from_secs(60);
        let reads = Cell::new(0);
        let read = |result: fn(i32) -> Result<&'static str, PlatformError>| {
            reads.set(reads.get() + 1);
            result(reads.get())
        };
        let nothing_selected: fn(i32) -> Result<&'static str, PlatformError> = |_| Err(PlatformError::NoSelection);

        // Tries again until the selection appears.
        let third = |n| if n < 3 { Err(PlatformError::NoSelection) } else { Ok("text") };
        assert_eq!(read_until(&clock, later, || false, || read(third)).unwrap(), "text");
        assert_eq!(reads.replace(0), 3);

        // Once the wait is over, or the field has settled, one read is all there is.
        assert!(matches!(
            read_until(&clock, clock.now(), || false, || read(nothing_selected)),
            Err(PlatformError::NoSelection)
        ));
        assert_eq!(reads.replace(0), 1);
        assert!(matches!(
            read_until(&clock, later, || true, || read(nothing_selected)),
            Err(PlatformError::NoSelection)
        ));
        assert_eq!(reads.replace(0), 1);

        // A password field never gets another read.
        let secure: fn(i32) -> Result<&'static str, PlatformError> = |_| Err(PlatformError::SecureField);
        assert!(matches!(read_until(&clock, later, || false, || read(secure)), Err(PlatformError::SecureField)));
        assert_eq!(reads.get(), 1);
    }

    /// The sentence `sentence_before` picks with the caret at `|`.
    fn pick(marked: &str) -> Option<String> {
        let caret = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let caret = text[..caret].encode_utf16().count();
        let range = sentence_before(&text, caret)?;
        let units: Vec<u16> = text.encode_utf16().collect();
        let start = range.location as usize;
        Some(String::from_utf16(&units[start..start + range.length as usize]).unwrap())
    }

    #[test]
    fn picks_the_sentence_that_ends_at_the_caret() {
        assert_eq!(pick("We met at noon. Their going to love this.|").as_deref(), Some("Their going to love this."));
    }

    #[test]
    fn skips_spaces_and_line_breaks_before_the_caret() {
        assert_eq!(pick("First one. Second one. |").as_deref(), Some("Second one."));
        assert_eq!(pick("Rhyan has tennis today.\n\n|").as_deref(), Some("Rhyan has tennis today."));
    }

    #[test]
    fn picks_the_whole_sentence_around_a_caret_inside_it() {
        assert_eq!(pick("First one. Sec|ond one. Third one.").as_deref(), Some("Second one."));
    }

    #[test]
    fn a_sentence_still_being_typed_has_no_full_stop() {
        assert_eq!(pick("Done here. And then we|").as_deref(), Some("And then we"));
    }

    #[test]
    fn abbreviations_stay_inside_the_sentence() {
        assert_eq!(
            pick("Bring snacks, e.g. apples and pears, to the KPI review.|").as_deref(),
            Some("Bring snacks, e.g. apples and pears, to the KPI review.")
        );
        assert_eq!(
            pick("We left early. Dr. Brynlee met us at 1234 Glenmoor Ct.|").as_deref(),
            Some("Dr. Brynlee met us at 1234 Glenmoor Ct.")
        );
    }

    #[test]
    fn a_new_line_starts_a_new_sentence() {
        assert_eq!(pick("Notes from Kyndal\nThe plan works.|").as_deref(), Some("The plan works."));
    }

    #[test]
    fn ranges_count_utf16_units() {
        assert_eq!(pick("Ship it 🚀 now. Then rest 😴.|").as_deref(), Some("Then rest 😴."));
    }

    #[test]
    fn nothing_before_the_caret_selects_nothing() {
        assert_eq!(pick("|First one."), None);
        assert_eq!(pick("   |"), None);
        assert_eq!(pick("|"), None);
    }

    #[test]
    fn a_caret_past_the_end_counts_as_the_end() {
        assert_eq!(sentence_before("One. Two.", 99).map(|r| (r.location, r.length)), Some((5, 4)));
    }

    const NOTE: &str = "Brynlee sent the KPI notes realy late.";

    /// A native text field in QuillPoint, with two words of a sentence selected.
    fn notes() -> World {
        World::new("com.example.QuillPoint").with_text(NOTE, "realy late")
    }

    /// Whether `read` came from the world's text field rather than from a copy.
    fn from_the_field(world: &World, read: &Read<Ui<'_>>) -> bool {
        read.field.as_ref().is_some_and(|field| field.same_as(&world.field()))
    }

    #[test]
    fn a_selection_in_a_native_field_is_read_through_accessibility() {
        let world = notes();
        let read = world.read_by_shortcut().unwrap();
        assert_eq!(read.capture.text, "realy late");
        assert!(from_the_field(&world, &read));
        assert!(read.capture.editable, "a field whose value can be set can be replaced");
        assert!(!read.capture.web);
        assert_eq!(read.capture.range, Some((27, 10)));
        assert_eq!(read.capture.bounds, Some(SELECTION_BOUNDS));
        assert!(world.keys_sent().is_empty(), "nothing is copied");
        assert_eq!(world.asked_for_tree(), 0, "a native app isn't asked for its tree");
        assert_eq!(world.waited(), Duration::ZERO);
    }

    #[test]
    fn without_accessibility_permission_nothing_is_read() {
        let world = notes().untrusted();
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NotTrusted)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn with_no_app_in_front_nothing_is_read() {
        let world = notes().with_nothing_in_front();
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_password_field_is_never_read_or_copied() {
        let world = notes().password();
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::SecureField)));
        assert!(world.keys_sent().is_empty(), "a shortcut never falls back to copying a password");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn a_selection_is_read_from_the_field_above_the_focused_element() {
        let world = notes().focus_inside_the_field();
        let read = world.read_automatically().unwrap();
        assert_eq!(read.capture.text, "realy late");
        assert!(from_the_field(&world, &read), "the element that owns the selection, not the focused one");
    }

    #[test]
    fn a_selection_more_than_eight_elements_above_focus_is_not_found() {
        assert!(notes().focus_nested(7).read_automatically().is_ok());
        assert!(matches!(notes().focus_nested(8).read_automatically(), Err(PlatformError::NoSelection)));
    }

    #[test]
    fn nothing_selected_in_a_text_field_is_not_waited_for_even_in_a_browser() {
        let world = World::new("com.google.Chrome").with_caret("Brynlee sent it.|").web().chromium();
        assert!(matches!(world.read_automatically(), Err(PlatformError::NoSelection)));
        assert_eq!(world.asked_for_tree(), 1);
        assert_eq!(world.waited(), Duration::ZERO, "focus in a text field means the tree is there");
    }

    #[test]
    fn an_automatic_read_never_copies() {
        let world = notes().hiding_its_selection();
        assert!(matches!(world.read_automatically(), Err(PlatformError::NoSelection)));
        assert!(world.keys_sent().is_empty());
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn a_browser_still_building_its_tree_is_waited_for() {
        let world = notes().web().chromium().building_its_tree_for(Duration::from_secs(1));
        let read = world.read_automatically().unwrap();
        assert_eq!(read.capture.text, "realy late");
        assert_eq!(world.asked_for_tree(), 1);
        assert!(world.waited() >= Duration::from_secs(1) && world.waited() < Duration::from_millis(1100));
    }

    #[test]
    fn a_tree_that_never_comes_is_waited_for_four_seconds_then_copied() {
        let world = notes().web().chromium().building_its_tree_for(Duration::from_secs(60));
        let read = world.read_by_shortcut().unwrap();
        assert_eq!(read.capture.text, "realy late");
        assert!(read.field.is_none(), "copied");
        assert_eq!(world.keys_sent(), ["Copy menu item"]);
        let waited = world.waited();
        assert!(waited >= Duration::from_secs(4) && waited < Duration::from_millis(4100), "{waited:?}");
    }

    #[test]
    fn a_native_app_with_no_focused_element_is_asked_for_its_tree() {
        let world = notes().building_its_tree_for(Duration::from_millis(200));
        let read = world.read_automatically().unwrap();
        assert!(from_the_field(&world, &read));
        assert_eq!(world.asked_for_tree(), 1);
    }

    #[test]
    fn a_read_only_text_area_such_as_a_terminal_is_read_but_not_replaceable() {
        let world = World::new("com.example.Terminal").with_text("$ make deploy", "make deploy").not_settable();
        let read = world.read_by_shortcut().unwrap();
        assert_eq!(read.capture.text, "make deploy");
        assert!(from_the_field(&world, &read));
        assert!(!read.capture.editable);
        assert!(!read.capture.web);
        assert!(world.keys_sent().is_empty());
    }

    #[test]
    fn a_web_field_with_an_editable_ancestor_is_replaceable() {
        let world = notes().browser().web().not_settable().with_editable_ancestor();
        let read = world.read_by_shortcut().unwrap();
        assert!(read.capture.editable);
        assert!(read.capture.web);
    }

    #[test]
    fn a_web_view_in_a_native_app_is_replaceable_when_its_cut_item_is_enabled() {
        let boxed = || notes().web().not_settable().role("AXButton");
        let capture = boxed().cut_item(Item::Enabled).read_automatically().unwrap().capture;
        assert!(capture.web, "inside a web area");
        assert!(capture.editable);
        let capture = boxed().cut_item(Item::Disabled).read_automatically().unwrap().capture;
        assert!(!capture.editable);
    }

    #[test]
    fn a_browsers_cut_item_never_makes_a_field_replaceable() {
        let world = notes().browser().web().not_settable().cut_item(Item::Enabled);
        let read = world.read_automatically().unwrap();
        assert!(read.capture.web);
        assert!(!read.capture.editable);
    }

    #[test]
    fn only_a_web_text_field_is_searched_for_formatting() {
        let world = notes().browser().formatted();
        assert!(world.read_automatically().unwrap().capture.formatted);
        assert_eq!(world.formatting_reads(), 1);

        // A selection read from a whole page, or from a native field, isn't searched.
        let world = notes().browser().formatted().role("AXWebArea");
        assert!(!world.read_automatically().unwrap().capture.formatted);
        assert_eq!(world.formatting_reads(), 0);
        let world = notes();
        world.read_automatically().unwrap();
        assert_eq!(world.formatting_reads(), 0);
    }

    #[test]
    fn implausible_selection_bounds_fall_back_to_the_text_marker_then_the_field() {
        let flat = Rect { width: 0.0, ..SELECTION_BOUNDS };
        let capture = notes().with_bounds(Some(flat), Some(MARKER_BOUNDS)).read_automatically().unwrap().capture;
        assert_eq!(capture.bounds, Some(MARKER_BOUNDS));
        let tall = Rect { height: 2400.0, ..SELECTION_BOUNDS };
        let capture = notes().with_bounds(Some(tall), None).read_automatically().unwrap().capture;
        assert_eq!(capture.bounds, Some(FIELD_FRAME));
    }

    #[test]
    fn with_nothing_selected_the_sentence_before_the_caret_is_selected_and_read() {
        let world = World::new("com.example.QuillPoint").with_caret("We met at noon. Their going to love this.|");
        let read = world.read(false, true).unwrap();
        assert_eq!(read.capture.text, "Their going to love this.");
        assert_eq!(read.capture.range, Some((16, 25)));
        assert_eq!(world.field().selected_range(), Some((16, 25)), "the user sees what is reviewed");
        assert!(read.capture.editable);
    }

    #[test]
    fn a_caret_at_the_start_of_a_field_selects_nothing() {
        let world = World::new("com.example.QuillPoint").with_caret("|First one.");
        assert!(matches!(world.read(false, true), Err(PlatformError::NoSelection)));
        assert_eq!(world.field().selected_range(), Some((0, 0)));
    }

    #[test]
    fn a_sentence_the_app_wont_select_is_not_read() {
        let world = World::new("com.example.QuillPoint").with_caret("Kyndal booked the room.|").keeping_its_selection();
        assert!(matches!(world.read(false, true), Err(PlatformError::NoSelection)));
        assert!(world.waited() >= Duration::from_millis(150), "waits briefly for the app to report it");
        assert!(world.keys_sent().is_empty(), "no key is ever sent to select instead");
    }

    #[test]
    fn a_copy_fallback_reads_the_copied_text_and_puts_the_clipboard_back() {
        let world = notes().hiding_its_selection();
        let read = world.read_by_shortcut().unwrap();
        assert_eq!(read.capture.text, "realy late");
        assert!(read.field.is_none());
        assert!(!read.capture.editable, "copied text has no field to write back to");
        assert!(read.capture.web);
        assert_eq!(read.capture.range, None);
        assert_eq!(read.capture.bounds, Some(FIELD_FRAME), "placed by the focused field");
        assert_eq!(world.keys_sent(), ["Copy menu item"], "exactly one copy request");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
        assert_eq!(world.clipboard_html().as_deref(), Some("<b>Rhyan's own copy</b>"), "every representation");
    }

    #[test]
    fn a_disabled_copy_item_means_nothing_is_selected() {
        let world = notes().hiding_its_selection().copy_item(Item::Disabled);
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        assert!(world.keys_sent().is_empty(), "no ⌘C on top");
        assert_eq!(world.waited(), Duration::ZERO);
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn without_a_copy_item_a_browser_in_front_gets_a_keystroke_and_other_apps_get_a_key_event() {
        let world = notes().browser().hiding_its_selection().copy_item(Item::Missing);
        assert_eq!(world.read_by_shortcut().unwrap().capture.text, "realy late");
        assert_eq!(world.keys_sent(), ["⌘C keystroke"]);

        let world = notes().hiding_its_selection().copy_item(Item::Missing);
        assert_eq!(world.read_by_shortcut().unwrap().capture.text, "realy late");
        assert_eq!(world.keys_sent(), ["⌘C to the app's process"]);
    }

    #[test]
    fn a_slow_copy_is_waited_for_up_to_a_second() {
        let world = notes().hiding_its_selection().copies(Copies::After(Duration::from_millis(600)));
        assert_eq!(world.read_by_shortcut().unwrap().capture.text, "realy late");
        assert!(world.waited() >= Duration::from_millis(600) && world.waited() < Duration::from_secs(1));
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn text_that_reaches_the_clipboard_a_moment_after_it_changes_is_still_read() {
        let world = notes().hiding_its_selection().copies(Copies::EmptiesThenAfter(Duration::from_millis(30)));
        assert_eq!(world.read_by_shortcut().unwrap().capture.text, "realy late");
        assert!(world.waited() >= Duration::from_millis(30) && world.waited() < Duration::from_millis(200));
        assert_eq!(world.keys_sent(), ["Copy menu item"], "exactly one copy request");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn a_copy_with_no_plain_text_is_nothing_selected_after_a_short_wait() {
        let world = notes().hiding_its_selection().copies(Copies::NoText);
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        let waited = world.waited();
        assert!(waited >= Duration::from_millis(200) && waited < Duration::from_millis(300), "{waited:?}");
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }

    #[test]
    fn another_copy_during_the_wait_for_text_is_neither_read_nor_erased() {
        let world = notes().hiding_its_selection().copies(Copies::NoTextThenAnother(Duration::from_millis(30)));
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        assert!(world.waited() < Duration::from_millis(100), "{:?}", world.waited());
        assert_eq!(world.clipboard_text(), "Brynlee's later copy");
    }

    #[test]
    fn a_copy_that_lands_after_a_second_is_left_alone() {
        let world = notes().hiding_its_selection().copies(Copies::After(Duration::from_secs(2)));
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        let waited = world.waited();
        assert!(waited >= Duration::from_secs(1) && waited < Duration::from_millis(1100), "{waited:?}");
        // Once it lands it can't be told from the user's own copy, so nothing erases it.
        world.sleep(Duration::from_secs(1));
        assert_eq!(world.read_string().as_deref(), Some("realy late"));
    }

    #[test]
    fn copied_whitespace_is_nothing_selected_and_the_clipboard_comes_back() {
        let world = World::new("com.example.QuillPoint").with_text("Kyndal   wrote", "   ");
        assert!(matches!(world.read_by_shortcut(), Err(PlatformError::NoSelection)));
        assert_eq!(world.keys_sent(), ["Copy menu item"]);
        assert_eq!(world.clipboard_text(), "Rhyan's own copy");
    }
}
