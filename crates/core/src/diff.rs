//! Word-level diffing between the user's text and a revision.
//!
//! Models return only the revised text; every individual change shown in the overlay is
//! derived here. That keeps responses small and streamable, works with any model, and lets the
//! user accept or reject changes one at a time.

use serde::Serialize;
use similar::{Algorithm, DiffOp, capture_diff_slices};
use std::collections::HashSet;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Spelling,
    Grammar,
    Punctuation,
    Capitalization,
    Spacing,
    Style,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Segment {
    Equal {
        text: String,
    },
    Change {
        id: usize,
        old: String,
        new: String,
        kind: ChangeKind,
        /// Character offset of `old` within the original text.
        #[serde(rename = "oldStart")]
        old_start: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// Made by the offline checker's final pass over an AI's revision, not by the AI.
        #[serde(rename = "ruleBased", skip_serializing_if = "std::ops::Not::not")]
        rule_based: bool,
    },
}

/// A renderable comparison. `pending` holds original text the model has not reached yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiffView {
    pub segments: Vec<Segment>,
    pub pending: String,
    #[serde(rename = "changeCount")]
    pub change_count: usize,
}

fn tokens(text: &str) -> Vec<&str> {
    text.split_word_bounds().collect()
}

/// Full comparison of `original` against a finished `revised` text.
pub fn diff(original: &str, revised: &str) -> DiffView {
    let old = tokens(original);
    let new = tokens(revised);
    let ops = capture_diff_slices(Algorithm::Patience, &old, &new);
    let segments = build_segments(&old, &new, &ops);
    finish(segments, String::new())
}

/// Comparison of `original` against a revision that is still streaming in.
///
/// Everything up to the last run of matching text is stable. Original text after it is
/// reported as `pending` instead of as deletions, so the overlay never flashes false removals.
pub fn stream_view(original: &str, partial: &str) -> DiffView {
    let old = tokens(original);
    let new = tokens(partial);
    let ops = capture_diff_slices(Algorithm::Patience, &old, &new);
    let Some(last_equal) = ops.iter().rposition(|op| matches!(op, DiffOp::Equal { .. })) else {
        return DiffView { segments: vec![], pending: original.to_string(), change_count: 0 };
    };
    let stable = &ops[..=last_equal];
    let consumed = stable.last().map(|op| op.old_range().end).unwrap_or(0);
    let pending: String = old[consumed..].concat();
    finish(build_segments(&old, &new, stable), pending)
}

fn finish(segments: Vec<Segment>, pending: String) -> DiffView {
    let change_count = segments.iter().filter(|s| matches!(s, Segment::Change { .. })).count();
    DiffView { segments, pending, change_count }
}

fn build_segments(old: &[&str], new: &[&str], ops: &[DiffOp]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut pending_old = String::new();
    let mut pending_new = String::new();
    let mut pending_start = 0usize;
    let mut char_pos = 0usize;
    let mut id = 0usize;

    let mut flush = |segments: &mut Vec<Segment>, old_text: &mut String, new_text: &mut String, start: usize| {
        if old_text.is_empty() && new_text.is_empty() {
            return;
        }
        let kind = classify(old_text, new_text);
        segments.push(Segment::Change {
            id,
            old: std::mem::take(old_text),
            new: std::mem::take(new_text),
            kind,
            old_start: start,
            note: None,
            rule_based: false,
        });
        id += 1;
    };

    for op in ops {
        match *op {
            DiffOp::Equal { old_index, len, .. } => {
                flush(&mut segments, &mut pending_old, &mut pending_new, pending_start);
                let text: String = old[old_index..old_index + len].concat();
                char_pos += text.chars().count();
                match segments.last_mut() {
                    Some(Segment::Equal { text: prev }) => prev.push_str(&text),
                    _ => segments.push(Segment::Equal { text }),
                }
            }
            _ => {
                if pending_old.is_empty() && pending_new.is_empty() {
                    pending_start = char_pos;
                }
                let removed: String = old[op.old_range()].concat();
                char_pos += removed.chars().count();
                pending_old.push_str(&removed);
                pending_new.push_str(&new[op.new_range()].concat());
            }
        }
    }
    flush(&mut segments, &mut pending_old, &mut pending_new, pending_start);
    segments
}

