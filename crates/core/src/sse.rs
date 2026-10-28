//! Minimal incremental parsers for server-sent events and newline-delimited JSON.
//! Both accept arbitrary chunk boundaries, including splits inside UTF-8 sequences.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, sep)) = find_boundary(&self.buffer) {
            let raw: Vec<u8> = self.buffer.drain(..end + sep).collect();
            let text = String::from_utf8_lossy(&raw[..end]);
            if let Some(event) = parse_event(&text) {
                events.push(event);
            }
        }
        events
    }
}

fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
    }
    None
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

#[derive(Default)]
pub struct LineParser {
    buffer: Vec<u8>,
}

impl LineParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw).trim().to_string();
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    pub fn finish(&mut self) -> Option<String> {
        let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).trim().to_string();
        (!line.is_empty()).then_some(line)
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
            events.extend(parser.push(chunk));
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(events[0].data, "{\"text\":\"caf\u{e9}\"}");
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn handles_crlf_and_comments() {
        let mut parser = SseParser::default();
        let events = parser.push(b": keepalive\r\n\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(events, vec![SseEvent { event: None, data: "a\nb".into() }]);
    }

    #[test]
    fn line_parser_splits_ndjson() {
        let mut parser = LineParser::default();
        assert_eq!(parser.push(b"{\"a\":1}\n{\"b\""), vec!["{\"a\":1}"]);
        assert_eq!(parser.push(b":2}\n"), vec!["{\"b\":2}"]);
        assert_eq!(parser.finish(), None);
    }
}
