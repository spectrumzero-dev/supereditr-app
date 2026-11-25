//! Minimal incremental parsers for server-sent events and newline-delimited JSON.
//! Both accept arbitrary chunk boundaries, including splits inside UTF-8 sequences.

// The provider fixtures in tests/providers/http.rs have SSE events up to 124 bytes.
// A synthetic 75,599-byte multilingual revision sent as one Gemini event takes 77,478 bytes.
// Reviews request up to 16,000 output tokens. Leave room for coalesced text, JSON escapes and
// provider metadata, including responses from compatible servers with their own output settings.
const SSE_EVENT_LIMIT: usize = 512 * 1024;

// Ollama chat fixture lines reach 44 bytes, and a representative pull progress line is 156 bytes.
// Claude CLI and the Apple helper can put a whole revision in one line. Synthetic final records
// take 77,517 bytes for Claude and 9,705 bytes for Apple (which requests at most 2,000 tokens).
// Leave extra room for CLI results and metadata since the CLI doesn't take our output token cap.
const LINE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The {frame} exceeded the {limit}-byte limit for a single response frame.")]
pub struct FrameTooLarge {
    pub frame: &'static str,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub struct SseParser(Framer);

impl Default for SseParser {
    fn default() -> Self {
        Self::with_limit(SSE_EVENT_LIMIT)
    }
}

impl SseParser {
    /// Sets the raw byte budget per event, excluding its terminating blank line.
    pub fn with_limit(limit: usize) -> Self {
        Self(Framer::new(Format::Sse, limit))
    }

    /// Consume the iterator before pushing the next chunk. A frame error ends parsing.
    pub fn push<'a>(&'a mut self, chunk: &'a [u8]) -> impl Iterator<Item = Result<SseEvent, FrameTooLarge>> + 'a {
        self.0.push(chunk, |raw| parse_event(&String::from_utf8_lossy(raw)))
    }
}

fn parse_event(text: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        } else if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        }
    }
    if data.is_empty() && event.is_none() {
        return None;
    }
    Some(SseEvent { event, data: data.join("\n") })
}

pub struct LineParser(Framer);

impl Default for LineParser {
    fn default() -> Self {
        Self::with_limit(LINE_LIMIT)
    }
}

impl LineParser {
    /// Sets the raw byte budget per line, excluding LF and an optional terminating CR.
    pub fn with_limit(limit: usize) -> Self {
        Self(Framer::new(Format::Line, limit))
    }

    /// Consume the iterator before pushing the next chunk. A frame error ends parsing.
    pub fn push<'a>(&'a mut self, chunk: &'a [u8]) -> impl Iterator<Item = Result<String, FrameTooLarge>> + 'a {
        self.0.push(chunk, parse_line)
    }

    pub fn finish(&mut self) -> Option<String> {
        parse_line(&std::mem::take(&mut self.0.buffer))
    }
}

fn parse_line(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let line = text.trim();
    (!line.is_empty()).then(|| line.to_string())
}

#[derive(Clone, Copy)]
enum Format {
    Sse,
    Line,
}

impl Format {
    // The complete separator length and the longest partial separator ending at this byte.
    fn boundary(self, buffer: &[u8], byte: u8) -> (usize, usize) {
        match (self, byte) {
            (Self::Sse, b'\n') if buffer.ends_with(b"\r\n\r") => (4, 0),
            (Self::Sse, b'\n') if buffer.ends_with(b"\n") => (2, 0),
            (Self::Sse, b'\n') if buffer.ends_with(b"\r") => (0, 2),
            (Self::Sse, b'\n') => (0, 1),
            (Self::Sse, b'\r') if buffer.ends_with(b"\r\n") => (0, 3),
            (Self::Sse | Self::Line, b'\r') => (0, 1),
            (Self::Line, b'\n') if buffer.ends_with(b"\r") => (2, 0),
            (Self::Line, b'\n') => (1, 0),
            _ => (0, 0),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sse => "SSE event",
            Self::Line => "JSON line",
        }
    }
}

struct Framer {
    buffer: Vec<u8>,
    format: Format,
    limit: usize,
    failed: bool,
}

impl Framer {
    fn new(format: Format, limit: usize) -> Self {
        Self { buffer: Vec::new(), format, limit, failed: false }
    }

