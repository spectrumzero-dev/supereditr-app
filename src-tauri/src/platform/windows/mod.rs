//! Windows: the platform interface.
//!
//! A review reads the selection through UI Automation, or with a guarded copy on a shortcut press
//! (`capture`), and writes a fix back only by a checked paste into a field UI Automation confirmed
//! (`replace`). Shortcuts are `RegisterHotKey` chords (`hotkey`). The panel takes keyboard focus for
//! a review started with the shortcut and gives it back when it closes (`overlay`). Windows needs no
//! permission to read other apps, so Accessibility always counts as granted.
//!
//! Positions are physical pixels on the virtual desktop, as UI Automation reports them.

mod apps;
mod capture;
mod clipboard;
pub mod hotkey;
mod input;
mod log;
pub mod login;
pub mod overlay;
mod replace;
#[cfg(test)]
mod testing;
mod uia;
mod watch;
pub mod webview;
mod window;

pub use log::{log_path, set_trace, trace};
pub use watch::{WindowWatch, mouse_button_down, pressed_on_window, watch_window};

use super::{PlatformError, Rect, ReplaceMethod, ReplaceOutcome};
use capture::{Class, Desktop, Key, Services};
use clipboard::Clipboard;
use std::sync::{LazyLock, Mutex};
use uia::{Field, PxRect};
use window::WindowRef;

/// Where a capture's text came from. Only a UI Automation capture can be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Uia,
    Copy,
    /// Settings' "Try it now" box.
    Playground,
}

#[derive(Debug, Clone)]
pub struct Capture {
    pub pid: i32,
    /// The program's file name, such as `notepad.exe`: how settings name apps on Windows.
    pub bundle_id: String,
    pub app_name: String,
    pub text: String,
    /// The selection in physical pixels.
    pub bounds: Option<Rect>,
    pub editable: bool,
    pub playground: bool,
    pub code: Vec<(usize, usize)>,
    pub source: Source,
    /// The window the text came from.
    pub window: Option<WindowRef>,
    /// The field as read, which a replacement checks hasn't changed.
    pub field: Option<Field>,
    pub class: Option<Class>,
}

impl Capture {
    pub fn protected(&self) -> Vec<(usize, usize)> {
        self.code.clone()
    }

    /// Whether `other` was read from the same place as this capture, as far as UI Automation can
    /// tell: the same window of the same process, the same field (its runtime ID, which Windows can
    /// reuse only after the field is destroyed), and the same selection in it with the same text on
    /// either side, read the same way and classed the same. A copied selection has no field to
    /// compare, and a field whose runtime ID couldn't be read never counts as the same.
    pub fn same_field(&self, other: &Capture) -> bool {
        let same_field = match (&self.field, &other.field) {
            (Some(a), Some(b)) => {
                a.pid == b.pid
                    && !a.runtime_id.is_empty()
                    && a.runtime_id == b.runtime_id
                    && (a.start, a.end) == (b.start, b.end)
                    && a.before == b.before
                    && a.after == b.after
            }
            (None, None) => true,
            _ => false,
        };
        self.source == other.source && self.window == other.window && self.class == other.class && same_field
    }

    /// How to make a copy-only app replaceable. Linux's hint; Windows has none to give.
    pub fn copy_only_hint(&self) -> Option<String> {
        None
    }

    fn read(window: WindowRef, image: &str, field: Field, class: Class, bounds: Option<Rect>) -> Self {
        Capture {
            pid: window.pid as i32,
            bundle_id: image.to_string(),
            app_name: apps::display_name(image),
            text: String::from_utf16_lossy(&field.text),
            bounds,
            editable: class == Class::KnownSafe,
            playground: false,
            code: Vec::new(),
            source: Source::Uia,
            window: Some(window),
            field: Some(field),
            class: Some(class),
        }
    }

    fn copied(window: WindowRef, image: &str, text: String, bounds: Option<Rect>) -> Self {
        Capture {
            pid: window.pid as i32,
            bundle_id: image.to_string(),
            app_name: apps::display_name(image),
            text,
            bounds,
            editable: false,
            playground: false,
            code: Vec::new(),
            source: Source::Copy,
            window: Some(window),
            field: None,
            class: None,
        }
    }
}

/// `bounds` is the "Try it now" box on screen, in physical pixels.
pub fn playground_capture(text: String, bounds: Rect) -> Capture {
    Capture {
        pid: own_pid(),
        bundle_id: String::new(),
        app_name: "SuperEditr".into(),
        text,
        bounds: Some(bounds),
        editable: true,
        playground: true,
        code: Vec::new(),
        source: Source::Playground,
        window: None,
        field: None,
        class: None,
    }
}

