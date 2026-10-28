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
            .or_else(|| self.marker_selection().map(|(text, _)| text).filter(real))
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
    if is_chromium_browser(bundle_id) || bundle_id == "com.apple.Safari" || bundle_id.starts_with("org.mozilla.") {
        return true;
    }
    app_bundle_path(pid).is_some_and(|path| {
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

/// Reads the current selection. With `allow_clipboard`, falls back to a guarded ⌘C when the
/// app doesn't expose its selection through Accessibility.
pub fn capture(allow_clipboard: bool) -> Result<Capture, PlatformError> {
    if !accessibility_trusted(false) {
        return Err(PlatformError::NotTrusted);
    }
    let Some((app, pid)) = focused_app() else {
        trace(format_args!("no app in front"));
        return Err(PlatformError::NoSelection);
    };
    let (bundle_id, app_name) = app_identity(pid);
    let started = Instant::now();
    let result = match capture_ax(&app, pid, &bundle_id, &app_name) {
        Ok(capture) => Ok(capture),
        Err(PlatformError::SecureField) => Err(PlatformError::SecureField),
        Err(error) => {
            trace(format_args!("{bundle_id}: Accessibility found no selection after {:?}", started.elapsed()));
            if !allow_clipboard {
                return Err(error);
            }
            capture_clipboard(&app, pid, bundle_id.clone(), app_name)
        }
    };
    match &result {
        Ok(capture) => trace(format_args!(
            "{bundle_id}: read {} characters via {} in {:?}",
            capture.text.chars().count(),
            if capture.element.is_some() { "Accessibility" } else { "⌘C" },
            started.elapsed()
        )),
        Err(error) => trace(format_args!("{bundle_id}: {error} ({:?})", started.elapsed())),
    }
    result
}

/// Appends one line about a selection read or write to ~/Library/Logs/SuperEditr/capture.log, so a
/// failure in a particular app can be diagnosed. It records app IDs, timings and lengths, never text.
fn trace(message: std::fmt::Arguments) {
    use std::io::Write;
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::Path::new(&home).join("Library/Logs/SuperEditr");
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
        let still_focused = focused_app()
            .filter(|(_, pid)| *pid == capture.pid)
            .and_then(|(app, _)| app.element("AXFocusedUIElement"))
            .is_some_and(|focus| is_same_or_descendant(&focus, element));
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
            if landed {
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
    // The app's own Paste menu item first. The review panel took key to read ↩, and when it hides
    // macOS can leave the app in front with no key window. Browsers and Electron apps then drop a
    // posted ⌘V, but a menu item still acts on the main window.
    let app = Element::from_create(unsafe { AXUIElementCreateApplication(capture.pid) });
    let how = if app.as_ref().and_then(|app| press_menu_item(app, "v")) == Some(true) {
        "the app's Paste menu item"
    } else {
        post_command_key(capture.pid, Key::V);
        "⌘V to the app's process"
    };
    let landed = verify_replacement(element, capture, before.as_deref(), text, Duration::from_millis(600));
    trace(format_args!(
        "{}: pasted {} characters with {how}, {window}; {} after {:?}, field value {}",
        capture.bundle_id,
        text.chars().count(),
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

/// Whether `pid` is the front app and its focused window is the main, focused one, for the log.
fn key_window_state(pid: i32) -> String {
    let front = focused_app().is_some_and(|(_, front)| front == pid);
    let window = Element::from_create(unsafe { AXUIElementCreateApplication(pid) })
        .and_then(|app| app.element("AXFocusedWindow"));
    let flag = |name| window.as_ref().and_then(|w| w.flag(name)).map_or("unknown".to_string(), |v| v.to_string());
    format!("front {front}, window main {} focused {}", flag("AXMain"), flag("AXFocused"))
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
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return };
    for down in [true, false] {
        if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) {
            // Explicit flags so modifiers still held from the hotkey don't leak into the paste.
            event.set_flags(CGEventFlags::CGEventFlagCommand);
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
            Input::KeyDown { navigation: navigation(), shift }
        } else if kind == NSEventType::KeyUp {
            let select_all = flags.contains(NSEventModifierFlags::Command) && event.keyCode() == KEY_A;
            Input::KeyUp { navigation: navigation(), shift, select_all }
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

unsafe extern "C" {
    fn CGEventTapEnable(tap: *const c_void, enable: bool);
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
pub fn start_double_press_tap(on_fire: impl Fn(usize) + Send + Sync + 'static) {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventTap, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField,
    };
    use std::sync::Arc;

    let on_fire = Arc::new(on_fire);
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
                        if chords.is_empty() {
                            return CallbackResult::Keep;
                        }
                        let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                        let key = KEY_CODES.iter().find(|(_, c)| *c == code).map(|(key, _)| *key);
                        let mods = modifiers(event.get_flags());
                        let matched =
                            key.and_then(|key| chords.iter().position(|c| c.key == key && c.modifiers == mods));
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
