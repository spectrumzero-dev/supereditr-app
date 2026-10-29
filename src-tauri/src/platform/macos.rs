//! macOS integration through the Accessibility (AX) API, Core Graphics keyboard events, and
//! NSPasteboard.
//!
//! Every AX call uses a short messaging timeout and runs off the main thread, so an
//! unresponsive app can delay a review by a fraction of a second but never hang SuperEditr.

#![allow(non_upper_case_globals, non_snake_case)]

use super::{PlatformError, Rect, ReplaceMethod, WatchEvent};
use crate::double_press::{self, Chord};
use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
type AXError = i32;
const kAXErrorSuccess: AXError = 0;
const kAXValueCGPointType: u32 = 1;
const kAXValueCGSizeType: u32 = 2;
const kAXValueCGRectType: u32 = 3;
const kAXValueCFRangeType: u32 = 4;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CFRange {
    location: isize,
    length: isize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef)
    -> AXError;
    fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: CFTypeRef) -> AXError;
    fn AXUIElementIsAttributeSettable(element: AXUIElementRef, attribute: CFStringRef, settable: *mut u8) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, kind: u32, out: *mut c_void) -> bool;
    fn AXValueGetTypeID() -> CFTypeID;
}

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

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: CFStringRef) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn UCKeyTranslate(
        layout: *const u8,
        key_code: u16,
        action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        unicode: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
}

/// AX timeout per message. Long enough for busy Electron apps, short enough to stay snappy.
const AX_TIMEOUT: f32 = 0.3;

/// An owned AXUIElement. AX element references are CFTypes and safe to use from any thread.
pub struct Element(AXUIElementRef);
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

impl Element {
    fn from_create(raw: AXUIElementRef) -> Option<Self> {
        if raw.is_null() {
            return None;
        }
        unsafe { AXUIElementSetMessagingTimeout(raw, AX_TIMEOUT) };
        Some(Self(raw))
    }

    fn attr(&self, name: &str) -> Option<CFType> {
        let name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, name.as_concrete_TypeRef(), &mut value) };
        (err == kAXErrorSuccess && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    fn string(&self, name: &str) -> Option<String> {
        self.attr(name)?.downcast::<CFString>().map(|s| s.to_string())
    }

    fn element(&self, name: &str) -> Option<Element> {
        let value = self.attr(name)?;
        if value.type_of() != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        let raw = value.as_CFTypeRef();
        unsafe { CFRetain(raw) };
        Element::from_create(raw)
    }

    fn children(&self) -> Vec<Element> {
        let Some(value) = self.attr("AXChildren") else { return Vec::new() };
        let Some(array) = value.downcast::<core_foundation::array::CFArray>() else { return Vec::new() };
        array
            .iter()
            .filter(|raw| unsafe { CFGetTypeID(**raw) } == unsafe { AXUIElementGetTypeID() })
            .filter_map(|raw| {
                unsafe { CFRetain(*raw) };
                Element::from_create(*raw)
            })
            .collect()
    }

    fn number(&self, name: &str) -> Option<i64> {
        self.attr(name)?.downcast::<core_foundation::number::CFNumber>()?.to_i64()
    }

    fn flag(&self, name: &str) -> Option<bool> {
        self.attr(name)?.downcast::<CFBoolean>().map(bool::from)
    }

    fn press(&self) -> bool {
        let action = CFString::new("AXPress");
        unsafe { AXUIElementPerformAction(self.0, action.as_concrete_TypeRef()) == kAXErrorSuccess }
    }

    fn range(&self, name: &str) -> Option<CFRange> {
        let value = self.attr(name)?;
        ax_value(value.as_CFTypeRef(), kAXValueCFRangeType)
    }

    fn settable(&self, name: &str) -> bool {
        let name = CFString::new(name);
        let mut settable = 0u8;
        let err = unsafe { AXUIElementIsAttributeSettable(self.0, name.as_concrete_TypeRef(), &mut settable) };
        err == kAXErrorSuccess && settable != 0
    }

    fn set(&self, name: &str, value: CFTypeRef) -> bool {
        let name = CFString::new(name);
        unsafe { AXUIElementSetAttributeValue(self.0, name.as_concrete_TypeRef(), value) == kAXErrorSuccess }
    }

    fn parameterized(&self, name: &str, param: CFTypeRef) -> Option<CFType> {
        let name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyParameterizedAttributeValue(self.0, name.as_concrete_TypeRef(), param, &mut value)
        };
        (err == kAXErrorSuccess && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    fn bounds_for_range(&self, range: CFRange) -> Option<CGRect> {
        let param = unsafe { AXValueCreate(kAXValueCFRangeType, &range as *const CFRange as *const c_void) };
        if param.is_null() {
            return None;
        }
        let param = unsafe { CFType::wrap_under_create_rule(param) };
        let value = self.parameterized("AXBoundsForRange", param.as_CFTypeRef())?;
        ax_value(value.as_CFTypeRef(), kAXValueCGRectType)
    }

    /// The selection in WebKit and Chromium content that only exposes it as a text-marker range.
    fn marker_selection(&self) -> Option<(String, Option<CGRect>)> {
        let range = self.attr("AXSelectedTextMarkerRange")?;
        let text =
            self.parameterized("AXStringForTextMarkerRange", range.as_CFTypeRef())?.downcast::<CFString>()?.to_string();
        let bounds = self
            .parameterized("AXBoundsForTextMarkerRange", range.as_CFTypeRef())
            .and_then(|v| ax_value::<CGRect>(v.as_CFTypeRef(), kAXValueCGRectType));
        Some((text, bounds))
    }

    /// Selected text through whichever interface the element supports.
    /// The selected text, unless nothing but whitespace is selected: some apps report a caret
    /// sitting in a field as a selected line break or space.
    fn selected_text(&self) -> Option<String> {
        let real = |s: &String| !s.trim().is_empty();
        self.string("AXSelectedText")
            .filter(real)
            .map(|text| self.web_text().and_then(|full| span_of(&full, &text)).unwrap_or(text))
            .or_else(|| self.marker_selection().map(|(text, _)| text).filter(real))
    }

    /// The text of a Chromium rich field (a text area whose children are paragraph groups) with
    /// every line break. Chromium's own text drops some: an empty paragraph adds nothing to
    /// `AXValue`, and `AXSelectedText` can leave out the break between two paragraphs. Each
    /// paragraph is still a child group, an empty one included, so the empty ones are put back.
    /// None for any other element, before its value is read.
    fn web_text(&self) -> Option<String> {
        let paragraphs = self.children();
        if paragraphs.is_empty()
            || paragraphs.len() > 500
            || self.string("AXRole").as_deref() != Some("AXTextArea")
            || !paragraphs.iter().all(|p| p.string("AXRole").as_deref() == Some("AXGroup"))
        {
            return None;
        }
        let lengths: Vec<usize> = paragraphs.iter().map(|p| visible_length(p, 0)).collect();
        let value = self.string("AXValue")?;
        Some(with_empty_paragraphs(&value, &lengths).unwrap_or(value))
    }

    fn frame(&self) -> Option<CGRect> {
        let position: CGPoint = ax_value(self.attr("AXPosition")?.as_CFTypeRef(), kAXValueCGPointType)?;
        let size: CGSize = ax_value(self.attr("AXSize")?.as_CFTypeRef(), kAXValueCGSizeType)?;
        Some(CGRect { origin: position, size })
    }

    fn pid(&self) -> Option<i32> {
        let mut pid = 0;
        (unsafe { AXUIElementGetPid(self.0, &mut pid) } == kAXErrorSuccess).then_some(pid)
    }

    fn same_as(&self, other: &Element) -> bool {
        unsafe { core_foundation::base::CFEqual(self.0, other.0) != 0 }
    }
}

impl Clone for Element {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        Self(self.0)
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AXUIElement")
    }
}

