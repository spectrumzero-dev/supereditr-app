//! Gliding SuperEditr's own panel to a new place, one step per frame the display shows.

use super::trace;
use core_foundation::base::TCFType;
use std::ffi::c_void;

// --- Gliding the panel -------------------------------------------------------------------------

/// A glide under way, on the main thread: the panel's window, where it set out from and is
/// headed, what drives its frames, and its steps so far for the log.
struct Glide {
    window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
    from: (f64, f64),
    to: (f64, f64),
    /// The first frame's time, in seconds; positions go by the time since then.
    start: Option<f64>,
    /// Frames still to let pass before the glide starts: the frame the glide was asked for in
    /// carries the work that asked for it, such as the end of a drag.
    wait: u32,
    at: (f64, f64),
    driver: GlideDriver,
    /// For the log, each step: the time of the frame it was for and the time it was made, both
    /// from the start in milliseconds, and its distance in points.
    steps: Vec<(f64, f64, f64)>,
}

enum GlideDriver {
    /// A display link (macOS 14 and later): one call per frame the display shows.
    Link(objc2::rc::Retained<objc2::runtime::AnyObject>),
    /// Before macOS 14, a main-thread timer at the display's refresh rate.
    Timer(core_foundation::runloop::CFRunLoopTimer),
}

thread_local! {
    static GLIDE: std::cell::RefCell<Option<Glide>> = const { std::cell::RefCell::new(None) };
}

/// The display link's target: calls [`glide_frame`] with the time of the frame being drawn.
struct GlideLinkIvars;

objc2::define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class doesn't implement Drop.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[name = "SuperEditrGlideLink"]
    #[ivars = GlideLinkIvars]
    struct GlideLink;

    impl GlideLink {
        #[unsafe(method(step:))]
        fn step(&self, link: &objc2::runtime::AnyObject) {
            // When the frame this step is for will be on screen, and now, on the same clock.
            let time: f64 = unsafe { objc2::msg_send![link, targetTimestamp] };
            glide_frame(time, unsafe { CACurrentMediaTime() });
        }
    }
);

extern "C" fn glide_timer(_timer: core_foundation::runloop::CFRunLoopTimerRef, _info: *mut c_void) {
    let now = unsafe { CFAbsoluteTimeGetCurrent() };
    glide_frame(now, now);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFAbsoluteTimeGetCurrent() -> f64;
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    fn CACurrentMediaTime() -> f64;
}

/// One frame of the glide: puts the panel where the curve says for this time. A late frame lands
/// further along the curve rather than a step behind.
fn glide_frame(time: f64, now: f64) {
    let finished = GLIDE.with_borrow_mut(|glide| {
        let Some(g) = glide.as_mut() else { return true };
        if g.wait > 0 {
            g.wait -= 1;
            return false;
        }
        // The first frame after the wait is the glide's time zero, where it stands still.
        let start = *g.start.get_or_insert(time);
        let (point, done) = crate::follow::glide_point(g.from, g.to, time - start);
        let distance = (point.0 - g.at.0).hypot(point.1 - g.at.1);
        if distance > 0.0 {
            g.steps.push(((time - start) * 1000.0, (now - start) * 1000.0, distance));
            g.at = point;
            set_top_left(&g.window, point);
        }
        done
    });
    if finished {
        end_glide("done");
    }
}

/// Ends the glide under way, if any, leaving the panel where it is, and logs its steps.
fn end_glide(how: &str) {
    let Some(glide) = GLIDE.with_borrow_mut(Option::take) else { return };
    match &glide.driver {
        GlideDriver::Link(link) => {
            let _: () = unsafe { objc2::msg_send![&**link, invalidate] };
        }
        GlideDriver::Timer(timer) => unsafe {
            core_foundation::runloop::CFRunLoopTimerInvalidate(timer.as_concrete_TypeRef())
        },
    }
    let driver = if matches!(glide.driver, GlideDriver::Link(_)) { "display link" } else { "timer" };
    // Each step as "frame time|time made|distance": milliseconds from the start, then points.
    let steps: Vec<String> =
        glide.steps.iter().map(|(frame, made, pt)| format!("{frame:.1}|{made:.1}|{pt:.2}")).collect();
    trace(format_args!(
        "follow: glide {how} by {driver}, {:.1} pt in {} steps: {}",
        (glide.to.0 - glide.from.0).hypot(glide.to.1 - glide.from.1),
        glide.steps.len(),
        steps.join(", ")
    ));
}

