//! Orchestrates one review: instant local check, then a streamed model revision.

use crate::diff::{self, DiffView};
use crate::guard;
use crate::lang;
use crate::local::LocalChecker;
use crate::prompts::{self, Preset, PromptInput};
use crate::provider::{ProviderKind, ProviderProfile, Providers};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// Offline checker result, typically within a few milliseconds.
    Local {
        view: DiffView,
        ms: u64,
    },
    /// The model's revision so far.
    Partial {
        view: DiffView,
        #[serde(rename = "firstTokenMs")]
        first_token_ms: u64,
        /// Which model is writing, so the overlay can say when the backup took over.
        source: String,
    },
    Done(Outcome),
    Failed {
        message: String,
        #[serde(rename = "localOnly")]
        local_only: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub view: DiffView,
    pub text: String,
    pub source: String,
    pub first_token_ms: Option<u64>,
    pub total_ms: u64,
    /// Links, mentions, numbers or code from the original that the revision changed.
    pub changed_tokens: Vec<String>,
    /// Readability grade before and after, for selections long enough to score.
    pub grade_before: Option<i32>,
    pub grade_after: Option<i32>,
    /// Set when Apple Intelligence stood in for the chosen AI, saying why.
    pub backup_for: Option<String>,
    /// Changes (by id) that replace one of the author's own terms, offered as rejected, and the
    /// terms they would have dropped.
    pub kept: Vec<usize>,
    pub kept_terms: Vec<String>,
    /// Words in the original that the dictionary doesn't know and the word list doesn't have yet;
    /// the panel offers to always keep one when its change is rejected.
    pub learnable: Vec<String>,
}

impl Outcome {
    fn new(
        original: &str,
        view: DiffView,
        text: String,
        source: String,
        first_token_ms: Option<u64>,
        total_ms: u64,
    ) -> Self {
        Self {
            changed_tokens: guard::missing_tokens(original, &text),
            grade_before: guard::readability(original).map(|r| r.grade),
            grade_after: guard::readability(&text).map(|r| r.grade),
            view,
            text,
            source,
            first_token_ms,
            total_ms,
            backup_for: None,
            kept: Vec::new(),
            kept_terms: Vec::new(),
            learnable: Vec::new(),
        }
    }

    /// Small models swap coined words and acronyms for synonyms even when told not to
    /// ("glanceable" → "quick overview"), and may "fix" a name. Changes that drop one of those,
    /// or an entry from the author's word list, start out rejected.
    fn keep_authors_terms(&mut self, original: &str, words: &[String]) {
        let chars: Vec<char> = original.chars().collect();
        // Each term as the original spells it, wherever it appears: a listed "Rhyan" also finds
        // "rhyan".
        let mut spellings: Vec<String> = Vec::new();
        let terms = crate::words::found_in(original, words).into_iter().chain(crate::local::unfamiliar_terms(original));
        for term in terms {
            for (s, e) in crate::words::occurrences(original, &term) {
                let spelling: String = chars[s..e].iter().collect();
                if !spellings.contains(&spelling) {
                    spellings.push(spelling);
                }
            }
        }
        // A term the revision has fewer of was dropped somewhere: "Rhyan met Rhyan" → "Rhyan met
        // Ryan". A reworded sentence that only moves a term keeps the count.
        let count = |text: &str, term: &str| crate::words::spans(text, term).len();
        self.kept_terms = spellings.into_iter().filter(|t| count(&self.text, t) < count(original, t)).collect();
        let spans: Vec<(usize, usize)> =
            self.kept_terms.iter().flat_map(|t| crate::words::spans(original, t)).collect();
        for segment in &self.view.segments {
            if let diff::Segment::Change { id, old, new, old_start, .. } = segment {
                let (a, b) = (*old_start, old_start + old.chars().count());
                // A change drops a term when the part of it the change covers is gone from its
                // replacement: "Ct" → "Court" in "1234 Glenmoor Ct", but not "Rhyan" → "Rhyan,".
                let drops = |&(s, e): &(usize, usize)| {
                    if a == b {
                        s < a && a < e
                    } else if a < e && s < b {
                        let part: String = chars[s.max(a)..e.min(b)].iter().collect();
                        !crate::words::contains(new, part.trim())
                    } else {
                        false
                    }
                };
                if spans.iter().any(drops) {
                    self.kept.push(*id);
                }
            }
        }
    }

