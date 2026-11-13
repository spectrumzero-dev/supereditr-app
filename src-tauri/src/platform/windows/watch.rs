//! The window a review belongs to, which the panel follows (`crate::follow`).
//!
//! A watch hooks the window's own process and thread with `SetWinEventHook`, out of context, on a
//! thread of its own with a message loop:
//!
//! - `EVENT_SYSTEM_MOVESIZESTART` and `EVENT_SYSTEM_MOVESIZEEND`: the user is moving or sizing the
//!   window by its frame, in Windows' own move loop. Nothing settles while that lasts, and its end
//!   settles a move at once: the button is up and the window is where the user left it.
//! - `EVENT_OBJECT_LOCATIONCHANGE` for the window itself (`OBJID_WINDOW`): it moved. The first
//!   report that finds it somewhere new starts a move (`WindowChange::Moving`). A move with no end
//!   event (Win+arrow, a snap, an app moving its own window) settles once the mouse button is up
//!   and no report has come for [`SETTLE`].
//! - `EVENT_SYSTEM_MINIMIZESTART`, and `EVENT_OBJECT_HIDE` or `EVENT_OBJECT_DESTROY` for the window
//!   itself: it went.
//!
//! With no move under way the thread waits in `GetMessageW`, so nothing runs while nothing moves;
//! only while a move without an end event is under way does it look at the clock, every
//! [`SETTLE_CHECK`]. Dropping the watch posts `WM_QUIT` to the thread, which unhooks and ends.

use super::trace;
use super::window::{self, WindowRef};
use crate::platform::{Capture, Rect, WindowChange};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GA_ROOT, GetAncestor,
    GetMessageW, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, OBJID_WINDOW, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
    PostThreadMessageW, QS_ALLINPUT, SM_SWAPBUTTON, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT,
    WindowFromPoint,
};

/// A window with the mouse button up and no move for this long has settled, when Windows didn't
/// say its move ended. Long enough to ride out a pause between an animation's steps, short enough
/// that the panel follows soon after the window stops.
const SETTLE: Duration = Duration::from_millis(150);
/// While such a move is under way, how often to look whether it has settled.
const SETTLE_CHECK: Duration = Duration::from_millis(30);
/// Posted by the thread to itself so its message loop looks again. `GetMessageW` delivers window
/// events without returning, so without this a move that starts while it waits would never settle.
const WAKE: u32 = WM_APP + 0x47;

/// Watches the window a review's text is in, until dropped.
pub struct WindowWatch {
    hwnd: isize,
    /// The watch's thread, which ends when it gets `WM_QUIT`.
    thread: u32,
    /// Set by the thread, under the lock, as it ends (the window went). While it's false the thread
    /// is still running, so its id can't belong to another thread yet.
    ended: Arc<Mutex<bool>>,
}

impl WindowWatch {
    /// The window's frame now, in physical pixels.
    pub fn frame(&self) -> Option<Rect> {
        frame(self.hwnd)
    }
}

impl Drop for WindowWatch {
    fn drop(&mut self) {
        // Never waits for the thread: its own event handler may be what's dropping the watch.
        let ended = self.ended.lock().unwrap_or_else(PoisonError::into_inner);
        if !*ended {
            let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }
}

fn to_rect(r: RECT) -> Rect {
    Rect {
        x: f64::from(r.left),
        y: f64::from(r.top),
        width: f64::from(r.right - r.left),
        height: f64::from(r.bottom - r.top),
    }
}

/// A top-level window's frame in physical pixels, None once it's minimised or gone. The frame
/// includes the invisible resize border; only its moves count, and they're the same.
fn frame(handle: isize) -> Option<Rect> {
    let hwnd = window::hwnd(handle);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        Some(to_rect(rect))
    }
}

/// Whether the primary mouse button is down. `GetAsyncKeyState` reads the physical buttons, so
/// with the buttons swapped in Settings the primary one is the right.
pub fn mouse_button_down() -> bool {
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let button = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(i32::from(button.0)) } as u16) & 0x8000 != 0
}

