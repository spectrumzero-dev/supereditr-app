//! The text of web fields: characters counted without line breaks, the code and list markers in
//! them, and code written back as Markdown or HTML for pasting.

use super::ax::Element;
use super::trace;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

pub(super) fn is_line_break(c: char) -> bool {
    c == '\n' || c == '\r'
}

/// Characters other than line breaks in an element's text and list markers, a few levels down
/// (links, bold text and list items nest it).
pub(super) fn visible_length(element: &Element, depth: usize) -> usize {
    if matches!(element.string("AXRole").as_deref(), Some("AXStaticText" | "AXListMarker")) {
        return element.string("AXValue").map_or(0, |v| v.chars().filter(|c| !is_line_break(*c)).count());
    }
    if depth >= 8 {
        return 0;
    }
    element.children().iter().map(|child| visible_length(child, depth + 1)).sum()
}

/// A piece of a web field formatted as code: its text, and where it starts in the field, counted
/// in characters other than line breaks.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CodePiece {
    pub(super) text: String,
    start: usize,
}

fn visible_count(text: &str) -> usize {
    text.chars().filter(|c| !is_line_break(*c)).count()
}

/// What [`code_in`] found in a web field.
pub(super) struct Formatting {
    pub(super) code: Vec<CodePiece>,
    /// List markers ("• ", "1. "): part of the field's text, but not text the user wrote.
    pub(super) markers: Vec<CodePiece>,
    /// The field has formatting a plain-text replace would lose: code, a link, a list or a heading.
    pub(super) any: bool,
}

/// The elements in a web field marked as code (`<code>`, which WebKit and Chromium give the
/// subrole `AXCodeStyleGroup`), in order, and whether it has other formatting. The log gets the
/// count and the subroles seen, never the text, so a field whose code goes unnoticed can be
/// diagnosed.
pub(super) fn code_in(field: &Element) -> Formatting {
    struct Walk {
        started: Instant,
        budget: usize,
        seen: usize,
        subroles: BTreeSet<String>,
        code: Vec<CodePiece>,
        markers: Vec<CodePiece>,
        formatted: bool,
    }
    fn walk(element: &Element, depth: usize, state: &mut Walk) {
        for child in element.children() {
            if state.budget == 0 || state.started.elapsed() > Duration::from_millis(150) {
                state.budget = 0;
                return;
            }
            state.budget -= 1;
            let subrole = child.string("AXSubrole");
            let role = child.string("AXRole");
            state.formatted |= matches!(role.as_deref(), Some("AXLink" | "AXList" | "AXHeading"))
                || subrole.as_deref().is_some_and(|s| s.ends_with("StyleGroup"));
            if subrole.as_deref() == Some("AXCodeStyleGroup") {
                let text = static_text(&child, 0);
                let length = visible_count(&text);
                if !text.trim().is_empty() {
                    state.code.push(CodePiece { text, start: state.seen });
                }
                state.seen += length;
            } else if role.as_deref() == Some("AXStaticText") {
                state.seen += child.string("AXValue").map_or(0, |v| visible_count(&v));
            } else if role.as_deref() == Some("AXListMarker") {
                let text = child.string("AXValue").unwrap_or_default();
                if !text.trim().is_empty() {
                    state.markers.push(CodePiece { text: text.clone(), start: state.seen });
                }
                state.seen += visible_count(&text);
            } else if depth < 8 {
                walk(&child, depth + 1, state);
            }
            state.subroles.extend(subrole);
        }
    }
    // A composer has a few dozen elements. A bigger field stops after 1,500 or 150 ms: each is a
    // call to the app, which can be slow while it is busy.
    let started = Instant::now();
    let mut state = Walk {
        started,
        budget: 1500,
        seen: 0,
        subroles: BTreeSet::new(),
        code: Vec::new(),
        markers: Vec::new(),
        formatted: false,
    };
    walk(field, 0, &mut state);
    trace(format_args!(
        "code in the field: {} pieces in {:?}{}; subroles {}",
        state.code.len(),
        started.elapsed(),
        if state.budget == 0 { " (stopped early)" } else { "" },
        state.subroles.into_iter().collect::<Vec<_>>().join(", ")
    ));
    Formatting { code: state.code, markers: state.markers, any: state.formatted }
}