    /// The revision with the changes in `kept` left out: what quick fix and the CLI apply.
    pub fn kept_text(&self) -> String {
        diff::apply(&self.view, &self.kept.iter().copied().collect())
    }

    /// Words the panel may offer to add to the word list: unknown to the dictionary and not
    /// already listed.
    fn offer_to_learn(&mut self, original: &str, words: &[String]) {
        let listed: Vec<String> = crate::words::dictionary_words(words).map(str::to_lowercase).collect();
        self.learnable =
            crate::local::unknown_words(original).into_iter().filter(|w| !listed.contains(&w.to_lowercase())).collect();
    }
}

pub struct Job {
    pub text: String,
    pub preset: Preset,
    pub steer: Option<String>,
    /// The AI the user chose. `None` means the built-in one: Apple Intelligence when this Mac has
    /// it, otherwise the offline checker alone.
    pub profile: Option<ProviderProfile>,
    pub style_notes: String,
    pub dialect: String,
    /// The author's word list: names and terms to keep exactly as written.
    pub words: Vec<String>,
    /// Character ranges of `text` the app shows as code but whose Markdown backticks are gone,
    /// such as a code span in a chat composer. They are kept exactly as written, like code in
    /// backticks.
    pub code: Vec<(usize, usize)>,
    /// Whether the offline checker may run. Off only for the CLI's `--no-local` and for tests that
    /// need the AI's answer alone.
    pub local_checks: bool,
    /// Whether Apple Intelligence may answer when `profile` is `None` or fails. Off for the CLI's
    /// `--offline` and for tests that need the offline checker alone.
    pub builtin_ai: bool,
}

#[derive(Clone)]
pub struct Engine {
    pub providers: Providers,
    local: Arc<Mutex<Option<Arc<LocalChecker>>>>,
    dialect: String,
}

/// Minimum spacing between partial updates; the UI cannot show more than this anyway.
const FRAME: Duration = Duration::from_millis(30);

impl Engine {
    pub fn new(dialect: &str) -> Self {
        Self { providers: Providers::new(), local: Arc::default(), dialect: dialect.to_string() }
    }

    /// Builds the local checker ahead of the first review (it loads a dictionary), and again after
    /// the word list changes.
    pub fn prepare(&self, words: &[String]) {
        self.local(words);
    }

    fn local(&self, words: &[String]) -> Arc<LocalChecker> {
        let mut local = self.local.lock().unwrap_or_else(|e| e.into_inner());
        match local.as_ref().filter(|checker| checker.words == words) {
            Some(checker) => checker.clone(),
            None => {
                let checker = Arc::new(LocalChecker::new(crate::local::dialect_from_name(&self.dialect), words));
                *local = Some(checker.clone());
                checker
            }
        }
    }

    /// Apple's on-device model, when it is ready on this Mac. It reviews when no other AI is chosen
    /// and takes over when the chosen one can't answer.
    pub async fn builtin(&self) -> Option<ProviderProfile> {
        let profile = ProviderProfile::new(ProviderKind::AppleIntelligence);
        crate::provider::apple_helper_path(&profile.base_url)?;
        self.providers.apple_status(&profile).await.available.then_some(profile)
    }