/// Whether the press that brought app `pid` to the front landed on its window at `frame`: the
/// button is still down, and the top-level window under the pointer is that app's, at that size.
/// Only the size is compared, since a drag may already have moved it.
pub fn pressed_on_window(pid: i32, frame: Rect) -> bool {
    if !mouse_button_down() {
        return false;
    }
    let Some(cursor) = window::cursor() else { return false };
    let root = unsafe { GetAncestor(WindowFromPoint(cursor), GA_ROOT) };
    if root.0.is_null() {
        return false;
    }
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(root, Some(&mut owner)) };
    let Some(found) = self::frame(root.0 as isize) else { return false };
    let same = |a: f64, b: f64| (a - b).abs() <= 2.0;
    let on = owner as i32 == pid && same(found.width, frame.width) && same(found.height, frame.height);
    if !on {
        let whose = if owner as i32 == pid { "another of its app's windows" } else { "another app's window" };
        trace(format_args!("follow: the press landed on {whose}"));
    }
    on
}

type OnChange = Rc<dyn Fn(WindowChange)>;

/// What the watch's thread knows, owned by that thread.
struct Watched {
    hwnd: HWND,
    on_change: OnChange,
    /// Inside Windows' move loop: the user is dragging or sizing the window by its frame.
    sizing: bool,
    /// When the last move came, while one is under way.
    moving: Option<Instant>,
    /// When the move under way started, for the log.
    started: Option<Instant>,
    /// Where the window was when it last settled.
    settled: RECT,
    gone: bool,
}

thread_local! {
    static WATCHED: RefCell<Option<Watched>> = const { RefCell::new(None) };
}

/// Starts watching the window `capture` came from, calling `on_change` on the watch's thread when it
/// starts moving, settles or goes, and returns the watch with the window's frame now.
pub fn watch_window(
    capture: &Capture,
    on_change: impl Fn(WindowChange) + Send + 'static,
) -> Option<(WindowWatch, Rect)> {
    if capture.playground {
        return None;
    }
    let target: WindowRef = capture.window.filter(|&w| window::alive(w))?;
    let hwnd = window::hwnd(target.hwnd);
    let start = frame(target.hwnd)?;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, None) };
    let (ready, wait) = std::sync::mpsc::channel();
    let handle = target.hwnd;
    let ended = Arc::new(Mutex::new(false));
    let thread_ended = ended.clone();
    std::thread::Builder::new()
        .name("window-watch".into())
        .spawn(move || {
            watch(handle, target.pid, thread, Box::new(on_change), ready);
            *thread_ended.lock().unwrap_or_else(PoisonError::into_inner) = true;
        })
        .ok()?;
    let watcher = wait.recv_timeout(Duration::from_secs(1)).ok().flatten()?;
    trace(format_args!(
        "follow: watching the reviewed window, {} × {} at {}, {}",
        start.width, start.height, start.x, start.y
    ));
    Some((WindowWatch { hwnd: handle, thread: watcher, ended }, start))
}