/// The top-left corner of one of SuperEditr's windows, in top-left screen points.
fn top_left(window: &objc2_app_kit::NSWindow, mtm: objc2::MainThreadMarker) -> Option<(f64, f64)> {
    let primary = objc2_app_kit::NSScreen::screens(mtm).iter().next()?.frame().size.height;
    let frame = window.frame();
    Some((frame.origin.x, primary - frame.origin.y - frame.size.height))
}

fn set_top_left(window: &objc2_app_kit::NSWindow, (x, y): (f64, f64)) {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let Some(primary) = objc2_app_kit::NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height) else {
        return;
    };
    // AppKit's screen coordinates start at the primary display's bottom-left corner.
    window.setFrameTopLeftPoint(objc2_foundation::NSPoint::new(x, primary - y));
}

/// Glides one of SuperEditr's own windows (an `NSWindow` pointer from Tauri) so its top-left
/// corner ends at (`x`, `y`) in top-left screen points, along `follow::glide_point`'s curve, one
/// step per frame the display shows. A glide under way stops where it is first. False off the
/// main thread.
pub fn glide_own_window(ns_window: *mut c_void, x: f64, y: f64) -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, msg_send, sel};
    let Some(mtm) = objc2::MainThreadMarker::new() else { return false };
    if ns_window.is_null() {
        return false;
    }
    end_glide("replaced");
    let Some(window) = (unsafe { Retained::retain(ns_window as *mut objc2_app_kit::NSWindow) }) else { return false };
    let Some(from) = top_left(&window, mtm) else { return false };
    let screen = objc2_app_kit::NSScreen::mainScreen(mtm);
    let responds = screen.as_ref().is_some_and(|screen| unsafe {
        msg_send![&**screen, respondsToSelector: sel!(displayLinkWithTarget:selector:)]
    });
    let driver = match screen.filter(|_| responds) {
        Some(screen) => {
            let target = GlideLink::alloc().set_ivars(GlideLinkIvars);
            let target: Retained<GlideLink> = unsafe { msg_send![super(target), init] };
            let link: Option<Retained<AnyObject>> =
                unsafe { msg_send![&screen, displayLinkWithTarget: &*target, selector: sel!(step:)] };
            let Some(link) = link else { return false };
            let run_loop = objc2_foundation::NSRunLoop::mainRunLoop();
            let _: () =
                unsafe { msg_send![&link, addToRunLoop: &*run_loop, forMode: objc2_foundation::NSRunLoopCommonModes] };
            GlideDriver::Link(link)
        }
        None => {
            use core_foundation::runloop::{CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes};
            let interval = 1.0 / display_refresh_rate();
            let now = unsafe { CFAbsoluteTimeGetCurrent() };
            let timer = CFRunLoopTimer::new(now, interval, 0, 0, glide_timer, std::ptr::null_mut());
            CFRunLoop::get_main().add_timer(&timer, unsafe { kCFRunLoopCommonModes });
            GlideDriver::Timer(timer)
        }
    };
    GLIDE.with_borrow_mut(|glide| {
        *glide = Some(Glide { window, from, to: (x, y), start: None, wait: 1, at: from, driver, steps: Vec::new() })
    });
    true
}

/// Stops a glide where it is. False off the main thread.
pub fn stop_own_window_glide(_ns_window: *mut c_void) -> bool {
    if objc2::MainThreadMarker::new().is_none() {
        return false;
    }
    end_glide("stopped by a move");
    true
}

/// The main display's refresh rate, in frames per second (60 when it doesn't say).
fn display_refresh_rate() -> f64 {
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
        fn CGDisplayModeGetRefreshRate(mode: *const c_void) -> f64;
        fn CGDisplayModeRelease(mode: *const c_void);
    }
    let mode = unsafe { CGDisplayCopyDisplayMode(CGMainDisplayID()) };
    if mode.is_null() {
        return 60.0;
    }
    let rate = unsafe { CGDisplayModeGetRefreshRate(mode) };
    unsafe { CGDisplayModeRelease(mode) };
    if (30.0..=240.0).contains(&rate) { rate } else { 60.0 }
}
