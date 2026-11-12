//! The last review the AI finished, kept for a short while. A panel closed without applying the
//! fix (Esc, another app coming to the front, a click elsewhere) and reopened on the same, unchanged
//! selection shows that result again at once instead of asking the AI a second time.
//!
//! Only one review is kept, and it is shown again only when everything that went into it matches:
//! the same app and field, the same text, the same style and instruction, the same AI and model,
//! and the same settings. Applying a fix, saving the settings or changing an API key forgets it.

use crate::platform::Capture;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use supereditr_core::{Job, Outcome};

/// How long a finished review can be shown again. Long enough to close the panel, look something up
/// and come back to the same text; short enough that a review reopened after a break asks the AI
/// afresh, and that an old suggestion never stands in for a new one. After that it is never shown
/// again, and the next review drops it.
pub const KEEP_FOR: Duration = Duration::from_secs(10 * 60);

pub struct LastReview {
    capture: Capture,
    job: Job,
    outcome: Outcome,
    finished: Instant,
}

impl LastReview {
    /// A finished review worth showing again, or None: when no AI wrote the result (the offline
    /// checker answers instantly anyway), or when Apple Intelligence answered because the chosen AI
    /// couldn't, which the user should get another chance at.
    pub fn new(capture: Capture, job: Job, outcome: Outcome, finished: Instant) -> Option<Self> {
        let model_answered = outcome.first_token_ms.is_some() && outcome.backup_for.is_none();
        model_answered.then_some(Self { capture, job, outcome, finished })
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.finished) >= KEEP_FOR
    }

    /// Whether a review of `capture` that would run `job` can show this result instead. The job
    /// holds the text, the protected code ranges, the style, the instruction, the AI and model, and
    /// the settings that shape the answer (style notes, dialect, word list).
    fn matches(&self, capture: &Capture, job: &Job, now: Instant) -> bool {
        !self.expired(now) && same_place(&self.capture, capture) && self.job == *job
    }
}

/// Whether two captures came from the same app and field, as far as the platform can tell.
fn same_place(a: &Capture, b: &Capture) -> bool {
    a.pid == b.pid
        && a.bundle_id == b.bundle_id
        && a.editable == b.editable
        && a.playground == b.playground
        && a.text == b.text
        && a.same_field(b)
}

/// Holds the one review that may be shown again.
#[derive(Default)]
pub struct Memory(Mutex<Option<LastReview>>);

impl Memory {
    /// Keeps the review that just finished in place of the one before. `None`, a review not worth
    /// keeping, still replaces it: only the latest review is ever shown again.
    pub fn remember(&self, review: Option<LastReview>) {
        *self.0.lock().unwrap() = review;
    }

    /// The remembered result, when a review of `capture` that would run `job` can show it.
    pub fn recall(&self, capture: &Capture, job: &Job, now: Instant) -> Option<Outcome> {
        let mut last = self.0.lock().unwrap();
        if last.as_ref().is_some_and(|review| review.expired(now)) {
            *last = None;
        }
        last.as_ref().filter(|review| review.matches(capture, job, now)).map(|review| review.outcome.clone())
    }

    pub fn forget(&self) {
        self.0.lock().unwrap().take();
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

    fn outcome(original: &str) -> Outcome {
        let text = original.replace("yesterday", "on Tuesday");
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

    /// A finished Proofread of `TEXT`, with the capture and job that would show it again.
    pub(crate) fn example(finished: Instant) -> (Option<LastReview>, Capture, Job) {
        let captured = capture(TEXT);
        let proofread = job(&captured, "proofread", None);
        (LastReview::new(captured.clone(), proofread.clone(), outcome(TEXT), finished), captured, proofread)
    }

    /// A memory holding `example`, and the moment it finished.
    fn remembered() -> (Memory, Capture, Job, Instant) {
        let memory = Memory::default();
        let finished = Instant::now();
        let (review, captured, proofread) = example(finished);
        memory.remember(review);
        (memory, captured, proofread, finished)
    }

    #[test]
    fn the_same_selection_reopened_shows_the_last_result() {
        let (memory, captured, proofread, finished) = remembered();
        let again = memory.recall(&captured, &proofread, finished + Duration::from_secs(5)).unwrap();
        assert_eq!(again.text, outcome(TEXT).text);
        // Shown again, it stays remembered for the next time.
        assert!(memory.recall(&captured, &proofread, finished + Duration::from_secs(10)).is_some());
    }

    #[test]
    fn a_different_app_or_field_asks_again() {
        let (memory, captured, proofread, finished) = remembered();
        let soon = finished + Duration::from_secs(5);
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
        let (memory, _, _, finished) = remembered();
        let edited = capture("Rhyan sent the KPI report to Brynlee on Monday.");
        let soon = finished + Duration::from_secs(5);
        assert!(memory.recall(&edited, &job(&edited, "proofread", None), soon).is_none());
        let padded = capture(&format!("{TEXT} "));
        assert!(memory.recall(&padded, &job(&padded, "proofread", None), soon).is_none(), "even a space");
    }

    #[test]
    fn another_style_instruction_or_model_asks_again() {
        let (memory, captured, proofread, finished) = remembered();
        let soon = finished + Duration::from_secs(5);
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
        let (memory, captured, proofread, finished) = remembered();
        let soon = finished + Duration::from_secs(5);
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
        let (memory, captured, proofread, finished) = remembered();
        assert!(memory.recall(&captured, &proofread, finished + KEEP_FOR - Duration::from_secs(1)).is_some());
        assert!(memory.recall(&captured, &proofread, finished + KEEP_FOR).is_none());
        // Gone for good, even if the clock were read again earlier.
        assert!(memory.recall(&captured, &proofread, finished).is_none());
    }

    #[test]
    fn forgetting_drops_it() {
        let (memory, captured, proofread, finished) = remembered();
        memory.forget();
        assert!(memory.recall(&captured, &proofread, finished).is_none());
    }

    #[test]
    fn only_the_latest_review_is_kept() {
        let (memory, captured, proofread, finished) = remembered();
        let other = capture("Kyndal moved the OKR review to 1234 Glenmoor Ct.");
        let other_job = job(&other, "proofread", None);
        memory.remember(LastReview::new(other.clone(), other_job.clone(), outcome(&other.text), finished));
        assert!(memory.recall(&captured, &proofread, finished).is_none());
        assert!(memory.recall(&other, &other_job, finished).is_some());
        // A newer review not worth keeping still replaces it.
        memory.remember(None);
        assert!(memory.recall(&other, &other_job, finished).is_none());
    }

    #[test]
    fn only_a_result_the_chosen_ai_wrote_is_kept() {
        let captured = capture(TEXT);
        let proofread = job(&captured, "proofread", None);
        let mut checker = outcome(TEXT);
        checker.first_token_ms = None;
        assert!(LastReview::new(captured.clone(), proofread.clone(), checker, Instant::now()).is_none());
        let mut backup = outcome(TEXT);
        backup.backup_for = Some("Claude couldn’t answer: rate limited".into());
        assert!(LastReview::new(captured, proofread, backup, Instant::now()).is_none());
    }
}