/// Rebuild text from a view, keeping the original wording for rejected change ids.
pub fn apply(view: &DiffView, rejected: &HashSet<usize>) -> String {
    let mut out = String::new();
    for segment in &view.segments {
        match segment {
            Segment::Equal { text } => out.push_str(text),
            Segment::Change { id, old, new, .. } => out.push_str(if rejected.contains(id) { old } else { new }),
        }
    }
    out.push_str(&view.pending);
    out
}

/// `view` with every change that touches one of `spans` (character ranges of the original) turned
/// back into original text, and the remaining changes numbered again. An insertion touches a span
/// when it falls inside it, not at its edge. A change at a span's edge that only adds or removes
/// spaces or invisible characters is turned back too: editors pad formatted code with them, and a
/// change there shows as an empty box.
pub fn without_changes_in(view: DiffView, spans: &[(usize, usize)]) -> DiffView {
    if spans.is_empty() {
        return view;
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut next_id = 0;
    for segment in view.segments {
        let segment = match segment {
            Segment::Change { old, new, old_start, .. }
                if touches(old_start, old.chars().count(), spans)
                    || (blank(&old) && blank(&new) && borders(old_start, old.chars().count(), spans)) =>
            {
                Segment::Equal { text: old }
            }
            Segment::Change { id: _, old, new, kind, old_start, note, rule_based } => {
                next_id += 1;
                Segment::Change { id: next_id - 1, old, new, kind, old_start, note, rule_based }
            }
            equal => equal,
        };
        match (&segment, segments.last_mut()) {
            (Segment::Equal { text }, Some(Segment::Equal { text: prev })) => prev.push_str(text),
            (Segment::Equal { text }, _) if text.is_empty() => {}
            _ => segments.push(segment),
        }
    }
    finish(segments, view.pending)
}

/// Marks each change in `view` that holds one of `pass`'s changes as rule-based, with that change's
/// note. `pass` revises `view`'s revised text, as the offline checker does an AI's; the two are
/// matched by where their new text sits in the final text.
pub fn mark_rule_based(view: &mut DiffView, pass: &DiffView) {
    let fixes: Vec<(usize, usize, Option<String>)> = placed(&pass.segments)
        .filter_map(|(start, end, segment)| match segment {
            Segment::Change { note, .. } => Some((start, end, note.clone())),
            Segment::Equal { .. } => None,
        })
        .collect();
    let ranges: Vec<(usize, usize)> = placed(&view.segments).map(|(start, end, _)| (start, end)).collect();
    for (segment, (start, end)) in view.segments.iter_mut().zip(ranges) {
        let Segment::Change { note, rule_based, .. } = segment else { continue };
        if let Some((_, _, fix_note)) = fixes.iter().find(|&&(s, e, _)| meet((start, end), (s, e))) {
            *rule_based = true;
            *note = fix_note.clone();
        }
    }
}

/// Whether two ranges share text. An empty range, where a removal was, meets a range it touches.
fn meet((a, b): (usize, usize), (c, d): (usize, usize)) -> bool {
    if a == b || c == d { a <= d && c <= b } else { a < d && c < b }
}

/// Each segment with the character range its text takes up in the revised text.
fn placed(segments: &[Segment]) -> impl Iterator<Item = (usize, usize, &Segment)> {
    segments.iter().scan(0, |pos, segment| {
        let start = *pos;
        *pos += match segment {
            Segment::Equal { text } => text.chars().count(),
            Segment::Change { new, .. } => new.chars().count(),
        };
        Some((start, *pos, segment))
    })
}

fn touches(start: usize, len: usize, spans: &[(usize, usize)]) -> bool {
    let end = start + len;
    spans.iter().any(|&(s, e)| if len == 0 { s < start && start < e } else { start < e && s < end })
}

fn borders(start: usize, len: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(s, e)| start <= e && s <= start + len)
}

/// Only spaces, line breaks and invisible characters: zero-width spaces and joiners, word joiners,
/// byte order marks and object replacement characters.
fn blank(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || is_invisible(c))
}

/// Zero-width spaces and joiners, word joiners, byte order marks and object replacement
/// characters: editors put them around formatted text.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{FFFC}')
}

