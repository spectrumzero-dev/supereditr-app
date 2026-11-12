//! Keys SuperEditr sends: Ctrl+C for a guarded copy, Ctrl+V for a paste.
//!
//! Every event carries `TAG` in `dwExtraInfo`, so a later keyboard hook can tell SuperEditr's own
//! input from the user's. The user's Ctrl and Shift are usually still down when the shortcut's
//! `WM_HOTKEY` arrives, and Ctrl+V sent then would be Ctrl+Shift+V, so held modifiers are released
//! first. Releasing a held Windows key on its own would open Start, so a key that does nothing is
//! sent in between, as PowerToys does.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
};

use super::capture::InputWatch;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// "SESE", marking events SuperEditr sent.
pub const TAG: usize = 0x5345_5345;
/// A virtual key no layout assigns (0xE8 is unassigned), sent so a released Windows key doesn't
/// open Start, and so Windows counts SuperEditr as the last to send input.
const NOTHING: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if matches!(vk, VK_RCONTROL | VK_RMENU | VK_RWIN | VK_LWIN | VK_INSERT) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: TAG } },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    LAST_SENT.store(since_start(), Ordering::SeqCst);
    sent as usize == inputs.len()
}

/// Milliseconds since SuperEditr first asked.
fn since_start() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// When SuperEditr last sent keys (`since_start`).
static LAST_SENT: AtomicU64 = AtomicU64::new(0);
/// For this long after SuperEditr's own keys, a key that's down is taken for one of them.
const OWN_KEYS_MS: u64 = 150;
const KEY_POLL: Duration = Duration::from_millis(10);
/// The longest a key watch runs, whatever its owner does.
const KEY_WATCH_LIMIT: Duration = Duration::from_secs(30);

/// Whether a copy key is down now: Ctrl with C, X or Insert.
fn copy_keys_down() -> bool {
    held(VK_CONTROL) && [VIRTUAL_KEY(u16::from(b'C')), VIRTUAL_KEY(u16::from(b'X')), VK_INSERT].into_iter().any(held)
}

/// Starts watching, on a thread of its own, for the user pressing a copy key themselves (Ctrl+C,
/// Ctrl+X or Ctrl+Insert); keys down just after SuperEditr sent its own don't count. The function
/// returned says whether they did, and says yes when the watch couldn't start. Dropping it ends the
/// watch.
pub fn watch_copy_keys() -> InputWatch {
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let pressed = Arc::new(AtomicBool::new(false));
    let stop = Stop(Arc::new(AtomicBool::new(false)));
    let (seen, stopped) = (pressed.clone(), stop.0.clone());
    let watching = std::thread::Builder::new().name("copy keys".into()).spawn(move || {
        let started = Instant::now();
        while !stopped.load(Ordering::SeqCst) && started.elapsed() < KEY_WATCH_LIMIT {
            let ours = since_start().saturating_sub(LAST_SENT.load(Ordering::SeqCst)) < OWN_KEYS_MS;
            if !ours && copy_keys_down() {
                seen.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(KEY_POLL);
        }
    });
    if watching.is_err() {
        pressed.store(true, Ordering::SeqCst);
    }
    Box::new(move || {
        let _ends_with_this = &stop;
        pressed.load(Ordering::SeqCst)
    })
}

pub fn held(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(vk.0)) }) < 0
}

const MODIFIERS: [VIRTUAL_KEY; 8] =
    [VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];

/// Sends a key-up for every modifier Windows thinks is down. Returns how many there were.
pub fn release_modifiers() -> usize {
    let down: Vec<VIRTUAL_KEY> = MODIFIERS.into_iter().filter(|&vk| held(vk)).collect();
    if down.is_empty() {
        return 0;
    }
    let mut inputs = vec![key(NOTHING, false), key(NOTHING, true)];
    inputs.extend(down.iter().map(|&vk| key(vk, true)));
    send(&inputs);
    down.len()
}

/// Waits up to `limit` for the user to let go of every modifier, then releases any still held.
pub fn wait_for_modifiers(limit: std::time::Duration) {
    let started = std::time::Instant::now();
    while MODIFIERS.into_iter().any(held) && started.elapsed() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    release_modifiers();
}

/// Ctrl and a letter (`b'C'`, `b'V'`), tagged. Returns whether Windows took every event; it says
/// nothing about whether the app got them (a higher-integrity window drops them silently).
pub fn ctrl(letter: u8) -> bool {
    let letter = VIRTUAL_KEY(u16::from(letter));
    send(&[key(VK_CONTROL, false), key(letter, false), key(letter, true), key(VK_CONTROL, true)])
}

/// A key that does nothing, so Windows lets SuperEditr bring its panel to the front right after the
/// user's shortcut: a process may take the foreground when it sent the last input.
pub fn nudge() -> bool {
    send(&[key(NOTHING, false), key(NOTHING, true)])
}
