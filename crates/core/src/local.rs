//! Instant, offline English checks powered by Harper (Apache-2.0, pure Rust).
//!
//! Results arrive in single-digit milliseconds, so the overlay can show real fixes before the
//! model's first token, and SuperEditr still works with no model configured at all.

use crate::diff::{self, DiffView, Segment};
use harper_core::linting::{Lint, LintGroup, LintKind, Linter, Suggestion};
use harper_core::spell::{Dictionary, FstDictionary, MergedDictionary, MutableDictionary};
use harper_core::{Dialect, DictWordMetadata, Document};
use std::sync::{Arc, LazyLock, Mutex};

pub struct LocalChecker {
    linter: Mutex<LintGroup>,
    /// Harper's dictionary, the title-case forms it drops, and the author's word list.
    dictionary: Arc<MergedDictionary>,
    /// The word list this checker was built with, so the engine knows when to rebuild it.
    pub words: Vec<String>,
}

/// Title-case spellings Harper's dictionary drops. It stores one spelling per word whatever its
/// case, the last one listed, so "Ida" is kept only as a flag on "IDA", and Harper then takes the
/// name for a typo ("Id").
static TITLE_CASE_FORMS: LazyLock<Arc<MutableDictionary>> = LazyLock::new(|| {
    let curated = FstDictionary::curated();
    let mut forms = MutableDictionary::new();
    forms.extend_words(curated.words_iter().filter_map(|word| {
        let metadata = curated.get_word_metadata(word)?;
        let (first, rest) = word.split_first()?;
        let title: Vec<char> = first.to_uppercase().chain(rest.iter().flat_map(|c| c.to_lowercase())).collect();
        (metadata.is_titlecase() && title != word).then(|| (title, metadata.into_owned()))
    }));
    Arc::new(forms)
});

pub fn dialect_from_name(name: &str) -> Dialect {
    let lower = name.to_lowercase();
    if lower.contains("brit") || lower.contains("uk") {
        Dialect::British
    } else if lower.contains("canad") {
        Dialect::Canadian
    } else if lower.contains("austral") {
        Dialect::Australian
    } else if lower.contains("india") {
        Dialect::Indian
    } else {
        Dialect::American
    }
}

impl LocalChecker {
    pub fn new(dialect: Dialect, words: &[String]) -> Self {
        let mut own = MutableDictionary::new();
        own.extend_words(
            crate::words::dictionary_words(words)
                .map(|w| (w.chars().collect::<Vec<char>>(), DictWordMetadata::default())),
        );
        let mut dictionary = MergedDictionary::new();
        dictionary.add_dictionary(FstDictionary::curated());
        dictionary.add_dictionary(TITLE_CASE_FORMS.clone());
        dictionary.add_dictionary(Arc::new(own));
        let dictionary = Arc::new(dictionary);
        let linter = LintGroup::new_curated(dictionary.clone(), dialect);
        Self { linter: Mutex::new(linter), dictionary, words: words.to_vec() }
    }

    /// Applies the first suggestion of every non-overlapping lint and returns the comparison,
    /// with Harper's explanation attached to each change.
    pub fn check(&self, text: &str) -> DiffView {
        let dictionary = self.dictionary.as_ref();
        let document = Document::new_plain_english(text, dictionary);
        let mut lints = match self.linter.lock() {
            Ok(mut linter) => linter.lint(&document),
            Err(_) => return diff::diff(text, text),
        };
        // Leave code, commands, paths, identifiers, and addresses alone: "fixing" `frobnix-try` or
        // `org/repo` as English breaks them.
        let mut technical = technical_spans(text, dictionary);
        technical.extend(crate::guard::code_spans(text));
        let chars_in: Vec<char> = text.chars().collect();
        lints.retain(|lint| {
            !lint.suggestions.is_empty()
                && !technical.iter().any(|&(start, end)| lint.span.start < end && start < lint.span.end)
                && confident_spelling(lint, &chars_in, dictionary)
                && !recases_a_known_capital(lint, &chars_in, dictionary)
        });
        lints.sort_by_key(|lint| std::cmp::Reverse(lint.span.start));

        let mut chars: Vec<char> = text.chars().collect();
        let mut boundary = usize::MAX;
        let mut applied = Vec::new();
        for lint in lints {
            if lint.span.end > boundary || lint.span.end > chars.len() {
                continue;
            }
            lint.suggestions[0].apply(lint.span, &mut chars);
            boundary = lint.span.start;
            applied.push(lint);
        }
        let revised: String = chars.into_iter().collect();
        let mut view = diff::diff(text, &revised);
        for segment in &mut view.segments {
            if let Segment::Change { old, old_start, note, .. } = segment {
                let end = *old_start + old.chars().count().max(1);
                if let Some(lint) =
                    applied.iter().find(|l| l.span.start < end && *old_start < l.span.end.max(l.span.start + 1))
                {
                    *note = Some(lint.message.clone());
                }
            }
        }
        view
    }
}