/// Character ranges of `text`, the selection, that the app formats as code. `selection_start` is
/// where the selection starts in the field, counted like [`CodePiece::start`]. A piece is placed by
/// its position when that is known and the selected text there matches it; otherwise wherever it
/// appears in `text` as whole words.
pub(super) fn code_ranges(text: &str, selection_start: Option<usize>, pieces: &[CodePiece]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    // Index in `chars` of each character other than a line break.
    let visible: Vec<usize> = (0..chars.len()).filter(|&i| !is_line_break(chars[i])).collect();
    let (mut ranges, mut by_position, mut by_text) = (Vec::new(), 0, 0);
    for piece in pieces {
        let code: Vec<char> = piece.text.chars().filter(|c| !is_line_break(*c)).collect();
        let placed = selection_start.and_then(|selected| {
            let start = piece.start.max(selected);
            let end = (piece.start + code.len()).min(selected + visible.len());
            if start >= end {
                return Some(None); // outside the selection
            }
            let (s, e) = (visible[start - selected], visible[end - selected - 1] + 1);
            let here: Vec<char> = chars[s..e].iter().copied().filter(|c| !is_line_break(*c)).collect();
            (here == code[start - piece.start..end - piece.start]).then_some(Some((s, e)))
        });
        match placed {
            Some(range) => {
                ranges.extend(range);
                by_position += 1;
            }
            None => {
                ranges.extend(supereditr_core::words::spans(text, piece.text.trim()));
                by_text += 1;
            }
        }
    }
    if !pieces.is_empty() {
        // Which invisible characters the field's text has, by code point: editors pad formatted
        // code with them.
        let mut invisible: Vec<String> = chars
            .iter()
            .filter(|c| {
                !c.is_ascii()
                    && (c.is_whitespace()
                        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' | '\u{FFFC}'))
            })
            .map(|c| format!("U+{:04X}", u32::from(*c)))
            .collect();
        invisible.dedup();
        trace(format_args!(
            "code placed: {by_position} by position, {by_text} by text; invisible characters: {}",
            if invisible.is_empty() { "none".to_string() } else { invisible.join(" ") }
        ));
    }
    ranges
}

/// Zero-width spaces and joiners, word joiners, byte order marks and object replacement
/// characters: editors put them around formatted text.
pub(super) fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{FFFC}')
}

/// `text` as Markdown for pasting, with `code` (character ranges) in backticks: editors that build
/// what is pasted from the plain text, as Claude's composer does, turn it back into code. The
/// zero-width spaces an editor pads code with are left out where they touch it, since the editor
/// adds its own; every other character is kept, a zero-width joiner in an emoji included.
pub(super) fn markdown_with_code(text: &str, code: &[(usize, usize)]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let in_code = |i: usize| code.iter().any(|&(s, e)| s <= i && i < e);
    let padding = |c: char| matches!(c, '\u{200B}' | '\u{2060}' | '\u{FEFF}');
    // A run of padding right before or after a span.
    let touches_code = |i: usize| {
        let (mut a, mut b) = (i, i);
        while a > 0 && padding(chars[a - 1]) && !in_code(a - 1) {
            a -= 1;
        }
        while b + 1 < chars.len() && padding(chars[b + 1]) && !in_code(b + 1) {
            b += 1;
        }
        (a > 0 && in_code(a - 1)) || (b + 1 < chars.len() && in_code(b + 1))
    };
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !in_code(i) || chars[i] == '\n' {
            if !(padding(chars[i]) && touches_code(i)) {
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }
        // The code up to the end of its span or line: Markdown code doesn't cross paragraphs.
        let start = i;
        while i < chars.len() && in_code(i) && chars[i] != '\n' {
            i += 1;
        }
        let piece: String = chars[start..i].iter().collect::<String>().trim_matches(padding).to_string();
        if piece.is_empty() {
            continue;
        }
        // One backtick more than the longest run inside, and a space against a backtick at an end.
        let longest = piece.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest + 1);
        let pad = if piece.starts_with('`') || piece.ends_with('`') { " " } else { "" };
        out.push_str(&format!("{fence}{pad}{piece}{pad}{fence}"));
    }
    out
}