/// The desktop through Win32.
struct System;

impl Desktop for System {
    fn foreground(&self) -> Option<WindowRef> {
        window::foreground()
    }
    fn image_name(&self, pid: u32) -> Option<String> {
        window::image_name(pid)
    }
    fn window_class(&self, window: WindowRef) -> String {
        window::class_name(window.hwnd)
    }
    fn elevated(&self, pid: u32) -> bool {
        window::elevated(pid)
    }
    fn alive(&self, window: WindowRef) -> bool {
        window::alive(window)
    }
    fn bring_to_front(&self, window: WindowRef) -> bool {
        overlay::bring_to_front(window::hwnd(window.hwnd))
    }
    fn release_modifiers(&self) {
        input::wait_for_modifiers(std::time::Duration::from_millis(500));
    }
    fn send_ctrl(&self, key: Key) -> bool {
        match key {
            Key::C => input::ctrl(b'C'),
            Key::V => input::ctrl(b'V'),
            Key::Insert => input::ctrl(0x2D),
        }
    }
    fn caret(&self, window: WindowRef) -> Option<PxRect> {
        window::caret_rect(window).map(|r| PxRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        window::cursor().map(|p| (p.x, p.y))
    }
    fn watch_copy_keys(&self) -> capture::InputWatch {
        input::watch_copy_keys()
    }
}

static UIA: LazyLock<uia::system::System> = LazyLock::new(uia::system::System::new);
static CLIPBOARD: LazyLock<Clipboard<clipboard::system::System>> =
    LazyLock::new(|| Clipboard::new(clipboard::system::System));
/// The window in front when the latest review began, which the panel gives focus back to.
static TARGET: Mutex<Option<WindowRef>> = Mutex::new(None);
/// Held while a capture or replacement runs: each is a clipboard transaction.
static CHAIN: Mutex<()> = Mutex::new(());

fn services() -> Services<'static, clipboard::system::System> {
    Services { desktop: &System, fields: &*UIA, clipboard: &CLIPBOARD }
}

/// At launch: the shortcut thread (which also follows the window in front), and a first contact
/// with UI Automation, so the first review doesn't pay for it.
pub fn start(on_event: impl Fn(hotkey::Event) + Send + 'static) {
    hotkey::start(move |event| {
        if let hotkey::Event::Foreground(_) = event {
            std::thread::spawn(|| UIA.touch());
        }
        on_event(event)
    });
    std::thread::spawn(|| UIA.warm());
}

/// Windows lets any app at the same integrity level read other apps through UI Automation, with no
/// permission to grant.
pub fn accessibility_trusted(_prompt: bool) -> bool {
    true
}

pub fn open_accessibility_settings() {}

/// System Settings is macOS's; this platform only ever reports it closed.
pub enum SettingsWindow {
    #[allow(dead_code)]
    Front(Rect, i64),
    #[allow(dead_code)]
    Prompt,
    #[allow(dead_code)]
    Behind,
    Closed,
}

pub fn system_settings_window() -> SettingsWindow {
    SettingsWindow::Closed
}

pub fn own_bundle() -> Option<std::path::PathBuf> {
    None
}

/// Double presses need a low-level keyboard hook, which Windows SuperEditr doesn't install yet: the
/// shortcuts are chords, and the panel takes focus for its keys.
pub fn set_double_presses(_chords: Vec<crate::double_press::Chord>) {}

pub fn pause_double_presses(_paused: bool) {}

pub fn start_double_press_tap(
    _on_fire: impl Fn(usize) + Send + Sync + 'static,
    _on_review_key: impl Fn(crate::review_keys::Key) + Send + Sync + 'static,
) {
}

pub fn take_review_keys(_take: bool) -> bool {
    false
}

/// Gives the keyboard back to the window the review started in.
pub fn focus_target() {
    let target = *TARGET.lock().unwrap();
    if let Some(target) = target.filter(|&t| window::alive(t)) {
        overlay::bring_to_front(window::hwnd(target.hwnd));
    }
}

/// Gives the keyboard back to the window the review started in, if it's still `pid`'s. Not while the
/// panel shows, as after an instruction typed into it: the panel takes its keys only with the
/// keyboard, and that window coming to the front would close the panel. Closing it gives the
/// keyboard back (`overlay::hide`).
pub fn return_focus(pid: i32) {
    if overlay::showing() {
        return;
    }
    let target = *TARGET.lock().unwrap();
    if target.is_some_and(|t| t.pid as i32 == pid) {
        focus_target();
    }
}

