//! Watching other apps: the selection watcher, where the mouse is and was last pressed, app switches
//! and Space changes.

use super::window::window_bounds;
use super::{own_pid, trace};
use crate::platform::{Rect, WatchEvent};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
/// reports when a selection may be finished; [`crate::platform::gesture`] decides when. Must be called on
/// the main thread.
pub fn start_selection_watcher(on_event: impl Fn(WatchEvent) + Send + Sync + 'static) {
    use crate::platform::gesture::Input;
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
            let (x, y) = mouse_location();
            *LAST_PRESS.lock().unwrap() = Some((Instant::now(), x, y));
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

/// When and where (top-left screen points) the watcher last saw the mouse button go down in another
/// app, for [`pressed_on_window`].
static LAST_PRESS: Mutex<Option<(Instant, f64, f64)>> = Mutex::new(None);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

/// Whether the mouse press the selection watcher just reported landed on the window at `frame` of
/// app `pid`: inside that frame, with no other app's window over it there. Asked once per press.
/// Where the window server lists no windows, the frame alone decides.
pub fn pressed_on_window(pid: i32, frame: Rect) -> bool {
    let Some((at, x, y)) = LAST_PRESS.lock().unwrap().take() else { return false };
    let inside = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
    if at.elapsed() > Duration::from_millis(100) || !inside(&frame) {
        return false;
    }
    match window_at(inside) {
        Some((owner, bounds)) => {
            let same = |a: f64, b: f64| (a - b).abs() <= 2.0;
            let on = owner == pid
                && same(bounds.x, frame.x)
                && same(bounds.y, frame.y)
                && same(bounds.width, frame.width)
                && same(bounds.height, frame.height);
            if !on {
                let whose = if owner == pid { "another of its app's windows" } else { "another app's window" };
                trace(format_args!("follow: the press landed on {whose}"));
            }
            on
        }
        None => true,
    }
}

/// The frontmost window, other than SuperEditr's own, that `inside` accepts: its app's pid and frame.
fn window_at(inside: impl Fn(&Rect) -> bool) -> Option<(i32, Rect)> {
    use core_foundation::number::CFNumber;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
    };
    let own = own_pid() as i64;
    let windows =
        copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)?;
    // Front to back.
    windows.iter().find_map(|item| {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef) };
        let pid = dict
            .find(unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) })
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())?;
        // Only ordinary windows: a full-screen overlay of the system's, at another level and often
        // invisible, isn't what the user clicked.
        let layer = dict
            .find(unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) })
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|n| n.to_i64());
        if pid == own || layer != Some(0) {
            return None;
        }
        let frame = window_bounds(&dict)?;
        inside(&frame).then_some((pid as i32, frame))
    })
}

/// Whether the left mouse button is down.
pub fn mouse_button_down() -> bool {
    // The combined session state (0), left button (0).
    unsafe { CGEventSourceButtonState(0, 0) }
}

/// The selection under way, shared by the watcher and [`selection_app_switched`].
static GESTURE: Mutex<crate::platform::gesture::Gesture> = Mutex::new(crate::platform::gesture::Gesture::new());

/// Another app came to the front: forget a selection the user was making in the previous one.
pub fn selection_app_switched() {
    GESTURE.lock().unwrap().feed(crate::platform::gesture::Input::AppSwitched);
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

/// Calls `on_change` each time the user moves to another Space. The app in front can stay the same
/// (on an empty desktop, say), so this isn't always an app switch. Must be called on the main thread.
pub fn start_space_change_watcher(on_change: impl Fn() + 'static) {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification};
    use objc2_foundation::NSNotification;
    use std::ptr::NonNull;

    let handler = RcBlock::new(move |_: NonNull<NSNotification>| on_change());
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceActiveSpaceDidChangeNotification),
            None,
            None,
            &handler,
        )
    };
    // Both live for the life of the app.
    std::mem::forget(observer);
    std::mem::forget(handler);
}
