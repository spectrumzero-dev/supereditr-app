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

/// The most edits applied in place. More than that replaces the whole selection.
pub const MAX_EDITS: usize = 8;
/// The longest text, in characters, an edit may replace or write.
const MAX_EDIT_LENGTH: usize = 40;

/// One change applied in place: `old`, at `start` in the original, becomes `new`. An insertion has
/// an empty `old` and an [`Anchor`] to find and confirm its place by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Where `old` starts in the original (where `new` goes, for an insertion), in characters.
    pub start: usize,
    /// The same, in UTF-16 code units, as Accessibility counts.
    pub start_utf16: usize,
    pub old: String,
    pub new: String,
    pub anchor: Option<Anchor>,
}

/// The visible text next to an insertion, read back before it is written. Code counts: reading it
/// back leaves it untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub text: String,
    /// Where `text` starts in the original, in characters.
    pub start: usize,
    /// `text` comes after the insertion rather than before it.
    pub after: bool,
    /// Invisible characters (an editor's padding around code) separate `text` from the insertion,
    /// so the insertion goes on the far side of the field's padding.
    pub padded: bool,
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
    /// A change to text the app formats as code.
    Code,
}

impl fmt::Display for Whole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rewrite => f.write_str("a rewrite, not a proofread"),
            Self::TooMany(n) => write!(f, "{n} changes, more than {MAX_EDITS}"),
            Self::NotLocal => f.write_str("a change that isn't local"),
            Self::Code => f.write_str("a change to code"),
        }
    }
}

/// The edits that turn `original` into `revised`, last first, so applying them in order leaves
/// the positions of those still to come unchanged. Each edit is exactly a change the revision
/// made: nothing around it is rewritten. `proofread` is whether the revision is a plain proofread;
/// `code` holds character ranges of `original` the app formats as code.
pub fn plan(original: &str, revised: &str, proofread: bool, code: &[(usize, usize)]) -> Result<Vec<Edit>, Whole> {
    if !proofread {
        return Err(Whole::Rewrite);
    }
    let chars: Vec<char> = original.chars().collect();
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
    let local = |text: &str| {
        text.chars().count() <= MAX_EDIT_LENGTH
            && !text.chars().any(|c| c == '\n' || c == '\r' || crate::diff::is_invisible(c))
    };
    let mut edits = Vec::with_capacity(changes.len());
    for (start, old, new) in changes {
        let end = start + old.chars().count();
        if code.iter().any(|&(s, e)| start < e && s < end) {
            return Err(Whole::Code);
        }
        if !local(&old) || !local(&new) {
            return Err(Whole::NotLocal);
        }
        let anchor = if old.is_empty() { Some(anchor(&chars, start).ok_or(Whole::NotLocal)?) } else { None };
        let start_utf16 = chars[..start].iter().map(|c| c.len_utf16()).sum();
        edits.push(Edit { start, start_utf16, old, new, anchor });
    }
    edits.reverse();
    Ok(edits)
}