/// After the panel closes and before `replace`: the window comes back to the front and its field has
/// focus again, or nothing is written.
pub fn return_to_field(capture: &Capture) -> bool {
    match (capture.window, &capture.field) {
        (Some(window), Some(field)) if capture.editable => {
            let started = std::time::Instant::now();
            let back = replace::hand_back(&services(), window, field);
            trace(format_args!("focus {} in {:?}", if back { "back" } else { "not back" }, started.elapsed()));
            back
        }
        _ => {
            focus_target();
            true
        }
    }
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

/// How long since the user last typed or moved the mouse in this session.
pub fn input_idle() -> std::time::Duration {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return std::time::Duration::ZERO;
    }
    // Both are milliseconds since startup, wrapping every 49.7 days.
    std::time::Duration::from_millis(u64::from(unsafe { GetTickCount() }.wrapping_sub(info.dwTime)))
}

/// Reads the selection in the window in front: through UI Automation, or, with `allow_clipboard` (a
/// shortcut press), a guarded copy. Selecting the sentence before the caret isn't done on Windows
/// yet. Runs on a worker thread.
pub fn capture(allow_clipboard: bool, _select_sentence: bool) -> Result<Capture, PlatformError> {
    let _one_at_a_time = CHAIN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *TARGET.lock().unwrap() = window::foreground().filter(|w| w.pid as i32 != own_pid());
    capture::capture(&services(), allow_clipboard)
}

pub fn focused_app_identity() -> Option<(i32, String, String)> {
    let front = window::foreground()?;
    let image = window::image_name(front.pid)?;
    let name = apps::display_name(&image);
    Some((front.pid as i32, image, name))
}

/// Pastes `text` over the captured selection. `_proofread` asks macOS to change only the words a
/// proofread fixes; Windows replaces the whole selection. Runs on a worker thread.
pub fn replace(capture: &Capture, text: &str, _proofread: bool) -> Result<ReplaceMethod, PlatformError> {
    replace_with(capture, text, replace::Clip::Restore)
}

/// Pastes `text` over the captured selection without putting the clipboard aside, after a
/// replacement that couldn't (`PlatformError::ClipboardUnrestorable`): the fix stays on the
/// clipboard. Every other check runs as for [`replace`]. Runs on a worker thread.
pub fn replace_anyway(capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
    replace_with(capture, text, replace::Clip::Leave)
}

fn replace_with(capture: &Capture, text: &str, clip: replace::Clip) -> Result<ReplaceMethod, PlatformError> {
    let _one_at_a_time = CHAIN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match replace::replace(&services(), capture, text, clip) {
        ReplaceOutcome::Applied => Ok(ReplaceMethod::Paste),
        ReplaceOutcome::CopyOnly(error) => Err(error),
        ReplaceOutcome::Uncertain(_) => Err(PlatformError::Unverified),
        ReplaceOutcome::ClipboardKept(error) => Err(error),
    }
}

/// Puts `text` on the clipboard for the user to paste. The copy is made on a thread of its own, one
/// at a time and between clipboard transactions: emptying the clipboard waits up to 5 s for an owner
/// that isn't answering, and the panel's Ctrl+C would otherwise wait on the main thread.
pub fn copy_to_clipboard(text: &str) {
    static COPIES: LazyLock<Option<std::sync::mpsc::Sender<String>>> = LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let copier = std::thread::Builder::new().name("clipboard copy".into()).spawn(move || {
            for text in rx {
                let _one_at_a_time = CHAIN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if !CLIPBOARD.copy(&text) {
                    trace(format_args!("copy: the clipboard stayed open in another app"));
                }
            }
        });
        copier.is_ok().then_some(tx)
    });
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let text = match COPIES.as_ref() {
        Some(copies) => match copies.send(text) {
            Ok(()) => return,
            Err(unsent) => unsent.0,
        },
        None => text,
    };
    if !CLIPBOARD.copy(&text) {
        trace(format_args!("copy: the clipboard stayed open in another app"));
    }
}

/// The program's clipboard-snapshot mode ([`CLIPBOARD_SNAPSHOT_COMMAND`]): reads the clipboard
/// with the limits in `args` and writes it to standard output for SuperEditr, which started it and
/// kills it at its deadline. Returns the exit code.
pub fn serve_clipboard_snapshot(args: &[String]) -> i32 {
    let mut out = std::io::stdout().lock();
    clipboard::serve_snapshot(&clipboard::system::System, args, &mut out)
}

