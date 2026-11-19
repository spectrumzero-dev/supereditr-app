//! The wording of the capture.log lines that the end-to-end UI drivers read.
//!
//! capture.log (`platform::trace`) is a diagnostic log, but three drivers also read it to see what
//! SuperEditr did: tests/ui/smoke.swift on macOS, scripts/ui-smoke-linux.sh with tests/ui/linux/ on
//! Linux, and tests/ui/windows/driver.ps1 on Windows. Each line one of them matches is written by a
//! type here, which every platform that writes the line uses, so its wording is in one place.
//! Lines no driver reads are written where they happen.
//!
//! tests/contracts/log-lines.json holds an example of each line, as these types write it, and the
//! patterns each driver matches on it. The tests below check both sides: that each type writes its
//! example, and that each pattern matches its example and still appears in that driver's source.
//! Changing a line's wording means changing the fixture, and with it the drivers.

use super::{PlatformError, Rect, ReplaceOutcome};
use std::fmt::{self, Debug, Display, Formatter};
use std::time::Duration;

/// What a read's line calls it: "shortcut" when the user pressed the shortcut, which may copy the
/// selection, and "automatic" for an automatic suggestion, which never does.
pub fn how(shortcut: bool) -> &'static str {
    if shortcut { "shortcut" } else { "automatic" }
}

/// A selection read: `shortcut com.example.quillpoint: read 42 characters via AT-SPI in 12ms;
/// pasting allowed`.
pub struct Read<'a> {
    /// [`how`].
    pub how: &'a str,
    pub app: &'a str,
    pub chars: usize,
    /// The field's whole length, where the platform reads it (macOS).
    pub of_field: Option<i64>,
    /// How the text was read, such as "Accessibility", "AT-SPI" or "a guarded copy".
    pub via: &'a str,
    /// The element the text came from, written after `via` (Windows); empty elsewhere.
    pub element: &'a str,
    pub took: Duration,
    /// Whether the fix can be written back, and if not, why.
    pub access: &'a str,
}

impl Display for Read<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self { how, app, chars, of_field, via, element, took, access } = self;
        write!(f, "{how} {app}: read {chars} characters")?;
        if let Some(n) = of_field {
            write!(f, " of {n} in the field")?;
        }
        write!(f, " via {via}{element} in {took:?}; {access}")
    }
}

/// A selection that couldn't be read: `shortcut com.example.quillpoint: <the error> (12ms)`.
pub struct ReadFailed<'a> {
    /// [`how`].
    pub how: &'a str,
    pub app: &'a str,
    pub error: &'a dyn Display,
    pub took: Duration,
}

impl Display for ReadFailed<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {} ({:?})", self.how, self.app, self.error, self.took)
    }
}

/// The error for an app that answers neither its accessibility interface nor a copy. A read's
/// [`ReadFailed`] line carries its message, which the Linux driver looks for.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub fn not_responding(app_name: &str) -> PlatformError {
    PlatformError::Other(format!("{app_name} isn’t responding, so SuperEditr couldn’t read the selection."))
}

/// How a replacement ended (Linux and Windows): `replace com.example.quillpoint: applied in 180ms`.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub struct Replaced<'a> {
    pub app: &'a str,
    /// The user chose to replace anyway, leaving the fix on the clipboard (Windows).
    pub anyway: bool,
    pub outcome: &'a ReplaceOutcome,
    pub took: Duration,
}

impl Display for Replaced<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let anyway = if self.anyway { " anyway, without putting the clipboard aside" } else { "" };
        write!(f, "replace {}{anyway}: ", self.app)?;
        match self.outcome {
            ReplaceOutcome::Applied => write!(f, "applied")?,
            ReplaceOutcome::CopyOnly(why) => write!(f, "copy-only ({why})")?,
            ReplaceOutcome::Uncertain(why) => write!(f, "uncertain ({why})")?,
            ReplaceOutcome::ClipboardKept(why) => write!(f, "clipboard kept ({why})")?,
        }
        write!(f, " in {:?}", self.took)
    }
}