/// `text` as HTML for pasting, with `code` (character ranges) marked as `<code>`. Each line is a
/// paragraph, as a plain-text paste into a rich editor makes it, an empty one included.
pub(super) fn html_with_code(text: &str, code: &[(usize, usize)]) -> String {
    let in_code = |i: usize| code.iter().any(|&(s, e)| s <= i && i < e);
    let lines: Vec<&str> = text.split('\n').collect();
    let paragraphs = lines.len() > 1;
    let mut html = String::from("<meta charset=\"utf-8\">");
    let mut i = 0;
    for line in lines {
        html.push_str(if paragraphs { "<p>" } else { "" });
        let mut open = false;
        for c in line.chars() {
            if in_code(i) != open {
                open = !open;
                html.push_str(if open { "<code>" } else { "</code>" });
            }
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                c => html.push(c),
            }
            i += 1;
        }
        if open {
            html.push_str("</code>");
        }
        if paragraphs {
            html.push_str(if line.is_empty() { "<br></p>" } else { "</p>" });
        }
        i += 1; // the line break
    }
    html
}

/// How many characters other than line breaks come before `location`, a UTF-16 offset into `value`.
pub(super) fn visible_before(value: &str, location: isize) -> Option<usize> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let prefix = String::from_utf16(units.get(..usize::try_from(location).ok()?)?).ok()?;
    Some(visible_count(&prefix))
}

/// An element's static text, a few levels down, in order.
fn static_text(element: &Element, depth: usize) -> String {
    if element.string("AXRole").as_deref() == Some("AXStaticText") {
        return element.string("AXValue").unwrap_or_default();
    }
    if depth >= 4 {
        return String::new();
    }
    element.children().iter().map(|child| static_text(child, depth + 1)).collect()
}

/// `value` with its empty paragraphs put back. Chromium separates a rich field's non-empty
/// paragraphs with fewer line breaks than there are paragraph breaks between them (one, for one
/// empty paragraph between two), and may end with a line break for an empty last paragraph.
/// `paragraphs` gives every paragraph's count of characters other than line breaks, 0 for an empty
/// one. None when `value` doesn't fit them.
pub(super) fn with_empty_paragraphs(value: &str, paragraphs: &[usize]) -> Option<String> {
    let mut rest = value;
    let mut texts: Vec<&str> = Vec::with_capacity(paragraphs.len());
    let (mut first, mut empty_run) = (true, 0);
    for &length in paragraphs {
        if length == 0 {
            texts.push("");
            empty_run += 1;
            continue;
        }
        if !first {
            // At least one line break, and at most one per paragraph break.
            rest = rest.strip_prefix('\n')?;
            for _ in 0..empty_run {
                rest = rest.strip_prefix('\n').unwrap_or(rest);
            }
        }
        first = false;
        empty_run = 0;
        let mut seen = 0;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                seen += usize::from(!is_line_break(c));
                seen == length
            })
            .map(|(i, c)| i + c.len_utf8())?;
        texts.push(&rest[..end]);
        rest = &rest[end..];
    }
    let trailing_breaks = rest.chars().count();
    (rest.chars().all(|c| c == '\n') && trailing_breaks <= empty_run).then(|| texts.join("\n"))
}