/// Where each of `ranges` (character ranges of `original` that a revision left as they were) is in
/// `revised`. A range the revision changed is left out.
pub fn carry_ranges(original: &str, revised: &str, ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // Each unchanged stretch: where it starts in the original, where in the revision, and its length.
    let mut stretches = Vec::new();
    let (mut old_pos, mut new_pos) = (0, 0);
    for segment in diff(original, revised).segments {
        match segment {
            Segment::Equal { text } => {
                let length = text.chars().count();
                stretches.push((old_pos, new_pos, length));
                old_pos += length;
                new_pos += length;
            }
            Segment::Change { old, new, .. } => {
                old_pos += old.chars().count();
                new_pos += new.chars().count();
            }
        }
    }
    ranges
        .iter()
        .filter_map(|&(s, e)| {
            let &(old_start, new_start, _) =
                stretches.iter().find(|&&(start, _, length)| start <= s && e <= start + length)?;
            Some((new_start + s - old_start, new_start + e - old_start))
        })
        .collect()
}

fn classify(old: &str, new: &str) -> ChangeKind {
    let strip_ws = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if strip_ws(old) == strip_ws(new) {
        return ChangeKind::Spacing;
    }
    if old.to_lowercase() == new.to_lowercase() {
        return ChangeKind::Capitalization;
    }
    let alnum = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    if alnum(old) == alnum(new) {
        return ChangeKind::Punctuation;
    }
    let old_words: Vec<&str> = old.unicode_words().collect();
    let new_words: Vec<&str> = new.unicode_words().collect();
    if old_words.len() == 1 && new_words.len() == 1 {
        let (a, b) = (old_words[0].to_lowercase(), new_words[0].to_lowercase());
        let distance = levenshtein(&a, &b);
        if distance <= 2 && a.chars().count() >= 3 {
            return ChangeKind::Spelling;
        }
    }
    if old_words.len().max(new_words.len()) <= 3 {
        return ChangeKind::Grammar;
    }
    ChangeKind::Style
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b_chars.len() + 1];
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            current[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(current[j] + 1);
        }
        prev = current;
    }
    prev[b_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(view: &DiffView) -> Vec<(String, String, ChangeKind)> {
        view.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Change { old, new, kind, .. } => Some((old.clone(), new.clone(), *kind)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn granular_changes_with_categories() {
        let view = diff("This are teh draft , ok", "This is the draft, OK");
        let c = changes(&view);
        assert_eq!(c[0], ("are".into(), "is".into(), ChangeKind::Grammar));
        assert_eq!(c[1], ("teh".into(), "the".into(), ChangeKind::Spelling));
        assert_eq!(c[2].2, ChangeKind::Spacing);
        assert_eq!(c[3], ("ok".into(), "OK".into(), ChangeKind::Capitalization));
    }

    #[test]
    fn identical_text_has_no_changes() {
        let view = diff("Nothing to fix here.", "Nothing to fix here.");
        assert_eq!(view.change_count, 0);
        assert_eq!(apply(&view, &HashSet::new()), "Nothing to fix here.");
    }

    #[test]
    fn apply_respects_rejections() {
        let view = diff("This are teh draft.", "This is the draft.");
        assert_eq!(apply(&view, &HashSet::new()), "This is the draft.");
        assert_eq!(apply(&view, &HashSet::from([0])), "This are the draft.");
        assert_eq!(apply(&view, &HashSet::from([0, 1])), "This are teh draft.");
    }

    #[test]
    fn changes_inside_spans_are_left_out() {
        let original = "should i run `npm i` now";
        let view = diff(original, "Should I run `npm install` now.");
        // `npm i` covers characters 13..20.
        let kept = without_changes_in(view, &[(13, 20)]);
        assert_eq!(apply(&kept, &HashSet::new()), "Should I run `npm i` now.");
        let ids: Vec<usize> = kept
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Change { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(kept.change_count, 3);
        // An insertion at a span's edge is kept; one inside it is not.
        let view = diff("run `x` now", "run `x`, now");
        assert_eq!(apply(&without_changes_in(view, &[(4, 7)]), &HashSet::new()), "run `x`, now");
        let view = diff("run `a b` now", "run `a, b` now");
        assert_eq!(apply(&without_changes_in(view, &[(4, 9)]), &HashSet::new()), "run `a b` now");
    }

    #[test]
    fn invisible_changes_beside_code_are_left_out() {
        // A zero-width space after the code, which the model dropped.
        let view = diff("does this code\u{200B} block work nw", "Does this code block work now?");
        let kept = without_changes_in(view, &[(10, 14)]);
        assert_eq!(apply(&kept, &HashSet::new()), "Does this code\u{200B} block work now?");
        assert_eq!(kept.change_count, 2);
        // Away from code, a spacing fix still counts.
        let view = diff("a  b `c`", "a b `c`");
        assert_eq!(without_changes_in(view, &[(5, 8)]).change_count, 1);
    }

    /// Whatever a model does to a revision, every code span comes through as written and can be
    /// found in the result. Prose fixes apply except where a change also covers code.
    #[test]
    fn code_survives_any_revision() {
        let cases = [
            // Capitalized, as Apple Intelligence did.
            (
                "should i run `wrangler deploy` now",
                "Should I run `Wrangler deploy` now?",
                "Should I run `wrangler deploy` now?",
            ),
            // Backticks dropped.
            (
                "should i run `wrangler deploy` now",
                "Should I run wrangler deploy now?",
                "Should I run `wrangler deploy` now?",
            ),
            // Code removed, and code reworded inside: "So" joins the removal, so it is left out too.
            ("so `npm i` then `npm  test` ok", "So then `npm run test`, OK.", "so `npm i` then `npm  test`, OK."),
            // Code at the start and the end, touching punctuation.
            ("`teh` is fine, run `teh`.", "`The` is fine, run `the`.", "`teh` is fine, run `teh`."),
            // A word merged across the edge, and a fenced block rewritten.
            (
                "use `ls`ing here\n```\nteh\n```\nok",
                "Use lsing here.\n```\nthe\n```\nOK",
                "Use `ls`ing here.\n```\nteh\n```\nOK",
            ),
        ];
        for (original, revised, expected) in cases {
            let spans = crate::guard::code_spans(original);
            let result = apply(&without_changes_in(diff(original, revised), &spans), &HashSet::new());
            assert_eq!(result, expected);
            // The paste marks code by where it is in the result.
            let (old, new): (Vec<char>, Vec<char>) = (original.chars().collect(), result.chars().collect());
            let carried = carry_ranges(original, &result, &spans);
            assert_eq!(carried.len(), spans.len(), "{result:?}");
            for (&(s, e), &(ns, ne)) in spans.iter().zip(&carried) {
                assert_eq!(new[ns..ne], old[s..e], "{result:?}");
            }
        }
    }

    #[test]
    fn ranges_follow_the_revision() {
        let original = "does this code block work nw";
        assert_eq!(carry_ranges(original, "Does this code block work now?", &[(10, 14)]), vec![(10, 14)]);
        assert_eq!(carry_ranges(original, "So does this code block work now?", &[(10, 14)]), vec![(13, 17)]);
        assert!(carry_ranges(original, "does this snippet block work", &[(10, 14)]).is_empty());
    }

    #[test]
    fn marks_the_checkers_fixes() {
        let original = "Their are typos we should of seen in the the draft.";
        let revised = "There are typos we should of seen in the the draft.";
        let checked = "There are typos we should have seen in the draft.";
        let mut pass = diff(revised, checked);
        for segment in &mut pass.segments {
            if let Segment::Change { note, .. } = segment {
                *note = Some("A rule".into());
            }
        }
        let mut view = diff(original, checked);
        mark_rule_based(&mut view, &pass);
        let marked: Vec<(&str, bool, Option<&str>)> = view
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Change { old, rule_based, note, .. } => Some((old.as_str(), *rule_based, note.as_deref())),
                _ => None,
            })
            .collect();
        // The AI's fix stays its own; a replacement and a removal are the checker's.
        assert_eq!(marked, vec![("Their", false, None), ("of", true, Some("A rule")), ("the ", true, Some("A rule"))]);
    }

    #[test]
    fn stream_view_reports_unreached_text_as_pending() {
        let view = stream_view("This are teh draft. It has mistakes.", "This is the dra");
        assert_eq!(view.pending, "draft. It has mistakes.");
        let c = changes(&view);
        assert_eq!(c.len(), 2);
        assert_eq!(apply(&view, &HashSet::new()), "This is the draft. It has mistakes.");
    }

    #[test]
    fn stream_view_before_first_match_is_all_pending() {
        let view = stream_view("Hello world", "");
        assert_eq!(view.pending, "Hello world");
        assert!(view.segments.is_empty());
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        let view = diff("Café teh menu", "Café the menu");
        let Segment::Change { old_start, .. } = view.segments[1].clone() else { panic!() };
        assert_eq!(old_start, 5);
    }

    #[test]
    fn multilingual_text_diffs_by_word() {
        let view = diff("Je suis allé au marché hier soir.", "Je suis allée au marché hier soir.");
        let c = changes(&view);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].0.as_str(), c[0].1.as_str()), ("allé", "allée"));
    }
}