/// Whether focus came back to the field before a replacement (Windows): `focus back in 40ms`.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct FocusReturned {
    pub back: bool,
    pub took: Duration,
}

impl Display for FocusReturned {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "focus {} in {:?}", if self.back { "back" } else { "not back" }, self.took)
    }
}

/// A proofread applied change by change (macOS): `com.example.quillpoint: applied in place: 2 edits
/// in 35 ms by typing; verified`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct AppliedInPlace<'a> {
    pub app: &'a str,
    pub edits: usize,
    pub ms: u128,
    /// Written with Accessibility, rather than typed.
    pub native: bool,
    pub verified: bool,
    /// What became of the field's code, starting "; ", or empty.
    pub code: &'a str,
}

impl Display for AppliedInPlace<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: applied in place: {} edits in {} ms by {}; {}{}",
            self.app,
            self.edits,
            self.ms,
            if self.native { "Accessibility writes" } else { "typing" },
            if self.verified { "verified" } else { "not verified" },
            self.code
        )
    }
}

/// A proofread applied change by change that stopped part way (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct AppliedPartly<'a> {
    pub app: &'a str,
    pub applied: usize,
    pub total: usize,
    pub reason: &'a str,
}

impl Display for AppliedPartly<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: applied in place partly: {} of {} edits, then {}",
            self.app, self.applied, self.total, self.reason
        )
    }
}

/// A fix that replaces the whole selection rather than being applied change by change (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct ApplyingWhole<'a> {
    pub app: &'a str,
    pub reason: &'a dyn Display,
}

impl Display for ApplyingWhole<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: applying the whole selection: {}", self.app, self.reason)
    }
}

/// The panel's glide after the reviewed window (macOS), with each step as "frame time|time
/// made|distance": milliseconds from the start, then points.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Glide<'a> {
    /// How it ended: "done", "replaced" or "stopped by a move".
    pub how: &'a str,
    /// What drove its steps: "display link" or "timer".
    pub driver: &'a str,
    pub distance: f64,
    pub steps: &'a [(f64, f64, f64)],
}

impl Display for Glide<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "follow: glide {} by {}, {:.1} pt in {} steps: ",
            self.how,
            self.driver,
            self.distance,
            self.steps.len()
        )?;
        for (i, (frame, made, pt)) in self.steps.iter().enumerate() {
            let comma = if i == 0 { "" } else { ", " };
            write!(f, "{comma}{frame:.1}|{made:.1}|{pt:.2}")?;
        }
        Ok(())
    }
}

/// The code a read found in the field (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct CodeInField<'a> {
    pub pieces: usize,
    pub took: Duration,
    /// The walk of the field ran out of its budget.
    pub stopped_early: bool,
    pub subroles: &'a str,
}

impl Display for CodeInField<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let early = if self.stopped_early { " (stopped early)" } else { "" };
        write!(f, "code in the field: {} pieces in {:?}{early}; subroles {}", self.pieces, self.took, self.subroles)
    }
}

/// What became of one automatic suggestion attempt: `automatic com.example.quillpoint: after drag,
/// shown`.
pub struct Automatic<'a> {
    pub app: Option<&'a str>,
    pub gesture: &'a dyn Display,
    pub outcome: &'a dyn Display,
}

impl Display for Automatic<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.app {
            Some(app) => write!(f, "automatic {app}: after {}, {}", self.gesture, self.outcome),
            None => write!(f, "automatic: after {}, {}", self.gesture, self.outcome),
        }
    }
}

/// An [`Automatic`] outcome: the selection is longer than automatic suggestions take.
pub struct OverMaximum {
    pub chars: usize,
    pub max: usize,
}

impl Display for OverMaximum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "skipped: {} characters, over the maximum of {}", self.chars, self.max)
    }
}

/// A proofread pasted change by change (Linux): `paste chromium: applied in place: 2 changes in 2
/// writes in 310 ms`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct PastedInPlace<'a> {
    pub app: &'a str,
    pub changes: usize,
    pub writes: usize,
    pub ms: u128,
}

