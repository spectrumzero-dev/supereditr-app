//! Text as HTML, for a paste into a rich-text editor that keeps code as code.
//!
//! A rich-text editor turns pasted plain text into paragraphs: a code block in the selection comes
//! back as ordinary lines, and inline code as ordinary words. The same text pasted as HTML keeps
//! both.

/// `text` as HTML for pasting, with `code` (character ranges of `text`) marked as code. The lines
/// of a code block are kept as one (`<pre><code>`): those inside one of `blocks`, the ranges the
/// app shows as code blocks, or inside a code range of more than one line, for an app that doesn't
/// say. Other code is inline (`<code>`). Each line outside a block is a paragraph, as a plain-text
/// paste into a rich editor makes it, an empty one included.
///
/// None when HTML can't hold the text exactly: outside a code block, HTML collapses runs of spaces
/// and drops them at the ends of a line.
pub fn with_code(text: &str, code: &[(usize, usize)], blocks: &[(usize, usize)]) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    // Each range without the line breaks at its ends: an editor may count a block's last one. A
    // code range of more than one line is a block too.
    let trimmed = |ranges: &[(usize, usize)]| -> Vec<(usize, usize)> {
        ranges
            .iter()
            .filter_map(|&(start, end)| {
                let (mut start, mut end) = (start.min(chars.len()), end.min(chars.len()));
                while start < end && chars[start] == '\n' {
                    start += 1;
                }
                while end > start && chars[end - 1] == '\n' {
                    end -= 1;
                }
                (start < end).then_some((start, end))
            })
            .collect()
    };
    let code = trimmed(code);
    let blocks: Vec<(usize, usize)> =
        trimmed(blocks).into_iter().chain(code.iter().copied().filter(|&(s, e)| chars[s..e].contains(&'\n'))).collect();
    let in_code = |i: usize| code.iter().any(|&(s, e)| s <= i && i < e);
    // The code block that holds all of a line.
    let block_of = |from: usize, to: usize| blocks.iter().position(|&(s, e)| s <= from && to <= e);
    let exact = |line: &[char]| {
        line.first() != Some(&' ')
            && line.last() != Some(&' ')
            && !line.windows(2).any(|pair| pair == [' ', ' '])
            && !line.iter().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    };
    // Each line's character range.
    let mut lines = Vec::new();
    let mut from = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            lines.push((from, i));
            from = i + 1;
        }
    }
    lines.push((from, chars.len()));
    let paragraphs = lines.len() > 1;
    let mut html = String::from("<meta charset=\"utf-8\">");
    let mut open_block = None;
    for (n, &(from, to)) in lines.iter().enumerate() {
        let block = block_of(from, to);
        if block.is_some() {
            html.push_str(if open_block == block { "\n" } else { "<pre><code>" });
            chars[from..to].iter().for_each(|c| escape(*c, &mut html));
            if lines.get(n + 1).and_then(|&(from, to)| block_of(from, to)) != block {
                html.push_str("</code></pre>");
            }
            open_block = block;
            continue;
        }
        open_block = None;
        if !exact(&chars[from..to]) {
            return None;
        }
        html.push_str(if paragraphs { "<p>" } else { "" });
        let mut open = false;
        for (i, c) in chars.iter().enumerate().take(to).skip(from) {
            if in_code(i) != open {
                open = !open;
                html.push_str(if open { "<code>" } else { "</code>" });
            }
            escape(*c, &mut html);
        }
        if open {
            html.push_str("</code>");
        }
        if paragraphs {
            html.push_str(if from == to { "<br></p>" } else { "</p>" });
        }
    }
    Some(html)
}

fn escape(c: char, html: &mut String) {
    match c {
        '&' => html.push_str("&amp;"),
        '<' => html.push_str("&lt;"),
        '>' => html.push_str("&gt;"),
        '"' => html.push_str("&quot;"),
        c => html.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = "<meta charset=\"utf-8\">";

    /// The character range of `part` in `text`.
    fn range(text: &str, part: &str) -> (usize, usize) {
        let start = text[..text.find(part).unwrap()].chars().count();
        (start, start + part.chars().count())
    }

    #[test]
    fn a_code_block_stays_one_block_with_its_indentation() {
        let block = "def get_users(db):\n    for usr in db:\n\n        print(f\"<{usr}>\")";
        let text = format!("Here’s the script:\n{block}\nDon’t copy it yet.");
        assert_eq!(
            with_code(&text, &[range(&text, block)], &[]).unwrap(),
            format!(
                "{META}<p>Here’s the script:</p><pre><code>def get_users(db):\n    for usr in db:\n\n        \
                 print(f&quot;&lt;{{usr}}&gt;&quot;)</code></pre><p>Don’t copy it yet.</p>"
            )
        );
    }

    #[test]
    fn a_blocks_last_line_break_may_be_counted_in_its_range() {
        let text = "Run it:\nnpm ci\nnpm test\nDone.";
        let (start, end) = range(text, "npm ci\nnpm test");
        let want = format!("{META}<p>Run it:</p><pre><code>npm ci\nnpm test</code></pre><p>Done.</p>");
        assert_eq!(with_code(text, &[(start, end + 1)], &[]).unwrap(), want);
        assert_eq!(with_code(text, &[(start - 1, end)], &[]).unwrap(), want);
    }

    #[test]
    fn two_blocks_in_a_row_stay_two() {
        let text = "a = 1\nb = 2\nc = 3\nd = 4";
        assert_eq!(
            with_code(text, &[range(text, "a = 1\nb = 2"), range(text, "c = 3\nd = 4")], &[]).unwrap(),
            format!("{META}<pre><code>a = 1\nb = 2</code></pre><pre><code>c = 3\nd = 4</code></pre>")
        );
    }

    #[test]
    fn a_block_of_one_line_is_a_block_when_the_app_says_so() {
        let text = "Run it:\nnpm ci\nDone.";
        let code = [range(text, "npm ci")];
        assert_eq!(
            with_code(text, &code, &code).unwrap(),
            format!("{META}<p>Run it:</p><pre><code>npm ci</code></pre><p>Done.</p>")
        );
        assert_eq!(
            with_code(text, &code, &[]).unwrap(),
            format!("{META}<p>Run it:</p><p><code>npm ci</code></p><p>Done.</p>")
        );
    }

    #[test]
    fn inline_code_stays_in_its_paragraph() {
        let text = "I checked config.yaml twice.\n\nThen ran make.";
        assert_eq!(
            with_code(text, &[range(text, "config.yaml"), range(text, "make")], &[]).unwrap(),
            format!(
                "{META}<p>I checked <code>config.yaml</code> twice.</p><p><br></p><p>Then ran <code>make</code>.</p>"
            )
        );
        assert_eq!(with_code("run make now", &[(4, 8)], &[]).unwrap(), format!("{META}run <code>make</code> now"));
    }

    #[test]
    fn text_html_would_change_isnt_made_into_html() {
        for lossy in ["two  spaces", " leading", "trailing ", "a\tb", "one\n two"] {
            assert_eq!(with_code(lossy, &[], &[]), None, "{lossy:?}");
        }
        // Inside a code block every space is kept.
        let text = "x:\n  a  b \n\tc";
        assert!(with_code(text, &[range(text, "  a  b \n\tc")], &[]).is_some());
    }
}
