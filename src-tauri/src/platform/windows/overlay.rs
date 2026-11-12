//! The suggestion panel's window on Windows.
//!
//! - Made once at launch and kept, with `WS_EX_NOACTIVATE` from Tauri's `set_focusable(false)`, so a
//!   click on it leaves the keyboard in the user's app. Tauri recomputes the window's extended style
//!   on its own calls, so the style must come from Tauri, and Tauri's `show()` and `set_focus()`
//!   are never used: it's shown with `SW_SHOWNA` and moved with `SWP_NOACTIVATE`.
//! - Shown once off-screen at launch, which makes the first real show about as fast as later ones.
//! - Clipped to the card with `SetWindowRgn`, so the transparent room around it doesn't take clicks
//!   meant for the app underneath.
//! - Placed in physical pixels on the selection's monitor, with that monitor's scale: the window's
//!   own scale is the old monitor's until it has moved.
//! - It takes keyboard focus only for a review the user asked for (the shortcut, or typing in it),
//!   and gives it back to the app when it closes. Whether it has the keyboard is read from the
//!   foreground window, never from the page, which thinks it has focus after any click.
//! - When the reviewed window has moved and settled, the panel glides to its new place
//!   (`crate::follow`): on a thread of its own, one step per frame the desktop composes
//!   (`DwmFlush`, or a high-resolution timer at the refresh rate when that returns early), each
//!   step's place worked out from the time since the glide's first frame.

use super::input;
use super::window::{self, Monitor};
use crate::overlay::{GAP, MARGIN, PAD_BOTTOM, PAD_SIDE, PAD_TOP, WIDTH};
use crate::platform::Rect;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo};
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, HWND_TOPMOST, IsWindowVisible, SC_KEYMENU, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow, WM_SYSCOMMAND,
};

/// The card's corner radius (theme.css `--radius`).
const RADIUS: f64 = 14.0;

/// The panel took the keyboard and hasn't closed since.
static FOCUSED: AtomicBool = AtomicBool::new(false);
static SHOWN: AtomicBool = AtomicBool::new(false);
/// Where the panel last opened at the pointer, kept while it changes height.
static POINTER: Mutex<Option<Rect>> = Mutex::new(None);

fn handle(window: &WebviewWindow) -> Option<HWND> {
    window.hwnd().ok()
}

/// Keeps Alt from opening the window menu. Alt+Enter is the panel's apply key: the Enter is
/// SuperEditr's shortcut, and the Alt released afterwards would otherwise put the panel's window in
/// menu mode, where it stops handling the page's requests until another key is pressed.
unsafe extern "system" fn no_menu_key(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_SYSCOMMAND && (wparam.0 & 0xFFF0) == SC_KEYMENU as usize {
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Once, right after the window is made.
pub fn init(window: &WebviewWindow) {
    // Also applies the undecorated frame: shown before this, the window has a title bar.
    let _ = window.set_focusable(false);
    let Some(hwnd) = handle(window) else { return };
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(no_menu_key), 0x5345, 0);
        let _ = SetWindowPos(hwnd, None, -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    let app = window.app_handle().clone();
    let warm = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        let _ = app.run_on_main_thread(move || {
            if !SHOWN.load(Ordering::SeqCst)
                && let Some(hwnd) = handle(&warm)
            {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
        });
    });
}

/// Where the window goes and the card's region in it, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// The card within the window: left, top, right, bottom.
    pub card: (i32, i32, i32, i32),
    pub radius: i32,
}

