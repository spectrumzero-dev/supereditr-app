//! Watching the window a review's text is in, through an Accessibility observer and the window
//! server.

use super::ax::{AXError, AXUIElementCreateApplication, AXUIElementRef, Element, kAXErrorSuccess, to_rect};
use super::settings_window::CGRectMakeWithDictionaryRepresentation;
use super::watch::mouse_button_down;
use super::{Capture, trace};
use crate::platform::Rect;
use core_foundation::base::{CFRelease, CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

// --- The reviewed window ---------------------------------------------------------------------

type AXObserverRef = *const c_void;
type AXObserverCallback = unsafe extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXObserverCreate(application: i32, callback: AXObserverCallback, observer: *mut AXObserverRef) -> AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverRemoveNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> core_foundation::runloop::CFRunLoopSourceRef;
}

/// Notifications from the window itself: it moved, was resized, minimised or closed.
const WINDOW_NOTIFICATIONS: &[&str] =
    &["AXWindowMoved", "AXWindowResized", "AXWindowMiniaturized", "AXUIElementDestroyed"];
/// From its app: all its windows were hidden.
const APP_NOTIFICATIONS: &[&str] = &["AXApplicationHidden"];

/// Watches the window a review's text is in, until dropped.
///
/// An Accessibility observer on the window's app, on a thread of its own, hears when the window
/// moves, is resized, minimised or closed, or its app hidden. A move is reported once as it starts
/// (`WindowChange::Moving`), and once more when the window has settled (`WindowChange::Moved`): the
/// mouse button is up and no move has come for [`SETTLE`]. Only while a move is under way does
/// the thread look at the button, every [`SETTLE_CHECK`]; otherwise it sleeps until a notification
/// comes. Dropping the watch stops the thread, which removes the observer.
pub struct WindowWatch {
    shared: std::sync::Arc<WatchShared>,
}

/// What the watch and its thread share.
struct WatchShared {
    /// The watch has been dropped: the review ended.
    stop: std::sync::atomic::AtomicBool,
    /// The thread's run loop, to wake it when the watch is dropped; None until it runs.
    run_loop: Mutex<Option<core_foundation::runloop::CFRunLoop>>,
    /// The window server's ID for the window, when it could be matched: its frame from there is
    /// current even mid-drag, and costs the app nothing.
    window_id: Option<u32>,
}

impl WindowWatch {
    /// The window's frame now, from the window server.
    pub fn frame(&self) -> Option<Rect> {
        window_server_frame(self.shared.window_id?)
    }
}

impl Drop for WindowWatch {
    fn drop(&mut self) {
        // Never waits for the thread: a notification handler may be what's dropping the watch.
        self.shared.stop.store(true, Ordering::Release);
        if let Some(run_loop) = self.shared.run_loop.lock().unwrap().as_ref() {
            run_loop.stop();
        }
    }
}

type OnChange = Box<dyn Fn(crate::platform::WindowChange) + Send>;

/// What an observer's notifications need, owned by its thread.
struct Observed {
    window: Element,
    on_change: OnChange,
    shared: std::sync::Arc<WatchShared>,
    gone: std::cell::Cell<bool>,
    /// When the last move came, while one is under way.
    moving: std::cell::Cell<Option<Instant>>,
}

impl Observed {
    /// The window's frame now: from the window server, or else from the app.
    fn frame(&self) -> Option<Rect> {
        self.shared.window_id.and_then(window_server_frame).or_else(|| self.window.frame().map(to_rect))
    }
}

/// A window with the mouse button up and no move for this long has settled. Long enough to ride
/// out a pause in the app's move notifications, short enough that the panel follows soon after the
/// user lets go.
const SETTLE: Duration = Duration::from_millis(150);
/// While a move is under way, how often to look whether it has settled.
const SETTLE_CHECK: Duration = Duration::from_millis(30);
/// With no move under way, how long the watch's thread sleeps between looks at whether the watch
/// was dropped, in case the wake-up from the drop came before its run loop started.
const IDLE_CHECK: Duration = Duration::from_secs(2);

/// One window's frame, from the window server: None once it's off screen.
fn window_server_frame(window_id: u32) -> Option<Rect> {
    use core_graphics::window::{copy_window_info, kCGWindowListOptionIncludingWindow};
    let windows = copy_window_info(kCGWindowListOptionIncludingWindow, window_id)?;
    let item = windows.iter().next()?;
    let dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef) };
    let onscreen = dict
        .find(unsafe { CFString::wrap_under_get_rule(core_graphics::window::kCGWindowIsOnscreen) })
        .and_then(|value| value.downcast::<CFBoolean>())
        .is_some_and(bool::from);
    if !onscreen {
        return None;
    }
    window_bounds(&dict)
}

pub(super) fn window_bounds(dict: &CFDictionary<CFString, CFType>) -> Option<Rect> {
    let bounds = dict.find(unsafe { CFString::wrap_under_get_rule(core_graphics::window::kCGWindowBounds) })?;
    let mut rect = core_graphics::geometry::CGRect::default();
    // The call fills `rect` before `then_some` reads it: a method's receiver is evaluated first.
    let found = unsafe { CGRectMakeWithDictionaryRepresentation(bounds.as_CFTypeRef() as _, &mut rect) };
    found.then_some(Rect { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height })
}