/// The author's own terms that an AI rewrite should keep: acronyms (KPI, OKR) and words the
/// dictionary doesn't know (glanceable, product and people's names). Uses only the dictionary,
/// not the linter, so it is cheap to run next to an AI review.
pub fn unfamiliar_terms(text: &str) -> Vec<String> {
    unknown(text, false)
}

/// Every word the dictionary doesn't know, near misses included ("Rhyan", one letter from "Ryan"):
/// the words the panel offers to always keep.
pub fn unknown_words(text: &str) -> Vec<String> {
    unknown(text, true)
}

fn unknown(text: &str, with_near_misses: bool) -> Vec<String> {
    let dictionary = FstDictionary::curated();
    let mut terms: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
        .map(|w| w.trim_matches(|c| c == '-' || c == '\''))
        // A possessive is the name itself: "Brynlee's" → "Brynlee".
        .map(|w| w.strip_suffix("'s").unwrap_or(w))
        .filter(|w| w.chars().count() >= 2 && w.chars().all(|c| c.is_alphabetic() || c == '-'))
        .filter(|w| {
            let acronym = w.chars().filter(|c| c.is_uppercase()).count() >= 2;
            acronym
                || w.split('-').any(|part| {
                    !part.is_empty()
                        && !dictionary.contains_word_str(part)
                        && (with_near_misses || !near_miss(part, dictionary.as_ref()))
                })
        })
        .map(str::to_string)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Whether an unknown word is one or two letters from a real one ("tomorow"), so a typo rather
/// than the author's own term.
fn near_miss(word: &str, dictionary: &FstDictionary) -> bool {
    let allowed = if word.chars().count() >= 8 { 2 } else { 1 };
    !dictionary.fuzzy_match_str(&word.to_lowercase(), allowed, 1).is_empty()
}

/// Keeps a fix to a word the dictionary doesn't know only when it is a near miss ("teh" → "the",
/// "realy" → "really"). An unfamiliar word is more often a term the dictionary lacks
/// ("glanceable", "KPI") than a typo, and Harper's best guess for it ("changeable") is wrong.
/// Fixes to known words used wrongly ("their" → "they're") are rules, and are kept.
fn confident_spelling(lint: &Lint, text: &[char], dictionary: &dyn Dictionary) -> bool {
    let Some(word) = text.get(lint.span.start..lint.span.end) else { return false };
    // Splitting one long word in two ("glanceable" → "glance able") is a guess at a coined
    // word, not a fix; short ones are real mistakes ("alot" → "a lot").
    if let Some(Suggestion::ReplaceWith(fix)) = lint.suggestions.first()
        && !word.contains(&' ')
        && fix.contains(&' ')
        && word.len() > 5
    {
        return false;
    }
    if lint.lint_kind != LintKind::Spelling {
        return true;
    }
    // Acronyms and other words in capitals: KPI, OKR, IA. Checked first: the dictionary matches
    // regardless of case, so it may "know" an acronym as a lowercase word.
    if word.iter().filter(|c| c.is_uppercase()).count() >= 2 {
        return false;
    }
    if dictionary.contains_exact_word(word) {
        return true;
    }
    // A capitalized word inside a sentence is almost always a name ("I am taking Rhyan to tennis"),
    // and Harper's guess for it ("Ryan") is wrong. At the start of a sentence it may be a typo.
    if word.first().is_some_and(|c| c.is_uppercase()) && !starts_sentence(text, lint.span.start) {
        return false;
    }
    let Some(Suggestion::ReplaceWith(fix)) = lint.suggestions.first() else { return false };
    let lower = |w: &[char]| w.iter().flat_map(|c| c.to_lowercase()).collect::<Vec<char>>();
    let allowed = if word.len() >= 8 { 2 } else { 1 };
    edit_distance(&lower(word), &lower(fix)) <= allowed
}

/// Whether a fix only changes the case of a capitalized word the dictionary lists exactly as
/// written. The dictionary has both "Sam" and the acronym "SAM", and Harper's casing rule turns
/// every such word into the acronym ("Hi SAM"); the author's own capitals are the better guess.
/// Lowercase words still get case fixes: "i" → "I", "nasa" → "NASA", a sentence's first word.
fn recases_a_known_capital(lint: &Lint, text: &[char], dictionary: &dyn Dictionary) -> bool {
    let Some(word) = text.get(lint.span.start..lint.span.end) else { return false };
    let Some(Suggestion::ReplaceWith(fix)) = lint.suggestions.first() else { return false };
    let lower = |w: &[char]| w.iter().flat_map(|c| c.to_lowercase()).collect::<Vec<char>>();
    word.first().is_some_and(|c| c.is_uppercase())
        && fix.as_slice() != word
        && lower(fix) == lower(word)
        && dictionary.contains_exact_word(word)
}

/// Whether the word at `start` begins a sentence, line, or list item.
fn starts_sentence(text: &[char], start: usize) -> bool {
    for &c in text[..start].iter().rev() {
        match c {
            '\n' => return true,
            c if c.is_whitespace() => {}
            '"' | '\'' | '“' | '‘' | '(' | '[' => {}
            c => return matches!(c, '.' | '!' | '?' | ':' | ';' | '…' | '-' | '–' | '—' | '*' | '•'),
        }
    }
    true
}

/// Edit distance counting a swap of neighboring letters as one edit ("teh" → "the").
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    d[0] = (0..=m).collect();
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// Character ranges of words that look like code rather than prose: paths, flags, identifiers,
/// file names, addresses, and compounds of unknown words such as `frobnix-try`.
fn technical_spans(text: &str, dictionary: &dyn Dictionary) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        // Trim surrounding punctuation so "repo/name." and "(path)" are judged by their contents.
        let is_edge =
            |c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '(' | ')' | '"' | '\'' | '“' | '”' | '‘' | '’');
        let (mut s, mut e) = (start, i);
        while s < e && is_edge(chars[s]) {
            s += 1;
        }
        while e > s && is_edge(chars[e - 1]) {
            e -= 1;
        }
        let word: String = chars[s..e].iter().collect();
        if !word.is_empty() && looks_technical(&word, dictionary) {
            spans.push((s, e));
        }
    }
    spans
}

