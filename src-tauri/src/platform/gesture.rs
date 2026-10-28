//! Decides when a selection the user is making in another app is finished, so an automatic
//! suggestion opens once, for the final selection, instead of after every step of it.
//!
//! A mouse drag, a double-click, and ⌘A are finished when they happen. A keyboard selection (Shift
//! with the arrows, Home/End, or Page Up/Down) and a Shift-click are still being adjusted while
//! Shift is held: they finish when Shift comes up. If Shift stays down with nothing changing, the
//! selection counts as finished after [`HELD`].

use super::{SelectionGesture, WatchEvent};
use std::time::Duration;

/// After a mouse selection or ⌘A: time for the app to settle its selection (and for a double-click
/// to become a word).
pub const AFTER_MOUSE: Duration = Duration::from_millis(70);
/// After Shift comes up: time to press it again and keep adjusting before the review starts.
pub const AFTER_RELEASE: Duration = Duration::from_millis(150);
/// While Shift is still held: how long the selection must stay unchanged to count as finished.
pub const HELD: Duration = Duration::from_millis(1500);

/// What the watcher saw, reduced to what matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    MouseDown,
    MouseUp {
        dragged: bool,
        clicks: isize,
        shift: bool,
    },
    /// `navigation`: an arrow, Home/End, or Page Up/Down key.
    KeyDown {
        navigation: bool,
        shift: bool,
    },
    KeyUp {
        navigation: bool,
        shift: bool,
        select_all: bool,
    },
    /// A modifier key went down or up; `shift` is whether Shift is now held.
    ModifiersChanged {
        shift: bool,
    },
    Scroll,
    /// Another app came to the front: a selection under way belonged to the one before.
    AppSwitched,
}

#[derive(Debug, Default)]
pub struct Gesture {
    /// A Shift selection is under way and will finish when Shift comes up.
    adjusting: bool,
    /// Whether Shift was held at the last modifier change, so only Shift's own changes count.
    shift_held: bool,
}

impl Gesture {
    pub const fn new() -> Self {
        Self { adjusting: false, shift_held: false }
    }