/// The watch's thread: hooks the window's events, runs the message loop that delivers them, and
/// unhooks when the watch is dropped or the window goes.
fn watch(
    handle: isize,
    pid: u32,
    thread: u32,
    on_change: Box<dyn Fn(WindowChange) + Send>,
    ready: std::sync::mpsc::Sender<Option<u32>>,
) {
    let hwnd = window::hwnd(handle);
    let mut message = MSG::default();
    // Makes this thread's message queue, so the watch's WM_QUIT can't arrive before it exists.
    let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
    let ranges = [
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZESTART),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
    ];
    let hooks: Vec<HWINEVENTHOOK> = ranges
        .iter()
        .map(|&(min, max)| unsafe {
            SetWinEventHook(min, max, None, Some(window_event), pid, thread, WINEVENT_OUTOFCONTEXT)
        })
        .collect();
    let unhook = |hooks: &[HWINEVENTHOOK]| {
        for &hook in hooks {
            if !hook.is_invalid() {
                let _ = unsafe { UnhookWinEvent(hook) };
            }
        }
    };
    if hooks.iter().any(|hook| hook.is_invalid()) {
        trace(format_args!("follow: couldn't watch the reviewed window"));
        unhook(&hooks);
        let _ = ready.send(None);
        return;
    }
    let mut settled = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut settled) };
    // Only this thread calls it; `Rc` keeps it callable without holding the state borrowed.
    let on_change: OnChange = Rc::from(on_change as Box<dyn Fn(WindowChange)>);
    WATCHED.with_borrow_mut(|watched| {
        *watched = Some(Watched { hwnd, on_change, sizing: false, moving: None, started: None, settled, gone: false })
    });
    let _ = ready.send(Some(unsafe { GetCurrentThreadId() }));

    'run: loop {
        let waiting = WATCHED.with_borrow(|w| w.as_ref().is_some_and(|w| w.moving.is_some() && !w.sizing));
        if waiting {
            unsafe {
                MsgWaitForMultipleObjectsEx(None, SETTLE_CHECK.as_millis() as u32, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        break 'run;
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let quiet = WATCHED.with_borrow(|w| {
                w.as_ref().is_some_and(|w| !w.sizing && w.moving.is_some_and(|last| last.elapsed() >= SETTLE))
            });
            if quiet && !mouse_button_down() {
                settle("no move for 150 ms");
            }
        } else {
            unsafe {
                if !GetMessageW(&mut message, None, 0, 0).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if WATCHED.with_borrow(|w| w.as_ref().is_none_or(|w| w.gone)) {
            break;
        }
    }
    unhook(&hooks);
    WATCHED.with_borrow_mut(Option::take);
}

/// The window has settled where it is now: reports it, once, if it moved since it last settled.
fn settle(how: &str) {
    let report = WATCHED.with_borrow_mut(|watched| {
        let w = watched.as_mut()?;
        w.moving = None;
        let started = w.started.take();
        let mut now = RECT::default();
        unsafe { GetWindowRect(w.hwnd, &mut now) }.ok()?;
        if unsafe { IsIconic(w.hwnd) }.as_bool() || now == w.settled {
            return None;
        }
        w.settled = now;
        Some((w.on_change.clone(), to_rect(now), started))
    });
    if let Some((on_change, frame, started)) = report {
        let took = started.map(|s| s.elapsed().as_millis()).unwrap_or_default();
        trace(format_args!(
            "follow: the window settled at {}, {} ({how}, {took} ms after it started moving)",
            frame.x, frame.y
        ));
        on_change(WindowChange::Moved(frame));
    }
}

/// The window's event hook, on the watch's thread.
unsafe extern "system" fn window_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    let itself = object == OBJID_WINDOW.0 && child == CHILDID_SELF as i32;
    let change = WATCHED.with_borrow_mut(|watched| {
        let w = watched.as_mut().filter(|w| !w.gone && w.hwnd == hwnd)?;
        match event {
            EVENT_SYSTEM_MOVESIZESTART => {
                w.sizing = true;
                None
            }
            EVENT_SYSTEM_MOVESIZEEND => {
                w.sizing = false;
                // Settled below, once this borrow ends.
                Some(None)
            }
            EVENT_OBJECT_LOCATIONCHANGE if itself => {
                let mut now = RECT::default();
                if unsafe { GetWindowRect(hwnd, &mut now) }.is_err() || unsafe { IsIconic(hwnd) }.as_bool() {
                    return None;
                }
                if now == w.settled && w.moving.is_none() {
                    return None;
                }
                let first = w.moving.replace(Instant::now()).is_none();
                if first {
                    w.started = Some(Instant::now());
                    return Some(Some((w.on_change.clone(), WindowChange::Moving)));
                }
                None
            }
            EVENT_SYSTEM_MINIMIZESTART => {
                w.gone = true;
                Some(Some((w.on_change.clone(), WindowChange::Gone)))
            }
            EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY if itself => {
                w.gone = true;
                Some(Some((w.on_change.clone(), WindowChange::Gone)))
            }
            _ => None,
        }
    });
    match change {
        Some(Some((on_change, change))) => {
            match change {
                WindowChange::Moving => trace(format_args!("follow: the window started moving")),
                WindowChange::Gone => trace(format_args!("follow: the window went (event {event:#06x})")),
                WindowChange::Moved(_) => {}
            }
            on_change(change);
            // A move under way, or the window gone: the loop has to look again.
            let _ = unsafe { PostThreadMessageW(GetCurrentThreadId(), WAKE, WPARAM(0), LPARAM(0)) };
        }
        Some(None) => settle("its move ended"),
        None => {}
    }
}
