// Prints Harper's raw lints for a sentence: `cargo run -p supereditr-core --example lints -- "text"`.
use harper_core::linting::{LintGroup, Linter};
use harper_core::spell::{Dictionary, FstDictionary};
use harper_core::{Dialect, Document};

fn main() {
    let text = std::env::args().nth(1).unwrap_or_default();
    let dictionary = FstDictionary::curated();
    let mut linter = LintGroup::new_curated(dictionary.clone(), Dialect::American);
    let chars: Vec<char> = text.chars().collect();
    for lint in linter.lint(&Document::new_plain_english_curated(&text)) {
        let word: String = chars[lint.span.start..lint.span.end].iter().collect();
        println!(
            "{:?} {:?} known={} -> {:?}",
            lint.lint_kind,
            word,
            dictionary.contains_word_str(&word),
            lint.suggestions.first()
        );
    }
}
