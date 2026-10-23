use std::{borrow::Cow, path::Path};

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, value::RawValue};

use crate::{Error, Result};

/// Owned or `'static` text: string literals are stored without allocating.
pub type Text = Cow<'static, str>;

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Instructions for the model.
    System,
    /// The user.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

/// One piece of message content.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Part {
    /// Text.
    Text(Text),
    /// An image (user messages and, on the Anthropic protocol, tool results).
    Image(Image),
    /// A content block sent as-is, for anything not modelled here (PDF documents, audio, …).
    /// You write it in the wire format of the protocol you use.
    Raw(Value),
}

impl From<&'static str> for Part {
    fn from(s: &'static str) -> Self {
        Part::Text(s.into())
    }
}

impl From<String> for Part {
    fn from(s: String) -> Self {
        Part::Text(s.into())
    }
}

impl From<Image> for Part {
    fn from(i: Image) -> Self {
        Part::Image(i)
    }
}

/// An image input: an `https://` URL or a `data:` URL holding base64 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// `https://…` or `data:<media type>;base64,<data>`.
    pub url: Text,
}

impl Image {
    /// Image at a URL the provider fetches itself (`https://…`), or a ready `data:` URL.
    pub fn url(url: impl Into<Text>) -> Self {
        Self { url: url.into() }
    }

    /// Image from base64 data you already have, e.g. `Image::base64("image/png", b64)`.
    pub fn base64(media_type: &str, data: &str) -> Self {
        Self { url: format!("data:{media_type};base64,{data}").into() }
    }

    /// Image from raw bytes, e.g. `Image::bytes("image/jpeg", &jpeg)`.
    pub fn bytes(media_type: &str, bytes: &[u8]) -> Self {
        let mut url = format!("data:{media_type};base64,");
        base64_into(bytes, &mut url, STD);
        Self { url: url.into() }
    }

    /// Image from a file; the media type comes from the extension (png, jpg/jpeg, gif, webp).
    /// A missing file or an unknown extension is an [`Error::Config`](crate::Error::Config).
    pub fn file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let media_type = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return Err(Error::Config(format!("{}: unknown image type, use Image::bytes", path.display()))),
        };
        let bytes = std::fs::read(path).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        Ok(Self::bytes(media_type, &bytes))
    }

    /// `(media_type, base64 data)` when this is a base64 `data:` URL.
    pub(crate) fn as_base64(&self) -> Option<(&str, &str)> {
        let rest = self.url.strip_prefix("data:")?;
        let (meta, data) = rest.split_once(',')?;
        Some((meta.strip_suffix(";base64")?, data))
    }
}

pub(crate) const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
#[cfg(feature = "mcp")]
pub(crate) const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64-encodes `bytes` onto `out`; `=` padding only with the standard alphabet.
pub(crate) fn base64_into(bytes: &[u8], out: &mut String, alphabet: &[u8; 64]) {
    out.reserve(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..=c.len() {
            out.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        if alphabet == STD {
            out.extend(std::iter::repeat_n('=', 3 - c.len()));
        }
    }
}

/// A reasoning block the model wrote. Kept on the assistant turn so it can be sent back:
/// the Anthropic protocol requires that in tool loops with thinking on, and DeepSeek, Moonshot
/// and Z.ai require it on the OpenAI protocol.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thinking {
    /// Reasoning text (may be empty when the provider hides it).
    pub text: String,
    /// Opaque provider signature (Anthropic `signature`, or `data` of a redacted block).
    pub signature: String,
    /// Anthropic `redacted_thinking` block: the content is encrypted in `signature`.
    pub redacted: bool,
}

/// One conversation turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// Author.
    pub role: Role,
    /// Content: usually one text part.
    pub content: Vec<Part>,
    /// Tool calls the assistant made in this turn.
    pub tool_calls: Vec<ToolCall>,
    /// For [`Role::Tool`]: the call this result answers.
    pub tool_call_id: Option<String>,
    /// Reasoning blocks of an assistant turn, sent back as the protocol requires.
    pub thinking: Vec<Thinking>,
    /// Put a prompt-cache breakpoint at the end of this message (Anthropic protocol).
    pub cache: bool,
}

