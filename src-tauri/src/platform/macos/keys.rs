//! The keyboard: the current layout's key codes, posting keys, and the event tap that watches for
//! double-press shortcuts and passes the review panel its keys.

use super::ax::{AXUIElementCreateSystemWide, Element};
use super::{accessibility_trusted, own_pid, trace};
use crate::double_press::{self, Chord};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::data::CFData;
use core_foundation::string::CFStringRef;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

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

// --- Keyboard -------------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) enum Key {
    C,
    V,
}

static KEY_C: AtomicU16 = AtomicU16::new(8);
static KEY_V: AtomicU16 = AtomicU16::new(9);
/// What each key types with ⌘ held in the current layout.
static COMMAND_KEYS: Mutex<Vec<(u16, char)>> = Mutex::new(Vec::new());

/// Finds the key codes that type "c" and "v" in the current layout, so ⌘C/⌘V work on Dvorak,
/// AZERTY and others, and what each key means with ⌘ held. Text Input Sources must be queried on
/// the main thread.
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
            let typed = |code: u16, modifiers: u32| {
                let mut dead = 0u32;
                let mut buf = [0u16; 4];
                let mut len = 0usize;
                let result =
                    UCKeyTranslate(ptr, code, 3, modifiers, kbd, 1, &mut dead, buf.len(), &mut len, buf.as_mut_ptr());
                (result == 0 && len == 1).then_some(buf[0])
            };
            let mut command_keys = Vec::new();
            for code in 0u16..128 {
                match typed(code, 0) {
                    Some(0x63) => KEY_C.store(code, Ordering::Relaxed),
                    Some(0x76) => KEY_V.store(code, Ordering::Relaxed),
                    _ => {}
                }
                // 1 is ⌘ in the form UCKeyTranslate takes modifiers. Some layouts, such as Dvorak's
                // "QWERTY ⌘", change their keys while it is held.
                if let Some(key) = typed(code, 1).and_then(|unit| char::from_u32(unit.into())) {
                    command_keys.push((code, key));
                }
            }
            *crate::platform::lock(&COMMAND_KEYS) = command_keys;
        }
        CFRelease(source as CFTypeRef);
    }
}

pub(super) fn post_command_key(pid: i32, key: Key) {
    let code = match key {
        Key::C => KEY_C.load(Ordering::Relaxed),
        Key::V => KEY_V.load(Ordering::Relaxed),
    };
    post_key(pid, code, CGEventFlags::CGEventFlagCommand);
}

/// Sends one key press with exactly `flags`, to `pid`, or as a keystroke to the front app when 0.
pub(super) fn post_key(pid: i32, code: u16, flags: CGEventFlags) -> bool {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else { return false };
    for down in [true, false] {
        let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) else { return false };
        // Explicit flags so modifiers still held from the hotkey don't leak into the key.
        event.set_flags(flags);
        if !accessibility_trusted(false) {
            return false;
        }
        if pid > 0 {
            event.post_to_pid(pid);
        } else {
            event.post(CGEventTapLocation::HID);
        }
    }
    true
}

// --- Double-press shortcuts ---------------------------------------------------------------

/// Marks events SuperEditr replays, so its own tap lets them through.
const REPLAY_MARK: i64 = 0x5345_4454; // "SEDT"
static DOUBLE_CHORDS: Mutex<Vec<Chord>> = Mutex::new(Vec::new());
static TAP_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Which keys the tap takes from the app for the panel (see `crate::review_keys`): none, esc alone
/// for a suggestion that opened by itself, or all the panel's keys for a review the user opened.
static REVIEW_KEYS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(TAP_OFFLINE);
const TAKE_NONE: u8 = 0;
const TAKE_ESCAPE: u8 = 1;
const TAKE_ALL: u8 = 2;
const TAP_OFFLINE: u8 = 3;

unsafe extern "C" {
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
    *crate::platform::lock(&DOUBLE_CHORDS) = chords;
}

/// Lets every key through, for example while Settings records a new shortcut.
pub fn pause_double_presses(paused: bool) {
    TAP_PAUSED.store(paused, Ordering::Relaxed);
}

