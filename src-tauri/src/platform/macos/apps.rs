//! The app in front, what it is called and built on, and asking web views for their accessibility
//! tree.

use super::ax::{AXUIElementCreateApplication, AXUIElementCreateSystemWide, Element};
use super::{Capture, trace};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use supereditr_core::policy::{Os, is_chromium_browser};

/// How long reads wait for an app's accessibility tree after asking for it. Chrome 156 takes about
/// 2 s to build one, even for a short page.
const WEB_TREE_WAIT: Duration = Duration::from_secs(4);

/// The flag VoiceOver sets, the only one Chromium's own runtime has: in Chrome, and in apps built
/// directly on it. Electron adds `AXManualAccessibility`.
const VOICEOVER_FLAG: &str = "AXEnhancedUserInterface";

/// Apps whose web views only build an accessibility tree when asked. The flags are
/// documented by Chromium and Electron and are scoped to the target process. Returns when reads
/// should stop waiting for the tree: already past for an app that didn't take a flag, and the same
/// moment for every read until then, so a second read can't overtake the first and give up early.
pub(super) fn enable_web_accessibility(app: &Element, pid: i32, bundle_id: &str) -> Instant {
    static ASKED: Mutex<Option<HashMap<i32, (Instant, bool)>>> = Mutex::new(None);
    let mut asked = ASKED.lock().unwrap();
    let asked = asked.get_or_insert_with(HashMap::new);
    // VoiceOver turns its flag off when it quits, which takes the tree away, so that flag is checked.
    if let Some(&(deadline, voiceover_flag)) = asked.get(&pid)
        && (!voiceover_flag || app.is_on(VOICEOVER_FLAG))
    {
        return deadline;
    }
    // Remembered only once accepted: a busy app can time out, and the next read should ask again.
    let yes = CFBoolean::true_value();
    let manual = app.set("AXManualAccessibility", yes.as_CFTypeRef());
    let mut accepted = manual;
    // Chrome 156 reports this write as not implemented yet acts on it, so the flag is read back
    // rather than the write's result trusted.
    if !manual && is_chromium_based(bundle_id, pid) {
        app.set(VOICEOVER_FLAG, yes.as_CFTypeRef());
        accepted = app.is_on(VOICEOVER_FLAG);
    }
    let now = Instant::now();
    if !accepted {
        return now;
    }
    trace(format_args!("{bundle_id}: asked for the app's accessibility tree"));
    asked.insert(pid, (now + WEB_TREE_WAIT, !manual));
    now + WEB_TREE_WAIT
}

pub(super) fn is_browser_or_electron(bundle_id: &str, pid: i32) -> bool {
    bundle_id == "com.apple.Safari" || bundle_id.starts_with("org.mozilla.") || is_chromium_based(bundle_id, pid)
}

/// Chrome-family browsers and apps built on Chromium, which share its clipboard code.
pub(super) fn is_chromium_based(bundle_id: &str, pid: i32) -> bool {
    is_chromium_browser(Os::MacOs, bundle_id)
        || app_bundle_path(pid).is_some_and(|path| has_chromium_runtime(std::path::Path::new(&path)))
}

/// Whether the app at `bundle` carries Chromium, whatever its framework is called: `Electron
/// Framework` in most, but `Codex Framework` in the Codex app. Any such framework holds Chromium's
/// resource pack. Its `icudtl.dat` wouldn't tell them apart, because Flutter's framework has one too.
fn has_chromium_runtime(bundle: &std::path::Path) -> bool {
    std::fs::read_dir(bundle.join("Contents/Frameworks")).is_ok_and(|frameworks| {
        frameworks.flatten().any(|framework| {
            framework.file_name() == "Electron Framework.framework"
                || framework.path().join("Resources/chrome_100_percent.pak").exists()
        })
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

/// A running app's (bundle ID, name). Reads nothing from the app, so it needs no permission.
pub fn app_identity(pid: i32) -> (String, String) {
    match running_app(pid) {
        Some(app) => (
            app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
            app.localizedName().map(|s| s.to_string()).unwrap_or_else(|| "this app".into()),
        ),
        None => (String::new(), "this app".into()),
    }
}

pub(super) fn focused_app() -> Option<(Element, i32)> {
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

/// The app in front, from NSWorkspace rather than Accessibility, which macOS counts as asking for
/// permission.
pub fn frontmost_pid() -> Option<i32> {
    Some(objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier())
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

/// Called once the panel has closed, before `replace`. The panel never took focus from the app (the
/// keyboard tap gave it the keys), and `replace` checks the field itself, so this only gives the
/// panel a moment to close.
pub fn return_to_field(_capture: &Capture) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(40));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_chromium_whatever_its_framework_is_called() {
        let root = std::env::temp_dir().join(format!("supereditr-runtime-{}", std::process::id()));
        // A framework laid out as they ship: its Resources is a link into the current version.
        let bundle = |app: &str, framework: &str, file: &str| {
            let path = root.join(app);
            let framework = path.join("Contents/Frameworks").join(framework);
            std::fs::create_dir_all(framework.join("Versions/1/Resources")).unwrap();
            std::fs::write(framework.join("Versions/1/Resources").join(file), "").unwrap();
            std::os::unix::fs::symlink("1", framework.join("Versions/Current")).unwrap();
            std::os::unix::fs::symlink("Versions/Current/Resources", framework.join("Resources")).unwrap();
            path
        };
        let pack = "chrome_100_percent.pak";
        assert!(has_chromium_runtime(&bundle("Electron.app", "Electron Framework.framework", pack)));
        assert!(has_chromium_runtime(&bundle("Renamed.app", "Codex Framework.framework", pack)));
        assert!(has_chromium_runtime(&bundle("Browser.app", "Google Chrome Framework.framework", pack)));
        assert!(has_chromium_runtime(&bundle("Stripped.app", "Electron Framework.framework", "other.pak")));
        assert!(!has_chromium_runtime(&bundle("Flutter.app", "FlutterMacOS.framework", "icudtl.dat")));
        assert!(!has_chromium_runtime(&bundle("Native.app", "Sparkle.framework", "other.pak")));
        assert!(!has_chromium_runtime(&root.join("NoFrameworks.app")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
