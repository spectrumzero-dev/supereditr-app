//! Applying a proofread by editing only what it changed.
//!
//! Replacing a whole selection loses formatting an app can't paste back (a code chip in a
//! composer that ignores pasted HTML, for example). A proofread changes a few words, so it can be
//! applied as a few small edits instead, each replacing only the words it changed and leaving the
//! formatted text around them untouched. That is only safe for a few local changes: each edit is a
//! separate write the app has to accept, with its own undo step. Anything else, a rewrite
//! included, replaces the whole selection.

use crate::diff::{Segment, diff};
use std::fmt;
use unicode_segmentation::UnicodeSegmentation;

/// The most edits applied in place. More than that replaces the whole selection.
pub const MAX_EDITS: usize = 8;
/// The longest text, in characters, an edit may replace or write.
const MAX_EDIT_LENGTH: usize = 40;

/// One change applied in place: `old`, at `start` in the original, becomes `new`. `old` is never
/// empty, so the text to replace can always be selected and read back before it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Where `old` starts in the original, in characters.
    pub start: usize,
    /// Where `old` starts in the original, in UTF-16 code units, as Accessibility counts.
    pub start_utf16: usize,
    pub old: String,
    pub new: String,
}

/// Why a revision replaces the whole selection rather than being applied in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whole {
    /// Not a plain proofread: a style such as Shorter, or an instruction.
    Rewrite,
    /// More changes than [`MAX_EDITS`].
    TooMany(usize),
    /// A change that crosses a line break, holds invisible characters or is longer than a few words.
    NotLocal,
    /// A change inside text the app formats as code, or an insertion with code on both sides.
    Code,
}

impl fmt::Display for Whole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rewrite => f.write_str("a rewrite, not a proofread"),
            Self::TooMany(n) => write!(f, "{n} changes, more than {MAX_EDITS}"),
            Self::NotLocal => f.write_str("a change that isn't local"),
            Self::Code => f.write_str("a change touching code"),
        }
    }
}