/// The visible word, space or mark beside an insertion at `at`: the one before it, past any
/// invisible padding, or at the start the one after it. None when there is none on its line.
fn anchor(chars: &[char], at: usize) -> Option<Anchor> {
    let skip = |c: char| crate::diff::is_invisible(c);
    let text = |s: usize, e: usize| chars[s..e].iter().collect::<String>();
    // Before: back over padding, then back to the start of that word or run of spaces/marks.
    let mut end = at;
    while end > 0 && skip(chars[end - 1]) {
        end -= 1;
    }
    if end > 0 && chars[end - 1] != '\n' {
        let kind = |c: char| c.is_alphanumeric();
        let mut start = end - 1;
        while start > 0
            && !skip(chars[start - 1])
            && chars[start - 1] != '\n'
            && kind(chars[start - 1]) == kind(chars[end - 1])
        {
            start -= 1;
        }
        return Some(Anchor { text: text(start, end), start, after: false, padded: end < at });
    }
    // After: forward over padding, then to the end of that word or run.
    let mut start = at;
    while start < chars.len() && skip(chars[start]) {
        start += 1;
    }
    if start < chars.len() && chars[start] != '\n' {
        let kind = |c: char| c.is_alphanumeric();
        let mut end = start + 1;
        while end < chars.len() && !skip(chars[end]) && chars[end] != '\n' && kind(chars[end]) == kind(chars[start]) {
            end += 1;
        }
        return Some(Anchor { text: text(start, end), start, after: true, padded: start > at });
    }
    None
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
            if let Some(anchor) = &edit.anchor {
                let len = anchor.text.chars().count();
                assert_eq!(chars[anchor.start..anchor.start + len].iter().collect::<String>(), anchor.text);
            }
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
        assert!(edits.iter().all(|e| e.anchor.is_none()));
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn an_insertion_is_anchored_on_the_text_before_it() {
        // "notes" may be a link: it is only read back, never rewritten.
        let original = "Read the notes and go.";
        let revised = "Read the notes, and go.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert_eq!(pairs(&edits), vec![(14, "", ",")]);
        let anchor = edits[0].anchor.as_ref().unwrap();
        assert_eq!((anchor.text.as_str(), anchor.start, anchor.after, anchor.padded), ("notes", 9, false, false));
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn an_insertion_at_the_start_is_anchored_on_the_text_after_it() {
        let original = "going now.";
        let revised = "I'm going now.";
        let edits = plan(original, revised, true, &[]).unwrap();
        assert!(edits.iter().all(|e| e.old.is_empty() == e.anchor.is_some()));
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn an_insertion_after_padded_code_goes_past_the_padding() {
        // Claude's composer pads a code chip with zero-width spaces; the full stop goes after them.
        let original = "this is \u{200B}code\u{200B}";
        let code = [(9, 13)];
        let revised = "This is \u{200B}code\u{200B}.";
        let edits = plan(original, revised, true, &code).unwrap();
        assert_eq!(pairs(&edits), vec![(14, "", "."), (0, "this", "This")]);
        let anchor = edits[0].anchor.as_ref().unwrap();
        assert_eq!((anchor.text.as_str(), anchor.start, anchor.after, anchor.padded), ("code", 9, false, true));
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
        assert_eq!(edits[0].start_utf16, "Café 👩‍💻 ".encode_utf16().count());
        assert_eq!(apply(original, &edits), revised);
    }

    #[test]
    fn a_change_next_to_code_is_applied_in_place() {
        let original = "realy make and run";
        let code = [(6, 10)];
        let edits = plan(original, "really make and run", true, &code).unwrap();
        assert_eq!(pairs(&edits), vec![(0, "realy", "really")]);
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
        assert_eq!(plan(original, &original.replace("teh", "the"), true, &[]), Err(Whole::TooMany(9)));
        let eight = "teh a teh b teh c teh d teh e teh f teh g teh h";
        assert_eq!(plan(eight, &eight.replace("teh", "the"), true, &[]).map(|e| e.len()), Ok(MAX_EDITS));
    }

    #[test]
    fn changes_across_lines_or_long_ones_replace_the_whole_selection() {
        assert_eq!(plan("One\nTwo", "One Two", true, &[]), Err(Whole::NotLocal));
        let reworded = "See you at the long quarterly planning session we hold on Monday.";
        assert_eq!(plan("See you Monday.", reworded, true, &[]), Err(Whole::NotLocal));
    }

    #[test]
    fn invisible_changes_replace_the_whole_selection() {
        assert_eq!(plan("a\u{200B}b c", "a b c", true, &[]), Err(Whole::NotLocal));
    }

    #[test]
    fn no_change_is_no_edits() {
        assert_eq!(plan("Fine as is.", "Fine as is.", true, &[]), Ok(vec![]));
    }
}
