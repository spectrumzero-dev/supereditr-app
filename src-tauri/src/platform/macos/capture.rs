//! Reading the selection: through Accessibility, after selecting the sentence before the caret, or
//! with a guarded copy.

use super::apps::{app_identity, enable_web_accessibility, focused_app, is_browser_or_electron, is_chromium_based};
use super::ax::{CFRange, CGRect, Element, to_rect};
use super::keys::{Key, post_command_key};
use super::log::TRACE_ON;
use super::replace::set_selection;
use super::text::{code_in, code_ranges, visible_before};
use super::{Capture, accessibility_trusted, pasteboard, trace};
use crate::platform::PlatformError;
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

/// Why a capture can't be replaced, for capture.log: what kind of element the selection was read
/// from and what kind has the keyboard, so an app SuperEditr can't write to can be told from a field
/// it misreads. Roles and yes/no answers only, never text.
fn why_copy_only(app: &Element, capture: &Capture, pid: i32) -> String {
    let Some(element) = &capture.element else {
        return "copy-only: copied text has no field to write back to".into();
    };
    let kind = |e: &Element| {
        let role = e.string("AXRole").unwrap_or_else(|| "no role".into());
        match e.string("AXSubrole") {
            Some(subrole) => format!("{role} ({subrole})"),
            None => role,
        }
    };
    let focused = app.element("AXFocusedUIElement");
    let mut above = 0;
    let mut current = focused.clone();
    while let Some(e) = current.filter(|e| !e.same_as(element) && above < 8) {
        above += 1;
        current = e.element("AXParent");
    }
    let yes = |b: bool| if b { "yes" } else { "no" };
    format!(
        "copy-only: the selection is in {}, {above} above the focused {}; focused value settable: {}; web area: {}; Chromium: {}",
        kind(element),
        focused.as_ref().map(kind).unwrap_or_else(|| "nothing".into()),
        yes(focused.as_ref().is_some_and(|f| f.settable("AXValue"))),
        yes(in_web_area(element)),
        yes(is_chromium_based(&capture.bundle_id, pid)),
    )
}

/// Reads the current selection. With `allow_clipboard`, falls back to a guarded ⌘C when the
/// app doesn't expose its selection through Accessibility. Only a shortcut allows the clipboard;
/// an automatic suggestion never copies, so the log calls the read "shortcut" or "automatic".
/// With `select_sentence` and nothing selected, first selects the sentence before the caret,
/// visibly, and reads that.
pub fn capture(allow_clipboard: bool, select_sentence: bool) -> Result<Capture, PlatformError> {
    if !accessibility_trusted(false) {
        return Err(PlatformError::NotTrusted);
    }
    let source = if allow_clipboard { "shortcut" } else { "automatic" };
    let Some((app, pid)) = focused_app() else {
        trace(format_args!("{source}: no app in front"));
        return Err(PlatformError::NoSelection);
    };
    let (bundle_id, app_name) = app_identity(pid);
    let started = Instant::now();
    let mut first = capture_ax(&app, pid, &bundle_id, &app_name);
    if select_sentence && matches!(first, Err(PlatformError::NoSelection)) {
        let caret = select_before_caret(&app);
        trace(format_args!("{bundle_id}: nothing selected; {caret:?}"));
        if caret != CaretSelection::Nothing {
            first = capture_ax(&app, pid, &bundle_id, &app_name);
        }
    }
    let result = match first {
        Ok(capture) => Ok(capture),
        Err(PlatformError::SecureField) => Err(PlatformError::SecureField),
        Err(error) => {
            trace(format_args!("{source} {bundle_id}: Accessibility found no selection after {:?}", started.elapsed()));
            if !allow_clipboard {
                return Err(error);
            }
            capture_clipboard(&app, pid, bundle_id.clone(), app_name)
        }
    };
    match &result {
        // Only with the diagnostic log on: describing the read asks the app more questions.
        Ok(capture) if TRACE_ON.load(Ordering::Relaxed) => {
            // The field's whole length shows a stale read: a selection made with ⌘A should match it.
            let field = capture.element.as_ref().and_then(|e| e.number("AXNumberOfCharacters"));
            trace(format_args!(
                "{source} {bundle_id}: read {} characters{} via {} in {:?}; {}",
                capture.text.chars().count(),
                field.map(|n| format!(" of {n} in the field")).unwrap_or_default(),
                if capture.element.is_some() { "Accessibility" } else { "⌘C" },
                started.elapsed(),
                if capture.editable { "replacing allowed".to_string() } else { why_copy_only(&app, capture, pid) }
            ))
        }
        Ok(_) => {}
        Err(error) => trace(format_args!("{source} {bundle_id}: {error} ({:?})", started.elapsed())),
    }
    result
}