    pub async fn run(&self, job: Job, on_event: &mut (dyn FnMut(Event) + Send)) {
        // A secret is never reviewed: not by the offline checker, and never sent to any AI.
        if let Some(found) =
            crate::sensitive::find(&job.text).or_else(|| job.steer.as_deref().and_then(crate::sensitive::find))
        {
            on_event(Event::Failed { message: found.message(), local_only: false });
            return;
        }
        let started = Instant::now();
        let code: Vec<(usize, usize)> =
            guard::code_spans(&job.text).into_iter().chain(job.code.iter().copied()).collect();
        let english = lang::is_english(&job.text);
        let is_proofread = job.preset.id == "proofread" && job.steer.is_none();

        let profile = match job.profile.clone() {
            Some(profile) => Some(profile),
            None if job.builtin_ai => self.builtin().await,
            None => None,
        };
        // The offline checker is the basic level: with no AI it is the review, and when the AI
        // fails it is the fallback. While an AI answers it stays out of the way, because its
        // guesses at unfamiliar words (acronyms, coined terms) are worse than the model's.
        let can_check_locally = job.local_checks && english;
        let Some(mut profile) = profile else {
            if !can_check_locally && english && is_proofread {
                on_event(Event::Failed {
                    message:
                        "Nothing is set up to check this text: no AI is available and the rule-based checker is off."
                            .into(),
                    local_only: false,
                });
                return;
            }
            let view = if can_check_locally {
                self.local_review(&job.text, &job.words, &code, started, on_event)
            } else {
                diff::diff(&job.text, &job.text)
            };
            if !is_proofread || !english {
                on_event(Event::Failed {
                    message:
                        "Turn on Apple Intelligence or connect an AI in Settings to rewrite or check non-English text."
                            .into(),
                    local_only: can_check_locally,
                });
                return;
            }
            let text = diff::apply(&view, &Default::default());
            let total = started.elapsed().as_millis() as u64;
            let mut outcome = Outcome::new(&job.text, view, text, "Rule-based checker".into(), None, total);
            outcome.offer_to_learn(&job.text, &job.words);
            on_event(Event::Done(outcome));
            return;
        };

        let detected = lang::detect(&job.text);
        let language = detected.as_ref().filter(|d| d.reliable && d.code != "eng").map(|d| d.name);
        let request = prompts::build(&PromptInput {
            text: &job.text,
            preset: &job.preset,
            steer: job.steer.as_deref(),
            style_notes: &job.style_notes,
            dialect: &job.dialect,
            words: &crate::words::found_in(&job.text, &job.words),
            language,
        });

        let original = job.text.clone();
        let mut raw = String::new();
        let mut first_token: Option<u64> = None;
        let mut last_frame = Instant::now() - FRAME;
        let mut source = source_name(&profile, false);
        let mut backup_for: Option<String> = None;
        let result = loop {
            let result = {
                let mut on_delta = |delta: &str| {
                    raw.push_str(delta);
                    let first = *first_token.get_or_insert_with(|| started.elapsed().as_millis() as u64);
                    if last_frame.elapsed() >= FRAME {
                        last_frame = Instant::now();
                        let partial = prompts::clean_partial(&original, &raw);
                        on_event(Event::Partial {
                            view: diff::stream_view(&original, &partial),
                            first_token_ms: first,
                            source: source.clone(),
                        });
                    }
                };
                self.providers.stream(&profile, &request, &mut on_delta).await
            };
            // A chosen AI that fails before writing anything (offline, bad key, rate limit) hands
            // the review to Apple Intelligence. Once text has streamed, the failure is reported.
            if let Err(error) = &result
                && raw.is_empty()
                && profile.kind != ProviderKind::AppleIntelligence
                && job.builtin_ai
                && let Some(builtin) = self.builtin().await
            {
                // The panel adds ". Apple Intelligence answered instead.", so no period of its own.
                let reason = error.to_string();
                backup_for =
                    Some(format!("{} couldn’t answer: {}", profile.kind.display_name(), reason.trim_end_matches('.')));
                profile = builtin;
                source = source_name(&profile, true);
                continue;
            }
            break result;
        };

        // On failure, fall back to the offline checker's result where it can help.
        let fail = |message: String, on_event: &mut (dyn FnMut(Event) + Send)| {
            if can_check_locally {
                self.local_review(&original, &job.words, &code, started, on_event);
            }
            on_event(Event::Failed { message, local_only: can_check_locally });
        };
        match result.map(|full| prompts::clean_output(&original, &full)) {
            // Checked after cleanup, so a reply of only wrapper tags can't erase the selection.
            Ok(text) if text.trim().is_empty() && !original.trim().is_empty() => {
                fail("The model returned no text. Try again.".into(), on_event)
            }
            Ok(text) => {
                // A small model can miss rule-level mistakes ("Their going" on some Macs' Apple
                // model). The offline checker's confident fixes (wrong-word rules, near-miss typos,
                // never guesses at unknown words) run over its result as a final pass, and the panel
                // shows them as the checker's. Cloud AIs get none: see `needs_final_pass`.
                let pass = (can_check_locally && profile.kind.needs_final_pass())
                    .then(|| self.local(&job.words).check(&text))
                    .filter(|pass| pass.change_count > 0);
                let text = pass.as_ref().map_or(text, |pass| diff::apply(pass, &Default::default()));
                let mut view = diff::diff(&original, &text);
                if let Some(pass) = &pass {
                    diff::mark_rule_based(&mut view, pass);
                }
                // Code stays exactly as written, whatever the model or the final pass did to it.
                let view = diff::without_changes_in(view, &code);
                let text = diff::apply(&view, &Default::default());
                let total = started.elapsed().as_millis() as u64;
                let mut outcome = Outcome::new(&original, view, text, source, first_token, total);
                outcome.backup_for = backup_for;
                outcome.keep_authors_terms(&original, &job.words);
                outcome.offer_to_learn(&original, &job.words);
                on_event(Event::Done(outcome));
            }
            Err(error) => fail(error.to_string(), on_event),
        }
    }