fn is_line_break(c: char) -> bool {
    c == '\n' || c == '\r'
}

/// Characters other than line breaks in an element's static text, a few levels down (links and
/// bold text nest it).
fn visible_length(element: &Element, depth: usize) -> usize {
    if element.string("AXRole").as_deref() == Some("AXStaticText") {
        return element.string("AXValue").map_or(0, |v| v.chars().filter(|c| !is_line_break(*c)).count());
    }
    if depth >= 4 {
        return 0;
    }
    element.children().iter().map(|child| visible_length(child, depth + 1)).sum()
}

/// `value` with its empty paragraphs put back. Chromium joins a rich field's non-empty paragraphs
/// with one line break each; `paragraphs` gives every paragraph's count of characters other than
/// line breaks, 0 for an empty one. None when `value` doesn't fit them.
fn with_empty_paragraphs(value: &str, paragraphs: &[usize]) -> Option<String> {
    let mut rest = value;
    let mut texts: Vec<&str> = Vec::with_capacity(paragraphs.len());
    let mut first = true;
    for &length in paragraphs {
        if length == 0 {
            texts.push("");
            continue;
        }
        if !first {
            rest = rest.strip_prefix('\n')?;
        }
        first = false;
        let mut seen = 0;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                seen += usize::from(!is_line_break(c));
                seen == length
            })
            .map(|(i, c)| i + c.len_utf8())?;
        texts.push(&rest[..end]);
        rest = &rest[end..];
    }
    rest.is_empty().then(|| texts.join("\n"))
}

/// The span of `full` with the same characters as `selected`, line breaks aside, from its first
/// such character to its last. None when there isn't exactly one.
fn span_of(full: &str, selected: &str) -> Option<String> {
    let target: String = selected.chars().filter(|c| !is_line_break(*c)).collect();
    if target.is_empty() {
        return None;
    }
    let (mut visible, mut starts) = (String::new(), Vec::new());
    for (i, c) in full.char_indices().filter(|&(_, c)| !is_line_break(c)) {
        visible.push(c);
        starts.push(i);
    }
    let mut found = visible.match_indices(target.as_str()).map(|(at, _)| at);
    let at = found.next()?;
    if found.next().is_some() {
        return None;
    }
    let first = visible[..at].chars().count();
    let last = first + target.chars().count() - 1;
    let end = starts[last] + full[starts[last]..].chars().next()?.len_utf8();
    Some(full[starts[first]..end].to_string())
}

fn ax_value<T: Default>(raw: CFTypeRef, kind: u32) -> Option<T> {
    if raw.is_null() || unsafe { CFGetTypeID(raw) } != unsafe { AXValueGetTypeID() } {
        return None;
    }
    let mut out = T::default();
    unsafe { AXValueGetValue(raw, kind, &mut out as *mut T as *mut c_void) }.then_some(out)
}

#[derive(Debug, Clone)]
pub struct Capture {
    pub pid: i32,
    pub bundle_id: String,
    pub app_name: String,
    pub text: String,
    pub bounds: Option<Rect>,
    /// The focused element is a text input (not static text on a page).
    pub editable: bool,
    /// Inside web content or an Electron app: replace by pasting so the page's editor sees it.
    web: bool,
    element: Option<Element>,
    range: Option<(isize, isize)>,
    /// Text from Settings' "Try it now" box: applying sends it back to the box, not to an app.
    pub playground: bool,
}