/// Below the selection, or above it near the bottom of the screen, kept within the monitor's work
/// area. `anchor` is in physical pixels; `height` is the window's height in logical pixels.
pub fn placement(anchor: Rect, monitor: &Monitor, height: f64) -> Placement {
    let s = monitor.scale;
    let work = monitor.work;
    let (left, top, right, bottom) =
        (f64::from(work.left), f64::from(work.top), f64::from(work.right), f64::from(work.bottom));
    let card_width = (WIDTH - 2.0 * PAD_SIDE) * s;
    let card_height = (height - PAD_TOP - PAD_BOTTOM) * s;
    let (gap, margin) = (GAP * s, MARGIN * s);
    let card_x = anchor.x.clamp(left + margin, (right - card_width - margin).max(left + margin));
    let below = anchor.y + anchor.height + gap;
    let card_y = if below + card_height <= bottom - margin || anchor.y - gap - card_height < top + margin {
        below.min(bottom - card_height - margin).max(top + margin)
    } else {
        (anchor.y - gap - card_height).max(top + margin)
    };
    let px = |v: f64| v.round() as i32;
    let (width, full_height) = (px(WIDTH * s), px(height * s));
    Placement {
        x: px(card_x - PAD_SIDE * s),
        y: px(card_y - PAD_TOP * s),
        width,
        height: full_height,
        card: (px(PAD_SIDE * s), px(PAD_TOP * s), width - px(PAD_SIDE * s), full_height - px(PAD_BOTTOM * s)),
        radius: px(RADIUS * s),
    }
}

fn rect_of(r: Rect) -> RECT {
    RECT {
        left: r.x.round() as i32,
        top: r.y.round() as i32,
        right: (r.x + r.width.max(1.0)).round() as i32,
        bottom: (r.y + r.height.max(1.0)).round() as i32,
    }
}

/// Places the panel for a selection at `anchor` (physical pixels), or at the pointer. With
/// `keep_pointer`, a panel that opened at the pointer stays there as it changes height. A glide
/// under way stops: the panel goes straight to its place.
pub fn place(window: &WebviewWindow, anchor: Option<Rect>, height: f64, keep_pointer: bool) {
    let Some(hwnd) = handle(window) else { return };
    stop_glide();
    let anchor = match anchor {
        Some(anchor) => anchor,
        None => {
            let kept = if keep_pointer { *POINTER.lock().unwrap() } else { None };
            let at = kept.unwrap_or_else(|| {
                let (x, y) = window::cursor().map_or((0, 0), |p| (p.x, p.y));
                Rect { x: f64::from(x) - 24.0, y: f64::from(y) - 10.0, width: 1.0, height: 20.0 }
            });
            *POINTER.lock().unwrap() = Some(at);
            at
        }
    };
    let monitor = window::monitor_for_rect(rect_of(anchor)).or_else(|| window::monitor_for_point(POINT::default()));
    let Some(monitor) = monitor else { return };
    put(hwnd, &placement(anchor, &monitor, height));
}

/// Moves and sizes the window to `at` at once, and clips it to the card.
fn put(hwnd: HWND, at: &Placement) {
    unsafe {
        // Twice: moving to a monitor with another scale makes Tauri resize the window for it.
        for _ in 0..2 {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), at.x, at.y, at.width, at.height, SWP_NOACTIVATE);
        }
        let (l, t, r, b) = at.card;
        let region = CreateRoundRectRgn(l, t, r + 1, b + 1, at.radius * 2, at.radius * 2);
        // The window owns the region from here on.
        let _ = SetWindowRgn(hwnd, Some(region), true);
    }
}

/// Counts glides; a glide goes on only while it's the latest, so bumping it stops one.
static GLIDES: AtomicU64 = AtomicU64::new(0);
/// The glide under way, by its number, and where it's headed.
static HEADED: Mutex<Option<(u64, i32, i32)>> = Mutex::new(None);

