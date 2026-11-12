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
    sent as usize == inputs.len()
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