impl Message {
    fn new(role: Role, content: impl Into<Text>) -> Self {
        let content = content.into();
        let content = if content.is_empty() { Vec::new() } else { vec![Part::Text(content)] };
        Self { role, content, tool_calls: Vec::new(), tool_call_id: None, thinking: Vec::new(), cache: false }
    }
    /// System message.
    pub fn system(content: impl Into<Text>) -> Self {
        Self::new(Role::System, content)
    }
    /// User message.
    pub fn user(content: impl Into<Text>) -> Self {
        Self::new(Role::User, content)
    }
    /// Assistant message.
    pub fn assistant(content: impl Into<Text>) -> Self {
        Self::new(Role::Assistant, content)
    }
    /// Result of tool call `call_id`.
    pub fn tool(call_id: impl Into<String>, content: impl Into<Text>) -> Self {
        Self { tool_call_id: Some(call_id.into()), ..Self::new(Role::Tool, content) }
    }
    /// Append an image.
    pub fn image(mut self, image: Image) -> Self {
        self.content.push(Part::Image(image));
        self
    }
    /// Append any part.
    pub fn part(mut self, part: impl Into<Part>) -> Self {
        self.content.push(part.into());
        self
    }
    /// Mark a prompt-cache breakpoint after this message (Anthropic protocol).
    pub fn cache(mut self) -> Self {
        self.cache = true;
        self
    }
    /// All text parts joined.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for p in &self.content {
            if let Part::Text(t) = p {
                s.push_str(t);
            }
        }
        s
    }
    /// The only part, if it is text: lets protocols send plain string content.
    pub(crate) fn single_text(&self) -> Option<&str> {
        match self.content.as_slice() {
            [Part::Text(t)] => Some(t),
            [] => Some(""),
            _ => None,
        }
    }
}

/// A function the model may call.
#[derive(Clone, Debug)]
pub struct Tool {
    /// Function name.
    pub name: Text,
    /// What the function does; the model reads this.
    pub description: Text,
    /// JSON Schema of the arguments, serialized once up front.
    pub parameters: Box<RawValue>,
}

impl Tool {
    /// New tool. `parameters` is a JSON Schema object, usually written with [`json!`](crate::json).
    pub fn new(name: impl Into<Text>, description: impl Into<Text>, parameters: Value) -> Self {
        let parameters = serde_json::value::to_raw_value(&parameters).expect("a Value always serializes");
        Self { name: name.into(), description: description.into(), parameters }
    }
}

/// A tool call made by the model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCall {
    /// Call id, echoed back in the tool result.
    pub id: String,
    /// Function name.
    pub name: String,
    /// Arguments as a raw JSON string.
    pub arguments: String,
}

impl ToolCall {
    /// Parse the arguments into your own type.
    pub fn args<T: DeserializeOwned>(&self) -> Result<T> {
        let raw = if self.arguments.trim().is_empty() { "{}" } else { &self.arguments };
        Ok(serde_json::from_str(raw)?)
    }
}

/// How the model may use tools.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ToolChoice {
    /// Model decides.
    #[default]
    Auto,
    /// No tools.
    None,
    /// Must call some tool.
    Required,
    /// Must call this tool.
    Tool(Text),
}

/// Prompt caching on the Anthropic protocol. OpenAI-style providers cache long prompts on
/// their own; this setting does nothing there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cache {
    /// No cache breakpoints (the provider may still cache on its own).
    #[default]
    Off,
    /// Breakpoints after the tools/system prompt and after the last message, kept 5 minutes.
    FiveMinutes,
    /// Same breakpoints, kept 1 hour (costs more to write).
    OneHour,
}

/// A chat request. Build it with chained calls; every field is public too.
#[derive(Clone, Debug)]
pub struct Request {
    /// Model id, passed through as-is.
    pub model: Text,
    /// Conversation so far.
    pub messages: Vec<Message>,
    /// Tools the model may call.
    pub tools: Vec<Tool>,
    /// Tool policy; provider default when `None`.
    pub tool_choice: Option<ToolChoice>,
    /// Output token limit. The Anthropic protocol requires one and uses 4096 when unset.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling.
    pub top_p: Option<f32>,
    /// Stop sequences.
    pub stop: Vec<Text>,
    /// Prompt-cache breakpoints (Anthropic protocol).
    pub cache: Cache,
    /// Extra top-level JSON fields, sent as-is (e.g. `seed`, `n`, `logprobs`, `reasoning_effort`, `thinking`).
    pub extra: Map<String, Value>,
}