/// A capture of text in Settings' "Try it now" box, anchored at `bounds` (screen points).
pub fn playground_capture(text: String, bounds: Rect) -> Capture {
    Capture {
        pid: own_pid(),
        bundle_id: "com.sghael.supereditr".into(),
        app_name: "SuperEditr".into(),
        text,
        bounds: Some(bounds),
        editable: true,
        web: false,
        element: None,
        range: None,
        playground: true,
    }
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

pub fn accessibility_trusted(prompt: bool) -> bool {
    if !prompt {
        return unsafe { AXIsProcessTrusted() };
    }
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) }
}

pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// Apps whose web views only build an accessibility tree when asked. The flags are
/// documented by Chromium and Electron and are scoped to the target process. Returns whether the
/// flags were just sent, so the caller can give the tree a moment to appear.
fn enable_web_accessibility(app: &Element, pid: i32, bundle_id: &str) -> bool {
    static DONE: Mutex<Option<HashSet<i32>>> = Mutex::new(None);
    let mut done = DONE.lock().unwrap();
    let done = done.get_or_insert_with(HashSet::new);
    if done.contains(&pid) {
        return false;
    }
    // Remembered only once accepted: a busy app can time out, and the next read should ask again.
    let yes = CFBoolean::true_value();
    let mut accepted = app.set("AXManualAccessibility", yes.as_CFTypeRef());
    if is_chromium_browser(bundle_id) {
        accepted |= app.set("AXEnhancedUserInterface", yes.as_CFTypeRef());
    }
    if accepted {
        done.insert(pid);
    }
    accepted
}

fn is_chromium_browser(bundle_id: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "com.google.Chrome",
        "com.brave.Browser",
        "com.microsoft.edgemac",
        "company.thebrowser",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
        "org.chromium.Chromium",
    ];
    PREFIXES.iter().any(|p| bundle_id.starts_with(p))
}

fn is_browser_or_electron(bundle_id: &str, pid: i32) -> bool {
    bundle_id == "com.apple.Safari" || bundle_id.starts_with("org.mozilla.") || is_chromium_based(bundle_id, pid)
}

/// Chrome-family browsers and Electron apps, which share Chromium's clipboard code.
fn is_chromium_based(bundle_id: &str, pid: i32) -> bool {
    is_chromium_browser(bundle_id)
        || app_bundle_path(pid).is_some_and(|path| {
            std::path::Path::new(&path).join("Contents/Frameworks/Electron Framework.framework").exists()
        })
}

fn running_app(pid: i32) -> Option<objc2::rc::Retained<objc2_app_kit::NSRunningApplication>> {
    objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}

fn app_bundle_path(pid: i32) -> Option<String> {
    let app = running_app(pid)?;
    let url = app.bundleURL()?;
    url.path().map(|p| p.to_string())
}

fn app_identity(pid: i32) -> (String, String) {
    match running_app(pid) {
        Some(app) => (
            app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
            app.localizedName().map(|s| s.to_string()).unwrap_or_else(|| "this app".into()),
        ),
        None => (String::new(), "this app".into()),
    }
}

fn focused_app() -> Option<(Element, i32)> {
    let system = Element::from_create(unsafe { AXUIElementCreateSystemWide() })?;
    if let Some(app) = system.element("AXFocusedApplication")
        && let Some(pid) = app.pid()
    {
        return Some((app, pid));
    }
    // Electron apps (Claude, VS Code) with their accessibility off can leave the system-wide
    // element without a focused application. The front app still owns the keyboard.
    let pid = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier();
    Some((Element::from_create(unsafe { AXUIElementCreateApplication(pid) })?, pid))
}

/// The app that owns keyboard focus: (pid, bundle ID, name). Reads no text.
pub fn focused_app_identity() -> Option<(i32, String, String)> {
    let (_, pid) = focused_app()?;
    let (bundle_id, name) = app_identity(pid);
    Some((pid, bundle_id, name))
}

/// Gives `pid`'s window keyboard focus back after the review panel had it for typing an
/// instruction. The panel doesn't activate SuperEditr, so the app is still in front, but macOS
/// leaves it without a key window until something asks for one. Runs on a thread of its own: AX
/// calls can block on a busy app.
pub fn return_focus(pid: i32) {
    std::thread::spawn(move || {
        let Some(app) = Element::from_create(unsafe { AXUIElementCreateApplication(pid) }) else { return };
        let yes = CFBoolean::true_value();
        if let Some(window) = app.element("AXFocusedWindow").or_else(|| app.element("AXMainWindow")) {
            window.set("AXMain", yes.as_CFTypeRef());
        }
        app.set("AXFrontmost", yes.as_CFTypeRef());
        if let Some(running) = running_app(pid) {
            running.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty());
        }
    });
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
        Ok(capture) => {
            // The field's whole length shows a stale read: a selection made with ⌘A should match it.
            let field = capture.element.as_ref().and_then(|e| e.number("AXNumberOfCharacters"));
            trace(format_args!(
                "{source} {bundle_id}: read {} characters{} via {} in {:?}",
                capture.text.chars().count(),
                field.map(|n| format!(" of {n} in the field")).unwrap_or_default(),
                if capture.element.is_some() { "Accessibility" } else { "⌘C" },
                started.elapsed()
            ))
        }
        Err(error) => trace(format_args!("{source} {bundle_id}: {error} ({:?})", started.elapsed())),
    }
    result
}

static TRACE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn log_dir() -> Option<std::path::PathBuf> {
    Some(std::path::Path::new(&std::env::var_os("HOME")?).join("Library/Logs/SuperEditr"))
}