    fn push<'a, T>(
        &'a mut self,
        chunk: &'a [u8],
        parse: impl Fn(&[u8]) -> Option<T> + 'a,
    ) -> impl Iterator<Item = Result<T, FrameTooLarge>> + 'a {
        let mut bytes = chunk.iter().copied();
        std::iter::from_fn(move || {
            if self.failed {
                return None;
            }
            for byte in bytes.by_ref() {
                // Look only at the new byte and at most three saved suffix bytes.
                let (separator, pending) = self.format.boundary(&self.buffer, byte);
                let frame_len = self.buffer.len() + 1 - separator - pending;
                if frame_len > self.limit {
                    self.buffer.clear();
                    self.failed = true;
                    return Some(Err(FrameTooLarge { frame: self.format.name(), limit: self.limit }));
                }
                if separator == 0 {
                    self.buffer.push(byte);
                } else {
                    let frame = parse(&self.buffer[..frame_len]);
                    self.buffer.clear();
                    if let Some(frame) = frame {
                        return Some(Ok(frame));
                    }
                }
            }
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_across_chunks_and_utf8_splits() {
        let mut parser = SseParser::default();
        let body = "event: content_block_delta\ndata: {\"text\":\"caf\u{e9}\"}\n\ndata: [DONE]\n\n".as_bytes();
        let mut events = Vec::new();
        for chunk in body.chunks(3) {
            events.extend(parser.push(chunk).map(Result::unwrap));
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(events[0].data, "{\"text\":\"caf\u{e9}\"}");
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn handles_crlf_and_comments() {
        let mut parser = SseParser::default();
        let events: Vec<_> =
            parser.push(b": keepalive\r\n\r\ndata: a\r\ndata: b\r\n\r\n").map(Result::unwrap).collect();
        assert_eq!(events, vec![SseEvent { event: None, data: "a\nb".into() }]);
    }

    #[test]
    fn line_parser_splits_ndjson() {
        let mut parser = LineParser::default();
        assert_eq!(parser.push(b"{\"a\":1}\n{\"b\"").map(Result::unwrap).collect::<Vec<_>>(), ["{\"a\":1}"]);
        assert_eq!(parser.push(b":2}\n").map(Result::unwrap).collect::<Vec<_>>(), ["{\"b\":2}"]);
        assert_eq!(parser.finish(), None);
    }

    #[test]
    fn sse_limit_excludes_delimiters_across_every_split() {
        let frame = "data: Café";
        for separator in ["\n\n", "\r\n\r\n"] {
            let body = format!("{frame}{separator}{frame}{separator}");
            for split in 0..=body.len() {
                let mut parser = SseParser::with_limit(frame.len());
                let (first, last) = body.as_bytes().split_at(split);
                let mut events: Vec<_> = parser.push(first).map(Result::unwrap).collect();
                events.extend(parser.push(last).map(Result::unwrap));
                assert_eq!(events, vec![SseEvent { event: None, data: "Café".into() }; 2], "split at {split}");
            }
        }
    }

    #[test]
    fn sse_reports_an_oversized_event_after_earlier_events() {
        let body = b"data: tea\n\ndata: minty\n\n";
        let error = FrameTooLarge { frame: "SSE event", limit: 10 };
        for split in 0..=body.len() {
            let mut parser = SseParser::with_limit(10);
            let (first, last) = body.split_at(split);
            let mut events: Vec<_> = parser.push(first).collect();
            events.extend(parser.push(last));
            assert_eq!(events, [Ok(SseEvent { event: None, data: "tea".into() }), Err(error)], "split at {split}");
        }
    }

    #[test]
    fn sse_counts_comments_and_internal_line_endings() {
        let mut parser = SseParser::with_limit(13);
        let body = b": note\r\ndata: a";
        assert_eq!(parser.push(body).next(), Some(Err(FrameTooLarge { frame: "SSE event", limit: 13 })));
    }

    #[test]
    fn line_limit_handles_utf8_crlf_and_unterminated_final_line() {
        let line = "\"Café\"";
        let body = format!("\n{line}\r\n{line}");
        for split in 0..=body.len() {
            let mut parser = LineParser::with_limit(line.len());
            let (first, last) = body.as_bytes().split_at(split);
            let mut lines: Vec<_> = parser.push(first).map(Result::unwrap).collect();
            lines.extend(parser.push(last).map(Result::unwrap));
            assert_eq!(lines, [line], "split at {split}");
            assert_eq!(parser.finish().as_deref(), Some(line));
            assert_eq!(parser.finish(), None);
        }
    }

    #[test]
    fn line_limit_counts_whitespace_and_stops_on_the_first_oversized_line() {
        let mut parser = LineParser::with_limit(3);
        let mut lines = parser.push(b" 1\n  42\n 3\n");
        assert_eq!(lines.next(), Some(Ok("1".into())));
        assert_eq!(lines.next(), Some(Err(FrameTooLarge { frame: "JSON line", limit: 3 })));
        assert_eq!(lines.next(), None);
        drop(lines);
        assert_eq!(parser.push(b"4\n").next(), None);
        assert_eq!(parser.finish(), None);
    }
}