impl Request {
    /// New request for `model`.
    pub fn new(model: impl Into<Text>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop: Vec::new(),
            cache: Cache::Off,
            extra: Map::new(),
        }
    }
    /// Add a system message.
    pub fn system(self, content: impl Into<Text>) -> Self {
        self.message(Message::system(content))
    }
    /// Add a user message.
    pub fn user(self, content: impl Into<Text>) -> Self {
        self.message(Message::user(content))
    }
    /// Add an image to the last user message (or start a user message if the last one is not).
    pub fn image(mut self, image: Image) -> Self {
        match self.messages.last_mut() {
            Some(m) if m.role == Role::User => m.content.push(Part::Image(image)),
            _ => self.messages.push(Message::user("").image(image)),
        }
        self
    }
    /// Add an assistant message.
    pub fn assistant(self, content: impl Into<Text>) -> Self {
        self.message(Message::assistant(content))
    }
    /// Add a tool result.
    pub fn tool_result(self, call_id: impl Into<String>, content: impl Into<Text>) -> Self {
        self.message(Message::tool(call_id, content))
    }
    /// Add any message.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }
    /// Add a tool.
    pub fn tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }
    /// Add tools.
    pub fn tools(mut self, tools: impl IntoIterator<Item = Tool>) -> Self {
        self.tools.extend(tools);
        self
    }
    /// Set the tool policy.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }
    /// Set the output token limit.
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }
    /// Set the temperature.
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }
    /// Set top-p.
    pub fn top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }
    /// Add a stop sequence.
    pub fn stop(mut self, s: impl Into<Text>) -> Self {
        self.stop.push(s.into());
        self
    }
    /// Turn on prompt-cache breakpoints (Anthropic protocol).
    pub fn cache(mut self, cache: Cache) -> Self {
        self.cache = cache;
        self
    }
    /// Set any other top-level field, e.g. `.extra("seed", 7)`.
    pub fn extra(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// Why the model stopped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum FinishReason {
    /// Not reported.
    #[default]
    Unknown,
    /// Natural end or stop sequence.
    Stop,
    /// Hit the token limit.
    Length,
    /// Wants tool results.
    ToolCalls,
    /// Blocked or refused.
    ContentFilter,
    /// Anything else, verbatim.
    Other(String),
}

impl FinishReason {
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "stop" | "end_turn" | "stop_sequence" | "pause_turn" | "completed" => Self::Stop,
            "length" | "max_tokens" | "max_output_tokens" | "model_context_window_exceeded" => Self::Length,
            "tool_calls" | "tool_use" | "function_call" => Self::ToolCalls,
            "content_filter" | "refusal" => Self::ContentFilter,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// Token counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Prompt tokens, cached ones included.
    pub input_tokens: u32,
    /// Generated tokens, reasoning included.
    pub output_tokens: u32,
    /// Prompt tokens read from the cache.
    pub cached_tokens: u32,
    /// Prompt tokens written to the cache (Anthropic protocol).
    pub cache_write_tokens: u32,
    /// Output tokens spent on reasoning, when reported.
    pub reasoning_tokens: u32,
}

/// Log probability of one generated token (OpenAI protocol, with `.extra("logprobs", true)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Logprob {
    /// The token.
    pub token: String,
    /// Its log probability.
    pub logprob: f32,
    /// Most likely alternatives at this position (`.extra("top_logprobs", n)`).
    pub top: Vec<(String, f32)>,
}

/// One extra completion choice, when you ask for more than one with `.extra("n", k)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    /// Generated text.
    pub text: String,
    /// Reasoning text.
    pub reasoning: String,
    /// Tool calls to run.
    pub tool_calls: Vec<ToolCall>,
    /// Why generation stopped.
    pub finish_reason: FinishReason,
    /// Token log probabilities, if requested.
    pub logprobs: Vec<Logprob>,
}

