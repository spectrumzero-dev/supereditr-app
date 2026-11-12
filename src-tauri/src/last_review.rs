//! The reviews the AI finished for the current selection, one per style, kept for a short while. A
//! panel closed without applying the fix (Esc, another app coming to the front, a click elsewhere)
//! and reopened on the same, unchanged selection shows the result again at once instead of asking
//! the AI a second time, and so does switching back to a style that already has a result.
//!
//! Results are kept for one selection only, at most one per style, and one is shown again only when
//! everything that went into it matches: the same app and field, the same text, the same style, no
//! instruction, the same AI and model, and the same settings. A result for another selection drops
//! them all, and so do applying a fix, saving the settings and changing an API key.

use crate::platform::Capture;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use supereditr_core::{Job, Outcome};

/// How long a finished review can be shown again. Long enough to close the panel, look something up
/// and come back to the same text; short enough that a review reopened after a break asks the AI
/// afresh, and that an old suggestion never stands in for a new one. After that it is never shown
/// again, and the next review drops it. Each style's result counts from when it arrived.
pub const KEEP_FOR: Duration = Duration::from_secs(10 * 60);

/// A review that just finished, with the selection and job it was asked about.
pub struct Finished {
    capture: Capture,
    job: Job,
    outcome: Outcome,
    at: Instant,
}

impl Finished {
    pub fn new(capture: Capture, job: Job, outcome: Outcome, at: Instant) -> Self {
        Self { capture, job, outcome, at }
    }

    /// Whether the result is worth showing again: not when it answered an instruction (a reopened
    /// panel or a switch of style never has one), not when no AI wrote it (the offline checker
    /// answers instantly anyway), and not when Apple Intelligence answered because the chosen AI
    /// couldn't, which the user should get another chance at.
    fn worth_keeping(&self) -> bool {
        self.job.steer.is_none() && self.outcome.first_token_ms.is_some() && self.outcome.backup_for.is_none()
    }
}

/// One style's result for the kept selection.
struct Entry {
    /// Holds the text, the protected code ranges, the style, the AI and model, and the settings
    /// that shape the answer (style notes, dialect, word list).
    job: Job,
    outcome: Outcome,
    finished: Instant,
}

impl Entry {
    fn style(&self) -> &str {
        &self.job.preset.id
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.finished) >= KEEP_FOR
    }
}

/// Whether two captures came from the same app and field, as far as the platform can tell, and hold
/// the same text.
fn same_place(a: &Capture, b: &Capture) -> bool {
    a.pid == b.pid
        && a.bundle_id == b.bundle_id
        && a.editable == b.editable
        && a.playground == b.playground
        && a.text == b.text
        && a.same_field(b)
}

/// Holds the results that may be shown again.
#[derive(Default)]
pub struct Memory(Mutex<Kept>);

#[derive(Default)]
struct Kept {
    /// The selection the entries are for. None when there are none.
    capture: Option<Capture>,
    /// At most one per style, the one shown most recently last.
    entries: Vec<Entry>,
    /// Counts `forget` calls. A review started before one finishes after it, and is not kept: the
    /// settings it was asked with may have changed in ways its job doesn't show.
    generation: u64,
}

impl Kept {
    /// Whether the entries are for `capture`'s selection.
    fn holds(&self, capture: &Capture) -> bool {
        self.capture.as_ref().is_some_and(|kept| same_place(kept, capture))
    }

    /// Drops the results older than `KEEP_FOR`, and the selection once none is left.
    fn drop_expired(&mut self, now: Instant) {
        self.entries.retain(|entry| !entry.expired(now));
        if self.entries.is_empty() {
            self.capture = None;
        }
    }
}

impl Memory {
    /// Taken when a review starts, and handed back to `remember` when it finishes.
    pub fn generation(&self) -> u64 {
        self.0.lock().unwrap().generation
    }