/// Starts or stops passing the review panel's keys to it instead of the app. Returns false when
/// the tap isn't running (it waits for Accessibility access), so the keys can't be taken.
pub fn take_review_keys(take: bool) -> bool {
    set_review_keys(if take { TAKE_ALL } else { TAKE_NONE })
}

fn set_review_keys(keys: u8) -> bool {
    REVIEW_KEYS.try_update(Ordering::Relaxed, Ordering::Relaxed, |state| (state != TAP_OFFLINE).then_some(keys)).is_ok()
}

/// The app under review, or 0 for none: the tap takes the panel's keys only while this app has the
/// keyboard.
static REVIEW_APP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// The app that has the keyboard, as last read by `watch_keyboard_app`, or 0 when it isn't known.
static KEYBOARD_APP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// How often the app with the keyboard is read while the panel takes keys. Short, so the first
/// key typed into a launcher that has just opened already goes to it.
const KEYBOARD_APP_EVERY: Duration = Duration::from_millis(40);

/// Names the app whose review the panel shows, or 0 for a notice, which belongs to no app.
pub fn set_review_app(pid: i32) {
    static WATCHING: std::sync::Once = std::sync::Once::new();
    REVIEW_APP.store(pid, Ordering::Relaxed);
    KEYBOARD_APP.store(0, Ordering::Relaxed);
    WATCHING.call_once(|| {
        let _ = std::thread::Builder::new().name("keyboard-app".into()).spawn(watch_keyboard_app);
    });
}