/// The window server's ID for app `pid`'s on-screen window at `frame`, matched by its bounds.
fn window_server_id(pid: i32, frame: Rect) -> Option<(u32, Rect)> {
    use core_foundation::number::CFNumber;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
        kCGWindowNumber, kCGWindowOwnerPID,
    };
    let windows =
        copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)?;
    let same = |a: f64, b: f64| (a - b).abs() <= 2.0;
    windows.iter().find_map(|item| {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef) };
        let number = |key: CFStringRef| {
            dict.find(unsafe { CFString::wrap_under_get_rule(key) })
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        if number(unsafe { kCGWindowOwnerPID }) != Some(pid as i64) {
            return None;
        }
        let bounds = window_bounds(&dict)?;
        let matches = same(bounds.x, frame.x)
            && same(bounds.y, frame.y)
            && same(bounds.width, frame.width)
            && same(bounds.height, frame.height);
        matches.then(|| Some((number(unsafe { kCGWindowNumber })? as u32, bounds))).flatten()
    })
}

/// Starts watching the window `capture` came from, calling `on_change` on the watch's thread when
/// it starts moving, settles or goes, and returns the watch with the window's frame now.
pub fn watch_window(
    capture: &Capture,
    on_change: impl Fn(crate::platform::WindowChange) + Send + 'static,
) -> Option<(WindowWatch, Rect)> {
    if capture.playground {
        return None;
    }
    let app = Element::from_create(unsafe { AXUIElementCreateApplication(capture.pid) })?;
    let window =
        capture.element.as_ref().and_then(|e| e.element("AXWindow")).or_else(|| app.element("AXFocusedWindow"))?;
    let mut frame = to_rect(window.frame()?);
    let server = window_server_id(capture.pid, frame);
    if let Some((_, bounds)) = server {
        // Later frames come from the window server, so the first does too.
        frame = bounds;
    }
    let shared = std::sync::Arc::new(WatchShared {
        stop: std::sync::atomic::AtomicBool::new(false),
        run_loop: Mutex::new(None),
        window_id: server.map(|(id, _)| id),
    });
    let observed = Observed {
        window,
        on_change: Box::new(on_change),
        shared: shared.clone(),
        gone: Default::default(),
        moving: Default::default(),
    };
    let pid = capture.pid;
    std::thread::Builder::new().name("window-watch".into()).spawn(move || observe(pid, app, observed)).ok()?;
    Some((WindowWatch { shared }, frame))
}

fn observe(pid: i32, app: Element, observed: Observed) {
    use core_foundation::runloop::{CFRunLoop, CFRunLoopSource, kCFRunLoopDefaultMode};

    let mut observer: AXObserverRef = std::ptr::null();
    if unsafe { AXObserverCreate(pid, window_notification, &mut observer) } != kAXErrorSuccess || observer.is_null() {
        trace(format_args!("couldn't watch the reviewed window"));
        return;
    }
    let observed = Box::new(observed);
    let window = observed.window.clone();
    let refcon = &*observed as *const Observed as *mut c_void;
    let mut added = Vec::new();
    for (element, names) in [(&window, WINDOW_NOTIFICATIONS), (&app, APP_NOTIFICATIONS)] {
        for name in names {
            let name = CFString::new(name);
            let err = unsafe { AXObserverAddNotification(observer, element.0, name.as_concrete_TypeRef(), refcon) };
            if err == kAXErrorSuccess {
                added.push((element.clone(), name));
            }
        }
    }
    if !added.is_empty() {
        let source = unsafe { CFRunLoopSource::wrap_under_get_rule(AXObserverGetRunLoopSource(observer)) };
        let run_loop = CFRunLoop::get_current();
        let mode = unsafe { kCFRunLoopDefaultMode };
        run_loop.add_source(&source, mode);
        *observed.shared.run_loop.lock().unwrap() = Some(run_loop.clone());
        while !observed.shared.stop.load(Ordering::Acquire) && !observed.gone.get() {
            let wait = if observed.moving.get().is_some() { SETTLE_CHECK } else { IDLE_CHECK };
            CFRunLoop::run_in_mode(mode, wait, true);
            if let Some(last) = observed.moving.get()
                && last.elapsed() >= SETTLE
                && !mouse_button_down()
                && !observed.shared.stop.load(Ordering::Acquire)
            {
                observed.moving.set(None);
                if let Some(frame) = observed.frame() {
                    (observed.on_change)(crate::platform::WindowChange::Moved(frame));
                }
            }
        }
        observed.shared.run_loop.lock().unwrap().take();
        run_loop.remove_source(&source, mode);
        for (element, name) in &added {
            unsafe { AXObserverRemoveNotification(observer, element.0, name.as_concrete_TypeRef()) };
        }
    }
    unsafe { CFRelease(observer) };
    drop(observed);
}

unsafe extern "C" fn window_notification(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    // The context outlives the observer's notifications: its thread removes them before freeing it.
    let observed = unsafe { &*(refcon as *const Observed) };
    if observed.gone.get() {
        return;
    }
    let name = unsafe { CFString::wrap_under_get_rule(notification) }.to_string();
    match name.as_str() {
        "AXWindowMoved" | "AXWindowResized" => {
            if observed.moving.replace(Some(Instant::now())).is_none() {
                (observed.on_change)(crate::platform::WindowChange::Moving);
            }
        }
        _ => {
            observed.gone.set(true);
            (observed.on_change)(crate::platform::WindowChange::Gone);
        }
    }
}