/// The span of `full` with the same characters as `selected`, line breaks aside, from its first
/// such character to its last, with `selected`'s own leading and trailing line breaks kept. None
/// when there isn't exactly one.
pub(super) fn span_of(full: &str, selected: &str) -> Option<String> {
    let target: String = selected.chars().filter(|c| !is_line_break(*c)).collect();
    if target.is_empty() {
        return None;
    }
    let (mut visible, mut starts) = (String::new(), Vec::new());
    for (i, c) in full.char_indices().filter(|&(_, c)| !is_line_break(c)) {
        visible.push(c);
        starts.push(i);
    }
    let mut found = visible.match_indices(target.as_str()).map(|(at, _)| at);
    let at = found.next()?;
    if found.next().is_some() {
        return None;
    }
    let first = visible[..at].chars().count();
    let last = first + target.chars().count() - 1;
    let end = starts[last] + full[starts[last]..].chars().next()?.len_utf8();
    let inner = selected.trim_matches(is_line_break);
    let (leading, trailing) = (&selected[..selected.find(inner)?], &selected[selected.rfind(inner)? + inner.len()..]);
    Some(format!("{leading}{}{trailing}", &full[starts[first]..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_code_by_position() {
        let piece = |text: &str, start| CodePiece { text: text.into(), start };
        // The field reads "Fix teh bug in teh parser"; only the second "teh" is code.
        let text = "Fix teh bug in teh parser";
        assert_eq!(code_ranges(text, Some(0), &[piece("teh", 15)]), vec![(15, 18)]);
        // Selected from "bug": the selection starts 8 characters into the field.
        assert_eq!(code_ranges("bug in teh parser", Some(8), &[piece("teh", 15)]), vec![(7, 10)]);
        // Code against prose, as in "foo" formatted and "bar" not.
        assert_eq!(code_ranges("run foobar", Some(0), &[piece("foo", 4)]), vec![(4, 7)]);
        // Line breaks don't count: the field's second paragraph starts after one.
        assert_eq!(code_ranges("one\ntwo teh", Some(0), &[piece("teh", 7)]), vec![(8, 11)]);
        // Code that runs past the selection is protected where it overlaps.
        assert_eq!(code_ranges("run wrangler", Some(0), &[piece("wrangler deploy", 4)]), vec![(4, 12)]);
        // Code at the very start and end of the selection.
        let text = "teh fix and run their going";
        let pieces = [piece("teh fix", 0), piece("their going", 16)];
        assert_eq!(code_ranges(text, Some(0), &pieces), vec![(0, 7), (16, 27)]);
        // Code outside the selection protects nothing.
        assert!(code_ranges("Fix teh bug", Some(0), &[piece("teh", 15)]).is_empty());
    }

    #[test]
    fn marks_code_in_pasted_markdown() {
        // Claude's composer pads its code with zero-width spaces.
        let text = "This is a test for the \u{200B}code\u{200B} block.";
        assert_eq!(markdown_with_code(text, &[(24, 28)]), "This is a test for the `code` block.");
        assert_eq!(markdown_with_code("run a`b now", &[(4, 7)]), "run ``a`b`` now");
        assert_eq!(markdown_with_code("use `x` here", &[(4, 7)]), "use `` `x` `` here");
        // Code across a line break is closed and reopened on each line.
        assert_eq!(markdown_with_code("one\ntwo", &[(0, 7)]), "`one`\n`two`");
        assert_eq!(markdown_with_code("no code", &[]), "no code");
        // A zero-width joiner holds an emoji together, in prose and in code; a zero-width space
        // away from code is the author's.
        let coder = "👩\u{200D}💻";
        assert_eq!(markdown_with_code(&format!("{coder} a b"), &[(4, 5)]), format!("{coder} `a` b"));
        assert_eq!(markdown_with_code(&format!("x {coder} y"), &[(2, 5)]), format!("x `{coder}` y"));
        assert_eq!(markdown_with_code("a\u{200B}b c", &[(4, 5)]), "a\u{200B}b `c`");
    }

    #[test]
    fn marks_code_in_pasted_html() {
        assert_eq!(
            html_with_code("Does this code block <work>?", &[(10, 14)]),
            "<meta charset=\"utf-8\">Does this <code>code</code> block &lt;work&gt;?"
        );
        assert_eq!(
            html_with_code("run npm\n\nthen ci", &[(4, 7), (14, 16)]),
            "<meta charset=\"utf-8\"><p>run <code>npm</code></p><p><br></p><p>then <code>ci</code></p>"
        );
    }

    #[test]
    fn places_code_by_text_when_its_position_is_unknown_or_wrong() {
        let piece = |text: &str, start| CodePiece { text: text.into(), start };
        assert_eq!(code_ranges("Fix teh bug", None, &[piece("teh", 4)]), vec![(4, 7)]);
        assert_eq!(code_ranges("Fix teh bug", Some(0), &[piece("teh", 0)]), vec![(4, 7)]);
        assert_eq!(visible_before("a\nb😀c", 5), Some(3));
        assert_eq!(visible_before("ab", 5), None);
    }

    // What Chrome reported for ProseMirror fields of two paragraphs around an empty one, of two
    // paragraphs, and of one paragraph with two hard breaks.
    #[test]
    fn puts_back_the_empty_paragraphs_chromium_leaves_out() {
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 0, 4]).as_deref(), Some("One.\n\nTwo."));
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 4]).as_deref(), Some("One.\nTwo."));
        assert_eq!(with_empty_paragraphs("One.\n\nTwo.", &[8]).as_deref(), Some("One.\n\nTwo."));
        assert_eq!(with_empty_paragraphs("Hi 🚀\nBye", &[0, 4, 0, 0, 3, 0]).as_deref(), Some("\nHi 🚀\n\n\nBye\n"));
    }

    // Grammarly's Chromium newline fix suggests a run of empty paragraphs can lose only one line
    // break, and an empty last paragraph can add one at the end; either way the result is the same.
    #[test]
    fn takes_any_number_of_line_breaks_chromium_keeps_for_empty_paragraphs() {
        let want = Some("One.\n\n\nTwo.\n");
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 0, 0, 4, 0]).as_deref(), want);
        assert_eq!(with_empty_paragraphs("One.\n\nTwo.\n", &[4, 0, 0, 4, 0]).as_deref(), want);
        assert_eq!(with_empty_paragraphs("One.\n\n\nTwo.", &[4, 0, 0, 4, 0]).as_deref(), want);
    }

    #[test]
    fn leaves_a_value_that_does_not_fit_the_paragraphs() {
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4, 0, 5]), None);
        // A list is one block: its items' line breaks don't count, its markers do.
        let list = "• This\n• is\n• a\n• list.";
        let value = format!("This is code.\n{list}");
        let blocks = [13, 0, list.chars().filter(|c| *c != '\n').count()];
        assert_eq!(with_empty_paragraphs(&value, &blocks), Some(format!("This is code.\n\n{list}")));
        assert_eq!(with_empty_paragraphs("One.\nTwo.", &[4]), None);
        assert_eq!(with_empty_paragraphs("One.Two.", &[4, 4]), None);
        assert_eq!(with_empty_paragraphs("One.\n\n", &[4, 0]), None);
    }

    #[test]
    fn maps_a_selection_onto_the_full_text() {
        let full = "this is a first sentence.\n\nIs this a newlie?";
        assert_eq!(span_of(full, "this is a first sentence.\nIs this a newlie?").as_deref(), Some(full));
        assert_eq!(span_of("One.\nTwo.", "One.Two.").as_deref(), Some("One.\nTwo."));
        assert_eq!(span_of(full, "sentence.Is this").as_deref(), Some("sentence.\n\nIs this"));
        assert_eq!(span_of(full, "first").as_deref(), Some("first"));
        assert_eq!(span_of("One\nTwo", "One\n").as_deref(), Some("One\n"));
        assert_eq!(span_of(full, "\nIs this").as_deref(), Some("\nIs this"));
    }

    #[test]
    fn keeps_a_selection_that_matches_nowhere_or_twice() {
        assert_eq!(span_of("One.\nTwo.", "Three."), None);
        assert_eq!(span_of("Go.\nGo.", "Go."), None);
        assert_eq!(span_of("One.", "\n"), None);
    }
}