impl Display for PastedInPlace<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "paste {}: applied in place: {} changes in {} writes in {} ms",
            self.app, self.changes, self.writes, self.ms
        )
    }
}

/// A fix pasted over the whole selection rather than change by change (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct PastedWhole<'a> {
    pub app: &'a str,
    pub why: &'a dyn Display,
}

impl Display for PastedWhole<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "paste {}: the whole selection: {}", self.app, self.why)
    }
}

/// A [`PastedWhole`] reason: applying the changes one by one takes more writes than are made.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct TooManyWrites {
    pub changes: usize,
    pub writes: usize,
    pub max: usize,
}

impl Display for TooManyWrites {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} changes take {} writes, more than {}", self.changes, self.writes, self.max)
    }
}

/// How a pasted fix carries its formatting (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct PastedFormatting<'a> {
    pub app: &'a str,
    pub html: bool,
}

impl Display for PastedFormatting<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let how = if self.html { "HTML" } else { "plain text: HTML can’t hold its spaces" };
        write!(f, "paste {}: the formatting goes as {how}", self.app)
    }
}

/// Where the panel goes (Linux): `panel opens at 412,230, 548×186`. The driver reads its height.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct PanelPlaced {
    /// The panel shows already, so it moves.
    pub shown: bool,
    pub at: Rect,
}

impl Display for PanelPlaced {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Rect { x, y, width, height } = self.at;
        let verb = if self.shown { "moves" } else { "opens" };
        write!(f, "panel {verb} at {x:.0},{y:.0}, {width:.0}×{height:.0}")
    }
}

/// The panel took keyboard focus (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct PanelFocused {
    pub took: Duration,
}

impl Display for PanelFocused {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "panel focused after {:?}", self.took)
    }
}

/// A shortcut the desktop's GlobalShortcuts portal reported (Linux): `shortcut review from the
/// portal`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct ShortcutFromPortal<'a> {
    pub action: &'a str,
}

impl Display for ShortcutFromPortal<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "shortcut {} from the portal", self.action)
    }
}

/// A shortcut Windows reported (`WM_HOTKEY`): `shortcut 0x1: Some(Review)`.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct HotkeyPressed<'a> {
    pub id: i32,
    pub action: &'a dyn Debug,
}

impl Display for HotkeyPressed<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "shortcut {:#x}: {:?}", self.id, self.action)
    }
}

/// The main thread went a while without answering (Linux's watchdog).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct MainThreadSilent<'a> {
    pub silent: Duration,
    /// The command it was running.
    pub command: &'a str,
    /// What the kernel says the thread waits on.
    pub wait: &'a str,
}

impl Display for MainThreadSilent<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "main thread hasn’t answered in {:.1} s: {}; {}", self.silent.as_secs_f64(), self.command, self.wait)
    }
}

/// Hyprland reloaded its configuration, and SuperEditr bound its keys again (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct HyprlandReloaded {
    pub binds: usize,
}

impl Display for HyprlandReloaded {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Hyprland reloaded its config; added {} binds and the panel rule again", self.binds)
    }
}

/// One of a window's WebView2 processes stopped (Windows): `webview: the overlay window's Renderer
/// process stopped`.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct WebviewStopped<'a> {
    pub window: &'a str,
    /// [`BROWSER`] or [`RENDERER`].
    pub process: &'a str,
}

/// A window's WebView2 browser process, in [`WebviewStopped`].
#[cfg_attr(not(windows), allow(dead_code))]
pub const BROWSER: &str = "Browser";
/// A window's WebView2 renderer, in [`WebviewStopped`]. The Windows driver waits for this word.
#[cfg_attr(not(windows), allow(dead_code))]
pub const RENDERER: &str = "Renderer";

impl Display for WebviewStopped<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "webview: the {} window's {} process stopped", self.window, self.process)
    }
}

/// A window made again after its WebView2 browser process stopped (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
pub struct WebviewMadeAgain<'a> {
    pub window: &'a str,
    pub took: Duration,
}

impl Display for WebviewMadeAgain<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "webview: made the {} window again in {:?}", self.window, self.took)
    }
}

#[cfg(test)]
mod tests;
