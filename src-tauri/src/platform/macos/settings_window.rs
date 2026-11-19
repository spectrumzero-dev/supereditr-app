//! Finding System Settings' window in the window list, for placing the permission guide beside it.

use super::own_pid;
use crate::platform::Rect;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

// --- System Settings window -----------------------------------------------------------------

/// Where System Settings is, for placing the permission guide beside it.
pub enum SettingsWindow {
    /// Frontmost (ignoring SuperEditr's own windows), with its frame in screen points and its
    /// window number.
    Front(Rect, i64),
    /// Frontmost, but showing a prompt of its own over its window, such as the password or Touch ID
    /// prompt for turning an app on. The guide stays out of its way.
    Prompt,
    /// Open, but another app is in front.
    Behind,
    Closed,
}

unsafe extern "C" {
    pub(super) fn CGRectMakeWithDictionaryRepresentation(
        dict: core_foundation::dictionary::CFDictionaryRef,
        rect: *mut core_graphics::geometry::CGRect,
    ) -> bool;
}

/// Finds System Settings' window from the window list. Owners and frames need no permission
/// (titles would need Screen Recording), so this works before Accessibility is granted.
pub fn system_settings_window() -> SettingsWindow {
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
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
    // System Settings' windows in front of every other app's, front to back. A prompt is a smaller
    // window in front of the one it belongs to.
    let mut stack: Vec<(Rect, i64)> = Vec::new();
    // Front to back.
    for item in windows.iter() {
        let window = ListedWindow::read(*item);
        let Some(pid) = window.pid else { continue };
        if window.layer != Some(0) || pid == own {
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
        if let Some(frame) = window.frame {
            stack.push((frame, window.number.unwrap_or(0)));
        }
    }
    match settings_front(&stack) {
        Some(state) => state,
        None if seen => SettingsWindow::Behind,
        // The on-screen list leaves out windows on other Spaces and minimised ones. Without this,
        // moving to another Space for a few seconds would make the guide give up.
        None => match copy_window_info(kCGWindowListExcludeDesktopElements, kCGNullWindowID) {
            Some(all) => {
                let all: Vec<ListedWindow> = all.iter().map(|item| ListedWindow::read(*item)).collect();
                if has_main_window(&all, &pids) { SettingsWindow::Behind } else { SettingsWindow::Closed }
            }
            None => SettingsWindow::Closed,
        },
    }
}

/// One entry of a window list: its owner, layer, window number and frame in screen points.
struct ListedWindow {
    pid: Option<i64>,
    layer: Option<i64>,
    number: Option<i64>,
    frame: Option<Rect>,
}

impl ListedWindow {
    fn read(item: *const c_void) -> Self {
        use core_foundation::number::CFNumber;
        use core_graphics::window::{kCGWindowBounds, kCGWindowLayer, kCGWindowNumber, kCGWindowOwnerPID};
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(item as core_foundation::dictionary::CFDictionaryRef) };
        let number = |key: CFStringRef| {
            dict.find(unsafe { CFString::wrap_under_get_rule(key) })
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        let frame = dict.find(unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) }).and_then(|bounds| {
            let mut rect = core_graphics::geometry::CGRect::default();
            let read = unsafe { CGRectMakeWithDictionaryRepresentation(bounds.as_CFTypeRef() as _, &mut rect) };
            read.then_some(Rect {
                x: rect.origin.x,
                y: rect.origin.y,
                width: rect.size.width,
                height: rect.size.height,
            })
        });
        ListedWindow {
            pid: number(unsafe { kCGWindowOwnerPID }),
            layer: number(unsafe { kCGWindowLayer }),
            number: number(unsafe { kCGWindowNumber }),
            frame,
        }
    }
}

/// Whether one of `pids` owns a window the size of an app's main window, in a list that includes
/// windows not on screen. Apps also keep small or empty windows that never show; those don't count.
fn has_main_window(windows: &[ListedWindow], pids: &[i64]) -> bool {
    windows.iter().any(|w| {
        w.pid.is_some_and(|pid| pids.contains(&pid))
            && w.layer == Some(0)
            && w.frame.is_some_and(|f| f.width >= 200.0 && f.height >= 200.0)
    })
}

/// Picks System Settings' main window from its windows in front of every other app's (front to
/// back): the frontmost, unless a window behind it contains it, which makes the frontmost a prompt.
fn settings_front(stack: &[(Rect, i64)]) -> Option<SettingsWindow> {
    let contains = |outer: &Rect, inner: &Rect| {
        (outer.width > inner.width || outer.height > inner.height)
            && inner.x >= outer.x - 1.0
            && inner.y >= outer.y - 1.0
            && inner.x + inner.width <= outer.x + outer.width + 1.0
            && inner.y + inner.height <= outer.y + outer.height + 1.0
    };
    let (first, number) = stack.first()?;
    if stack[1..].iter().any(|(behind, _)| contains(behind, first)) {
        return Some(SettingsWindow::Prompt);
    }
    Some(SettingsWindow::Front(*first, *number))
}

/// SuperEditr's own app bundle, which the guide lets the user drag into System Settings.
pub fn own_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_inside_settings_main_window_is_a_prompt() {
        let main = Rect { x: 300.0, y: 60.0, width: 715.0, height: 760.0 };
        let prompt = Rect { x: 500.0, y: 300.0, width: 300.0, height: 330.0 };
        assert!(matches!(settings_front(&[(main, 7)]), Some(SettingsWindow::Front(f, 7)) if f == main));
        assert!(matches!(settings_front(&[(prompt, 9), (main, 7)]), Some(SettingsWindow::Prompt)));
        // A second Settings window beside the first is not a prompt.
        let other = Rect { x: 1100.0, y: 60.0, width: 300.0, height: 330.0 };
        assert!(matches!(settings_front(&[(other, 9), (main, 7)]), Some(SettingsWindow::Front(f, 9)) if f == other));
        assert!(settings_front(&[]).is_none());
    }

    #[test]
    fn a_settings_window_off_screen_counts_only_at_full_size() {
        let window = |pid, layer, width| ListedWindow {
            pid: Some(pid),
            layer: Some(layer),
            number: Some(1),
            frame: Some(Rect { x: 0.0, y: 0.0, width, height: 600.0 }),
        };
        // System Settings' main window, on another Space.
        assert!(has_main_window(&[window(42, 0, 715.0)], &[42]));
        // Another app's window, a menu-bar-level window, and a small helper window don't count.
        assert!(!has_main_window(&[window(7, 0, 715.0)], &[42]));
        assert!(!has_main_window(&[window(42, 25, 715.0)], &[42]));
        assert!(!has_main_window(&[window(42, 0, 1.0)], &[42]));
        assert!(!has_main_window(&[], &[42]));
    }
}
