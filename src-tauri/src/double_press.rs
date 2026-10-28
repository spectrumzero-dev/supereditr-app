//! Double-press shortcuts such as ⌘E E: hold ⌘ and press E twice (pressing ⌘E twice also works).
//!
//! The system's hotkey API fires on every press, so a double press needs a keyboard event tap that
//! sees each key as it happens. The first matching press is held back for `WINDOW`. A second press
//! within that time fires the action and neither press reaches the app. Otherwise the held press is
//! replayed, so ⌘E keeps doing what the app expects (centering text in Google Docs, for example).
//!
//! This module is the platform-neutral part: parsing and the decision for each key press. The macOS
//! event tap lives in `platform::macos`.

use std::time::{Duration, Instant};

/// How long the first press waits for the second.
pub const WINDOW: Duration = Duration::from_millis(300);

pub const COMMAND: u8 = 1;
pub const CONTROL: u8 = 2;
pub const ALT: u8 = 4;
pub const SHIFT: u8 = 8;

/// One key with its modifiers, as written in an accelerator ("Command+E").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: u8,
    /// The physical key: "A"–"Z" or "0"–"9".
    pub key: String,
}

/// Parses "Command+E Command+E" into the repeated chord. Returns `None` for an ordinary single
/// shortcut, and `Err` for a two-part accelerator this can't handle.
pub fn parse(accelerator: &str) -> Option<Result<Chord, String>> {
    let parts: Vec<&str> = accelerator.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    Some(match parts.as_slice() {
        [first, second] if first.eq_ignore_ascii_case(second) => parse_chord(first),
        _ => Err("only a shortcut pressed twice, such as ⌘E E, can have two parts".into()),
    })
}

fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut modifiers = 0;
    let mut key = None;
    for part in text.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "command" | "cmd" | "super" | "cmdorctrl" => modifiers |= COMMAND,
            "control" | "ctrl" => modifiers |= CONTROL,
            "alt" | "option" => modifiers |= ALT,
            "shift" => modifiers |= SHIFT,
            other if other.len() == 1 && other.chars().all(|c| c.is_ascii_alphanumeric()) => {
                key = Some(other.to_ascii_uppercase())
            }
            _ => return Err(format!("{part} can't be pressed twice; use a letter or digit")),
        }
    }
    let key = key.ok_or("a double press needs a letter or digit")?;
    // Without a modifier other than Shift, every doubled letter in normal typing would trigger it.
    if modifiers & !SHIFT == 0 {
        return Err("a double press needs ⌘, ⌃, or ⌥".into());
    }
    Ok(Chord { modifiers, key })
}

/// What to do with one key press.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Step {
    /// Replay this held chord to the app first (index into the registered chords).
    pub replay: Option<usize>,
    /// Let the current press through. When `replay` is set, the press must be re-sent after the
    /// replay to keep the order, so the caller drops it and posts a copy.
    pub pass: bool,
    /// Run this chord's action.
    pub fire: Option<usize>,
    /// Hold the current press; call `expire` with this generation after `WINDOW`.
    pub hold: Option<u64>,
}

struct Held {
    chord: usize,
    at: Instant,
    generation: u64,
}

#[derive(Default)]
pub struct Detector {
    held: Option<Held>,
    generation: u64,
}

impl Detector {
    /// `matched` is the index of the registered chord this press is, if any.
    pub fn press(&mut self, matched: Option<usize>, autorepeat: bool, now: Instant) -> Step {
        let held = self.held.take();
        match (matched, held) {
            // Holding the key down repeats it; swallow the repeats while waiting for the second press.
            (Some(chord), Some(h)) if autorepeat && h.chord == chord => {
                self.held = Some(h);
                Step::default()
            }
            (_, None) if autorepeat => Step { pass: true, ..Step::default() },
            (Some(chord), Some(h)) if h.chord == chord && now.duration_since(h.at) <= WINDOW => {
                Step { fire: Some(chord), ..Step::default() }
            }
            (Some(chord), held) => {
                self.generation += 1;
                self.held = Some(Held { chord, at: now, generation: self.generation });
                Step { replay: held.map(|h| h.chord), hold: Some(self.generation), ..Step::default() }
            }
            (None, held) => Step { replay: held.map(|h| h.chord), pass: true, ..Step::default() },
        }
    }

    /// The wait for a second press is over. Returns the chord to replay, if it is still held.
    pub fn expire(&mut self, generation: u64) -> Option<usize> {
        match &self.held {
            Some(h) if h.generation == generation => self.held.take().map(|h| h.chord),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_doubled_shortcuts() {
        assert_eq!(parse("Command+E Command+E"), Some(Ok(Chord { modifiers: COMMAND, key: "E".into() })));
        assert_eq!(parse("Control+Alt+E"), None, "single shortcuts are left to the system hotkey API");
        assert!(matches!(parse("Command+E Command+R"), Some(Err(_))));
        assert!(matches!(parse("Shift+E Shift+E"), Some(Err(_))), "would fire while typing capitals");
        assert!(matches!(parse("Command+Space Command+Space"), Some(Err(_))));
    }

    #[test]
    fn two_quick_presses_fire_and_neither_reaches_the_app() {
        let mut d = Detector::default();
        let t = Instant::now();
        let first = d.press(Some(0), false, t);
        assert_eq!(first.hold, Some(1));
        assert!(!first.pass);
        let second = d.press(Some(0), false, t + Duration::from_millis(150));
        assert_eq!(second, Step { fire: Some(0), ..Step::default() });
        assert_eq!(d.expire(1), None, "the timer finds nothing left to replay");
    }

    #[test]
    fn a_single_press_is_replayed_to_the_app() {
        let mut d = Detector::default();
        let first = d.press(Some(0), false, Instant::now());
        assert_eq!(d.expire(first.hold.unwrap()), Some(0));
    }

    #[test]
    fn another_key_releases_the_held_press_first() {
        let mut d = Detector::default();
        let t = Instant::now();
        d.press(Some(0), false, t);
        let next = d.press(None, false, t + Duration::from_millis(50));
        assert_eq!(next, Step { replay: Some(0), pass: true, ..Step::default() });
        assert_eq!(d.expire(1), None);
    }

    #[test]
    fn holding_the_key_down_is_not_a_second_press() {
        let mut d = Detector::default();
        let t = Instant::now();
        let first = d.press(Some(0), false, t);
        assert_eq!(d.press(Some(0), true, t + Duration::from_millis(100)), Step::default());
        assert_eq!(d.expire(first.hold.unwrap()), Some(0));
    }

    #[test]
    fn a_slow_second_press_starts_over() {
        let mut d = Detector::default();
        let t = Instant::now();
        d.press(Some(0), false, t);
        // The timer would normally have replayed the first press by now; if it is late, the late
        // press replays it and waits for its own second press.
        let late = d.press(Some(0), false, t + WINDOW + Duration::from_millis(10));
        assert_eq!(late.replay, Some(0));
        assert_eq!(late.hold, Some(2));
    }
}