/// Turns capture.log on or off (Settings → Behavior → Keep a diagnostic log). Turning it off
/// deletes the log, so nothing is left behind once a problem has been diagnosed.
pub fn set_trace(on: bool) {
    TRACE_ON.store(on, Ordering::Relaxed);
    if !on && let Some(dir) = log_dir() {
        let _ = std::fs::remove_file(dir.join("capture.log"));
        let _ = std::fs::remove_file(dir.join("capture.old.log"));
    }
}

/// When the diagnostic log is on, appends one line about a selection read or write to
/// ~/Library/Logs/SuperEditr/capture.log, so a failure in a particular app can be diagnosed. It
/// records app IDs, timings and lengths, never text.
pub fn trace(message: std::fmt::Arguments) {
    use std::io::Write;
    if !TRACE_ON.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("capture.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, dir.join("capture.old.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{seconds} {message}");
    }
}

fn capture_ax(app: &Element, pid: i32, bundle_id: &str, app_name: &str) -> Result<Capture, PlatformError> {
    // Electron apps (Claude, Slack, VS Code) can report a focused window before their text fields
    // are exposed, so ask for the tree before the first read rather than only when focus is missing.
    if is_browser_or_electron(bundle_id, pid) && enable_web_accessibility(app, pid, bundle_id) {
        std::thread::sleep(Duration::from_millis(150));
    }
    let mut focused = app.element("AXFocusedUIElement");
    if focused.is_none() && enable_web_accessibility(app, pid, bundle_id) {
        focused = app.element("AXFocusedUIElement");
    }
    let focused = focused.ok_or(PlatformError::NoSelection)?;

    // Chromium reports focus on a descendant of the editable root; walk up to the element that
    // owns the selection.
    let mut element = focused;
    let mut text = None;
    for _ in 0..8 {
        if element.string("AXSubrole").as_deref() == Some("AXSecureTextField") {
            return Err(PlatformError::SecureField);
        }
        if let Some(selected) = element.selected_text() {
            text = Some(selected);
            break;
        }
        match element.element("AXParent") {
            Some(parent) => element = parent,
            None => break,
        }
    }
    let text = text.ok_or(PlatformError::NoSelection)?;
    // A text role alone isn't enough: terminals, consoles, and read-only text views are text areas
    // too. Editable fields let their value be set; web editors report an editable ancestor.
    let editable = element.settable("AXValue") || element.attr("AXEditableAncestor").is_some();
    let range = element.range("AXSelectedTextRange");
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
        web: is_browser_or_electron(bundle_id, pid) || in_web_area(&element),
        element: Some(element),
        range: range.map(|r| (r.location, r.length)),
        playground: false,
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
    let value = unsafe { AXValueCreate(kAXValueCFRangeType, &range as *const CFRange as *const c_void) };
    if value.is_null() {
        return false;
    }
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    if !element.set("AXSelectedTextRange", value.as_CFTypeRef()) {
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

fn to_rect(r: CGRect) -> Rect {
    Rect { x: r.origin.x, y: r.origin.y, width: r.size.width, height: r.size.height }
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
fn press_menu_item(app: &Element, key: &str) -> Option<bool> {
    // A busy app answers each query slowly; give up and send the keystroke rather than make the
    // user wait.
    let deadline = Instant::now() + Duration::from_millis(500);
    let bar = app.element("AXMenuBar")?;
    // Skip the Apple and application menus; Copy and Paste live in Edit, normally the third.
    for menu_title in bar.children().into_iter().skip(2) {
        for menu in menu_title.children() {
            for item in menu.children() {
                if Instant::now() > deadline {
                    return None;
                }
                // Electron sets lowercase key equivalents; modifiers 0 means ⌘ alone.
                if !item.string("AXMenuItemCmdChar").is_some_and(|c| c.eq_ignore_ascii_case(key))
                    || item.number("AXMenuItemCmdModifiers") != Some(0)
                {
                    continue;
                }
                if item.flag("AXEnabled") == Some(false) {
                    return Some(false);
                }
                item.press();
                return Some(true);
            }
        }
    }
    None
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
    })
}

/// True while the captured selection is still exactly what the user selected.
pub fn selection_unchanged(capture: &Capture) -> bool {
    match &capture.element {
        Some(element) => element.selected_text().as_deref() == Some(capture.text.as_str()),
        None => true,
    }
}

/// Replaces the captured selection with `text`, verifying the result where the app allows it.
pub fn replace(capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
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
        if !capture.web && element.settable("AXSelectedText") {
            let before = element.string("AXValue");
            let value = CFString::new(text);
            if element.set("AXSelectedText", value.as_CFTypeRef()) {
                if verify_replacement(element, capture, before.as_deref(), text, Duration::from_millis(150)) {
                    return Ok(ReplaceMethod::Accessibility);
                }
                // The write may have been applied without being verifiable; never paste on top of it.
                if element.string("AXValue") != before || !selection_unchanged(capture) {
                    trace(format_args!(
                        "{}: Accessibility write changed the field but didn't verify",
                        capture.bundle_id
                    ));
                    copy_to_clipboard(text);
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
fn verify_replacement(element: &Element, capture: &Capture, before: Option<&str>, text: &str, wait: Duration) -> bool {
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
            // Chromium's value drops empty paragraphs; its rebuilt text keeps them.
            if landed || element.web_text().is_some_and(|full| full.contains(text)) {
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
    let ours = pasteboard::write_transient_string(text);
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
        landed = verify_replacement(element, capture, before.as_deref(), text, Duration::from_millis(*wait));
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

// --- Keyboard -------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Key {
    C,
    V,
}

static KEY_C: AtomicU16 = AtomicU16::new(8);
static KEY_V: AtomicU16 = AtomicU16::new(9);

/// Finds the key codes that type "c" and "v" in the current layout, so ⌘C/⌘V work on Dvorak,
/// AZERTY and others. Text Input Sources must be queried on the main thread.
pub fn refresh_keyboard_layout() {
    unsafe {
        let mut source = TISCopyCurrentKeyboardLayoutInputSource();
        let mut data = if source.is_null() {
            std::ptr::null()
        } else {
            TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        };
        if data.is_null() {
            if !source.is_null() {
                CFRelease(source as CFTypeRef);
            }
            source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
            if source.is_null() {
                return;
            }
            data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        }
        if !data.is_null() {
            let layout = CFData::wrap_under_get_rule(data as _);
            let ptr = layout.bytes().as_ptr();
            let kbd = LMGetKbdType() as u32;
            for code in 0u16..128 {
                let mut dead = 0u32;
                let mut buf = [0u16; 4];
                let mut len = 0usize;
                if UCKeyTranslate(ptr, code, 3, 0, kbd, 1, &mut dead, buf.len(), &mut len, buf.as_mut_ptr()) == 0
                    && len == 1
                {
                    match buf[0] {
                        0x63 => KEY_C.store(code, Ordering::Relaxed),
                        0x76 => KEY_V.store(code, Ordering::Relaxed),
                        _ => {}
                    }
                }
            }
        }
        CFRelease(source as CFTypeRef);
    }
}

fn post_command_key(pid: i32, key: Key) {
    let code = match key {
        Key::C => KEY_C.load(Ordering::Relaxed),
        Key::V => KEY_V.load(Ordering::Relaxed),
    };
    post_key(pid, code, CGEventFlags::CGEventFlagCommand);
}

/// Sends one key press with exactly `flags`, to `pid`, or as a keystroke to the front app when 0.
fn post_key(pid: i32, code: u16, flags: CGEventFlags) {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return };
    for down in [true, false] {
        if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) {
            // Explicit flags so modifiers still held from the hotkey don't leak into the key.
            event.set_flags(flags);
            if pid > 0 {
                event.post_to_pid(pid);
            } else {
                event.post(CGEventTapLocation::HID);
            }
        }
    }
}

pub fn mouse_location() -> (f64, f64) {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .ok()
        .and_then(|source| CGEvent::new(source).ok())
        .map(|event| {
            let p = event.location();
            (p.x, p.y)
        })
        .unwrap_or((200.0, 200.0))
}

// --- Selection watcher ----------------------------------------------------------------------

/// Watches mouse and keyboard activity in other apps (passively; nothing is intercepted) and
/// reports when a selection may be finished; [`super::gesture`] decides when. Must be called on
/// the main thread.
pub fn start_selection_watcher(on_event: impl Fn(WatchEvent) + Send + Sync + 'static) {
    use super::gesture::Input;
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
    use std::ptr::NonNull;

    let down_at = Mutex::new((0.0f64, 0.0f64));
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        let event = unsafe { event.as_ref() };
        let kind = event.r#type();
        let flags = event.modifierFlags();
        let shift = flags.contains(NSEventModifierFlags::Shift);
        // Arrows, Home/End, and Page Up/Down.
        let navigation = || matches!(event.keyCode(), 115 | 116 | 119 | 121 | 123 | 124 | 125 | 126);
        let input = if kind == NSEventType::LeftMouseDown {
            let p = event.locationInWindow();
            *down_at.lock().unwrap() = (p.x, p.y);
            Input::MouseDown
        } else if kind == NSEventType::LeftMouseUp {
            let p = event.locationInWindow();
            let (x, y) = *down_at.lock().unwrap();
            let dragged = (p.x - x).abs() > 3.0 || (p.y - y).abs() > 3.0;
            Input::MouseUp { dragged, clicks: event.clickCount(), shift }
        } else if kind == NSEventType::KeyDown {
            let select_all = flags.contains(NSEventModifierFlags::Command) && event.keyCode() == KEY_A;
            if select_all && event.isARepeat() {
                // Holding ⌘A selects nothing new; a repeat must not cancel the pending review.
                return;
            }
            Input::KeyDown { navigation: navigation(), shift, select_all }
        } else if kind == NSEventType::KeyUp {
            Input::KeyUp { navigation: navigation(), shift }
        } else if kind == NSEventType::FlagsChanged {
            Input::ModifiersChanged { shift }
        } else if kind == NSEventType::ScrollWheel {
            Input::Scroll
        } else {
            return;
        };
        if let Some(watch) = GESTURE.lock().unwrap().feed(input) {
            on_event(watch);
        }
    });
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseUp
        | NSEventMask::KeyDown
        | NSEventMask::KeyUp
        | NSEventMask::FlagsChanged
        | NSEventMask::ScrollWheel;
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &handler);
    // The monitor lives for the life of the app.
    std::mem::forget(monitor);
    std::mem::forget(handler);
}

/// The selection under way, shared by the watcher and [`selection_app_switched`].
static GESTURE: Mutex<super::gesture::Gesture> = Mutex::new(super::gesture::Gesture::new());

/// Another app came to the front: forget a selection the user was making in the previous one.
pub fn selection_app_switched() {
    GESTURE.lock().unwrap().feed(super::gesture::Input::AppSwitched);
}

const KEY_A: u16 = 0;

// --- App switches ---------------------------------------------------------------------------

/// Calls `on_switch` with the process ID of each app that comes to the front. Must be called on
/// the main thread.
pub fn start_app_switch_watcher(on_switch: impl Fn(i32) + 'static) {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification};
    use objc2_foundation::NSNotification;
    use std::ptr::NonNull;

    let handler = RcBlock::new(move |note: NonNull<NSNotification>| {
        let note = unsafe { note.as_ref() };
        let pid = note
            .userInfo()
            .and_then(|info| info.objectForKey(unsafe { NSWorkspaceApplicationKey }))
            .and_then(|app| app.downcast::<objc2_app_kit::NSRunningApplication>().ok())
            .map(|app| app.processIdentifier());
        if let Some(pid) = pid {
            on_switch(pid);
        }
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &handler,
        )
    };
    // Both live for the life of the app.
    std::mem::forget(observer);
    std::mem::forget(handler);
}

// --- System Settings window -----------------------------------------------------------------

/// Where System Settings is, for placing the permission guide beside it.
pub enum SettingsWindow {
    /// Frontmost (ignoring SuperEditr's own windows), with its frame in screen points and its
    /// window number.
    Front(Rect, i64),
    /// Open, but another app is in front.
    Behind,
    Closed,
}

unsafe extern "C" {
    fn CGRectMakeWithDictionaryRepresentation(
        dict: core_foundation::dictionary::CFDictionaryRef,
        rect: *mut core_graphics::geometry::CGRect,
    ) -> bool;
}

/// Finds System Settings' window from the window list. Owners and frames need no permission
/// (titles would need Screen Recording), so this works before Accessibility is granted.
pub fn system_settings_window() -> SettingsWindow {
    use core_foundation::number::CFNumber;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly, kCGWindowNumber, kCGWindowOwnerPID,
    };
    let name = objc2_foundation::NSString::from_str("com.apple.systempreferences");
    let pids: Vec<i64> = objc2_app_kit::NSRunningApplication::runningApplicationsWithBundleIdentifier(&name)
        .iter()
        .map(|app| app.processIdentifier() as i64)
        .collect();
    if pids.is_empty() {
        return SettingsWindow::Closed;
    }
    let own = own_pid() as i64;
    let Some(windows) =
        copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)
    else {
        return SettingsWindow::Closed;
    };
    let mut seen = false;
    let mut front = true;
    // Front to back.
    for item in windows.iter() {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef) };
        let number = |key: CFStringRef| {
            dict.find(unsafe { CFString::wrap_under_get_rule(key) })
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        let Some(pid) = number(unsafe { kCGWindowOwnerPID }) else { continue };
        if number(unsafe { kCGWindowLayer }) != Some(0) || pid == own {
            continue;
        }
        if !pids.contains(&pid) {
            front = false;
            continue;
        }
        seen = true;
        if !front {
            break;
        }
        let Some(bounds) = dict.find(unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) }) else { continue };
        let mut rect = core_graphics::geometry::CGRect::default();
        if unsafe { CGRectMakeWithDictionaryRepresentation(bounds.as_CFTypeRef() as _, &mut rect) } {
            let frame = Rect { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height };
            return SettingsWindow::Front(frame, number(unsafe { kCGWindowNumber }).unwrap_or(0));
        }
    }
    if seen { SettingsWindow::Behind } else { SettingsWindow::Closed }
}