    pub fn feed(&mut self, input: Input) -> Option<WatchEvent> {
        if let Input::ModifiersChanged { shift } = input {
            let was_held = std::mem::replace(&mut self.shift_held, shift);
            if shift == was_held {
                // Another modifier (⌥, ⌘) changed while Shift stayed as it was.
                return None;
            }
        }
        match input {
            Input::AppSwitched => {
                self.adjusting = false;
                Some(WatchEvent::Wait)
            }
            Input::KeyDown { navigation: true, shift: true } => {
                self.adjusting = true;
                Some(WatchEvent::Interaction)
            }
            // Anything else ends the selection under way (Shift+Tab may even move to another
            // field). A Shift-click re-arms it at mouse-up.
            Input::MouseDown | Input::KeyDown { .. } | Input::Scroll => {
                self.adjusting = false;
                Some(WatchEvent::Interaction)
            }
            Input::MouseUp { shift: true, .. } | Input::KeyUp { navigation: true, shift: true, .. } => {
                self.adjusting = true;
                Some(WatchEvent::MaybeSelection { settle: HELD, gesture: SelectionGesture::Shift })
            }
            Input::MouseUp { dragged, clicks, .. } if dragged || clicks >= 2 => {
                let gesture = if dragged { SelectionGesture::Drag } else { SelectionGesture::DoubleClick };
                Some(WatchEvent::MaybeSelection { settle: AFTER_MOUSE, gesture })
            }
            Input::KeyUp { select_all: true, .. } => {
                Some(WatchEvent::MaybeSelection { settle: AFTER_MOUSE, gesture: SelectionGesture::SelectAll })
            }
            Input::ModifiersChanged { shift: false } if self.adjusting => {
                self.adjusting = false;
                Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
            }
            // Shift going down again may continue a selection: hold off a review about to start.
            Input::ModifiersChanged { shift: true } => Some(WatchEvent::Wait),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(inputs: &[Input]) -> Vec<Option<WatchEvent>> {
        let mut gesture = Gesture::default();
        inputs.iter().map(|&input| gesture.feed(input)).collect()
    }

    const SHIFT_DOWN: Input = Input::ModifiersChanged { shift: true };
    const SHIFT_UP: Input = Input::ModifiersChanged { shift: false };
    const SHIFT_ARROW_DOWN: Input = Input::KeyDown { navigation: true, shift: true };
    const SHIFT_ARROW_UP: Input = Input::KeyUp { navigation: true, shift: true, select_all: false };
    const KEY_DOWN: Input = Input::KeyDown { navigation: false, shift: false };

    #[test]
    fn a_shift_arrow_selection_finishes_when_shift_comes_up() {
        let events = run(&[
            SHIFT_DOWN,
            SHIFT_ARROW_DOWN,
            SHIFT_ARROW_UP,
            SHIFT_ARROW_DOWN,
            SHIFT_ARROW_UP,
            SHIFT_ARROW_DOWN,
            SHIFT_ARROW_UP,
            SHIFT_UP,
        ]);
        let quick: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Some(WatchEvent::MaybeSelection { settle, .. }) if *settle < HELD))
            .collect();
        assert_eq!(
            quick,
            [&Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })],
            "{events:?}"
        );
        assert_eq!(
            events.last(),
            Some(&Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift }))
        );
        // Each step only arms the fallback for a Shift held without change.
        assert_eq!(events[2], Some(WatchEvent::MaybeSelection { settle: HELD, gesture: SelectionGesture::Shift }));
    }

    #[test]
    fn releasing_the_arrow_after_shift_still_finishes_on_shift() {
        let arrow_up = Input::KeyUp { navigation: true, shift: false, select_all: false };
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, SHIFT_UP, arrow_up]);
        assert_eq!(
            events[2],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
        assert_eq!(events[3], None);
    }

    #[test]
    fn a_shift_click_waits_for_shift() {
        let click = Input::MouseUp { dragged: false, clicks: 1, shift: true };
        let events = run(&[SHIFT_DOWN, Input::MouseDown, click, SHIFT_UP]);
        assert_eq!(events[2], Some(WatchEvent::MaybeSelection { settle: HELD, gesture: SelectionGesture::Shift }));
        assert_eq!(
            events[3],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
    }

    #[test]
    fn mouse_selections_and_select_all_are_finished_at_once() {
        let drag = Input::MouseUp { dragged: true, clicks: 1, shift: false };
        let double = Input::MouseUp { dragged: false, clicks: 2, shift: false };
        let select_all = Input::KeyUp { navigation: false, shift: false, select_all: true };
        for (input, gesture) in [
            (drag, SelectionGesture::Drag),
            (double, SelectionGesture::DoubleClick),
            (select_all, SelectionGesture::SelectAll),
        ] {
            assert_eq!(run(&[input]), [Some(WatchEvent::MaybeSelection { settle: AFTER_MOUSE, gesture })], "{input:?}");
        }
        assert_eq!(run(&[Input::MouseUp { dragged: false, clicks: 1, shift: false }]), [None]);
    }

    #[test]
    fn typing_capitals_with_shift_is_not_a_selection() {
        let capital = Input::KeyUp { navigation: false, shift: true, select_all: false };
        assert_eq!(
            run(&[SHIFT_DOWN, Input::KeyDown { navigation: false, shift: true }, capital, SHIFT_UP]),
            [Some(WatchEvent::Wait), Some(WatchEvent::Interaction), None, None]
        );
    }

    #[test]
    fn pressing_shift_again_holds_off_a_pending_review() {
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, SHIFT_ARROW_UP, SHIFT_UP, SHIFT_DOWN, KEY_DOWN]);
        assert_eq!(
            events[3],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
        assert_eq!(events[4], Some(WatchEvent::Wait));
        assert_eq!(events[5], Some(WatchEvent::Interaction));
    }

    #[test]
    fn plain_arrows_are_not_a_selection() {
        let arrow_down = Input::KeyDown { navigation: true, shift: false };
        let arrow_up = Input::KeyUp { navigation: true, shift: false, select_all: false };
        assert_eq!(run(&[arrow_down, arrow_up]), [Some(WatchEvent::Interaction), None]);
    }

    #[test]
    fn other_modifiers_changing_while_shift_is_held_change_nothing() {
        let option_up = Input::ModifiersChanged { shift: true };
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, SHIFT_ARROW_UP, option_up, SHIFT_UP]);
        // Releasing ⌥ leaves the held-Shift fallback in place: no Wait cancels it.
        assert_eq!(events[3], None);
        assert_eq!(
            events[4],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
    }

    #[test]
    fn switching_apps_abandons_a_selection_under_way() {
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, SHIFT_ARROW_UP, Input::AppSwitched, SHIFT_UP]);
        assert_eq!(events[3], Some(WatchEvent::Wait));
        assert_eq!(events[4], None, "Shift coming up in the new app must not review its selection");
    }

    #[test]
    fn another_key_or_scroll_ends_the_selection() {
        let shift_tab = Input::KeyDown { navigation: false, shift: true };
        for interruption in [shift_tab, Input::Scroll, Input::MouseDown] {
            let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, SHIFT_ARROW_UP, interruption, SHIFT_UP]);
            assert_eq!(events[3], Some(WatchEvent::Interaction), "{interruption:?}");
            assert_eq!(events[4], None, "{interruption:?}: Shift coming up must not review");
        }
        // A new Shift step after the interruption starts a new selection.
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_DOWN, Input::Scroll, SHIFT_ARROW_DOWN, SHIFT_ARROW_UP, SHIFT_UP]);
        assert_eq!(
            events[5],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
    }

    /// Key repeat while an arrow is held sends key-downs only; a lone key-up still counts.
    #[test]
    fn a_step_seen_only_at_key_up_still_counts() {
        let events = run(&[SHIFT_DOWN, SHIFT_ARROW_UP, SHIFT_UP, SHIFT_DOWN]);
        assert_eq!(
            events[2],
            Some(WatchEvent::MaybeSelection { settle: AFTER_RELEASE, gesture: SelectionGesture::Shift })
        );
        assert_eq!(events[3], Some(WatchEvent::Wait));
    }
}