/// One read of the selection: the element that owns it, and its text.
fn read_selection(app: &Element) -> Result<(Element, String), PlatformError> {
    // Chromium reports focus on a descendant of the editable root; walk up to the element that
    // owns the selection.
    let mut element = app.element("AXFocusedUIElement").ok_or(PlatformError::NoSelection)?;
    for _ in 0..8 {
        if element.string("AXSubrole").as_deref() == Some("AXSecureTextField") {
            return Err(PlatformError::SecureField);
        }
        if let Some(selected) = element.selected_text() {
            return Ok((element, selected));
        }
        match element.element("AXParent") {
            Some(parent) => element = parent,
            None => break,
        }
    }
    Err(PlatformError::NoSelection)
}

/// Reads until a read finds a selection, `deadline` passes, or `settled` says there is no point
/// waiting. A read past the deadline still happens once.
fn read_until<T>(
    deadline: Instant,
    settled: impl Fn() -> bool,
    read: impl Fn() -> Result<T, PlatformError>,
) -> Result<T, PlatformError> {
    loop {
        let result = read();
        if !matches!(result, Err(PlatformError::NoSelection)) || Instant::now() >= deadline || settled() {
            return result;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn capture_ax(app: &Element, pid: i32, bundle_id: &str, app_name: &str) -> Result<Capture, PlatformError> {
    // Electron apps (Claude, Slack, VS Code) can report a focused window before their text fields
    // are exposed, so ask for the tree before the first read rather than only when focus is missing.
    let tree_deadline = if is_browser_or_electron(bundle_id, pid) || app.element("AXFocusedUIElement").is_none() {
        enable_web_accessibility(app, pid, bundle_id)
    } else {
        Instant::now()
    };
    // While the tree is being built a read finds nothing, and the ⌘C read that would follow can't
    // replace anything, so wait for it. Focus in a text field means it's there and nothing is selected.
    let in_text_field = || {
        let role = app.element("AXFocusedUIElement").and_then(|focused| focused.string("AXRole"));
        matches!(role.as_deref(), Some("AXTextArea" | "AXTextField"))
    };
    let (element, text) = read_until(tree_deadline, in_text_field, || read_selection(app))?;
    // A text role alone isn't enough: terminals, consoles, and read-only text views are text areas
    // too. Editable fields let their value be set; web editors report an editable ancestor.
    let web = is_browser_or_electron(bundle_id, pid) || in_web_area(&element);
    // A web view inside another app can hold an editable box that doesn't say so: one given a
    // button's role reports no settable value and no editable ancestor, though the user types in it.
    // There the app's Cut menu item answers instead. Browsers and Chromium apps describe their
    // fields well enough, and their menus are left out of it.
    let editable = element.settable("AXValue")
        || element.attr("AXEditableAncestor").is_some()
        || (web && !is_browser_or_electron(bundle_id, pid) && cut_enabled(app));
    let range = element.range("AXSelectedTextRange");
    // Only a text field is searched for code: when the selection is read from a whole page, the
    // page can have thousands of elements.
    let field = matches!(element.string("AXRole").as_deref(), Some("AXTextArea" | "AXTextField"));
    let (code, markers, formatted) = if web && field {
        let selection_start =
            range.and_then(|r| element.string("AXValue").and_then(|value| visible_before(&value, r.location)));
        let found = code_in(&element);
        let markers = code_ranges(&text, selection_start, &found.markers);
        (code_ranges(&text, selection_start, &found.code), markers, found.any)
    } else {
        (Vec::new(), Vec::new(), false)
    };
    let plausible = |r: &CGRect| r.size.width > 0.5 && r.size.height > 0.5 && r.size.height < 2000.0;
    let bounds = range
        .and_then(|r| element.bounds_for_range(r))
        .filter(plausible)
        .or_else(|| element.marker_selection().and_then(|(_, bounds)| bounds).filter(plausible))
        .or_else(|| element.frame())
        .map(to_rect);
    Ok(Capture {
        pid,
        bundle_id: bundle_id.to_string(),
        app_name: app_name.to_string(),
        text,
        bounds,
        editable,
        web,
        element: Some(element),
        range: range.map(|r| (r.location, r.length)),
        playground: false,
        code,
        markers,
        formatted,
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
fn select_before_caret(app: &Element) -> CaretSelection {
    let mut current = app.element("AXFocusedUIElement");
    for _ in 0..8 {
        let Some(element) = current else { break };
        if element.string("AXSubrole").as_deref() == Some("AXSecureTextField") {
            return CaretSelection::Nothing;
        }
        if let Some(caret) = element.range("AXSelectedTextRange").filter(|r| r.length == 0)
            && let Some(value) = element.string("AXValue")
        {
            let Some(sentence) = sentence_before(&value, caret.location.max(0) as usize) else {
                return CaretSelection::Nothing;
            };
            if select_range(&element, sentence) {
                return CaretSelection::Sentence;
            }
            break;
        }
        current = element.element("AXParent");
    }
    CaretSelection::Nothing
}

/// Sets the field's selection to `range` and waits briefly for the app to report it.
fn select_range(element: &Element, range: CFRange) -> bool {
    if !set_selection(element, range.location.max(0) as usize, range.length.max(0) as usize) {
        return false;
    }
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        if element
            .range("AXSelectedTextRange")
            .is_some_and(|r| r.location == range.location && r.length == range.length)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
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

fn in_web_area(element: &Element) -> bool {
    let mut current = element.clone();
    for _ in 0..40 {
        if current.string("AXRole").as_deref() == Some("AXWebArea") {
            return true;
        }
        match current.element("AXParent") {
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
fn press_copy_menu(app: &Element) -> Option<CopyRequest> {
    // A slow app can time out the press yet still copy, so it counts as sent either way; sending
    // ⌘C as well could copy twice and land after the clipboard is restored.
    press_menu_item(app, "c").map(|pressed| if pressed { CopyRequest::Menu } else { CopyRequest::MenuDisabled })
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
fn cut_enabled(app: &Element) -> bool {
    menu_item(app, "x").and_then(|item| item.flag("AXEnabled")) == Some(true)
}

fn capture_clipboard(app: &Element, pid: i32, bundle_id: String, app_name: String) -> Result<Capture, PlatformError> {
    let snapshot = pasteboard::Snapshot::take();
    let before = pasteboard::change_count();
    // Exactly one copy request per read (see press_copy_menu). Without a Copy menu item, browsers
    // and Electron apps in front get ⌘C as a keystroke, because some ignore keys sent to their
    // process; other apps get it directly.
    let request = press_copy_menu(app).unwrap_or_else(|| {
        if is_browser_or_electron(&bundle_id, pid) && focused_app().is_some_and(|(_, front)| front == pid) {
            post_command_key(0, Key::C);
            CopyRequest::Keystroke
        } else {
            post_command_key(pid, Key::C);
            CopyRequest::KeyToApp
        }
    });
    if request == CopyRequest::MenuDisabled {
        trace(format_args!("{bundle_id}: Copy is disabled in the app's Edit menu"));
        return Err(PlatformError::NoSelection);
    }
    // Electron copies from the page's process, which can take several hundred milliseconds for a
    // long selection in a busy app.
    let deadline = Instant::now() + Duration::from_millis(1000);
    while pasteboard::change_count() == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if pasteboard::change_count() == before {
        trace(format_args!("{bundle_id}: no copy after {request:?}"));
        // Nothing to restore. A copy that lands later can't be told apart from one the user makes,
        // so it is left alone rather than risk erasing theirs.
        return Err(PlatformError::NoSelection);
    }
    let copied = pasteboard::read_string();
    if copied.is_none() {
        trace(format_args!("{bundle_id}: the clipboard changed but holds no plain text"));
    }
    trace(format_args!("{bundle_id}: copied via {request:?}"));
    snapshot.restore_if_unchanged_since(pasteboard::change_count());
    let text = copied.filter(|t| !t.trim().is_empty()).ok_or(PlatformError::NoSelection)?;
    let focused = app.element("AXFocusedUIElement");
    let bounds = focused.as_ref().and_then(Element::frame).map(to_rect);
    // Copied text has no field identity to re-check before writing, so it is copy-only.
    Ok(Capture {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_waits_for_the_selection_only_while_it_might_appear() {
        use std::cell::Cell;
        let later = Instant::now() + Duration::from_secs(60);
        let reads = Cell::new(0);
        let read = |result: fn(i32) -> Result<&'static str, PlatformError>| {
            reads.set(reads.get() + 1);
            result(reads.get())
        };
        let nothing_selected: fn(i32) -> Result<&'static str, PlatformError> = |_| Err(PlatformError::NoSelection);

        // Tries again until the selection appears.
        let third = |n| if n < 3 { Err(PlatformError::NoSelection) } else { Ok("text") };
        assert_eq!(read_until(later, || false, || read(third)).unwrap(), "text");
        assert_eq!(reads.replace(0), 3);

        // Once the wait is over, or the field has settled, one read is all there is.
        assert!(matches!(
            read_until(Instant::now(), || false, || read(nothing_selected)),
            Err(PlatformError::NoSelection)
        ));
        assert_eq!(reads.replace(0), 1);
        assert!(matches!(read_until(later, || true, || read(nothing_selected)), Err(PlatformError::NoSelection)));
        assert_eq!(reads.replace(0), 1);

        // A password field never gets another read.
        let secure: fn(i32) -> Result<&'static str, PlatformError> = |_| Err(PlatformError::SecureField);
        assert!(matches!(read_until(later, || false, || read(secure)), Err(PlatformError::SecureField)));
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
}
