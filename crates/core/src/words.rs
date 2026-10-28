//! The author's word list: names, places, and terms SuperEditr keeps exactly as written.
//!
//! An entry is a word ("Rhyan") or a phrase ("1234 Glenmoor Ct"). Harper's dictionary learns each
//! word of it, the AI is told about the entries a selection contains, and a revision that drops one
//! starts out rejected.

/// The entries that appear in `text`, as the author spelled them. Case is ignored when searching,
/// so a sentence-start "Rhyan" or a lowercase "rhyan" both find "Rhyan".
pub fn found_in(text: &str, words: &[String]) -> Vec<String> {
    let text = folded(text);
    let mut found: Vec<String> = Vec::new();
    for word in words.iter().map(|w| w.trim()) {
        if find(&text, &folded(word)) && !found.iter().any(|f| f == word) {
            found.push(word.to_string());
        }
    }
    found
}

/// Whether `term` appears in `text` as whole words, with the same capitalization.
pub fn contains(text: &str, term: &str) -> bool {
    !spans(text, term).is_empty()
}

/// Character ranges where `term` appears in `text` as whole words, with the same capitalization.
pub fn spans(text: &str, term: &str) -> Vec<(usize, usize)> {
    let (text, term) = (text.chars().collect::<Vec<char>>(), term.chars().collect::<Vec<char>>());
    starts(&text, &term).map(|i| (i, i + term.len())).collect()
}

/// Character ranges where `term` appears in `text` as whole words, in any capitalization.
pub fn occurrences(text: &str, term: &str) -> Vec<(usize, usize)> {
    let (text, term) = (folded(text), folded(term));
    starts(&text, &term).map(|i| (i, i + term.len())).collect()
}

/// The single words in the list, for the spelling dictionary: "1234 Glenmoor Ct" gives "Glenmoor"
/// and "Ct".
pub fn dictionary_words(words: &[String]) -> impl Iterator<Item = &str> {
    words
        .iter()
        .flat_map(|w| w.split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '’')))
        .map(|w| w.trim_matches(['\'', '’']))
        .filter(|w| !w.is_empty())
}

/// Adds an entry unless the list already has it, ignoring case. Returns whether it was added.
pub fn add(words: &mut Vec<String>, entry: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() || words.iter().any(|w| folded(w.trim()) == folded(entry)) {
        return false;
    }
    words.push(entry.to_string());
    true
}

/// Lowercased one character at a time, so positions line up with the original.
fn folded(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn find(text: &[char], term: &[char]) -> bool {
    starts(text, term).next().is_some()
}

/// Where `term` starts in `text`, matching whole words only.
fn starts<'a>(text: &'a [char], term: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    let n = term.len();
    (0..(text.len() + 1).saturating_sub(n)).filter(move |&i| {
        n > 0
            && text[i..i + n] == *term
            && (i == 0 || !text[i - 1].is_alphanumeric())
            && text.get(i + n).is_none_or(|c| !c.is_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn finds_whole_words_and_phrases_ignoring_case() {
        let words = list(&["Rhyan", "1234 Glenmoor Ct", "Ty", "QuillPoint"]);
        assert_eq!(found_in("rhyan's lesson is at 1234 glenmoor ct.", &words), list(&["Rhyan", "1234 Glenmoor Ct"]));
        assert!(found_in("Type it at QuillPoints.", &words).is_empty(), "only whole words count");
    }

    #[test]
    fn contains_keeps_capitalization() {
        assert!(contains("I am taking Rhyan to tennis.", "Rhyan"));
        assert!(!contains("I am taking rhyan to tennis.", "Rhyan"));
        assert!(contains("Meet at 1234 Glenmoor Ct.", "1234 Glenmoor Ct"));
        assert!(!contains("Meet at 1234 Glenmoor Court.", "1234 Glenmoor Ct"));
        assert_eq!(spans("Rhyan and Rhyan", "Rhyan"), vec![(0, 5), (10, 15)]);
        assert_eq!(occurrences("rhyan and Rhyan", "Rhyan"), vec![(0, 5), (10, 15)]);
        assert!(spans("", "Rhyan").is_empty() && spans("Rhyan", "").is_empty());
    }

    #[test]
    fn dictionary_gets_each_word() {
        let words = list(&["1234 Glenmoor Ct", "O'Neill"]);
        assert_eq!(dictionary_words(&words).collect::<Vec<_>>(), vec!["Glenmoor", "Ct", "O'Neill"]);
    }

    #[test]
    fn add_skips_duplicates() {
        let mut words = list(&["Rhyan"]);
        assert!(!add(&mut words, "rhyan"));
        assert!(!add(&mut words, "  "));
        assert!(add(&mut words, "Glenmoor"));
        assert_eq!(words, list(&["Rhyan", "Glenmoor"]));
    }
}
