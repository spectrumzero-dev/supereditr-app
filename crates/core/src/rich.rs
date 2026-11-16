//! A selection in a rich-text editor: its text, the formatting the app reports for it, and the
//! same formatting as HTML around a revision of the text.
//!
//! A fix that replaces a whole selection is pasted. A rich-text editor turns pasted plain text into
//! ordinary paragraphs, so a heading, a list, a code block, bold or a link in the selection would
//! come back as plain lines and words. Pasted as HTML, the revision keeps them: the platform reads
//! the selection as [`Piece`]s, [`assemble`] turns them into the text to review and a tree of what
//! is formatted how, and [`html`] writes the revised text inside that tree.

use crate::diff::{Segment, diff};

/// Part of an element's text as an app reports it: a run of its own, or an element inside it.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    /// Text with where the app holds it: `offset` characters into the text of the element the
    /// platform numbers `object`. A fix applied in place selects its words there.
    Run {
        text: String,
        object: usize,
        offset: usize,
    },
    Object(Object),
}

impl Piece {
    fn own_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) | Self::Run { text, .. } => Some(text),
            Self::Object(_) => None,
        }
    }
}

/// Characters `start..end` of a selection's text, held by an element from `offset` in its own
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub object: usize,
    pub offset: usize,
}

/// An element inside a rich-text field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Object {
    /// The HTML tag, lower case: "p", "strong", "li".
    pub tag: String,
    /// Shown on lines of its own, as a paragraph is.
    pub block: bool,
    /// Where a link goes.
    pub href: Option<String>,
    /// The bullet or number a list item starts with, as its text holds it: "• ", "1. ".
    pub marker: Option<String>,
    pub pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// The selection itself.
    Root,
    Paragraph,
    Heading(u8),
    Quote,
    List {
        ordered: bool,
    },
    /// A list item. One list can hold bullets and numbers: each item's own marker says which.
    /// `number` is the number its marker shows, when it shows one.
    Item {
        ordered: bool,
        number: Option<u32>,
    },
    CodeBlock,
    /// Any other block.
    Block,
    Bold,
    Italic,
    Strike,
    Code,
    Link(String),
    /// A list item's bullet or number, or other text a style sheet adds (`::before`): text the app
    /// shows, which pasted HTML gets from the editor again.
    Marker,
    /// Any other inline element.
    Inline,
}

impl Object {
    /// Text a style sheet adds, or an element that holds nothing else, as one editor wraps its
    /// bullets.
    fn generated(&self) -> bool {
        self.tag.starts_with("::") || matches!(self.pieces.as_slice(), [Piece::Object(only)] if only.generated())
    }
}

impl Kind {
    fn of(object: &Object) -> Self {
        if object.generated() {
            return Self::Marker;
        }
        match object.tag.as_str() {
            "p" => Self::Paragraph,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Self::Heading(object.tag.as_bytes()[1] - b'0'),
            "blockquote" => Self::Quote,
            "ul" => Self::List { ordered: false },
            "ol" => Self::List { ordered: true },
            "li" => Self::Item { ordered: false, number: None },
            "pre" => Self::CodeBlock,
            "code" if object.block => Self::CodeBlock,
            "code" => Self::Code,
            "strong" | "b" => Self::Bold,
            "em" | "i" => Self::Italic,
            "s" | "del" | "strike" => Self::Strike,
            "a" => match &object.href {
                Some(href) if safe_link(href) => Self::Link(href.clone()),
                _ => Self::Inline,
            },
            _ if object.block => Self::Block,
            _ => Self::Inline,
        }
    }

    fn is_block(&self) -> bool {
        matches!(
            self,
            Self::Root
                | Self::Paragraph
                | Self::Heading(_)
                | Self::Quote
                | Self::List { .. }
                | Self::Item { .. }
                | Self::CodeBlock
                | Self::Block
        )
    }

    /// Formatting a plain-text paste would lose.
    fn is_formatting(&self) -> bool {
        !matches!(self, Self::Root | Self::Paragraph | Self::Block | Self::Inline | Self::Marker)
    }
}

/// Only links a page could hold anyway: nothing that runs when pasted.
fn safe_link(href: &str) -> bool {
    let href = href.trim_start().to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|scheme| href.starts_with(scheme))
        && !href.chars().any(char::is_control)
}

/// An element and the characters of the text it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub children: Vec<Node>,
}