fn looks_technical(word: &str, dictionary: &dyn Dictionary) -> bool {
    let has_letter = word.chars().any(char::is_alphabetic);
    if word.starts_with('-') && has_letter {
        return true; // a command-line flag
    }
    if word.contains(['/', '\\', '_', '@', '#', '=', '`', '<', '>', '{', '}', '|', '~']) || word.contains("::") {
        return true;
    }
    // file.ext, example.com, v1.2 — a dot between letters or digits, not at the end.
    let chars: Vec<char> = word.chars().collect();
    if chars.windows(3).any(|w| w[1] == '.' && w[0].is_alphanumeric() && w[2].is_alphanumeric()) {
        return true;
    }
    // camelCase, or letters and digits mixed (utf8, h264), but not 4th, 1990s, or 10am.
    let digits = word.chars().take_while(char::is_ascii_digit).count();
    let number_suffix =
        digits > 0 && matches!(&word[digits..].to_ascii_lowercase()[..], "st" | "nd" | "rd" | "th" | "s" | "am" | "pm");
    if chars.windows(2).any(|w| w[0].is_lowercase() && w[1].is_uppercase())
        || (has_letter && word.chars().any(|c| c.is_ascii_digit()) && !number_suffix)
    {
        return true;
    }
    // Hyphenated compounds are prose ("well-known") unless a part isn't an English word.
    word.contains('-') && word.split('-').filter(|p| !p.is_empty()).any(|p| !dictionary.contains_word_str(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_common_mistakes() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let view = checker.check("This is an test of teh system. It works good.");
        let fixed = diff::apply(&view, &Default::default());
        assert!(fixed.contains("a test"), "{fixed}");
        assert!(fixed.contains("the system"), "{fixed}");
        assert!(view.segments.iter().any(|s| matches!(s, Segment::Change { note: Some(_), .. })));
    }

    #[test]
    fn leaves_commands_paths_and_identifiers_alone() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let text = "frobnix-try: No build found in frobnix-org/frobnix. Run cargo_test with --fresh.";
        let fixed = diff::apply(&checker.check(text), &Default::default());
        for token in ["frobnix-try", "frobnix-org/frobnix", "cargo_test", "--fresh"] {
            assert!(fixed.contains(token), "{token} was changed: {fixed}");
        }
        // Ordinary prose around them is still checked.
        let fixed = diff::apply(&checker.check("Open config.json and fix teh well-known bug."), &Default::default());
        assert!(fixed.contains("config.json") && fixed.contains("the well-known"), "{fixed}");
        // Nor is anything in a code span or fenced block.
        let text = "Run `npm i teh pakage` then:\n```\nteh pakage\n```\nfix teh docs.";
        let fixed = diff::apply(&checker.check(text), &Default::default());
        assert_eq!(fixed, "Run `npm i teh pakage` then:\n```\nteh pakage\n```\nfix the docs.");
        let dictionary = FstDictionary::curated();
        for prose in ["4th", "1990s", "10am", "well-known"] {
            assert!(!looks_technical(prose, dictionary.as_ref()), "{prose}");
        }
        for code in ["utf8", "camelCase", "README.md", "a/b", "--fresh"] {
            assert!(looks_technical(code, dictionary.as_ref()), "{code}");
        }
    }

    #[test]
    fn spelling_fixes_only_near_misses() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let fixed = diff::apply(
            &checker.check(
                "How much of this year's KPI is on track? Show it on a glanceable dashboard. It's realy teh best.",
            ),
            &Default::default(),
        );
        assert!(fixed.contains("year's KPI") && fixed.contains("glanceable dashboard"), "{fixed}");
        assert!(fixed.contains("really the best"), "near-miss typos are still fixed: {fixed}");
        assert_eq!(edit_distance(&['t', 'e', 'h'], &['t', 'h', 'e']), 1);
    }

    #[test]
    fn keeps_names_inside_sentences() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let fixed = |text: &str| diff::apply(&checker.check(text), &Default::default());
        assert_eq!(fixed("I am taking Rhyan to tennis lessons."), "I am taking Rhyan to tennis lessons.");
        assert_eq!(fixed("Notes for Kyndal, then call Rhyan."), "Notes for Kyndal, then call Rhyan.");
        // At the start of a sentence a capitalized word may be a typo, so Harper's fix stands.
        assert_eq!(fixed("Teh lesson is today."), "The lesson is today.");
        assert_eq!(fixed("Rhyan has tennis today."), "Ryan has tennis today.");
    }

    #[test]
    fn keeps_names_that_are_also_acronyms() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let fixed = |text: &str| diff::apply(&checker.check(text), &Default::default());
        // The dictionary lists "Sam" and "SAM", "Mia" and "MIA", "Dec" and "DEC", "Ida" and "IDA".
        assert_eq!(fixed("Hi Sam, thanks for the draft!"), "Hi Sam, thanks for the draft!");
        assert_eq!(fixed("Thanks, Mia! I took Sam to tennis."), "Thanks, Mia! I took Sam to tennis.");
        assert_eq!(fixed("Sam has tennis on Dec 3."), "Sam has tennis on Dec 3.");
        assert_eq!(fixed("Ida read Sam's draft."), "Ida read Sam's draft.");
        // Real capitalization fixes still apply.
        assert_eq!(fixed("i need it by Friday."), "I need it by Friday.");
        assert_eq!(fixed("Done. the lesson is today."), "Done. The lesson is today.");
        assert_eq!(fixed("She works at nasa."), "She works at NASA.");
    }

    #[test]
    fn learns_the_authors_words() {
        let checker = LocalChecker::new(Dialect::American, &["Rhyan".into(), "1234 Glenmoor Ct".into()]);
        let fixed = |text: &str| diff::apply(&checker.check(text), &Default::default());
        assert_eq!(fixed("Rhyan has tennis today."), "Rhyan has tennis today.");
        assert_eq!(fixed("Glenmoor is closed. Teh lesson moved."), "Glenmoor is closed. The lesson moved.");
    }

    #[test]
    fn finds_sentence_starts() {
        let chars = |s: &str| s.chars().collect::<Vec<char>>();
        let at = |s: &str, word: &str| starts_sentence(&chars(s), s[..s.find(word).unwrap()].chars().count());
        assert!(at("Rhyan went.", "Rhyan"));
        assert!(at("Done. “Rhyan went.”", "Rhyan"));
        assert!(at("Hi\nRhyan went.", "Rhyan"));
        assert!(at("- Rhyan", "Rhyan"));
        assert!(!at("I took Rhyan.", "Rhyan"));
        assert!(!at("Me, (Rhyan) and you.", "Rhyan"));
    }

    #[test]
    fn finds_the_authors_own_terms() {
        let terms = unfamiliar_terms("Show this year's KPI on a glanceable dashboard before the OKR review tomorow.");
        assert_eq!(terms, vec!["KPI", "OKR", "glanceable"]);
        assert_eq!(unknown_words("Rhyan's and Brynlee’s lessons, tomorow."), vec!["Brynlee", "Rhyan", "tomorow"]);
    }

    #[test]
    fn clean_text_is_unchanged() {
        let checker = LocalChecker::new(Dialect::American, &[]);
        let view = checker.check("The quick brown fox jumps over the lazy dog.");
        assert_eq!(view.change_count, 0);
    }
}