/// The edits that turn `original` into `revised`, last first, so applying them in order leaves
/// the positions of those still to come unchanged. `proofread` is whether the revision is a plain
/// proofread; `code` holds character ranges of `original` the app formats as code. An insertion
/// takes in the word before it (or after it, at the start or next to code), so every edit replaces
/// text that can be read back first.
pub fn plan(original: &str, revised: &str, proofread: bool, code: &[(usize, usize)]) -> Result<Vec<Edit>, Whole> {
    if !proofread {
        return Err(Whole::Rewrite);
    }
    let chars: Vec<char> = original.chars().collect();
    let slice = |s: usize, e: usize| chars[s..e].iter().collect::<String>();
    // Character offsets where each word, space or punctuation mark of the original starts or ends.
    let mut bounds = vec![0];
    for token in original.split_word_bounds() {
        bounds.push(bounds.last().unwrap() + token.chars().count());
    }
    let in_code = |s: usize, e: usize| code.iter().any(|&(cs, ce)| s < ce && cs < e);

    let changes: Vec<(usize, String, String)> = diff(original, revised)
        .segments
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Change { old_start, old, new, .. } => Some((old_start, old, new)),
            Segment::Equal { .. } => None,
        })
        .collect();
    if changes.len() > MAX_EDITS {
        return Err(Whole::TooMany(changes.len()));
    }
    let mut edits = Vec::with_capacity(changes.len());
    for (start, old, new) in changes {
        let (start, old, new) = if old.is_empty() {
            // The token before the insertion, or the one after it.
            let before = bounds.iter().rev().find(|&&b| b < start).map(|&b| (b, start));
            let after = bounds.iter().find(|&&b| b > start).map(|&b| (start, b));
            match [before, after].into_iter().flatten().find(|&(s, e)| !in_code(s, e)) {
                Some((s, e)) if s < start => (s, slice(s, e), format!("{}{new}", slice(s, e))),
                Some((s, e)) => (s, slice(s, e), format!("{new}{}", slice(s, e))),
                None => return Err(Whole::Code),
            }
        } else {
            (start, old, new)
        };
        let end = start + old.chars().count();
        if in_code(start, end) {
            return Err(Whole::Code);
        }
        let local = |text: &str| {
            text.chars().count() <= MAX_EDIT_LENGTH
                && !text.chars().any(|c| c == '\n' || c == '\r' || crate::diff::is_invisible(c))
        };
        if !local(&old) || !local(&new) {
            return Err(Whole::NotLocal);
        }
        let start_utf16 = chars[..start].iter().map(|c| c.len_utf16()).sum();
        edits.push(Edit { start, start_utf16, old, new });
    }
    edits.reverse();
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(edits: &[Edit]) -> Vec<(usize, &str, &str)> {
        edits.iter().map(|e| (e.start, e.old.as_str(), e.new.as_str())).collect()
    }

    /// Applies the edits in the order given, as the platform does, by character offsets.
    fn apply(original: &str, edits: &[Edit]) -> String {
        let mut chars: Vec<char> = original.chars().collect();
        for edit in edits {
            let end = edit.start + edit.old.chars().count();
            assert_eq!(chars[edit.start..end].iter().collect::<String>(), edit.old);
            chars.splice(edit.start..end, edit.new.chars());
        }
        chars.into_iter().collect()
    }

    #[test]
    fn replacements_come_last_first() {
        let original = "Their going to love this, it is realy fast.";
        let revised = "They're going to love this, it is really fast.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert_eq!(pairs(&edits), vec![(32, "realy", "really"), (0, "Their", "They're")]);
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn an_insertion_takes_in_the_word_before_it() {
        let original = "Thanks for the notes see you soon.";
        let revised = "Thanks for the notes; see you soon.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert_eq!(pairs(&edits), vec![(15, "notes", "notes;")]);
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn an_insertion_at_the_start_takes_in_the_word_after_it() {
        let original = "going now.";
        let revised = "I'm going now.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert!(edits.iter().all(|e| !e.old.is_empty()));
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn deletions_replace_text_with_nothing() {
        let original = "It is is ready now.";
        let revised = "It is ready now.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert!(edits.iter().all(|e| !e.old.is_empty()));
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn offsets_count_utf16_units_for_emoji_and_accents() {
        let original = "Café 👩‍💻 teh plan.";
        let revised = "Café 👩‍💻 the plan.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert_eq!(pairs(&edits), vec![(9, "teh", "the")]);
        let utf16: usize = "Café 👩‍💻 ".encode_utf16().count();
        assert_eq!(edits[0].start_utf16, utf16);
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn a_change_next_to_code_is_applied_in_place() {
        // "run" and "realy" sit right against code at each end.
        let original = "realy `teh fix` and run";
        let code = [(6, 15)];
        let revised = "really `teh fix` and run";
        let edits = plan(original, revised, true, &code).unwrap();
        assert_eq!(pairs(&edits), vec![(0, "realy", "really")]);
    }

    #[test]
    fn an_insertion_after_code_takes_in_the_word_after_it() {
        let original = "Run `make` now please.";
        let code = [(4, 10)];
        let revised = "Run `make`, now please.";
        let edits = plan(original, revised, true, &code).unwrap();
        assert_eq!(pairs(&edits), vec![(10, " ", ", ")]);
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn a_change_inside_code_replaces_the_whole_selection() {
        let original = "Run teh fix now.";
        let code = [(4, 11)];
        assert_eq!(plan(original, "Run the fix now.", true, &code), Err(Whole::Code));
    }

    #[test]
    fn rewrites_replace_the_whole_selection() {
        assert_eq!(plan("Their going.", "They're going.", false, &[]), Err(Whole::Rewrite));
    }

    #[test]
    fn more_than_the_cap_replaces_the_whole_selection() {
        let original = "teh a teh b teh c teh d teh e teh f teh g teh h teh i";
        let revised = original.replace("teh", "the");
        assert_eq!(plan(original, &revised, true, &[]), Err(Whole::TooMany(9)));
        let eight = "teh a teh b teh c teh d teh e teh f teh g teh h";
        assert_eq!(plan(eight, &eight.replace("teh", "the"), true, &[]).map(|e| e.len()), Ok(MAX_EDITS));
    }

    #[test]
    fn changes_across_lines_or_long_ones_replace_the_whole_selection() {
        assert_eq!(plan("One\nTwo", "One Two", true, &[]), Err(Whole::NotLocal));
        let long = "See you Monday.";
        let reworded = "See you at the long quarterly planning session we hold on Monday.";
        assert_eq!(plan(long, reworded, true, &[]), Err(Whole::NotLocal));
    }

    #[test]
    fn invisible_padding_replaces_the_whole_selection() {
        assert_eq!(plan("a\u{200B}b c", "a b c", true, &[]), Err(Whole::NotLocal));
    }

    #[test]
    fn no_change_is_no_edits() {
        assert_eq!(plan("Fine as is.", "Fine as is.", true, &[]), Ok(vec![]));
    }
}