impl Node {
    /// Character ranges of code: inline code and code blocks.
    pub fn code(&self) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        self.collect_code(&mut ranges);
        ranges
    }

    fn collect_code(&self, ranges: &mut Vec<(usize, usize)>) {
        if matches!(self.kind, Kind::Code | Kind::CodeBlock) {
            if self.end > self.start {
                ranges.push((self.start, self.end));
            }
            return;
        }
        self.children.iter().for_each(|child| child.collect_code(ranges));
    }

    /// Whether anything here is formatted in a way a plain-text paste would lose. `text` is the
    /// text the node covers part of: a line break inside a paragraph counts, since pasted plain
    /// text would start a new paragraph there.
    pub fn formatted(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.formatted_in(&chars)
    }

    fn formatted_in(&self, chars: &[char]) -> bool {
        if self.kind.is_formatting() || self.children.iter().any(|child| child.formatted_in(chars)) {
            return true;
        }
        let leaf = matches!(self.kind, Kind::Paragraph | Kind::Block)
            && !self.children.iter().any(|child| child.kind.is_block());
        leaf && (self.start..self.end.min(chars.len())).any(|i| chars[i] == '\n')
    }

    fn clamp(&mut self, end: usize) {
        self.end = self.end.min(end);
        self.start = self.start.min(self.end);
        self.children.iter_mut().for_each(|child| child.clamp(end));
    }
}

/// A selection's text and its formatting.
#[derive(Debug, Clone, PartialEq)]
pub struct Rich {
    pub text: String,
    pub root: Node,
    /// Where the app holds each stretch of the text, for the pieces that say.
    pub runs: Vec<Run>,
}

/// Puts the pieces' text together as the field shows it: each block (a paragraph, a list item) on a
/// line of its own, an empty paragraph as an empty line, and inline elements (links, bold, code) in
/// the line. The tree says which characters each element covers.
pub fn assemble(pieces: &[Piece]) -> Rich {
    let mut text = String::new();
    let mut chars = 0;
    let mut runs = Vec::new();
    let children = build(pieces, None, &mut text, &mut chars, &mut runs);
    Rich { text, root: Node { kind: Kind::Root, start: 0, end: chars, children }, runs }
}

/// `marker` is the bullet or number of the list item these pieces are in.
fn build(
    pieces: &[Piece],
    marker: Option<&str>,
    text: &mut String,
    chars: &mut usize,
    runs: &mut Vec<Run>,
) -> Vec<Node> {
    let is_block =
        |piece: Option<&Piece>| matches!(piece, Some(Piece::Object(object)) if object.block && !object.generated());
    let mut nodes = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        let before = i.checked_sub(1).and_then(|i| pieces.get(i));
        // A list item's first paragraph is on the line of its bullet.
        let after_marker = i == 1
            && match (before, marker) {
                (Some(Piece::Object(object)), _) => object.generated(),
                (Some(own), Some(marker)) => own.own_text() == Some(marker),
                _ => false,
            };
        if i > 0 && !after_marker && (is_block(Some(piece)) || is_block(before)) {
            text.push('\n');
            *chars += 1;
        }
        let object = match piece {
            Piece::Object(object) => object,
            own => {
                let run = own.own_text().unwrap_or_default();
                let length = run.chars().count();
                if let Piece::Run { object, offset, .. } = own {
                    runs.push(Run { start: *chars, end: *chars + length, object: *object, offset: *offset });
                }
                text.push_str(run);
                *chars += length;
                continue;
            }
        };
        let start = *chars;
        let mut children = build(&object.pieces, object.marker.as_deref(), text, chars, runs);
        // An empty paragraph holds a line break, and a paragraph may end with one.
        if is_block(Some(piece)) && *chars > start && text.ends_with('\n') {
            text.pop();
            *chars -= 1;
            children.iter_mut().for_each(|child| child.clamp(*chars));
            while let Some(last) = runs.last_mut().filter(|last| last.end > *chars) {
                last.end = *chars;
                if last.start >= last.end {
                    runs.pop();
                }
            }
        }
        // A bullet an editor draws itself has no space after it; the text reads better with one.
        if object.generated() && *chars > start && !text.ends_with(char::is_whitespace) {
            text.push(' ');
            *chars += 1;
        }
        let mut kind = Kind::of(object);
        // Inline code with a line to itself between blocks is a code block: one editor shows a
        // code block as a <code> alone.
        let alone = (i == 0 || is_block(before)) && (i + 1 == pieces.len() || is_block(pieces.get(i + 1)));
        if kind == Kind::Code && alone && pieces.iter().any(|piece| is_block(Some(piece))) {
            kind = Kind::CodeBlock;
        }
        if let Some(marker) = object.marker.as_deref().filter(|marker| !marker.is_empty()) {
            let length = marker.chars().count();
            let starts_with_it = text.chars().skip(start).take(length).eq(marker.chars());
            if starts_with_it && children.first().is_none_or(|first| first.start >= start + length) {
                children.insert(0, Node { kind: Kind::Marker, start, end: start + length, children: Vec::new() });
            }
        }
        if let Kind::List { ordered } = kind {
            // An item with a number is numbered, one with a bullet isn't; one with no marker goes
            // by its list.
            for child in &mut children {
                let marker =
                    child.children.first().filter(|first| first.kind == Kind::Marker && first.start == child.start);
                let marker: Option<String> =
                    marker.map(|marker| text.chars().skip(marker.start).take(marker.end - marker.start).collect());
                if let Kind::Item { ordered: item, number } = &mut child.kind {
                    *item = marker.as_ref().map_or(ordered, |marker| marker.chars().any(char::is_alphanumeric));
                    let digits: String = marker.iter().flat_map(|m| m.chars()).filter(char::is_ascii_digit).collect();
                    *number = digits.parse().ok().filter(|_| *item);
                }
            }
        }
        nodes.push(Node { kind, start, end: *chars, children });
    }
    nodes
}

