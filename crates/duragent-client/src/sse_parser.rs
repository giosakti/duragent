//! Shared SSE (Server-Sent Events) line parser.
//!
//! Provides a common stream adapter that handles:
//! - Byte buffering and UTF-8 conversion
//! - Line splitting (handles both `\n` and `\r\n`)
//! - Field parsing (`data:`, `event:`, `id:`, `retry:`)
//! - Event assembly (multi-line `data:` until blank line)
//!
//! Providers use the parsed SSE events and then decode JSON according
//! to their own format.
//!
//! # Design Decision
//!
//! This is a custom SSE parser rather than using crates like `eventsource-stream`
//! or `reqwest-eventsource` for the following reasons:
//!
//! - **Minimal dependencies**: Keeps the binary size small for edge deployment
//! - **Provider-specific handling**: Different LLM providers have quirks in their
//!   SSE implementations that are easier to handle with direct control
//! - **No reconnection logic needed**: We handle connection lifecycle at a higher
//!   level, so we don't need the reconnection features those crates provide

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;

/// An SSE line extracted from the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum SseLine {
    /// A `data:` line with the payload (prefix stripped).
    Data(String),
    /// An `event:` line with the event type.
    Event(String),
    /// An `id:` line with the event ID.
    Id(String),
    /// A `retry:` line with reconnection time (ms).
    Retry(u64),
    /// An empty line (event boundary in SSE).
    Empty,
    /// A comment line (starts with `:`).
    Comment(String),
}

/// A parsed SSE event (assembled from one or more lines).
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub data: String,
    pub event: Option<String>,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

fn parse_line(line: &str) -> SseLine {
    if line.is_empty() {
        return SseLine::Empty;
    }

    if let Some(data) = line.strip_prefix("data:") {
        let data = data.strip_prefix(' ').unwrap_or(data);
        return SseLine::Data(data.to_string());
    }

    if let Some(event) = line.strip_prefix("event:") {
        let event = event.strip_prefix(' ').unwrap_or(event);
        return SseLine::Event(event.to_string());
    }

    if let Some(id) = line.strip_prefix("id:") {
        let id = id.strip_prefix(' ').unwrap_or(id);
        return SseLine::Id(id.to_string());
    }

    if let Some(retry) = line.strip_prefix("retry:") {
        let retry = retry.strip_prefix(' ').unwrap_or(retry).trim();
        if let Ok(value) = retry.parse::<u64>() {
            return SseLine::Retry(value);
        }
        return SseLine::Comment(line.to_string());
    }

    if let Some(comment) = line.strip_prefix(':') {
        let comment = comment.strip_prefix(' ').unwrap_or(comment);
        return SseLine::Comment(comment.to_string());
    }

    // Unknown field, treat as comment
    SseLine::Comment(line.to_string())
}

/// A stream adapter that parses SSE lines from a byte stream.
///
/// Handles buffering, UTF-8 conversion, and line splitting. Yields
/// `SseLine` values for each meaningful line in the SSE stream.
pub struct SseLineStream<S> {
    inner: S,
    buffer: String,
    done: bool,
}

impl<S> SseLineStream<S> {
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: String::new(),
            done: false,
        }
    }
}

impl<S, E> Stream for SseLineStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<SseLine, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        loop {
            // Try to extract a complete line from buffer
            // Handle both \n and \r\n line endings
            if let Some(line_end) = self.buffer.find('\n') {
                let mut line = self.buffer[..line_end].to_string();
                self.buffer = self.buffer[line_end + 1..].to_string();

                // Strip trailing \r if present (for \r\n endings)
                if line.ends_with('\r') {
                    line.pop();
                }

                let sse_line = parse_line(&line);

                return Poll::Ready(Some(Ok(sse_line)));
            }

