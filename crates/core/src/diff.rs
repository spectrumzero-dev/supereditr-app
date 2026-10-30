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
/// when it falls inside it, not at its edge.
pub fn without_changes_in(view: DiffView, spans: &[(usize, usize)]) -> DiffView {
    if spans.is_empty() {
        return view;
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut next_id = 0;
    for segment in view.segments {
        let segment = match segment {
            Segment::Change { old, old_start, .. } if touches(old_start, old.chars().count(), spans) => {
                Segment::Equal { text: old }
            }
            Segment::Change { id: _, old, new, kind, old_start, note } => {
                next_id += 1;
                Segment::Change { id: next_id - 1, old, new, kind, old_start, note }
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

fn touches(start: usize, len: usize, spans: &[(usize, usize)]) -> bool {
    let end = start + len;
    spans.iter().any(|&(s, e)| if len == 0 { s < start && start < e } else { start < e && s < end })
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