/// One write of a proofread applied in place: in the element the platform numbers `object`,
/// characters `start..end` of its own text, which read `old`, become `new`. Neither is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub object: usize,
    pub start: usize,
    pub end: usize,
    pub old: String,
    pub new: String,
}

/// `edits` (from [`crate::edits::plan`], last first) as writes into the elements that hold the
/// text, in the same order. Each write replaces text with text, so the platform can select what is
/// there, check it, and paste over it: words to add or remove take one character beside them along,
/// which is written back.
///
/// None when an edit doesn't lie inside one stretch of one element's text, so it can't be selected
/// there, or when two writes would touch.
pub fn steps(original: &str, runs: &[Run], edits: &[crate::edits::Edit]) -> Option<Vec<Step>> {
    let chars: Vec<char> = original.chars().collect();
    let mut steps = Vec::with_capacity(edits.len());
    // Where the write before this one (later in the text) starts.
    let mut floor = usize::MAX;
    for edit in edits {
        let (mut start, mut end) = (edit.start, edit.start + edit.old.chars().count());
        // Text added goes with its anchor: after the text before it, or before the text after it.
        let with_before = edit.anchor.as_ref().is_none_or(|anchor| !anchor.after);
        let run = if end > start {
            runs.iter().find(|run| run.start <= start && end <= run.end)?
        } else {
            let before = |run: &&Run| run.start < start && start <= run.end;
            let after = |run: &&Run| run.start <= start && start < run.end;
            match with_before {
                true => runs.iter().find(before).or_else(|| runs.iter().find(after))?,
                false => runs.iter().find(after).or_else(|| runs.iter().find(before))?,
            }
        };
        let mut new = edit.new.clone();
        if end == start || new.is_empty() {
            let usable = |at: usize| chars.get(at).is_some_and(|c| *c != '\n' && !crate::diff::is_invisible(*c));
            let left = start > run.start && usable(start - 1);
            let right = end < run.end && usable(end);
            if left && (with_before || !right) {
                start -= 1;
                new.insert(0, chars[start]);
            } else if right {
                new.push(chars[end]);
                end += 1;
            } else {
                return None;
            }
        }
        if end > floor || end > chars.len() {
            return None;
        }
        floor = start;
        steps.push(Step {
            object: run.object,
            start: run.offset + start - run.start,
            end: run.offset + end - run.start,
            old: chars[start..end].iter().collect(),
            new,
        });
    }
    Some(steps)
}

/// Where the selection ends once `steps` are written: the element that holds its last stretch of
/// text, and the offset in that element's own text. Every write before that point in the element
/// moves it, whichever stretch the write was in.
pub fn end_after(runs: &[Run], steps: &[Step]) -> Option<(usize, usize)> {
    let last = runs.iter().max_by_key(|run| run.end)?;
    let end = last.offset + (last.end - last.start);
    let moved = |step: &&Step| step.object == last.object && step.end <= end;
    let (added, removed) = steps
        .iter()
        .filter(moved)
        .fold((0, 0), |(added, removed), step| (added + step.new.chars().count(), removed + (step.end - step.start)));
    Some((last.object, (end + added).saturating_sub(removed)))
}

