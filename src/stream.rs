use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll, ready},
};

use futures_core::Stream;
use http_body::Body as _;

use crate::{Error, Event, Response, Result, anthropic, openai};

pub(crate) enum Decoder {
    OpenAi(openai::StreamState),
    Anthropic(anthropic::StreamState),
}

impl Decoder {
    fn data(&mut self, data: &[u8], out: &mut VecDeque<Event>) -> Result<()> {
        match self {
            Decoder::OpenAi(s) => s.data(data, out),
            Decoder::Anthropic(s) => s.data(data, out),
        }
    }

    fn finish(&mut self, out: &mut VecDeque<Event>) {
        if let Decoder::OpenAi(s) = self {
            s.finish(out);
        }
    }

    fn into_response(self) -> Response {
        match self {
            Decoder::OpenAi(s) => s.res,
            Decoder::Anthropic(s) => s.res,
        }
    }
}

/// A streaming chat response.
///
/// Read events with [`next`](ChatStream::next) (or as a [`futures_core::Stream`]),
/// then call [`response`](ChatStream::response) for the assembled result.
pub struct ChatStream {
    body: reqwest::Body,
    /// Bytes of an unfinished line carried over between network frames.
    partial: Vec<u8>,
    decoder: Decoder,
    queue: VecDeque<Event>,
    error: Option<Error>,
    done: bool,
}

impl ChatStream {
    pub(crate) fn new(res: reqwest::Response, decoder: Decoder) -> Self {
        Self { body: res.into(), partial: Vec::new(), decoder, queue: VecDeque::new(), error: None, done: false }
    }

    /// Next event, or `None` at the end of the stream.
    pub async fn next(&mut self) -> Option<Result<Event>> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }

    /// Drain the rest of the stream and return everything it carried: text, tool calls, usage.
    pub async fn response(mut self) -> Result<Response> {
        while let Some(ev) = self.next().await {
            ev?;
        }
        Ok(self.decoder.into_response())
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<()> {
        let (decoder, queue) = (&mut self.decoder, &mut self.queue);
        let mut on_data = |d: &[u8]| decoder.data(d, queue);
        if self.partial.is_empty() {
            // Fast path: parse straight out of the network buffer, keep only the tail.
            let used = data_lines(chunk, &mut on_data)?;
            self.partial.extend_from_slice(&chunk[used..]);
        } else {
            self.partial.extend_from_slice(chunk);
            let used = data_lines(&self.partial, &mut on_data)?;
            self.partial.drain(..used);
        }
        Ok(())
    }
}

impl Stream for ChatStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if let Some(ev) = this.queue.pop_front() {
                return Poll::Ready(Some(Ok(ev)));
            }
            if let Some(e) = this.error.take() {
                return Poll::Ready(Some(Err(e)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        if let Err(e) = this.feed(&data) {
                            this.error = Some(e);
                            this.done = true;
                        }
                    }
                }
                Some(Err(e)) => {
                    this.error = Some(e.into());
                    this.done = true;
                }
                None => {
                    if !this.partial.is_empty() {
                        this.partial.push(b'\n');
                        let tail = std::mem::take(&mut this.partial);
                        if let Err(e) = this.feed(&tail) {
                            this.error = Some(e);
                        }
                    }
                    this.decoder.finish(&mut this.queue);
                    this.done = true;
                }
            }
        }
    }
}

/// Calls `f` with the payload of every complete `data:` line in `buf`.
/// Returns how many bytes were consumed (up to the last newline).
// ponytail: each data line is one event; multi-line `data:` fields are not joined (no LLM API sends them).
pub(crate) fn data_lines(buf: &[u8], f: &mut impl FnMut(&[u8]) -> Result<()>) -> Result<usize> {
    let mut start = 0;
    while let Some(n) = buf[start..].iter().position(|&b| b == b'\n') {
        let line = &buf[start..start + n];
        start += n + 1;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(d) = line.strip_prefix(b"data:") {
            let d = d.strip_prefix(b" ").unwrap_or(d);
            if !d.is_empty() && d != b"[DONE]" {
                f(d)?;
            }
        }
    }
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FinishReason, ToolCall};

    /// Feeds `sse` split at every possible byte position, like a hostile network would.
    fn run(sse: &str, mk: fn() -> Decoder) -> (Vec<Event>, Response) {
        let mut first = None;
        for step in 1..=sse.len() {
            let mut s = ChatStream {
                body: reqwest::Body::from(""),
                partial: Vec::new(),
                decoder: mk(),
                queue: VecDeque::new(),
                error: None,
                done: false,
            };
            for chunk in sse.as_bytes().chunks(step) {
                s.feed(chunk).unwrap();
            }
            s.decoder.finish(&mut s.queue);
            let got = (s.queue.into_iter().collect::<Vec<_>>(), s.decoder.into_response());
            let first = first.get_or_insert_with(|| got.clone());
            assert_eq!(first.0, got.0, "split at {step}");
        }
        first.unwrap()
    }

    #[test]
    fn openai_sse() {
        let sse = concat!(
            "data: {\"id\":\"c1\",\"model\":\"m\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n",
            ": keep-alive\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"t\",\"type\":\"function\",\"function\":{\"name\":\"f\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"a\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );
        let (events, res) = run(sse, || Decoder::OpenAi(Default::default()));
        let call = ToolCall { id: "t".into(), name: "f".into(), arguments: "{\"a\":1}".into() };
        assert_eq!(events, [Event::Text("Hel".into()), Event::Text("lo".into()), Event::ToolCall(call.clone())]);
        assert_eq!((res.id.as_str(), res.text.as_str()), ("c1", "Hello"));
        assert_eq!(res.tool_calls, [call]);
        assert_eq!(res.finish_reason, FinishReason::ToolCalls);
        assert_eq!(res.usage.output_tokens, 3);
    }

    #[test]
    fn anthropic_sse() {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"c\",\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hm\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: ping\ndata: {\"type\": \"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi \\u00e9\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"f\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\":1}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":12}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let (events, res) = run(sse, || Decoder::Anthropic(Default::default()));
        let call = ToolCall { id: "t".into(), name: "f".into(), arguments: "{\"a\":1}".into() };
        assert_eq!(events, [Event::Reasoning("hm".into()), Event::Text("Hi é".into()), Event::ToolCall(call.clone())]);
        assert_eq!(res.tool_calls, [call]);
        assert_eq!(res.finish_reason, FinishReason::ToolCalls);
        assert_eq!((res.usage.input_tokens, res.usage.output_tokens), (7, 12));
    }

    #[test]
    fn stream_error_event() {
        let mut d = Decoder::Anthropic(Default::default());
        let err = d
            .data(
                br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                &mut VecDeque::new(),
            )
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 0, ref message } if message == "Overloaded"));
    }
}