/// Places the panel for a selection at `anchor` (physical pixels) by the usual rules, gliding there
/// from where it is: for following a window that has moved and settled. A glide already headed
/// there is left to finish. A panel that's hidden, changes size, or would cross to a monitor at
/// another scale (where Windows resizes the window on the way) goes straight to its place.
pub fn glide(window: &WebviewWindow, anchor: Rect, height: f64) {
    let Some(hwnd) = handle(window) else { return };
    let Some(monitor) = window::monitor_for_rect(rect_of(anchor)) else { return };
    let to = placement(anchor, &monitor, height);
    let latest = GLIDES.load(Ordering::SeqCst);
    if *HEADED.lock().unwrap() == Some((latest, to.x, to.y)) {
        return;
    }
    let mut now = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut now) }.is_err() {
        return;
    }
    let same_size = now.right - now.left == to.width && now.bottom - now.top == to.height;
    if same_size && (now.left, now.top) == (to.x, to.y) {
        return;
    }
    let same_scale = window::monitor_for_rect(now).is_some_and(|here| here.scale == monitor.scale);
    if !same_size || !same_scale || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        stop_glide();
        return put(hwnd, &to);
    }
    let number = GLIDES.fetch_add(1, Ordering::SeqCst) + 1;
    *HEADED.lock().unwrap() = Some((number, to.x, to.y));
    let raw = hwnd.0 as isize;
    let (from, end) = ((f64::from(now.left), f64::from(now.top)), (f64::from(to.x), f64::from(to.y)));
    let started =
        std::thread::Builder::new().name("panel glide".into()).spawn(move || run_glide(raw, number, from, end));
    if started.is_err() {
        put(hwnd, &to);
    }
}

/// The reviewed window has started moving again, or the panel is placed or hidden: a glide under
/// way stops where it is.
pub fn stop_glide() {
    GLIDES.fetch_add(1, Ordering::SeqCst);
}

/// Paces a glide to the frames the desktop composes.
struct Pacer {
    /// One frame at the refresh rate.
    period: Duration,
    /// When the last frame came.
    last: Option<Instant>,
    /// A high-resolution waitable timer, for when `DwmFlush` returns without waiting.
    timer: Option<HANDLE>,
    /// Frames that `DwmFlush` paced, and frames the timer did, for the log.
    flushed: u32,
    timed: u32,
}

impl Pacer {
    fn new() -> Self {
        let timer =
            unsafe { CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0) }
                .ok();
        Pacer { period: refresh_period(), last: None, timer, flushed: 0, timed: 0 }
    }

    /// Waits for the next frame and returns when it came. `DwmFlush` returns once the desktop has
    /// composed a frame; when nothing else on screen is changing it can return at once, and then
    /// the rest of the frame is waited out on the timer.
    fn next(&mut self) -> Instant {
        let flushed = unsafe { DwmFlush() }.is_ok();
        let mut now = Instant::now();
        match self.last {
            Some(last) if !flushed || now.duration_since(last) < self.period / 2 => {
                self.wait_until(last + self.period);
                now = Instant::now();
                self.timed += 1;
            }
            _ => self.flushed += 1,
        }
        self.last = Some(now);
        now
    }

    fn wait_until(&self, due: Instant) {
        let left = due.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        match self.timer {
            Some(timer) => unsafe {
                // Negative: relative, in 100-nanosecond units.
                let due = -((left.as_nanos() / 100) as i64).max(1);
                if SetWaitableTimer(timer, &due, 0, None, None, false).is_ok() {
                    WaitForSingleObject(timer, INFINITE);
                } else {
                    std::thread::sleep(left);
                }
            },
            None => std::thread::sleep(left),
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            let _ = unsafe { CloseHandle(timer) };
        }
    }
}

/// One frame at the rate the desktop composes, from DWM; 60 Hz when it doesn't say.
fn refresh_period() -> Duration {
    let mut info = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
    let rate = match unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) } {
        Ok(()) if info.rateRefresh.uiDenominator != 0 => {
            f64::from(info.rateRefresh.uiNumerator) / f64::from(info.rateRefresh.uiDenominator)
        }
        _ => 60.0,
    };
    Duration::from_secs_f64(1.0 / if (30.0..=500.0).contains(&rate) { rate } else { 60.0 })
}