/// `revised`, a revision of `original`, as HTML for pasting, with the formatting `root` gives
/// `original`: text the revision changed keeps the formatting of what it replaced.
///
/// None when the selection has no formatting to keep, or when HTML can't hold the text exactly:
/// outside a code block, HTML collapses runs of spaces and drops them at the ends of a line.
pub fn html(original: &str, root: &Node, revised: &str) -> Option<String> {
    if !root.formatted(original) {
        return None;
    }
    let length = original.chars().count();
    if root.end > length {
        return None;
    }
    // Where each position of the original is in the revision: the first and last place it can be
    // (they differ where text was added), and whether it is inside text the revision replaced.
    let (mut lo, mut hi, mut inside) = (vec![usize::MAX; length + 1], vec![0; length + 1], vec![false; length + 1]);
    let mut at = |old: usize, new: usize| {
        if let (Some(lo), Some(hi)) = (lo.get_mut(old), hi.get_mut(old)) {
            *lo = (*lo).min(new);
            *hi = (*hi).max(new);
        }
    };
    let (mut old, mut new) = (0, 0);
    for segment in diff(original, revised).segments {
        match segment {
            Segment::Equal { text } => {
                let count = text.chars().count();
                (0..=count).for_each(|k| at(old + k, new + k));
                old += count;
                new += count;
            }
            Segment::Change { old: removed, new: added, .. } => {
                let (removed, added) = (removed.chars().count(), added.chars().count());
                at(old, new);
                (1..removed).for_each(|k| {
                    at(old + k, new);
                    at(old + k, new + added);
                    if let Some(inside) = inside.get_mut(old + k) {
                        *inside = true;
                    }
                });
                at(old + removed, new + added);
                old += removed;
                new += added;
            }
        }
    }
    if old != length || lo.contains(&usize::MAX) {
        return None;
    }
    let chars: Vec<char> = revised.chars().collect();
    let mut writer = Writer {
        chars: &chars,
        lo,
        hi,
        inside,
        html: String::from("<meta charset=\"utf-8\">"),
        run: String::new(),
        exact: true,
        item_value: None,
    };
    writer.node(root, 0, chars.len());
    writer.end_run();
    writer.exact.then_some(writer.html)
}

struct Writer<'a> {
    /// The revision.
    chars: &'a [char],
    lo: Vec<usize>,
    hi: Vec<usize>,
    inside: Vec<bool>,
    html: String,
    /// The text written since the last line began, outside code blocks.
    run: String,
    /// Whether HTML holds every run so far exactly.
    exact: bool,
    /// The number the next list item must say it has: one that doesn't follow the item before it.
    item_value: Option<u32>,
}