    /// Runs the offline checker, leaving `code` alone, and reports its result as a `Local` event.
    fn local_review(
        &self,
        text: &str,
        words: &[String],
        code: &[(usize, usize)],
        started: Instant,
        on_event: &mut (dyn FnMut(Event) + Send),
    ) -> DiffView {
        let view = diff::without_changes_in(self.local(words).check(text), code);
        on_event(Event::Local { view: view.clone(), ms: started.elapsed().as_millis() as u64 });
        view
    }
}

/// How the overlay names the model that wrote a revision.
fn source_name(profile: &ProviderProfile, backup: bool) -> String {
    match (profile.kind, backup) {
        (ProviderKind::AppleIntelligence, false) => "Apple Intelligence".into(),
        (ProviderKind::AppleIntelligence, true) => "Apple Intelligence (backup)".into(),
        _ => profile.model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The changes a revision would start out with rejected, as their original text.
    fn kept(original: &str, revised: &str, words: &[&str]) -> Vec<String> {
        let mut outcome = Outcome::new(original, diff::diff(original, revised), revised.into(), String::new(), None, 0);
        outcome.keep_authors_terms(original, &words.iter().map(|w| w.to_string()).collect::<Vec<_>>());
        outcome
            .view
            .segments
            .iter()
            .filter_map(|s| match s {
                diff::Segment::Change { id, old, .. } if outcome.kept.contains(id) => Some(old.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn keeps_every_occurrence_of_a_listed_word() {
        assert_eq!(kept("Rhyan met Rhyan today.", "Rhyan met Ryan today.", &["Rhyan"]), vec!["Rhyan"]);
        assert_eq!(kept("then rhyan left.", "then Ryan left.", &["Rhyan"]), vec!["rhyan"]);
        assert!(kept("Rhyan met Rhyan today.", "Rhyan met Rhyan, today.", &["Rhyan"]).is_empty());
    }

    #[test]
    fn kept_text_leaves_out_rejected_changes() {
        let (original, revised) = ("I took Rhyan to tenis.", "I took Ryan to tennis.");
        let mut outcome = Outcome::new(original, diff::diff(original, revised), revised.into(), String::new(), None, 0);
        outcome.keep_authors_terms(original, &["Rhyan".to_string()]);
        assert_eq!(outcome.kept_text(), "I took Rhyan to tennis.");
    }

    #[test]
    fn keeps_phrases_whole() {
        let original = "Meet at 1234 Glenmoor Ct today.";
        assert_eq!(kept(original, "Meet at 1234 Glenmoor Court today.", &["1234 Glenmoor Ct"]), vec!["Ct"]);
        assert!(kept(original, "Meet at 1234 Glenmoor Ct, today.", &["1234 Glenmoor Ct"]).is_empty());
    }

    #[test]
    fn a_moved_term_is_not_dropped() {
        let original = "Show a glanceable dashboard.";
        assert!(kept(original, "Show a dashboard that is glanceable.", &[]).is_empty());
    }
}