/// SuperEditr's own app bundle, which the guide lets the user drag into System Settings.
pub fn own_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf())
}

// --- Double-press shortcuts ---------------------------------------------------------------

/// Marks events SuperEditr replays, so its own tap lets them through.
const REPLAY_MARK: i64 = 0x5345_4454; // "SEDT"
static DOUBLE_CHORDS: Mutex<Vec<Chord>> = Mutex::new(Vec::new());
static TAP_PORT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TAP_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A review is open: the tap takes the panel's keys from the app (see `crate::review_keys`).
static REVIEW_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" {
    fn CGEventTapEnable(tap: *const c_void, enable: bool);
    fn CGEventKeyboardGetUnicodeString(event: *const c_void, max: usize, actual: *mut usize, chars: *mut u16);
}

/// Physical keys a double press can use, by macOS virtual key code (ANSI positions, matching the
/// physical key codes that shortcut settings record).
const KEY_CODES: [(&str, u16); 36] = [
    ("A", 0),
    ("S", 1),
    ("D", 2),
    ("F", 3),
    ("H", 4),
    ("G", 5),
    ("Z", 6),
    ("X", 7),
    ("C", 8),
    ("V", 9),
    ("B", 11),
    ("Q", 12),
    ("W", 13),
    ("E", 14),
    ("R", 15),
    ("Y", 16),
    ("T", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("9", 25),
    ("7", 26),
    ("8", 28),
    ("0", 29),
    ("O", 31),
    ("U", 32),
    ("I", 34),
    ("P", 35),
    ("L", 37),
    ("J", 38),
    ("K", 40),
    ("N", 45),
    ("M", 46),
];

/// Replaces the double-press shortcuts the tap watches for.
pub fn set_double_presses(chords: Vec<Chord>) {
    *DOUBLE_CHORDS.lock().unwrap() = chords;
}

/// Lets every key through, for example while Settings records a new shortcut.
pub fn pause_double_presses(paused: bool) {
    TAP_PAUSED.store(paused, Ordering::Relaxed);
}

/// Starts or stops passing the review panel's keys to it instead of the app. Returns false when
/// the tap isn't running (it waits for Accessibility access), so the keys can't be taken.
pub fn take_review_keys(take: bool) -> bool {
    REVIEW_KEYS.store(take, Ordering::Relaxed);
    TAP_PORT.load(Ordering::Relaxed) != 0
}

/// Names a key press for the review panel, or `None` for a key it has no name for (F-keys, Home).
fn review_key(event: &CGEvent, code: u16) -> Option<crate::review_keys::Key> {
    use foreign_types::ForeignType;
    let named = match code {
        36 | 76 => Some("Enter"),
        48 => Some("Tab"),
        49 => Some(" "),
        51 => Some("Backspace"),
        53 => Some("Escape"),
        117 => Some("Delete"),
        123 => Some("ArrowLeft"),
        124 => Some("ArrowRight"),
        125 => Some("ArrowDown"),
        126 => Some("ArrowUp"),
        _ => None,
    };
    let key = match named {
        Some(name) => name.to_string(),
        None => {
            // The character the key types in the current layout, with ⇧ and ⌥ applied, as the web's
            // KeyboardEvent.key gives it.
            let mut chars = [0u16; 4];
            let mut len = 0;
            unsafe { CGEventKeyboardGetUnicodeString(event.as_ptr() as _, chars.len(), &mut len, chars.as_mut_ptr()) };
            let typed = String::from_utf16_lossy(&chars[..len.min(chars.len())]);
            // Control characters (⌃C) and the private-use codes of function keys aren't text.
            if typed.chars().any(|c| c.is_control() || ('\u{E000}'..='\u{F8FF}').contains(&c)) {
                return None;
            }
            typed
        }
    };
    let flags = event.get_flags();
    Some(crate::review_keys::Key {
        key,
        meta_key: flags.contains(CGEventFlags::CGEventFlagCommand),
        ctrl_key: flags.contains(CGEventFlags::CGEventFlagControl),
        alt_key: flags.contains(CGEventFlags::CGEventFlagAlternate),
        shift_key: flags.contains(CGEventFlags::CGEventFlagShift),
    })
    .filter(|key| key.for_panel())
}

fn modifiers(flags: CGEventFlags) -> u8 {
    [
        (CGEventFlags::CGEventFlagCommand, double_press::COMMAND),
        (CGEventFlags::CGEventFlagControl, double_press::CONTROL),
        (CGEventFlags::CGEventFlagAlternate, double_press::ALT),
        (CGEventFlags::CGEventFlagShift, double_press::SHIFT),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// Sends a held chord on to the app, as a press and release.
fn replay(chord: &Chord, proxy: Option<core_graphics::event::CGEventTapProxy>) {
    let Some(&(_, code)) = KEY_CODES.iter().find(|(key, _)| *key == chord.key) else { return };
    let mut flags = CGEventFlags::empty();
    for (bit, flag) in [
        (double_press::COMMAND, CGEventFlags::CGEventFlagCommand),
        (double_press::CONTROL, CGEventFlags::CGEventFlagControl),
        (double_press::ALT, CGEventFlags::CGEventFlagAlternate),
        (double_press::SHIFT, CGEventFlags::CGEventFlagShift),
    ] {
        if chord.modifiers & bit != 0 {
            flags |= flag;
        }
    }
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return };
    for down in [true, false] {
        let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) else { continue };
        event.set_flags(flags);
        event.set_integer_value_field(core_graphics::event::EventField::EVENT_SOURCE_USER_DATA, REPLAY_MARK);
        match proxy {
            Some(proxy) => event.post_from_tap(proxy),
            None => event.post(CGEventTapLocation::Session),
        }
    }
}

/// Starts watching for double-press shortcuts on a thread of its own. The tap needs Accessibility
/// access, so this waits for it; `on_fire` gets the index of the chord that was pressed twice.
/// While `take_review_keys` is on, the same tap takes the review panel's keys from the app and
/// gives them to `on_review_key`.
pub fn start_double_press_tap(
    on_fire: impl Fn(usize) + Send + Sync + 'static,
    on_review_key: impl Fn(crate::review_keys::Key) + Send + Sync + 'static,
) {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventTap, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField,
    };
    use std::sync::Arc;

    let on_fire = Arc::new(on_fire);
    let on_review_key = Arc::new(on_review_key);
    let detector = Arc::new(Mutex::new(double_press::Detector::default()));
    std::thread::Builder::new()
        .name("double-press".into())
        .spawn(move || {
            loop {
                if !accessibility_trusted(false) {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                let detector = detector.clone();
                let on_fire = on_fire.clone();
                let on_review_key = on_review_key.clone();
                let tap = CGEventTap::new(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    vec![CGEventType::KeyDown],
                    move |proxy, kind, event| {
                        if matches!(kind, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
                            let port = TAP_PORT.load(Ordering::Relaxed);
                            if port != 0 {
                                unsafe { CGEventTapEnable(port as *const c_void, true) };
                            }
                            return CallbackResult::Keep;
                        }
                        if !matches!(kind, CGEventType::KeyDown)
                            || TAP_PAUSED.load(Ordering::Relaxed)
                            || event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == REPLAY_MARK
                        {
                            return CallbackResult::Keep;
                        }
                        let chords = DOUBLE_CHORDS.lock().unwrap().clone();
                        let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                        let key = KEY_CODES.iter().find(|(_, c)| *c == code).map(|(key, _)| *key);
                        let mods = modifiers(event.get_flags());
                        let matched =
                            key.and_then(|key| chords.iter().position(|c| c.key == key && c.modifiers == mods));
                        // A double-press shortcut still works during a review (⌘E E starts a new one).
                        if matched.is_none()
                            && REVIEW_KEYS.load(Ordering::Relaxed)
                            && let Some(key) = review_key(event, code)
                        {
                            on_review_key(key);
                            return CallbackResult::Drop;
                        }
                        if chords.is_empty() {
                            return CallbackResult::Keep;
                        }
                        let autorepeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
                        let step = detector.lock().unwrap().press(matched, autorepeat, Instant::now());
                        if let Some(held) = step.replay.and_then(|i| chords.get(i)) {
                            replay(held, Some(proxy));
                        }
                        if let Some(generation) = step.hold {
                            let detector = detector.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(double_press::WINDOW);
                                let held = detector.lock().unwrap().expire(generation);
                                if let Some(chord) = held.and_then(|i| DOUBLE_CHORDS.lock().unwrap().get(i).cloned()) {
                                    replay(&chord, None);
                                }
                            });
                        }
                        if let Some(index) = step.fire {
                            on_fire(index);
                        }
                        if step.pass { CallbackResult::Keep } else { CallbackResult::Drop }
                    },
                );
                let Ok(tap) = tap else {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                };
                let Ok(source) = tap.mach_port().create_runloop_source(0) else { return };
                CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
                TAP_PORT.store(tap.mach_port().as_concrete_TypeRef() as usize, Ordering::Relaxed);
                tap.enable();
                CFRunLoop::run_current();
                return;
            }
        })
        .expect("thread spawns");
}