/// A finished chat completion.
#[derive(Clone, Debug, Default)]
pub struct Response {
    /// Response id.
    pub id: String,
    /// Model that answered.
    pub model: String,
    /// Generated text.
    pub text: String,
    /// Reasoning / thinking text, if the provider returns it.
    pub reasoning: String,
    /// Signed reasoning blocks (Anthropic protocol), kept for [`into_message`](Self::into_message).
    pub thinking: Vec<Thinking>,
    /// Tool calls to run.
    pub tool_calls: Vec<ToolCall>,
    /// Why generation stopped.
    pub finish_reason: FinishReason,
    /// Token counts.
    pub usage: Usage,
    /// Token log probabilities, if requested.
    pub logprobs: Vec<Logprob>,
    /// Choices after the first, when you asked for `n > 1`. The first choice is this response.
    pub other_choices: Vec<Choice>,
    /// Request id from the response headers; quote it to the provider when reporting a problem.
    pub request_id: Option<String>,
}

impl Response {
    /// The assistant turn, ready to push onto [`Request::messages`] for the next round.
    /// Carries the reasoning along so tool loops with thinking models keep working.
    pub fn into_message(self) -> Message {
        let thinking = if self.thinking.is_empty() && !self.reasoning.is_empty() {
            vec![Thinking { text: self.reasoning, ..Thinking::default() }]
        } else {
            self.thinking
        };
        let content = if self.text.is_empty() { Vec::new() } else { vec![Part::Text(self.text.into())] };
        Message {
            role: Role::Assistant,
            content,
            tool_calls: self.tool_calls,
            tool_call_id: None,
            thinking,
            cache: false,
        }
    }

    pub(crate) fn set_first(&mut self, c: Choice) {
        self.text = c.text;
        self.reasoning = c.reasoning;
        self.tool_calls = c.tool_calls;
        self.finish_reason = c.finish_reason;
        self.logprobs = c.logprobs;
    }
}

/// One streaming event.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// Text delta.
    Text(String),
    /// Reasoning / thinking delta.
    Reasoning(String),
    /// A tool call, emitted once its arguments are complete.
    ToolCall(ToolCall),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            let mut s = String::new();
            base64_into(raw.as_bytes(), &mut s, STD);
            assert_eq!(s, enc);
        }
        let all: Vec<u8> = (0..=255u8).cycle().take(768).collect();
        let mut s = String::new();
        base64_into(&all, &mut s, STD);
        assert_eq!(
            s,
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/wABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj9AQUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVpbXF1eX2BhYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ent8fX5/gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp+goaKjpKWmp6ipqqusra6vsLGys7S1tre4ubq7vL2+v8DBwsPExcbHyMnKy8zNzs/Q0dLT1NXW19jZ2tvc3d7f4OHi4+Tl5ufo6err7O3u7/Dx8vP09fb3+Pn6+/z9/v8AAQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyAhIiMkJSYnKCkqKywtLi8wMTIzNDU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl9gYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXp7fH1+f4CBgoOEhYaHiImKi4yNjo+QkZKTlJWWl5iZmpucnZ6foKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr/AwcLDxMXGx8jJysvMzc7P0NHS09TV1tfY2drb3N3e3+Dh4uPk5ebn6Onq6+zt7u/w8fLz9PX29/j5+vv8/f7/"
        );
        let img = Image::bytes("image/png", &[0xfb, 0xff]);
        assert_eq!(img.url, "data:image/png;base64,+/8=");
        assert_eq!(img.as_base64(), Some(("image/png", "+/8=")));
        assert_eq!(Image::url("https://x/y.png").as_base64(), None);
        assert!(Image::file("missing.png").unwrap_err().to_string().contains("missing.png"));
        assert!(Image::file("notes.txt").is_err());
    }

    #[test]
    fn image_attaches_to_last_user_message() {
        let img = Image::url("https://x/a.png");
        let r = Request::new("m").user("look").image(img.clone());
        assert_eq!(r.messages[0].content, [Part::Text("look".into()), Part::Image(img.clone())]);
        let r = Request::new("m").system("s").image(img.clone());
        assert_eq!(r.messages[1].content, [Part::Image(img)]);
    }
}
