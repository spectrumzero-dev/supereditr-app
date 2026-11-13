//! Global shortcuts with `RegisterHotKey`, on a thread with a message-only window, and the window
//! events SuperEditr follows from the same thread.
//!
//! - Every registration adds `MOD_NOREPEAT`, so holding a chord fires it once.
//! - A shortcut is registered by the virtual key Windows reported when it was recorded, so it
//!   follows the layout the way Windows matches hotkeys: by virtual key, in the layout of the app
//!   in front.
//! - `ERROR_HOTKEY_ALREADY_REGISTERED` means Windows or another app has the chord.
//! - The thread also follows the window in front (`EVENT_SYSTEM_FOREGROUND`), which closes the
//!   panel when the user moves to another app, and its window owns what SuperEditr puts on the
//!   clipboard, since it answers clipboard messages.
//!
//! The thread never runs app code: shortcuts and window changes go to `on_event`'s own thread, so
//! the message loop can't wait on the main thread while the main thread waits on it.

use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, GetWindowThreadProcessId,
    HWND_MESSAGE, MSG, PostMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_HOTKEY, WNDCLASSW,
};
use windows::core::w;

/// A chord: `MOD_*` flags and a virtual key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

impl Chord {
    fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT;
        for (on, flag) in
            [(self.control, MOD_CONTROL), (self.alt, MOD_ALT), (self.shift, MOD_SHIFT), (self.win, MOD_WIN)]
        {
            if on {
                m |= flag;
            }
        }
        m
    }
}

impl From<crate::keys::Chord> for Chord {
    fn from(chord: crate::keys::Chord) -> Self {
        use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};
        Chord {
            control: chord.modifiers & CONTROL != 0,
            alt: chord.modifiers & ALT != 0,
            shift: chord.modifiers & SHIFT != 0,
            win: chord.modifiers & COMMAND != 0,
            vk: chord.vk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// Windows or another app already has this chord.
    Taken,
    Failed(String),
}

/// What the thread hears about.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    Hotkey(i32),
    /// A window of this process came to the front.
    Foreground(u32),
}

enum Request {
    Register(i32, Chord, Sender<Result<(), RegisterError>>),
    Unregister(i32),
}

const WAKE: u32 = WM_APP + 7;

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static REQUESTS: Mutex<Option<Sender<Request>>> = Mutex::new(None);
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();
/// When the last shortcut arrived, in milliseconds since the thread started.
static LAST_HOTKEY: AtomicU64 = AtomicU64::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();

/// The message-only window, for the clipboard's owner. 0 until the thread runs.
pub fn window() -> isize {
    WINDOW.load(Ordering::Relaxed)
}

/// How long ago the last shortcut was pressed.
pub fn since_last_hotkey() -> Option<Duration> {
    let at = LAST_HOTKEY.load(Ordering::Relaxed);
    let started = STARTED.get()?;
    (at != 0).then(|| started.elapsed().saturating_sub(Duration::from_millis(at)))
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_HOTKEY {
        if let Some(started) = STARTED.get() {
            LAST_HOTKEY.store(started.elapsed().as_millis().max(1) as u64, Ordering::Relaxed);
        }
        if let Some(events) = EVENTS.get() {
            let _ = events.send(Event::Hotkey(wparam.0 as i32));
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if let Some(events) = EVENTS.get() {
        let _ = events.send(Event::Foreground(pid));
    }
}

/// Starts the thread. `on_event` runs on a thread of its own, one event at a time.
pub fn start(on_event: impl Fn(Event) + Send + 'static) {
    if STARTED.set(Instant::now()).is_err() {
        return;
    }
    let (events, inbox) = channel();
    let _ = EVENTS.set(events);
    let _ = std::thread::Builder::new().name("shortcut events".into()).spawn(move || {
        for event in inbox {
            on_event(event);
        }
    });
    let (requests, queue) = channel::<Request>();
    *REQUESTS.lock().unwrap() = Some(requests);
    let (ready, wait) = channel();
    let _ = std::thread::Builder::new().name("shortcuts".into()).spawn(move || unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: w!("SuperEditrShortcuts"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SuperEditrShortcuts"),
            w!("SuperEditr shortcuts"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_default();
        WINDOW.store(window.0 as isize, Ordering::Relaxed);
        super::clipboard::system::OWNER.store(window.0 as isize, Ordering::Relaxed);
        let _hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = ready.send(());
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.message == WAKE && message.hwnd == window {
                while let Ok(request) = queue.try_recv() {
                    handle(window, request);
                }
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
    let _ = wait.recv_timeout(Duration::from_secs(2));
}

fn handle(window: HWND, request: Request) {
    match request {
        Request::Register(id, chord, reply) => {
            let result = unsafe { RegisterHotKey(Some(window), id, chord.modifiers(), u32::from(chord.vk)) };
            let _ = reply.send(result.map_err(|error| {
                if error.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() {
                    RegisterError::Taken
                } else {
                    RegisterError::Failed(error.message())
                }
            }));
        }
        Request::Unregister(id) => {
            let _ = unsafe { UnregisterHotKey(Some(window), id) };
        }
    }
}

fn send(request: Request) -> bool {
    let Some(requests) = REQUESTS.lock().unwrap().clone() else { return false };
    if requests.send(request).is_err() {
        return false;
    }
    let window = HWND(window() as *mut _);
    unsafe { PostMessageW(Some(window), WAKE, WPARAM(0), LPARAM(0)) }.is_ok()
}

/// Registers `chord` under `id`.
pub fn register(id: i32, chord: Chord) -> Result<(), RegisterError> {
    let (reply, answer) = channel();
    if !send(Request::Register(id, chord, reply)) {
        return Err(RegisterError::Failed("the shortcut thread isn't running".into()));
    }
    answer
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| Err(RegisterError::Failed("the shortcut thread didn’t answer".into())))
}

pub fn unregister(id: i32) {
    send(Request::Unregister(id));
}

/// The id a test registration uses, outside the ones SuperEditr hands out.
const PROBE: i32 = 0xBFFE;

/// Whether `chord` can be registered now: registers it under a spare id and removes it at once.
pub fn available(chord: Chord) -> Result<(), RegisterError> {
    register(PROBE, chord)?;
    unregister(PROBE);
    Ok(())
}