// --- Pasteboard -----------------------------------------------------------------------------

mod pasteboard {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData, NSString};

    fn general() -> Retained<NSPasteboard> {
        NSPasteboard::generalPasteboard()
    }

    pub fn change_count() -> isize {
        general().changeCount()
    }

    pub fn read_string() -> Option<String> {
        general().stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string())
    }

    pub fn write_string(text: &str) -> isize {
        let pb = general();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        pb.changeCount()
    }

    /// Writes text marked transient so clipboard managers skip it (nspasteboard.org convention).
    pub fn write_transient_string(text: &str) -> isize {
        let pb = general();
        pb.clearContents();
        let item = NSPasteboardItem::new();
        item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        for marker in ["org.nspasteboard.TransientType", "org.nspasteboard.AutoGeneratedType"] {
            item.setData_forType(&NSData::new(), &NSString::from_str(marker));
        }
        let objects: Retained<NSArray<ProtocolObject<dyn NSPasteboardWriting>>> =
            NSArray::from_retained_slice(&[ProtocolObject::from_retained(item)]);
        pb.writeObjects(&objects);
        pb.changeCount()
    }

    /// Every item and representation on the clipboard, so it can be put back exactly.
    pub struct Snapshot {
        items: Vec<Vec<(Retained<NSString>, Retained<NSData>)>>,
    }

    unsafe impl Send for Snapshot {}

    impl Snapshot {
        pub fn take() -> Self {
            let pb = general();
            let mut items = Vec::new();
            if let Some(list) = pb.pasteboardItems() {
                for item in list.iter() {
                    let mut reps = Vec::new();
                    for kind in item.types().iter() {
                        if let Some(data) = item.dataForType(&kind) {
                            reps.push((kind.clone(), data));
                        }
                    }
                    items.push(reps);
                }
            }
            Self { items }
        }

        /// Restores the snapshot unless something else has written to the clipboard since
        /// `expected_change_count`.
        pub fn restore_if_unchanged_since(self, expected_change_count: isize) {
            let pb = general();
            if pb.changeCount() != expected_change_count {
                return;
            }
            pb.clearContents();
            if self.items.is_empty() {
                return;
            }
            let restored: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = self
                .items
                .into_iter()
                .map(|reps| {
                    let item = NSPasteboardItem::new();
                    for (kind, data) in reps {
                        item.setData_forType(&data, &kind);
                    }
                    ProtocolObject::from_retained(item)
                })
                .collect();
            pb.writeObjects(&NSArray::from_retained_slice(&restored));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // What Chrome reported for ProseMirror fields of two paragraphs around an empty one, of two
    // paragraphs, and of one paragraph with two hard breaks.
    #[test]
    fn puts_back_the_empty_paragraphs_chromium_leaves_out() {
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 0, 4]).as_deref(), Some("One.\n\nTwo."));
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 4]).as_deref(), Some("One.\nTwo."));
        assert_eq!(with_empty_paragraphs("One.\n\nTwo.", &[8]).as_deref(), Some("One.\n\nTwo."));
        assert_eq!(with_empty_paragraphs("Hi 🚀\nBye", &[0, 4, 0, 0, 3, 0]).as_deref(), Some("\nHi 🚀\n\n\nBye\n"));
    }

    #[test]
    fn leaves_a_value_that_does_not_fit_the_paragraphs() {
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 0, 5]), None);
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4]), None);
        assert_eq!(with_empty_paragraphs("One.Two.", &[4, 4]), None);
    }

    #[test]
    fn maps_a_selection_onto_the_full_text() {
        let full = "this is a first sentence.\n\nIs this a newlie?";
        assert_eq!(span_of(full, "this is a first sentence.\nIs this a newlie?").as_deref(), Some(full));
        assert_eq!(span_of("One.\nTwo.", "One.Two.").as_deref(), Some("One.\nTwo."));
        assert_eq!(span_of(full, "sentence.Is this").as_deref(), Some("sentence.\n\nIs this"));
        assert_eq!(span_of(full, "first").as_deref(), Some("first"));
    }

    #[test]
    fn keeps_a_selection_that_matches_nowhere_or_twice() {
        assert_eq!(span_of("One.\nTwo.", "Three."), None);
        assert_eq!(span_of("Go.\nGo.", "Go."), None);
        assert_eq!(span_of("One.", "\n"), None);
    }

    #[test]
    fn a_caret_past_the_end_counts_as_the_end() {
        assert_eq!(sentence_before("One. Two.", 99).map(|r| (r.location, r.length)), Some((5, 4)));
    }
}