/// Keeps `KEYBOARD_APP` current while the panel takes keys for a review. The keyboard tap must
/// answer at once: a tap that waits on Accessibility stalls typing, and macOS turns off a tap that
/// is slow. So the question is asked here, and the tap reads the last answer.
fn watch_keyboard_app() {
    loop {
        let taking = matches!(REVIEW_KEYS.load(Ordering::Relaxed), TAKE_ALL | TAKE_ESCAPE)
            && REVIEW_APP.load(Ordering::Relaxed) != 0;
        if !taking {
            KEYBOARD_APP.store(0, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let pid = Element::from_create(unsafe { AXUIElementCreateSystemWide() })
            .and_then(|system| system.element("AXFocusedApplication"))
            .and_then(|app| app.pid())
            .unwrap_or(0);
        KEYBOARD_APP.store(pid, Ordering::Relaxed);
        std::thread::sleep(KEYBOARD_APP_EVERY);
    }
}

/// Whether something other than the app under review has the keyboard (see
/// `review_keys::keyboard_is_elsewhere`), from the last read.
fn keyboard_is_elsewhere() -> bool {
    crate::review_keys::keyboard_is_elsewhere(
        REVIEW_APP.load(Ordering::Relaxed),
        KEYBOARD_APP.load(Ordering::Relaxed),
        own_pid(),
    )
}

/// A suggestion that opened by itself shows: the tap takes esc alone from the app, to close the
/// panel, and every other key still reaches the app. Without the tap, esc reaches the app too.
pub fn take_escape_key() {
    set_review_keys(TAKE_ESCAPE);
}

/// Names a key press the panel takes while it takes all its keys (`all`) or esc alone, or `None`
/// for a key it leaves to the app or has no name for (F-keys, Home).
fn review_key(event: &CGEvent, code: u16, all: bool) -> Option<crate::review_keys::Key> {
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
    let shortcut = || {
        // ⌘ alone is named by what the key types with ⌘ held. The event's own text can't be relied
        // on: a ⌘C posted to the session arrived without its "c", so it went to the app.
        let command_keys = crate::platform::lock(&COMMAND_KEYS);
        let key = command_keys.iter().find(|(c, _)| *c == code).map(|(_, key)| key.to_string());
        key.filter(|_| modifiers(event.get_flags()) == double_press::COMMAND)
    };
    let key = match named.map(str::to_string).or_else(shortcut) {
        Some(key) => key,
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
    .filter(|key| if all { key.for_panel() } else { key.closes_panel() })
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
        if !accessibility_trusted(false) {
            return;
        }
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
    use core_foundation::runloop::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode};
    use core_graphics::event::{
        CGEventTap, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField,
    };
    use std::sync::Arc;

    let on_fire = Arc::new(on_fire);
    let on_review_key = Arc::new(on_review_key);
    std::thread::Builder::new()
        .name("double-press".into())
        .spawn(move || {
            loop {
                if !accessibility_trusted(false) {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                let detector = Arc::new(Mutex::new(double_press::Detector::default()));
                let callback_detector = detector.clone();
                let on_fire = on_fire.clone();
                let on_review_key = on_review_key.clone();
                let tap = CGEventTap::new(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    vec![CGEventType::KeyDown],
                    move |proxy, kind, event| {
                        if matches!(kind, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
                            // Return to the owner loop to recheck trust before enabling the tap.
                            CFRunLoop::get_current().stop();
                            return CallbackResult::Keep;
                        }
                        if !matches!(kind, CGEventType::KeyDown)
                            || TAP_PAUSED.load(Ordering::Relaxed)
                            || event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == REPLAY_MARK
                        {
                            return CallbackResult::Keep;
                        }
                        let chords = crate::platform::lock(&DOUBLE_CHORDS).clone();
                        let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                        let key = KEY_CODES.iter().find(|(_, c)| *c == code).map(|(key, _)| *key);
                        let mods = modifiers(event.get_flags());
                        let matched =
                            key.and_then(|key| chords.iter().position(|c| c.key == key && c.modifiers == mods));
                        // A double-press shortcut still works during a review (⌘E E starts a new one).
                        let taking = REVIEW_KEYS.load(Ordering::Relaxed);
                        if matched.is_none() && taking != TAKE_NONE {
                            let key = review_key(event, code, taking == TAKE_ALL).filter(|_| !keyboard_is_elsewhere());
                            // Shortcuts are what can reach the wrong place. A key code, so never text.
                            if taking == TAKE_ALL && mods & (double_press::COMMAND | double_press::CONTROL) != 0 {
                                let to = if key.is_some() { "the panel" } else { "the app" };
                                trace(format_args!("review: ⌘ or ⌃ with key code {code} goes to {to}"));
                            }
                            if let Some(key) = key {
                                on_review_key(key);
                                return CallbackResult::Drop;
                            }
                        }
                        if chords.is_empty() {
                            return CallbackResult::Keep;
                        }
                        let autorepeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
                        let step = callback_detector.lock().unwrap().press(matched, autorepeat, Instant::now());
                        if let Some(held) = step.replay.and_then(|i| chords.get(i)) {
                            replay(held, Some(proxy));
                        }
                        if let Some(generation) = step.hold {
                            let detector = callback_detector.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(double_press::WINDOW);
                                let mut detector = detector.lock().unwrap();
                                let held = detector.expire(generation);
                                if let Some(chord) =
                                    held.and_then(|i| crate::platform::lock(&DOUBLE_CHORDS).get(i).cloned())
                                {
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
                let Ok(source) = tap.mach_port().create_runloop_source(0) else {
                    drop(tap);
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                };
                let run_loop = CFRunLoop::get_current();
                let mode = unsafe { kCFRunLoopDefaultMode };
                run_loop.add_source(&source, mode);
                REVIEW_KEYS.store(TAKE_NONE, Ordering::Relaxed);
                let mut enable = true;
                while accessibility_trusted(false) {
                    if enable {
                        tap.enable();
                    }
                    match CFRunLoop::run_in_mode(mode, Duration::from_secs(1), false) {
                        CFRunLoopRunResult::Stopped => enable = true,
                        CFRunLoopRunResult::TimedOut => enable = false,
                        _ => break,
                    }
                }
                REVIEW_KEYS.store(TAP_OFFLINE, Ordering::Relaxed);
                run_loop.remove_source(&source, mode);
                drop(tap); // Invalidates its Mach port.
                // A pending timeout belongs to this tap. Cancel it before installing another one.
                *detector.lock().unwrap() = double_press::Detector::default();
            }
        })
        .expect("thread spawns");
}