    /// Takes in a review that just finished. A review of another selection drops every result kept
    /// for the one before, even when its own result isn't worth keeping. A review of the same
    /// selection replaces its style's result only; one with an instruction leaves them all as they
    /// are. A review that started before the last `forget` is dropped.
    pub fn remember(&self, started_at: u64, review: Finished) {
        let mut kept = self.0.lock().unwrap();
        if started_at != kept.generation {
            return;
        }
        if !kept.holds(&review.capture) {
            kept.entries.clear();
        }
        if review.job.steer.is_none() {
            kept.entries.retain(|entry| entry.style() != review.job.preset.id);
            if review.worth_keeping() {
                kept.capture = Some(review.capture.clone());
                kept.entries.push(Entry {
                    job: review.job.clone(),
                    outcome: review.outcome.clone(),
                    finished: review.at,
                });
            }
        }
        kept.drop_expired(review.at);
    }

    /// The kept result, when a review of `capture` that would run `job` can show it.
    pub fn recall(&self, capture: &Capture, job: &Job, now: Instant) -> Option<Outcome> {
        let mut kept = self.0.lock().unwrap();
        kept.drop_expired(now);
        if !kept.holds(capture) {
            return None;
        }
        let index = kept.entries.iter().position(|entry| entry.job == *job)?;
        // Shown again, it becomes the one shown most recently.
        let entry = kept.entries.remove(index);
        let outcome = entry.outcome.clone();
        kept.entries.push(entry);
        Some(outcome)
    }

    /// The style whose result was shown most recently for `capture`'s selection, for a panel
    /// reopened on it without a style of its own to open with.
    pub fn last_style(&self, capture: &Capture, now: Instant) -> Option<String> {
        let mut kept = self.0.lock().unwrap();
        kept.drop_expired(now);
        if !kept.holds(capture) {
            return None;
        }
        kept.entries.last().map(|entry| entry.style().to_string())
    }