impl Writer<'_> {
    /// Where `node` is in the revision. Text added at the edge of a block is inside it; text added
    /// at the edge of an inline element is outside, so a word added before bold isn't bold.
    fn span(&self, node: &Node) -> (usize, usize) {
        let block = node.kind.is_block();
        let place = |position: usize, start: bool| {
            if self.inside[position] {
                if start { self.lo[position] } else { self.hi[position] }
            } else if block == start {
                self.lo[position]
            } else {
                self.hi[position]
            }
        };
        (place(node.start, true), place(node.end, false))
    }

    fn end_run(&mut self) {
        let run = std::mem::take(&mut self.run);
        self.exact &= !run.starts_with(' ')
            && !run.ends_with(' ')
            && !run.contains("  ")
            && !run.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'));
    }

    fn escaped(&mut self, c: char) {
        match c {
            '&' => self.html.push_str("&amp;"),
            '<' => self.html.push_str("&lt;"),
            '>' => self.html.push_str("&gt;"),
            '"' => self.html.push_str("&quot;"),
            c => self.html.push(c),
        }
    }

    /// The revision's characters `from..to`, an element's own text. A line break next to a block
    /// only separates it from its neighbour; any other is a line break in the text. Loose text
    /// among blocks (`wrap`) becomes paragraphs.
    fn text(&mut self, mut from: usize, mut to: usize, after_block: bool, before_block: bool, wrap: bool) {
        if after_block && from < to && self.chars[from] == '\n' {
            from += 1;
        }
        if before_block && from < to && self.chars[to - 1] == '\n' {
            to -= 1;
        }
        if from >= to {
            return;
        }
        if wrap {
            self.end_run();
            self.html.push_str("<p>");
        }
        for i in from..to {
            match self.chars[i] {
                '\n' => {
                    self.end_run();
                    self.html.push_str(if wrap { "</p><p>" } else { "<br>" });
                }
                c => {
                    self.run.push(c);
                    self.escaped(c);
                }
            }
        }
        if wrap {
            self.end_run();
            self.html.push_str("</p>");
        }
    }

    fn node(&mut self, node: &Node, from: usize, to: usize) {
        let (open, close): (String, &str) = match &node.kind {
            Kind::Marker => return,
            Kind::CodeBlock => {
                self.end_run();
                self.html.push_str("<pre><code>");
                (from..to).for_each(|i| self.escaped(self.chars[i]));
                self.html.push_str("</code></pre>");
                return;
            }
            // A list's tags go around each run of its items of one kind.
            Kind::Root | Kind::Inline | Kind::List { .. } => (String::new(), ""),
            Kind::Paragraph => ("<p>".into(), "</p>"),
            Kind::Heading(level) => (
                format!("<h{level}>"),
                ["</h1>", "</h2>", "</h3>", "</h4>", "</h5>", "</h6>"][usize::from(level.clamp(&1, &6) - 1)],
            ),
            Kind::Quote => ("<blockquote>".into(), "</blockquote>"),
            Kind::Item { .. } => match self.item_value.take() {
                Some(value) => (format!("<li value=\"{value}\">"), "</li>"),
                None => ("<li>".into(), "</li>"),
            },
            Kind::Block => ("<div>".into(), "</div>"),
            Kind::Bold => ("<strong>".into(), "</strong>"),
            Kind::Italic => ("<em>".into(), "</em>"),
            Kind::Strike => ("<s>".into(), "</s>"),
            Kind::Code => ("<code>".into(), "</code>"),
            Kind::Link(href) => {
                self.html.push_str("<a href=\"");
                href.chars().for_each(|c| self.escaped(c));
                ("\">".into(), "</a>")
            }
        };
        let block = node.kind.is_block() && node.kind != Kind::Root;
        if block {
            self.end_run();
        }
        self.html.push_str(&open);
        let written = self.html.len();
        // Loose text among a container's blocks needs a paragraph of its own; a list item's doesn't.
        let wrap = matches!(node.kind, Kind::Root | Kind::Quote | Kind::Block)
            && node.children.iter().any(|child| child.kind.is_block());
        let (mut cursor, mut after_block) = (from, false);
        let mut list: Option<bool> = None;
        // The number a numbered item gets when it says nothing.
        let mut expected: Option<u32> = None;
        let list_end = |ordered: bool| if ordered { "</ol>" } else { "</ul>" };
        for child in &node.children {
            if let Kind::List { ordered } = node.kind {
                let (ordered, number) = match child.kind {
                    Kind::Item { ordered, number } => (ordered, number),
                    _ => (list.unwrap_or(ordered), None),
                };
                if list != Some(ordered) {
                    if let Some(open) = list {
                        self.html.push_str(list_end(open));
                    }
                    // A numbered list that doesn't start at 1 keeps its first number.
                    match (ordered, number) {
                        (true, Some(number)) if number != 1 => self.html.push_str(&format!("<ol start=\"{number}\">")),
                        (true, _) => self.html.push_str("<ol>"),
                        (false, _) => self.html.push_str("<ul>"),
                    }
                    list = Some(ordered);
                    expected = number.or(Some(1));
                }
                if ordered && matches!(child.kind, Kind::Item { .. }) {
                    // A list that skips a number keeps the skip.
                    self.item_value = number.filter(|number| Some(*number) != expected);
                    expected = number.or(expected).map(|number| number.saturating_add(1));
                }
            }
            let (start, end) = self.span(child);
            let start = start.clamp(cursor, to);
            let end = end.clamp(start, to);
            let child_block = child.kind.is_block();
            self.text(cursor, start, after_block, child_block, wrap);
            // An inline element the revision emptied is left out.
            if child_block || end > start {
                self.node(child, start, end);
            }
            cursor = end;
            after_block = child_block;
        }
        self.text(cursor, to, after_block, false, wrap);
        if let Some(open) = list {
            self.html.push_str(list_end(open));
        }
        if block {
            // An empty paragraph needs a line break to be a line.
            if self.html.len() == written && matches!(node.kind, Kind::Paragraph | Kind::Block) {
                self.html.push_str("<br>");
            }
            self.end_run();
        }
        self.html.push_str(close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = "<meta charset=\"utf-8\">";

    fn text(run: &str) -> Piece {
        Piece::Text(run.into())
    }

    fn block(tag: &str, pieces: Vec<Piece>) -> Piece {
        Piece::Object(Object { tag: tag.into(), block: true, pieces, ..Object::default() })
    }

    fn inline(tag: &str, run: &str) -> Piece {
        Piece::Object(Object { tag: tag.into(), pieces: vec![text(run)], ..Object::default() })
    }

    fn item(marker: &str, mut pieces: Vec<Piece>) -> Piece {
        pieces.insert(0, text(marker));
        Piece::Object(Object {
            tag: "li".into(),
            block: true,
            marker: Some(marker.into()),
            pieces,
            ..Object::default()
        })
    }

    fn link(run: &str, href: &str) -> Piece {
        Piece::Object(Object { tag: "a".into(), href: Some(href.into()), pieces: vec![text(run)], ..Object::default() })
    }

    /// The HTML of `revised`, a revision of the pieces' text.
    fn pasted(pieces: &[Piece], revise: impl Fn(&str) -> String) -> Option<String> {
        let rich = assemble(pieces);
        html(&rich.text, &rich.root, &revise(&rich.text)).map(|html| html.strip_prefix(META).unwrap().to_string())
    }

    fn same(text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn a_web_editors_paragraphs_are_put_together_line_by_line_with_its_code() {
        // A browser's tree for a rich-text editor: three paragraphs, the middle one empty (a line
        // break), and code, bold and a link inside the first.
        let first = block(
            "p",
            vec![
                inline("code", "teh fix"),
                text(" first. Their going to love this "),
                inline("strong", "today"),
                text(". Read "),
                link("the notes", "https://example.com/notes"),
                text(" and realy run "),
                inline("code", "their going to work"),
            ],
        );
        let rich = assemble(&[first, block("p", vec![text("\n")]), block("p", vec![text("See you soon.")])]);
        assert_eq!(
            rich.text,
            "teh fix first. Their going to love this today. Read the notes and realy run their going to work\n\nSee you soon."
        );
        let spans: Vec<String> =
            rich.root.code().iter().map(|&(from, to)| rich.text.chars().skip(from).take(to - from).collect()).collect();
        assert_eq!(spans, ["teh fix", "their going to work"]);
        let fixed = rich.text.replace("Their going", "They’re going").replace("realy", "really");
        assert_eq!(
            html(&rich.text, &rich.root, &fixed).unwrap(),
            format!(
                "{META}<p><code>teh fix</code> first. They’re going to love this <strong>today</strong>. Read \
                 <a href=\"https://example.com/notes\">the notes</a> and really run <code>their going to work</code></p>\
                 <p><br></p><p>See you soon.</p>"
            )
        );
    }

    #[test]
    fn code_offsets_count_characters_and_empty_lines_survive_at_either_end() {
        let pieces = [
            block("p", vec![text("\n")]),
            block("p", vec![text("Rhyan 👋🏽 ran "), inline("code", "make teh")]),
            block("p", vec![]),
        ];
        let rich = assemble(&pieces);
        assert_eq!(rich.text, "\nRhyan 👋🏽 ran make teh\n");
        assert_eq!(rich.root.code(), [(14, 22)], "characters: the line break, \"Rhyan \", the emoji as two, \" ran \"");
        assert_eq!(pasted(&pieces, same).unwrap(), "<p><br></p><p>Rhyan 👋🏽 ran <code>make teh</code></p><p><br></p>");
    }

    #[test]
    fn a_code_block_is_found_whatever_wraps_its_text_and_keeps_its_spaces() {
        // <pre><code> around blocks that hold the text, as one editor nests it; a <code> alone
        // between blocks, as another shows a code block; and a <pre> that holds its text itself.
        let nested = block("pre", vec![inline_of("code", vec![block("div", vec![text("def a():\n    return  1\n")])])]);
        let alone = inline("code", "make");
        let bare = block("pre", vec![text("git "), inline("strong", "push")]);
        let pieces = [block("p", vec![text("Run it:")]), nested, alone, bare, block("p", vec![text("Done.")])];
        let rich = assemble(&pieces);
        assert_eq!(rich.text, "Run it:\ndef a():\n    return  1\nmake\ngit push\nDone.");
        assert_eq!(rich.root.code(), [(8, 30), (31, 35), (36, 44)]);
        assert_eq!(
            pasted(&pieces, |text| text.replace("Run it", "Run this")).unwrap(),
            "<p>Run this:</p><pre><code>def a():\n    return  1</code></pre><pre><code>make</code></pre>\
             <pre><code>git push</code></pre><p>Done.</p>"
        );
    }

    fn inline_of(tag: &str, pieces: Vec<Piece>) -> Piece {
        Piece::Object(Object { tag: tag.into(), pieces, ..Object::default() })
    }

    #[test]
    fn inline_code_in_a_single_line_needs_no_paragraph() {
        let pieces = [text("run "), inline("code", "make"), text(" now")];
        assert_eq!(pasted(&pieces, |text| text.replace("now", "today")).unwrap(), "run <code>make</code> today");
    }

    #[test]
    fn headings_lists_quotes_and_line_breaks_keep_their_structure() {
        let pieces = [
            block("h3", vec![text("Weekly notes")]),
            block(
                "ul",
                vec![
                    item("• ", vec![block("p", vec![text("Apples")])]),
                    item(
                        "• ",
                        vec![
                            block("p", vec![text("Pears")]),
                            block("ul", vec![item("◦ ", vec![block("p", vec![text("Ripe ones")])])]),
                        ],
                    ),
                ],
            ),
            block("ol", vec![item("1. ", vec![text("First step")]), item("2. ", vec![text("Secend step")])]),
            block("blockquote", vec![block("p", vec![text("Quoted line with realy good news.")])]),
            block("p", vec![text("Line one"), text("\n"), text("Line two after a "), inline("em", "break"), text(".")]),
        ];
        let rich = assemble(&pieces);
        assert_eq!(
            rich.text,
            "Weekly notes\n• Apples\n• Pears\n◦ Ripe ones\n1. First step\n2. Secend step\nQuoted line with realy good news.\nLine one\nLine two after a break."
        );
        // One editor keeps bullets and numbers in one list, each marker in an element of its own.
        let bullet = |marker: &str, run: &str| {
            let before =
                Object { tag: "::before".into(), block: true, pieces: vec![text(marker)], ..Object::default() };
            let wrapper =
                Object { tag: "span".into(), block: true, pieces: vec![Piece::Object(before)], ..Object::default() };
            block("li", vec![Piece::Object(wrapper), text(run)])
        };
        let mixed = [block("ol", vec![bullet("•", "Apples"), bullet("•", "Pears"), bullet("1.", "First step")])];
        assert_eq!(assemble(&mixed).text, "• Apples\n• Pears\n1. First step");
        assert_eq!(pasted(&mixed, same).unwrap(), "<ul><li>Apples</li><li>Pears</li></ul><ol><li>First step</li></ol>");
        let fixed = rich.text.replace("Secend", "Second").replace("realy", "really");
        assert_eq!(
            html(&rich.text, &rich.root, &fixed).unwrap(),
            format!(
                "{META}<h3>Weekly notes</h3><ul><li><p>Apples</p></li><li><p>Pears</p><ul><li><p>Ripe ones</p></li></ul></li></ul>\
                 <ol><li>First step</li><li>Second step</li></ol><blockquote><p>Quoted line with really good news.</p></blockquote>\
                 <p>Line one<br>Line two after a <em>break</em>.</p>"
            )
        );
    }

    fn run(text: &str, object: usize, offset: usize) -> Piece {
        Piece::Run { text: text.into(), object, offset }
    }

    #[test]
    fn a_proofread_becomes_writes_into_the_elements_that_hold_the_words() {
        // A paragraph (element 1) with bold (element 2) in it, then a second paragraph (element 3).
        // In its element's text an inner element counts as one character.
        let bold =
            Piece::Object(Object { tag: "strong".into(), pieces: vec![run("realy good", 2, 0)], ..Object::default() });
        let pieces = [
            block("p", vec![run("Their going to love ", 1, 0), bold, run(" news news.", 1, 21)]),
            block("p", vec![run("See you soon", 3, 0)]),
        ];
        let rich = assemble(&pieces);
        assert_eq!(rich.text, "Their going to love realy good news news.\nSee you soon");
        let fixed = "They’re going to love really good news.\nSee you soon.";
        let edits = crate::edits::plan(&rich.text, fixed, true, &[]).unwrap();
        let steps = steps(&rich.text, &rich.runs, &edits).unwrap();
        let written: Vec<(usize, usize, usize, &str, &str)> =
            steps.iter().map(|s| (s.object, s.start, s.end, s.old.as_str(), s.new.as_str())).collect();
        // The selection ends in the second paragraph, one character longer.
        assert_eq!(end_after(&rich.runs, &steps), Some((3, 13)));
        // Ending in the first paragraph's element, after its bold: "Their" grew by two there and
        // " news" went, while the fix inside the bold is in another element.
        let first = &rich.runs[..3];
        assert_eq!(end_after(first, &steps[1..]), Some((1, 21 + 11 + 2 - 5)));
        assert_eq!(
            written,
            [
                // A full stop added: the letter before it is written again with it.
                (3, 11, 12, "n", "n."),
                // A word removed: the letter before it is written again alone.
                (1, 25, 31, "s news", "s"),
                (2, 0, 5, "realy", "really"),
                (1, 0, 5, "Their", "They’re"),
            ]
        );
    }

    #[test]
    fn a_change_that_no_single_element_holds_isnt_applied_in_place() {
        // "alot" is split between plain text and bold.
        let bold = Piece::Object(Object { tag: "strong".into(), pieces: vec![run("lot", 2, 0)], ..Object::default() });
        let rich = assemble(&[run("Thanks a", 1, 0), bold, run(" for it.", 1, 9)]);
        let edits = crate::edits::plan(&rich.text, "Thanks a lot for it.", true, &[]).unwrap();
        assert_eq!(steps(&rich.text, &rich.runs, &edits), None);
        // Text read without its place can't be written in place either.
        let rich = assemble(&[text("Their going")]);
        let edits = crate::edits::plan(&rich.text, "They’re going", true, &[]).unwrap();
        assert_eq!(steps(&rich.text, &rich.runs, &edits), None);
    }

    #[test]
    fn a_numbered_list_keeps_the_number_it_starts_at() {
        let pieces = [block("ol", vec![item("3. ", vec![text("Third step")]), item("4. ", vec![text("Forth step")])])];
        assert_eq!(
            pasted(&pieces, |text| text.replace("Forth", "Fourth")).unwrap(),
            "<ol start=\"3\"><li>Third step</li><li>Fourth step</li></ol>"
        );
        let skips = [block(
            "ol",
            vec![item("3. ", vec![text("Third")]), item("5. ", vec![text("Fifth")]), item("6. ", vec![text("Sixth")])],
        )];
        assert_eq!(
            pasted(&skips, same).unwrap(),
            "<ol start=\"3\"><li>Third</li><li value=\"5\">Fifth</li><li>Sixth</li></ol>"
        );
    }

    #[test]
    fn a_line_break_inside_a_paragraph_is_formatting_enough() {
        let pieces = [block("p", vec![text("Line one"), text("\n"), text("Line too")]), block("p", vec![text("Bye.")])];
        assert_eq!(
            pasted(&pieces, |text| text.replace("too", "two")).unwrap(),
            "<p>Line one<br>Line two</p><p>Bye.</p>"
        );
    }

    #[test]
    fn changed_words_keep_the_formatting_of_what_they_replace() {
        let pieces =
            [text("This is "), inline("strong", "realy importent"), text(" to "), inline("em", "Kyndal"), text(".")];
        // A word fixed inside bold stays bold; words added beside bold and italics stay plain.
        assert_eq!(
            pasted(&pieces, |_| "Today this is really important to our Kyndal.".into()).unwrap(),
            "Today this is <strong>really important</strong> to our <em>Kyndal</em>."
        );
        // Bold the revision removed is left out.
        assert_eq!(pasted(&pieces, |_| "This is to Kyndal.".into()).unwrap(), "This is to <em>Kyndal</em>.");
    }

    #[test]
    fn a_link_that_could_run_something_is_pasted_as_its_text() {
        let pieces = [text("Read "), link("the notes", "javascript:alert(1)"), text(" and "), inline("strong", "go")];
        assert_eq!(pasted(&pieces, same).unwrap(), "Read the notes and <strong>go</strong>");
        let quoted = [link("a \"b\"", "https://example.com/?q=\"<a>\"&r=1")];
        assert_eq!(
            pasted(&quoted, same).unwrap(),
            "<a href=\"https://example.com/?q=&quot;&lt;a&gt;&quot;&amp;r=1\">a &quot;b&quot;</a>"
        );
    }

    #[test]
    fn plain_paragraphs_need_no_html() {
        let pieces = [block("p", vec![text("Their going")]), block("p", vec![text("See you soon.")])];
        assert_eq!(pasted(&pieces, same), None);
        assert_eq!(pasted(&[text("Their going")], same), None);
    }

    #[test]
    fn text_html_would_change_isnt_made_into_html() {
        for lossy in ["two  spaces", " leading", "trailing ", "a\tb"] {
            let pieces = [block("p", vec![text(lossy)]), block("p", vec![inline("strong", "bold")])];
            assert_eq!(pasted(&pieces, same), None, "{lossy:?}");
            let broken = [block("p", vec![text("one"), text("\n"), text(lossy), inline("strong", "bold")])];
            assert_eq!(pasted(&broken, same).is_none(), lossy != "trailing ", "{lossy:?} after a line break");
        }
    }
}
