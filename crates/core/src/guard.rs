//! Post-checks on a model's revision that need no model: tokens that must survive an edit, and
//! a readability grade so rewrites can show whether they actually got simpler.

use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

/// Spans a writing edit should never alter: links, e-mail addresses, @mentions, #channels,
/// inline code, template placeholders, and numbers.
pub fn protected_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Inline code spans first, so their contents aren't also split into words.
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        match after.find('`') {
            Some(end) => {
                out.push(format!("`{}`", &after[..end]));
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            matches!(c, '(' | ')' | ',' | '.' | ';' | ':' | '!' | '?' | '"' | '\'' | '“' | '”')
        });
        let is_url = trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with("www.");
        let is_email = trimmed.contains('@') && trimmed.contains('.') && !trimmed.starts_with('@');
        let is_mention = (trimmed.starts_with('@') || trimmed.starts_with('#')) && trimmed.len() > 1;
        let is_placeholder = (trimmed.starts_with("{{") && trimmed.ends_with("}}"))
            || (trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() > 2);
        let is_number = trimmed.chars().any(|c| c.is_ascii_digit())
            && trimmed.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%' | '$' | '-' | '/' | ':'));
        if is_url || is_email || is_mention || is_placeholder || is_number {
            out.push(trimmed.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Protected tokens from `original` that no longer appear in `revised`.
pub fn missing_tokens(original: &str, revised: &str) -> Vec<String> {
    protected_tokens(original).into_iter().filter(|t| !revised.contains(t.as_str())).collect()
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Readability {
    /// US school grade from the Automated Readability Index.
    pub grade: i32,
    pub words: usize,
    pub sentences: usize,
}

/// Automated Readability Index: 4.71 × letters/words + 0.5 × words/sentences − 21.43.
/// Returns `None` below 30 words, where the score is too unstable to be useful.
pub fn readability(text: &str) -> Option<Readability> {
    let words: Vec<&str> = text.unicode_words().collect();
    if words.len() < 30 {
        return None;
    }
    let letters: usize = words.iter().map(|w| w.chars().filter(|c| c.is_alphanumeric()).count()).sum();
    let sentences = text.unicode_sentences().filter(|s| s.chars().any(char::is_alphanumeric)).count().max(1);
    let score = 4.71 * letters as f64 / words.len() as f64 + 0.5 * words.len() as f64 / sentences as f64 - 21.43;
    Some(Readability { grade: score.round().max(0.0) as i32, words: words.len(), sentences })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tokens_that_must_survive() {
        let text = "Ping @dana in #launch about https://example.com/a?b=1, the `cargo test` run, {{name}}, and $4.50 by 10/21.";
        let tokens = protected_tokens(text);
        for expected in ["@dana", "#launch", "https://example.com/a?b=1", "`cargo test`", "{{name}}", "$4.50", "10/21"]
        {
            assert!(tokens.contains(&expected.to_string()), "{expected} missing from {tokens:?}");
        }
        assert!(!tokens.iter().any(|t| t == "Ping"));
    }

    #[test]
    fn reports_changed_tokens() {
        let original = "See https://example.com and ask @sam.";
        assert!(missing_tokens(original, "See https://example.com and ask @sam!").is_empty());
        assert_eq!(missing_tokens(original, "See the site and ask Sam."), vec!["@sam", "https://example.com"]);
    }

    #[test]
    fn readability_needs_enough_words() {
        assert!(readability("Too short to score.").is_none());
        let simple = "The cat sat on the mat. The dog ran to the park. We had fun in the sun. It was a good day for all of us. Then we went home to eat.";
        let dense = "Organizational transformation initiatives necessitate comprehensive stakeholder engagement methodologies, \
                     facilitating multidimensional collaboration frameworks across geographically distributed operational \
                     infrastructures, notwithstanding considerable implementation complexities and institutional resistance.";
        let easy = readability(simple).unwrap();
        let hard = readability(&format!("{dense} {dense}")).unwrap();
        assert!(easy.grade < 5, "{easy:?}");
        assert!(hard.grade > 14, "{hard:?}");
    }
}