/// A glide of the panel's top-left corner from `from` to `to` (physical pixels), along
/// `follow::glide_point`'s curve, one step per frame, until it arrives or a newer glide or a
/// `stop_glide` replaces it. Logs each step: the time of its frame and the time it was made, both
/// in milliseconds from the glide's first frame, and its distance in pixels.
fn run_glide(handle: isize, number: u64, from: (f64, f64), to: (f64, f64)) {
    let hwnd = window::hwnd(handle);
    let mut pacer = Pacer::new();
    // The frame the glide was asked in carries the end of the move: let it pass first.
    pacer.next();
    let mut start: Option<Instant> = None;
    let mut at = (from.0.round() as i32, from.1.round() as i32);
    let mut steps: Vec<(f64, f64, f64)> = Vec::new();
    let how = loop {
        let frame = pacer.next();
        if GLIDES.load(Ordering::SeqCst) != number {
            break "stopped";
        }
        // The first frame is the glide's time zero, where it stands still.
        let start = *start.get_or_insert(frame);
        let elapsed = frame.duration_since(start).as_secs_f64();
        let (point, done) = crate::follow::glide_point(from, to, elapsed);
        let step = (point.0.round() as i32, point.1.round() as i32);
        if step != at {
            unsafe {
                let _ = SetWindowPos(hwnd, None, step.0, step.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let made = start.elapsed().as_secs_f64();
            let distance = f64::from(step.0 - at.0).hypot(f64::from(step.1 - at.1));
            steps.push((elapsed * 1000.0, made * 1000.0, distance));
            at = step;
        }
        if done {
            break "done";
        }
    };
    {
        let mut headed = HEADED.lock().unwrap();
        if headed.is_some_and(|(n, _, _)| n == number) {
            *headed = None;
        }
    }
    // Each step as "frame time|time made|distance": milliseconds from the start, then pixels.
    let list: Vec<String> = steps.iter().map(|(frame, made, px)| format!("{frame:.1}|{made:.1}|{px:.1}")).collect();
    super::trace(format_args!(
        "follow: glide {how}, {:.1} px in {} steps at {:.1} Hz ({} frames by DwmFlush, {} by the timer): {}",
        (to.0 - from.0).hypot(to.1 - from.1),
        steps.len(),
        1.0 / pacer.period.as_secs_f64(),
        pacer.flushed,
        pacer.timed,
        list.join(", ")
    ));
}

/// Shows the panel without activating it; with `take_focus`, then gives it the keyboard.
pub fn show(window: &WebviewWindow, take_focus: bool) {
    let Some(hwnd) = handle(window) else { return };
    SHOWN.store(true, Ordering::SeqCst);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    if take_focus {
        let started = std::time::Instant::now();
        let focused = bring_to_front(hwnd) && wait_for_front(hwnd, std::time::Duration::from_millis(150));
        FOCUSED.store(focused, Ordering::SeqCst);
        super::trace(format_args!(
            "panel {} focus in {:?}, {:?} after the shortcut",
            if focused { "took" } else { "couldn’t take" },
            started.elapsed(),
            super::hotkey::since_last_hotkey()
        ));
    }
}

/// `SetForegroundWindow`, which Windows allows only to the process that sent the last input: right
/// after the user's shortcut that's SuperEditr, and otherwise a key that does nothing makes it so.
/// True when Windows took the request; the window may come to the front a moment later, so callers
/// that must know poll the foreground window.
pub fn bring_to_front(hwnd: HWND) -> bool {
    unsafe {
        if GetForegroundWindow() == hwnd || SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        input::nudge();
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// Waits up to `limit` for `hwnd` to be the foreground window.
fn wait_for_front(hwnd: HWND, limit: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        if unsafe { GetForegroundWindow() } == hwnd {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The panel is closing: returns whether it had the keyboard. Called at once, before the window
/// hides on the main thread, so the app coming back isn't mistaken for the user moving on.
pub fn hiding() -> bool {
    FOCUSED.swap(false, Ordering::SeqCst)
}

/// Whether the panel is showing.
pub fn showing() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// Whether the panel has the keyboard now.
pub fn had_focus() -> bool {
    FOCUSED.load(Ordering::SeqCst)
}

/// Gives the panel the keyboard back if it had it, after the user dragged the reviewed window by
/// its title bar, which took it. Any thread but the main one, which a glide's steps wait on.
pub fn refocus(window: &WebviewWindow) {
    if !FOCUSED.load(Ordering::SeqCst) || !SHOWN.load(Ordering::SeqCst) {
        return;
    }
    let Some(hwnd) = handle(window) else { return };
    let started = Instant::now();
    let back = bring_to_front(hwnd) && wait_for_front(hwnd, Duration::from_millis(150));
    super::trace(format_args!(
        "follow: the panel {} the keyboard after the drag, in {:?}",
        if back { "took back" } else { "couldn’t take back" },
        started.elapsed()
    ));
}

/// Where the panel last opened at the pointer, for a review with no selection bounds to go by.
pub fn pointer_anchor() -> Option<Rect> {
    *POINTER.lock().unwrap()
}

pub fn hide(window: &WebviewWindow, had_focus: bool) {
    SHOWN.store(false, Ordering::SeqCst);
    stop_glide();
    let Some(hwnd) = handle(window) else { return };
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    // Windows would otherwise give focus to whichever window it chooses.
    if had_focus {
        super::focus_target();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(left: i32, top: i32, width: i32, height: i32, scale: f64) -> Monitor {
        let area = RECT { left, top, right: left + width, bottom: top + height };
        Monitor { area, work: RECT { bottom: area.bottom - 48, ..area }, scale }
    }

    fn at(x: f64, y: f64) -> Rect {
        Rect { x, y, width: 40.0, height: 20.0 }
    }

    #[test]
    fn the_card_opens_below_the_selection_at_the_monitors_scale() {
        let primary = monitor(0, 0, 1920, 1080, 1.25);
        let p = placement(at(400.0, 300.0), &primary, 240.0);
        // The card's left edge is at the selection, the window 26 logical pixels further left.
        assert_eq!((p.x, p.y), (368, 310), "32.5 pixels left of the selection, rounded");
        assert_eq!((p.width, p.height), (685, 300));
        assert_eq!(p.card, (33, 20, 685 - 33, 300 - 48));
        assert_eq!(p.radius, 18);
    }

    #[test]
    fn near_the_bottom_it_opens_above() {
        let primary = monitor(0, 0, 1920, 1080, 1.0);
        let p = placement(at(400.0, 950.0), &primary, 240.0);
        let card_top = p.y + 16;
        assert_eq!(card_top + 186, 950 - 8, "the card ends just above the selection");
    }

    #[test]
    fn monitors_left_of_and_above_the_primary_have_negative_origins() {
        let left = monitor(-1600, 100, 1600, 900, 1.0);
        let p = placement(at(-1500.0, 300.0), &left, 240.0);
        assert_eq!((p.x, p.y), (-1500 - 26, 300 + 28 - 16));
        assert_eq!(p.width, 548, "the left monitor's scale, not the primary's");

        let above = monitor(0, -900, 1600, 900, 1.0);
        let p = placement(at(1580.0, -400.0), &above, 240.0);
        assert!(p.x + p.card.2 <= 1600 - 10, "kept on its monitor: {p:?}");
        assert!(p.y < 0);
    }

    #[test]
    fn a_selection_off_the_work_area_is_clamped_into_it() {
        let primary = monitor(0, 0, 1920, 1080, 1.5);
        let p = placement(at(-300.0, -50.0), &primary, 240.0);
        assert_eq!(p.x + p.card.0, 15, "the margin, at the monitor's scale");
        assert_eq!(p.y + p.card.1, 15);
    }
}