pub use clipboard::SNAPSHOT_COMMAND as CLIPBOARD_SNAPSHOT_COMMAND;

/// The pointer, in physical pixels.
pub fn mouse_location() -> (f64, f64) {
    window::cursor().map_or((0.0, 0.0), |p| (f64::from(p.x), f64::from(p.y)))
}

pub fn frontmost_pid() -> Option<i32> {
    window::foreground().map(|w| w.pid as i32)
}

pub fn app_identity(pid: i32) -> (String, String) {
    match window::image_name(pid as u32) {
        Some(image) => {
            let name = apps::display_name(&image);
            (image, name)
        }
        None => (String::new(), "this app".into()),
    }
}

pub fn selection_app_switched() {}

/// What each key saved by virtual key types on the keyboard layout of the window in front (Settings,
/// when it asks), upper-cased: "VkBA" is "Ü" on a German layout and ";" on a US one. Named keys
/// keep SuperEditr's own names.
pub fn key_labels() -> std::collections::HashMap<String, String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MAPVK_VK_TO_VSC_EX, MapVirtualKeyExW, ToUnicodeEx,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let thread = unsafe { GetWindowThreadProcessId(GetForegroundWindow(), None) };
    let layout = unsafe { GetKeyboardLayout(thread) };
    let mut labels = std::collections::HashMap::new();
    let state = [0u8; 256];
    for vk in (0xBAu16..=0xC0).chain(0xDB..=0xDF).chain(0xE1..=0xE2) {
        let scan = unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC_EX, Some(layout)) };
        if scan == 0 {
            continue;
        }
        let mut buffer = [0u16; 8];
        // Flag 4 leaves the keyboard state alone, so a dead key isn't left pending.
        let n = unsafe { ToUnicodeEx(u32::from(vk), scan & 0xFF, &state, &mut buffer, 4, Some(layout)) };
        let typed = String::from_utf16_lossy(&buffer[..(n.unsigned_abs() as usize).min(buffer.len())]);
        let label = typed.trim().to_uppercase();
        if label.is_empty() {
            continue;
        }
        let name = crate::keys::name(vk);
        for (named, code) in [
            ("Semicolon", 0xBA),
            ("Equal", 0xBB),
            ("Comma", 0xBC),
            ("Minus", 0xBD),
            ("Period", 0xBE),
            ("Slash", 0xBF),
            ("Backquote", 0xC0),
            ("BracketLeft", 0xDB),
            ("Backslash", 0xDC),
            ("BracketRight", 0xDD),
            ("Quote", 0xDE),
        ] {
            if code == vk {
                labels.insert(named.to_string(), label.clone());
            }
        }
        labels.insert(name, label);
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_field_is_the_same_window_element_and_selection() {
        let window = WindowRef { hwnd: 0x5A0B12, pid: 4242 };
        let read = || {
            Capture::read(window, "notepad.exe", testing::field("Rhyan sent the KPI report."), Class::KnownSafe, None)
        };
        assert!(read().same_field(&read()));
        let mut other_window = read();
        other_window.window = Some(WindowRef { hwnd: 0x5A0C40, pid: 4242 });
        assert!(!read().same_field(&other_window), "another window");
        let mut other_element = read();
        other_element.field.as_mut().unwrap().runtime_id = vec![42, 0x1_0000, 8];
        assert!(!read().same_field(&other_element), "another field in the window");
        let mut moved = read();
        let field = moved.field.as_mut().unwrap();
        (field.start, field.end) = (4, 30);
        assert!(!read().same_field(&moved), "the same text elsewhere in the field");
        let mut duplicate = read();
        duplicate.field.as_mut().unwrap().before = "Brynlee wrote: ".encode_utf16().collect();
        assert!(!read().same_field(&duplicate), "an identical sentence with other text around it");
        let mut unknown = read();
        unknown.field.as_mut().unwrap().runtime_id.clear();
        assert!(!unknown.same_field(&unknown.clone()), "a field with no runtime ID is never known to be the same");
        let copied = || Capture::copied(window, "notepad.exe", "Rhyan sent the KPI report.".into(), None);
        assert!(copied().same_field(&copied()), "two copies from the same window");
        assert!(!read().same_field(&copied()), "read by copying");
        let mut classed = read();
        classed.class = Some(Class::CopyOnly(capture::Why::ReadOnly));
        assert!(!read().same_field(&classed), "classed differently");
    }
}