            // Need more data from the underlying stream
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(text);
                    }
                    // Continue loop to try parsing
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    self.done = true;
                    // Process any remaining buffer content
                    if !self.buffer.is_empty() {
                        let line = std::mem::take(&mut self.buffer);
                        let sse_line = parse_line(&line);
                        return Poll::Ready(Some(Ok(sse_line)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[derive(Default)]
struct EventBuilder {
    data_lines: Vec<String>,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
}

impl EventBuilder {
    fn push_line(&mut self, line: SseLine) {
        match line {
            SseLine::Data(data) => self.data_lines.push(data),
            SseLine::Event(event) => self.event = Some(event),
            SseLine::Id(id) => self.id = Some(id),
            SseLine::Retry(retry) => self.retry = Some(retry),
            SseLine::Empty | SseLine::Comment(_) => {}
        }
    }

    fn has_content(&self) -> bool {
        !self.data_lines.is_empty()
            || self.event.is_some()
            || self.id.is_some()
            || self.retry.is_some()
    }

    fn build(&mut self) -> SseEvent {
        let data = self.data_lines.join("\n");
        let event = self.event.take();
        let id = self.id.take();
        let retry = self.retry.take();
        self.data_lines.clear();
        SseEvent {
            data,
            event,
            id,
            retry,
        }
    }
}

/// A stream adapter that emits assembled SSE events.
pub struct SseEventStream<S> {
    inner: SseLineStream<S>,
    builder: EventBuilder,
    done: bool,
}

impl<S> SseEventStream<S> {
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self {
            inner: SseLineStream::new(inner),
            builder: EventBuilder::default(),
            done: false,
        }
    }
}

impl<S, E> Stream for SseEventStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<SseEvent, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(line))) => match line {
                    SseLine::Empty => {
                        if self.builder.has_content() {
                            return Poll::Ready(Some(Ok(self.builder.build())));
                        }
                    }
                    SseLine::Comment(_) => {}
                    other => self.builder.push_line(other),
                },
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    self.done = true;
                    if self.builder.has_content() {
                        return Poll::Ready(Some(Ok(self.builder.build())));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn bytes_stream(
        chunks: Vec<&str>,
    ) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>> {
        futures::stream::iter(chunks.into_iter().map(|s| Ok(Bytes::from(s.to_string()))))
    }

    #[tokio::test]
    async fn parses_data_lines() {
        let stream = bytes_stream(vec!["data: hello\n", "data: world\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("hello".to_string())
        );
        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("world".to_string())
        );
        assert!(sse.next().await.is_none());
    }

    #[tokio::test]
    async fn handles_crlf_line_endings() {
        let stream = bytes_stream(vec!["data: test\r\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("test".to_string())
        );
    }

    #[tokio::test]
    async fn parses_event_lines() {
        let stream = bytes_stream(vec!["event: message\ndata: content\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Event("message".to_string())
        );
        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("content".to_string())
        );
    }

    #[tokio::test]
    async fn handles_empty_lines() {
        let stream = bytes_stream(vec!["data: first\n\ndata: second\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("first".to_string())
        );
        assert_eq!(sse.next().await.unwrap().unwrap(), SseLine::Empty);
        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("second".to_string())
        );
    }

    #[tokio::test]
    async fn handles_chunked_data() {
        // Data split across multiple chunks
        let stream = bytes_stream(vec!["dat", "a: hel", "lo\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("hello".to_string())
        );
    }

    #[tokio::test]
    async fn handles_comments() {
        let stream = bytes_stream(vec![": this is a comment\ndata: value\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Comment("this is a comment".to_string())
        );
        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("value".to_string())
        );
    }

    #[tokio::test]
    async fn data_without_space_after_colon() {
        let stream = bytes_stream(vec!["data:no-space\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("no-space".to_string())
        );
    }

    #[tokio::test]
    async fn aggregates_multiline_data_events() {
        let stream = bytes_stream(vec!["data: hello\n", "data: world\n", "\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.data, "hello\nworld");
        assert!(event.event.is_none());
    }

    #[tokio::test]
    async fn captures_event_metadata() {
        let stream = bytes_stream(vec![
            "event: message\n",
            "id: 7\n",
            "retry: 1500\n",
            "data: payload\n",
            "\n",
        ]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.event, Some("message".to_string()));
        assert_eq!(event.id, Some("7".to_string()));
        assert_eq!(event.retry, Some(1500));
        assert_eq!(event.data, "payload");
    }

    #[tokio::test]
    async fn emits_event_on_eof_without_trailing_blank_line() {
        let stream = bytes_stream(vec!["data: final\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.data, "final");
        assert!(events.next().await.is_none());
    }

    // ==========================================================================
    // parse_line() - Edge cases
    // ==========================================================================

    #[test]
    fn parse_line_id_without_space() {
        assert_eq!(parse_line("id:123"), SseLine::Id("123".to_string()));
    }

    #[test]
    fn parse_line_event_without_space() {
        assert_eq!(
            parse_line("event:update"),
            SseLine::Event("update".to_string())
        );
    }

    #[test]
    fn parse_line_retry_without_space() {
        assert_eq!(parse_line("retry:3000"), SseLine::Retry(3000));
    }

    #[test]
    fn parse_line_invalid_retry_becomes_comment() {
        // Non-numeric retry value should become a comment
        assert_eq!(
            parse_line("retry: abc"),
            SseLine::Comment("retry: abc".to_string())
        );
    }

    #[test]
    fn parse_line_retry_with_whitespace() {
        // Retry should handle extra whitespace
        assert_eq!(parse_line("retry:  5000  "), SseLine::Retry(5000));
    }

    #[test]
    fn parse_line_unknown_field_becomes_comment() {
        assert_eq!(
            parse_line("unknown: value"),
            SseLine::Comment("unknown: value".to_string())
        );
    }

    #[test]
    fn parse_line_comment_without_space() {
        assert_eq!(
            parse_line(":keepalive"),
            SseLine::Comment("keepalive".to_string())
        );
    }

    // ==========================================================================
    // SseLineStream - Edge cases
    // ==========================================================================

    #[tokio::test]
    async fn handles_remaining_buffer_on_eof() {
        // Data without trailing newline
        let stream = bytes_stream(vec!["data: incomplete"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(
            sse.next().await.unwrap().unwrap(),
            SseLine::Data("incomplete".to_string())
        );
        assert!(sse.next().await.is_none());
    }

    #[tokio::test]
    async fn handles_empty_stream() {
        let stream = bytes_stream(vec![]);
        let mut sse = SseLineStream::new(stream);

        assert!(sse.next().await.is_none());
    }

    #[tokio::test]
    async fn handles_only_newlines() {
        let stream = bytes_stream(vec!["\n\n\n"]);
        let mut sse = SseLineStream::new(stream);

        assert_eq!(sse.next().await.unwrap().unwrap(), SseLine::Empty);
        assert_eq!(sse.next().await.unwrap().unwrap(), SseLine::Empty);
        assert_eq!(sse.next().await.unwrap().unwrap(), SseLine::Empty);
        assert!(sse.next().await.is_none());
    }

    // ==========================================================================
    // SseEventStream - Edge cases
    // ==========================================================================

    #[tokio::test]
    async fn multiple_events_in_stream() {
        let stream = bytes_stream(vec![
            "data: first\n",
            "\n",
            "data: second\n",
            "\n",
            "data: third\n",
            "\n",
        ]);
        let mut events = SseEventStream::new(stream);

        let e1 = events.next().await.unwrap().unwrap();
        assert_eq!(e1.data, "first");

        let e2 = events.next().await.unwrap().unwrap();
        assert_eq!(e2.data, "second");

        let e3 = events.next().await.unwrap().unwrap();
        assert_eq!(e3.data, "third");

        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn comments_ignored_in_event_assembly() {
        let stream = bytes_stream(vec![
            ": this is a comment\n",
            "data: value\n",
            ": another comment\n",
            "\n",
        ]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.data, "value");
    }

    #[tokio::test]
    async fn empty_lines_without_data_skipped() {
        let stream = bytes_stream(vec!["\n", "\n", "data: payload\n", "\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.data, "payload");
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn event_only_without_data() {
        let stream = bytes_stream(vec!["event: ping\n", "\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.event, Some("ping".to_string()));
        assert_eq!(event.data, ""); // Empty data when only event type present
    }

    #[tokio::test]
    async fn id_only_event() {
        let stream = bytes_stream(vec!["id: 42\n", "\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.id, Some("42".to_string()));
        assert_eq!(event.data, "");
    }

    #[tokio::test]
    async fn retry_only_event() {
        let stream = bytes_stream(vec!["retry: 5000\n", "\n"]);
        let mut events = SseEventStream::new(stream);

        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.retry, Some(5000));
        assert_eq!(event.data, "");
    }

    #[tokio::test]
    async fn event_stream_empty_input() {
        let stream = bytes_stream(vec![]);
        let mut events = SseEventStream::new(stream);

        assert!(events.next().await.is_none());
    }

    // ==========================================================================
    // SseLine and SseEvent - Debug/Clone/PartialEq
    // ==========================================================================

    #[test]
    fn sse_line_debug_and_clone() {
        let line = SseLine::Data("test".to_string());
        let cloned = line.clone();
        assert_eq!(line, cloned);
        let _ = format!("{:?}", line);
    }

    #[test]
    fn sse_event_debug_and_clone() {
        let event = SseEvent {
            data: "test".to_string(),
            event: Some("message".to_string()),
            id: Some("1".to_string()),
            retry: Some(1000),
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
        let _ = format!("{:?}", event);
    }
}