    /// Drops every kept result, and any review still running when this is called.
    pub fn forget(&self) {
        let mut kept = self.0.lock().unwrap();
        kept.entries.clear();
        kept.capture = None;
        kept.generation += 1;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::{self, Rect};
    use supereditr_core::Config;
    use supereditr_core::provider::{ProviderKind, ProviderProfile};

    const TEXT: &str = "Rhyan sent the KPI report to Brynlee yesterday.";

    fn capture(text: &str) -> Capture {
        platform::playground_capture(text.into(), Rect { x: 10.0, y: 10.0, width: 300.0, height: 20.0 })
    }

    fn job(capture: &Capture, preset_id: &str, steer: Option<&str>) -> Job {
        let config = Config::default();
        let mut profile = ProviderProfile::new(ProviderKind::Anthropic);
        profile.model = "claude-sonnet-5".into();
        Job {
            text: capture.text.clone(),
            preset: config.preset(preset_id).unwrap().clone(),
            steer: steer.map(Into::into),
            profile: Some(profile),
            style_notes: config.style_notes.clone(),
            dialect: config.dialect.clone(),
            words: config.words.clone(),
            code: capture.protected(),
            local_checks: true,
            builtin_ai: true,
        }
    }

    /// What the AI answered for `original`, told apart by `answer`.
    fn outcome(original: &str, answer: &str) -> Outcome {
        let text = original.replace("yesterday", answer);
        Outcome {
            view: supereditr_core::diff::diff(original, &text),
            text,
            source: "claude-sonnet-5".into(),
            first_token_ms: Some(900),
            total_ms: 6100,
            changed_tokens: Vec::new(),
            grade_before: None,
            grade_after: None,
            backup_for: None,
            kept: Vec::new(),
            kept_terms: Vec::new(),
            learnable: Vec::new(),
        }
    }

    /// A finished `preset_id` review of `TEXT` that answered `answer`.
    fn finished(preset_id: &str, answer: &str, at: Instant) -> Finished {
        let captured = capture(TEXT);
        Finished::new(captured.clone(), job(&captured, preset_id, None), outcome(TEXT, answer), at)
    }

    /// What a review of `TEXT` in `preset_id` would show again, if anything.
    fn shown(memory: &Memory, preset_id: &str, now: Instant) -> Option<String> {
        let captured = capture(TEXT);
        memory.recall(&captured, &job(&captured, preset_id, None), now).map(|outcome| outcome.text)
    }

    fn answered(answer: &str) -> Option<String> {
        Some(TEXT.replace("yesterday", answer))
    }

    /// A finished Proofread of `TEXT`, with the capture and job that would show it again.
    pub(crate) fn example(at: Instant) -> (Finished, Capture, Job) {
        let captured = capture(TEXT);
        let proofread = job(&captured, "proofread", None);
        (Finished::new(captured.clone(), proofread.clone(), outcome(TEXT, "on Tuesday"), at), captured, proofread)
    }

    /// A memory holding `example`, and the moment it finished.
    fn remembered() -> (Memory, Capture, Job, Instant) {
        let memory = Memory::default();
        let at = Instant::now();
        let (review, captured, proofread) = example(at);
        memory.remember(memory.generation(), review);
        (memory, captured, proofread, at)
    }

    fn kept_styles(memory: &Memory) -> Vec<String> {
        memory.0.lock().unwrap().entries.iter().map(|entry| entry.style().to_string()).collect()
    }

    #[test]
    fn the_same_selection_reopened_shows_the_last_result() {
        let (memory, captured, proofread, at) = remembered();
        let again = memory.recall(&captured, &proofread, at + Duration::from_secs(5)).unwrap();
        assert_eq!(Some(again.text), answered("on Tuesday"));
        // Shown again, it stays remembered for the next time.
        assert!(memory.recall(&captured, &proofread, at + Duration::from_secs(10)).is_some());
    }

    #[test]
    fn switching_back_to_a_style_shows_its_result() {
        let memory = Memory::default();
        let at = Instant::now();
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        // Polish asks the AI, and its result is kept beside Proofread's.
        assert_eq!(shown(&memory, "polish", at), None);
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at + Duration::from_secs(8)));
        let later = at + Duration::from_secs(20);
        assert_eq!(shown(&memory, "proofread", later), answered("on Tuesday"));
        assert_eq!(shown(&memory, "polish", later), answered("last Tuesday"));
        assert_eq!(shown(&memory, "shorter", later), None, "a style not asked yet");
    }

    #[test]
    fn a_rerun_replaces_only_its_own_style() {
        let memory = Memory::default();
        let at = Instant::now();
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));
        // R in Polish asks the AI again.
        memory.remember(memory.generation(), finished("polish", "on Tuesday morning", at + Duration::from_secs(30)));
        let later = at + Duration::from_secs(40);
        assert_eq!(shown(&memory, "polish", later), answered("on Tuesday morning"));
        assert_eq!(shown(&memory, "proofread", later), answered("on Tuesday"));
        // One result per style, however often a style is asked.
        let mut styles = kept_styles(&memory);
        styles.sort();
        assert_eq!(styles, ["polish", "proofread"]);
    }

    #[test]
    fn each_style_expires_on_its_own() {
        let memory = Memory::default();
        let at = Instant::now();
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        let polished = at + Duration::from_secs(5 * 60);
        memory.remember(memory.generation(), finished("polish", "last Tuesday", polished));
        let after_proofread = at + KEEP_FOR;
        assert_eq!(shown(&memory, "proofread", after_proofread), None);
        assert_eq!(shown(&memory, "polish", after_proofread), answered("last Tuesday"));
        assert_eq!(shown(&memory, "polish", polished + KEEP_FOR - Duration::from_secs(1)), answered("last Tuesday"));
        assert_eq!(shown(&memory, "polish", polished + KEEP_FOR), None);
        assert!(memory.0.lock().unwrap().capture.is_none(), "nothing of the selection is left");
    }

    #[test]
    fn reopening_opens_the_style_shown_last() {
        let memory = Memory::default();
        let at = Instant::now();
        let captured = capture(TEXT);
        assert_eq!(memory.last_style(&captured, at), None);
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));
        assert_eq!(memory.last_style(&captured, at).as_deref(), Some("polish"));
        // Switching back to Proofread shows its result, and makes it the one shown last.
        assert!(shown(&memory, "proofread", at).is_some());
        assert_eq!(memory.last_style(&captured, at).as_deref(), Some("proofread"));
        // Not for another selection.
        assert_eq!(memory.last_style(&capture("Kyndal moved the OKR review."), at), None);
        assert_eq!(memory.last_style(&captured, at + KEEP_FOR), None, "nor once expired");
    }

    #[test]
    fn a_different_app_or_field_asks_again() {
        let (memory, captured, proofread, at) = remembered();
        let soon = at + Duration::from_secs(5);
        let mut other_app = captured.clone();
        other_app.pid += 1;
        assert!(memory.recall(&other_app, &proofread, soon).is_none(), "another process");
        let mut other_bundle = captured.clone();
        other_bundle.bundle_id = "com.quillpoint.notes".into();
        assert!(memory.recall(&other_bundle, &proofread, soon).is_none(), "another app");
        let mut read_only = captured.clone();
        read_only.editable = false;
        assert!(memory.recall(&read_only, &proofread, soon).is_none(), "text that can't be replaced");
        let mut elsewhere = captured.clone();
        elsewhere.playground = false;
        assert!(memory.recall(&elsewhere, &proofread, soon).is_none(), "Settings' box against an app");
        // Still there for the original place.
        assert!(memory.recall(&captured, &proofread, soon).is_some());
    }

    #[test]
    fn changed_text_asks_again() {
        let (memory, _, _, at) = remembered();
        let edited = capture("Rhyan sent the KPI report to Brynlee on Monday.");
        let soon = at + Duration::from_secs(5);
        assert!(memory.recall(&edited, &job(&edited, "proofread", None), soon).is_none());
        let padded = capture(&format!("{TEXT} "));
        assert!(memory.recall(&padded, &job(&padded, "proofread", None), soon).is_none(), "even a space");
    }

    #[test]
    fn another_style_instruction_or_model_asks_again() {
        let (memory, captured, proofread, at) = remembered();
        let soon = at + Duration::from_secs(5);
        assert!(memory.recall(&captured, &job(&captured, "polish", None), soon).is_none(), "another style");
        let steered = job(&captured, "proofread", Some("make it warmer"));
        assert!(memory.recall(&captured, &steered, soon).is_none(), "an instruction");

        let mut other_model = proofread.clone();
        other_model.profile.as_mut().unwrap().model = "claude-haiku-5".into();
        assert!(memory.recall(&captured, &other_model, soon).is_none(), "another model");
        let mut other_provider = proofread.clone();
        other_provider.profile = Some(ProviderProfile::new(ProviderKind::OpenAi));
        assert!(memory.recall(&captured, &other_provider, soon).is_none(), "another provider");
        let mut built_in = proofread.clone();
        built_in.profile = None;
        assert!(memory.recall(&captured, &built_in, soon).is_none(), "the built-in AI");
    }

    #[test]
    fn settings_that_shape_the_answer_ask_again() {
        let (memory, captured, proofread, at) = remembered();
        let soon = at + Duration::from_secs(5);
        let mut words = proofread.clone();
        words.words.push("Kyndal".into());
        assert!(memory.recall(&captured, &words, soon).is_none(), "the word list");
        let mut notes = proofread.clone();
        notes.style_notes = "No em dashes.".into();
        assert!(memory.recall(&captured, &notes, soon).is_none(), "style notes");
        let mut dialect = proofread.clone();
        dialect.dialect = "British English".into();
        assert!(memory.recall(&captured, &dialect, soon).is_none(), "the dialect");
        let mut edited_style = proofread.clone();
        edited_style.preset.instruction.push_str(" Keep it short.");
        assert!(memory.recall(&captured, &edited_style, soon).is_none(), "an edited style");
    }

    #[test]
    fn an_old_result_is_forgotten() {
        let (memory, captured, proofread, at) = remembered();
        assert!(memory.recall(&captured, &proofread, at + KEEP_FOR - Duration::from_secs(1)).is_some());
        assert!(memory.recall(&captured, &proofread, at + KEEP_FOR).is_none());
        // Gone for good, even if the clock were read again earlier.
        assert!(memory.recall(&captured, &proofread, at).is_none());
    }

    #[test]
    fn forgetting_drops_every_style() {
        let memory = Memory::default();
        let at = Instant::now();
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));
        memory.forget();
        assert_eq!(shown(&memory, "proofread", at), None);
        assert_eq!(shown(&memory, "polish", at), None);
        assert_eq!(memory.last_style(&capture(TEXT), at), None);
    }

    #[test]
    fn a_review_that_started_before_forgetting_is_not_kept() {
        let memory = Memory::default();
        let at = Instant::now();
        let started_at = memory.generation();
        // The settings are saved while the AI is still answering.
        memory.forget();
        let (review, captured, proofread) = example(at);
        memory.remember(started_at, review);
        assert!(memory.recall(&captured, &proofread, at).is_none());
        // A review started after the save is kept.
        memory.remember(memory.generation(), example(at).0);
        assert!(memory.recall(&captured, &proofread, at).is_some());
    }

    #[test]
    fn a_review_of_another_selection_drops_every_style() {
        let memory = Memory::default();
        let at = Instant::now();
        memory.remember(memory.generation(), finished("proofread", "on Tuesday", at));
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));
        let other = capture("Kyndal moved the OKR review to 1234 Glenmoor Ct yesterday.");
        let other_job = job(&other, "proofread", None);
        memory.remember(
            memory.generation(),
            Finished::new(other.clone(), other_job.clone(), outcome(&other.text, "today"), at),
        );
        assert_eq!(shown(&memory, "proofread", at), None);
        assert_eq!(shown(&memory, "polish", at), None);
        assert!(memory.recall(&other, &other_job, at).is_some());

        // Even when the newer review's own result isn't worth keeping.
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));
        let mut checker = outcome(&other.text, "today");
        checker.first_token_ms = None;
        memory.remember(memory.generation(), Finished::new(other.clone(), other_job.clone(), checker, at));
        assert_eq!(shown(&memory, "polish", at), None);
        assert!(memory.0.lock().unwrap().capture.is_none());
    }

    #[test]
    fn a_result_with_an_instruction_or_not_from_the_chosen_ai_is_never_kept() {
        let memory = Memory::default();
        let at = Instant::now();
        let captured = capture(TEXT);
        memory.remember(memory.generation(), finished("polish", "last Tuesday", at));

        // An instruction: not kept, and the style's own result stays.
        let steered = job(&captured, "polish", Some("make it warmer"));
        memory.remember(
            memory.generation(),
            Finished::new(captured.clone(), steered.clone(), outcome(TEXT, "on Tuesday"), at),
        );
        assert!(memory.recall(&captured, &steered, at).is_none());
        assert_eq!(shown(&memory, "polish", at), answered("last Tuesday"));

        // The offline checker and the backup: not kept, and a newer answer for the style than the
        // kept one, which goes.
        let mut checker = outcome(TEXT, "on Tuesday");
        checker.first_token_ms = None;
        memory.remember(
            memory.generation(),
            Finished::new(captured.clone(), job(&captured, "proofread", None), checker, at),
        );
        assert_eq!(shown(&memory, "proofread", at), None);
        let mut backup = outcome(TEXT, "on Tuesday");
        backup.backup_for = Some("Claude couldn’t answer: rate limited".into());
        memory
            .remember(memory.generation(), Finished::new(captured.clone(), job(&captured, "polish", None), backup, at));
        assert_eq!(shown(&memory, "polish", at), None);
        assert!(kept_styles(&memory).is_empty());
    }
}
