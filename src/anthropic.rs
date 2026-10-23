//! Anthropic Messages wire format.

use std::{borrow::Cow, collections::VecDeque, iter::Peekable};

use serde::{Deserialize, Serialize, Serializer, ser::Error as _, ser::SerializeMap, ser::SerializeSeq};
use serde_json::{Value, value::RawValue};

use crate::{Result, error::ApiError, openai::BaseResp, openai::Seq, types::*};

/// The Anthropic protocol requires `max_tokens`; every model accepts this much.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 4096;

pub(crate) struct ChatBody<'a> {
    pub req: &'a Request,
    pub stream: bool,
}

/// A `cache_control` marker.
#[derive(Clone, Copy)]
struct Mark(Cache);

impl Serialize for Mark {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("type", "ephemeral")?;
        if self.0 == Cache::OneHour {
            m.serialize_entry("ttl", "1h")?;
        }
        m.end()
    }
}

impl Serialize for ChatBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.req;
        let auto = (r.cache != Cache::Off).then_some(Mark(r.cache));
        let ttl = Mark(if r.cache == Cache::Off { Cache::FiveMinutes } else { r.cache });
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        m.serialize_entry("max_tokens", &r.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS))?;
        let system: Vec<&Message> = r.messages.iter().filter(|m| m.role == Role::System).collect();
        if !system.is_empty() {
            // The breakpoint on the last system block caches tools + system together.
            let n = system.len();
            let blocks = system.iter().enumerate().map(|(i, msg)| TextBlock {
                kind: "text",
                text: msg.single_text().map_or_else(|| Cow::Owned(msg.text()), Cow::Borrowed),
                cache_control: if i + 1 == n { auto.or(msg.cache.then_some(ttl)) } else { msg.cache.then_some(ttl) },
            });
            m.serialize_entry("system", &Seq(blocks))?;
        }
        m.serialize_entry("messages", &Msgs { msgs: &r.messages, auto, ttl })?;
        if !r.tools.is_empty() {
            let n = r.tools.len();
            let tools = r.tools.iter().enumerate().map(|(i, t)| ToolDef {
                name: &t.name,
                description: &t.description,
                input_schema: &t.parameters,
                cache_control: if i + 1 == n && system.is_empty() { auto } else { None },
            });
            m.serialize_entry("tools", &Seq(tools))?;
        }
        if let Some(c) = &r.tool_choice {
            let (kind, name) = match c {
                ToolChoice::Auto => ("auto", None),
                ToolChoice::None => ("none", None),
                ToolChoice::Required => ("any", None),
                ToolChoice::Tool(n) => ("tool", Some(&**n)),
            };
            m.serialize_entry("tool_choice", &ChoiceOut { kind, name })?;
        }
        if let Some(t) = r.temperature {
            m.serialize_entry("temperature", &t)?;
        }
        if let Some(p) = r.top_p {
            m.serialize_entry("top_p", &p)?;
        }
        if !r.stop.is_empty() {
            m.serialize_entry("stop_sequences", &r.stop)?;
        }
        if self.stream {
            m.serialize_entry("stream", &true)?;
        }
        for (k, v) in &r.extra {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

#[derive(Serialize)]
struct TextBlock<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<Mark>,
}

#[derive(Serialize)]
struct ToolDef<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    input_schema: &'a RawValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<Mark>,
}

#[derive(Serialize)]
struct ChoiceOut<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

/// Skips system messages (sent top-level) and merges runs of tool results into one user turn.
struct Msgs<'a> {
    msgs: &'a [Message],
    /// Breakpoint for the end of the conversation.
    auto: Option<Mark>,
    /// TTL for breakpoints the caller placed with [`Message::cache`].
    ttl: Mark,
}

impl Serialize for Msgs<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msgs = self.msgs;
        let last = msgs.iter().rposition(|m| m.role != Role::System);
        let mark = |i: usize| {
            if Some(i) == last {
                self.auto.or(msgs[i].cache.then_some(self.ttl))
            } else {
                msgs[i].cache.then_some(self.ttl)
            }
        };
        let mut seq = s.serialize_seq(None)?;
        let mut i = 0;
        while i < msgs.len() {
            match msgs[i].role {
                Role::System => i += 1,
                Role::Tool => {
                    let end = msgs[i..].iter().position(|m| m.role != Role::Tool).map_or(msgs.len(), |k| i + k);
                    let marks: Vec<Option<Mark>> = (i..end).map(mark).collect();
                    seq.serialize_element(&ToolResults { msgs: &msgs[i..end], marks })?;
                    i = end;
                }
                _ => {
                    seq.serialize_element(&Msg { msg: &msgs[i], mark: mark(i) })?;
                    i += 1;
                }
            }
        }
        seq.end()
    }
}

struct Msg<'a> {
    msg: &'a Message,
    mark: Option<Mark>,
}

impl Serialize for Msg<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.msg;
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("role", if msg.role == Role::Assistant { "assistant" } else { "user" })?;
        let plain = msg.tool_calls.is_empty() && !msg.thinking.iter().any(|t| !t.signature.is_empty());
        match msg.single_text() {
            Some(t) if plain && self.mark.is_none() => m.serialize_entry("content", t)?,
            _ => m.serialize_entry("content", &Blocks { msg, mark: self.mark })?,
        }
        m.end()
    }
}

/// A message's content blocks, in the order the API wants: thinking, content, tool calls.
struct Blocks<'a> {
    msg: &'a Message,
    mark: Option<Mark>,
}

enum B<'a> {
    Thinking(&'a Thinking),
    Part(&'a Part),
    ToolUse(&'a ToolCall),
}

impl<'a> Blocks<'a> {
    fn iter(&self) -> impl Iterator<Item = B<'a>> + Clone {
        let msg = self.msg;
        let thinking = msg.thinking.iter().filter(|t| !t.signature.is_empty()).map(B::Thinking);
        let parts = msg.content.iter().filter(|p| !matches!(p, Part::Text(t) if t.is_empty())).map(B::Part);
        thinking.chain(parts).chain(msg.tool_calls.iter().map(B::ToolUse))
    }
}

impl Serialize for Blocks<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_marked(s, self.iter().peekable(), self.mark, |b, mark| BlockOut { b, mark })
    }
}

/// Serializes a sequence, giving `mark` to the last element only.
fn serialize_marked<S: Serializer, I: Iterator, T: Serialize>(
    s: S,
    mut it: Peekable<I>,
    mark: Option<Mark>,
    f: impl Fn(I::Item, Option<Mark>) -> T,
) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(None)?;
    while let Some(x) = it.next() {
        let last = it.peek().is_none();
        seq.serialize_element(&f(x, if last { mark } else { None }))?;
    }
    seq.end()
}

struct BlockOut<'a> {
    b: B<'a>,
    mark: Option<Mark>,
}

impl Serialize for BlockOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if let B::Part(Part::Raw(v)) = self.b {
            return match (self.mark, v) {
                (Some(mark), Value::Object(o)) => {
                    let mut o = o.clone();
                    o.insert("cache_control".into(), serde_json::to_value(mark).map_err(S::Error::custom)?);
                    o.serialize(s)
                }
                _ => v.serialize(s),
            };
        }
        let mut m = s.serialize_map(None)?;
        match self.b {
            B::Thinking(t) if t.redacted => {
                m.serialize_entry("type", "redacted_thinking")?;
                m.serialize_entry("data", &t.signature)?;
            }
            B::Thinking(t) => {
                m.serialize_entry("type", "thinking")?;
                m.serialize_entry("thinking", &t.text)?;
                m.serialize_entry("signature", &t.signature)?;
            }
            B::Part(Part::Text(t)) => {
                m.serialize_entry("type", "text")?;
                m.serialize_entry("text", t)?;
            }
            B::Part(Part::Image(i)) => {
                m.serialize_entry("type", "image")?;
                m.serialize_entry("source", &ImageSource(i))?;
            }
            B::Part(Part::Raw(_)) => unreachable!("handled above"),
            B::ToolUse(c) => {
                let args = if c.arguments.trim().is_empty() { "{}" } else { &c.arguments };
                let input: &RawValue = serde_json::from_str(args)
                    .map_err(|e| S::Error::custom(format!("tool call {} has invalid JSON arguments: {e}", c.id)))?;
                m.serialize_entry("type", "tool_use")?;
                m.serialize_entry("id", &c.id)?;
                m.serialize_entry("name", &c.name)?;
                m.serialize_entry("input", input)?;
            }
        }
        if let Some(mark) = self.mark {
            m.serialize_entry("cache_control", &mark)?;
        }
        m.end()
    }
}

struct ImageSource<'a>(&'a Image);

impl Serialize for ImageSource<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(3))?;
        match self.0.as_base64() {
            Some((media_type, data)) => {
                m.serialize_entry("type", "base64")?;
                m.serialize_entry("media_type", media_type)?;
                m.serialize_entry("data", data)?;
            }
            None => {
                m.serialize_entry("type", "url")?;
                m.serialize_entry("url", &self.0.url)?;
            }
        }
        m.end()
    }
}

struct ToolResults<'a> {
    msgs: &'a [Message],
    marks: Vec<Option<Mark>>,
}

impl Serialize for ToolResults<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let blocks = self.msgs.iter().zip(&self.marks).map(|(msg, &mark)| ToolResult { msg, mark });
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("role", "user")?;
        m.serialize_entry("content", &Seq(blocks))?;
        m.end()
    }
}

#[derive(Clone, Copy)]
struct ToolResult<'a> {
    msg: &'a Message,
    mark: Option<Mark>,
}

impl Serialize for ToolResult<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.msg;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("type", "tool_result")?;
        m.serialize_entry("tool_use_id", msg.tool_call_id.as_deref().unwrap_or_default())?;
        match msg.single_text() {
            Some(t) => m.serialize_entry("content", t)?,
            None => m.serialize_entry("content", &Blocks { msg, mark: None })?,
        }
        if let Some(mark) = self.mark {
            m.serialize_entry("cache_control", &mark)?;
        }
        m.end()
    }
}

#[derive(Deserialize, Default)]
struct AUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
    output_tokens_details: Option<OutDetails>,
}

#[derive(Deserialize)]
struct OutDetails {
    thinking_tokens: Option<u32>,
    reasoning_tokens: Option<u32>,
}

impl AUsage {
    /// Anthropic reports uncached input separately; `Usage::input_tokens` counts all of it.
    fn merge_into(&self, u: &mut Usage) {
        if let Some(n) = self.input_tokens {
            let read = self.cache_read_input_tokens.unwrap_or(0);
            let write = self.cache_creation_input_tokens.unwrap_or(0);
            u.input_tokens = n + read + write;
            u.cached_tokens = read;
            u.cache_write_tokens = write;
        }
        if let Some(n) = self.output_tokens {
            u.output_tokens = n;
        }
        if let Some(n) = self.output_tokens_details.as_ref().and_then(|d| d.thinking_tokens.or(d.reasoning_tokens)) {
            u.reasoning_tokens = n;
        }
    }
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    data: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct ChatResp {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    model: Option<String>,
    content: Option<Vec<Block>>,
    stop_reason: Option<String>,
    usage: Option<AUsage>,
    error: Option<Value>,
    request_id: Option<String>,
    base_resp: Option<BaseResp>,
}

pub(crate) fn parse_chat(body: &[u8]) -> Result<Response> {
    let r: ChatResp = serde_json::from_slice(body)?;
    if r.kind.as_deref() == Some("error") || r.error.as_ref().is_some_and(|e| !e.is_null()) {
        let mut e = ApiError::from_value(r.error.as_ref().unwrap_or(&Value::Null));
        e.request_id = r.request_id;
        return Err(e.into());
    }
    if let Some(b) = &r.base_resp {
        b.check()?;
    }
    let mut out = Response {
        id: r.id.unwrap_or_default(),
        model: r.model.unwrap_or_default(),
        finish_reason: r.stop_reason.as_deref().map(FinishReason::parse).unwrap_or_default(),
        ..Response::default()
    };
    if let Some(u) = r.usage {
        u.merge_into(&mut out.usage);
    }
    for b in r.content.into_iter().flatten() {
        match b.kind.as_str() {
            "text" => out.text.push_str(b.text.as_deref().unwrap_or_default()),
            "thinking" => {
                let text = b.thinking.unwrap_or_default();
                out.reasoning.push_str(&text);
                out.thinking.push(Thinking { text, signature: b.signature.unwrap_or_default(), redacted: false });
            }
            "redacted_thinking" => out.thinking.push(Thinking {
                text: String::new(),
                signature: b.data.unwrap_or_default(),
                redacted: true,
            }),
            "tool_use" => out.tool_calls.push(ToolCall {
                id: b.id.unwrap_or_default(),
                name: b.name.unwrap_or_default(),
                arguments: b.input.map_or_else(|| "{}".to_owned(), |i| i.get().to_owned()),
            }),
            _ => {}
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct Ev<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    message: Option<StartMsg>,
    content_block: Option<Block>,
    delta: Option<Delta>,
    usage: Option<AUsage>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct StartMsg {
    id: Option<String>,
    model: Option<String>,
    usage: Option<AUsage>,
}

#[derive(Deserialize)]
struct Delta {
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

/// The content block being streamed.
#[derive(Clone, Copy)]
enum Open {
    Tool(usize),
    Thinking(usize),
}

/// Folds Messages stream events into a [`Response`].
#[derive(Default)]
pub(crate) struct StreamState {
    pub res: Response,
    open: Option<Open>,
    /// Tool input given whole in `content_block_start` (some gateways send no deltas).
    start_input: Option<String>,
}

impl StreamState {
    pub fn data(&mut self, data: &[u8], out: &mut VecDeque<Event>) -> Result<()> {
        let ev: Ev = serde_json::from_slice(data)?;
        let res = &mut self.res;
        match &*ev.kind {
            "message_start" => {
                if let Some(m) = ev.message {
                    res.id = m.id.unwrap_or_default();
                    res.model = m.model.unwrap_or_default();
                    if let Some(u) = m.usage {
                        u.merge_into(&mut res.usage);
                    }
                }
            }
            "content_block_start" => {
                self.open = None;
                let Some(b) = ev.content_block else { return Ok(()) };
                match b.kind.as_str() {
                    "tool_use" => {
                        self.open = Some(Open::Tool(res.tool_calls.len()));
                        self.start_input = b.input.map(|i| i.get().to_owned()).filter(|i| i != "{}");
                        res.tool_calls.push(ToolCall {
                            id: b.id.unwrap_or_default(),
                            name: b.name.unwrap_or_default(),
                            arguments: String::new(),
                        });
                    }
                    "thinking" => {
                        self.open = Some(Open::Thinking(res.thinking.len()));
                        let text = b.thinking.unwrap_or_default();
                        if !text.is_empty() {
                            res.reasoning.push_str(&text);
                            out.push_back(Event::Reasoning(text.clone()));
                        }
                        res.thinking.push(Thinking {
                            text,
                            signature: b.signature.unwrap_or_default(),
                            redacted: false,
                        });
                    }
                    "redacted_thinking" => res.thinking.push(Thinking {
                        text: String::new(),
                        signature: b.data.unwrap_or_default(),
                        redacted: true,
                    }),
                    "text" => {
                        if let Some(t) = b.text.filter(|t| !t.is_empty()) {
                            res.text.push_str(&t);
                            out.push_back(Event::Text(t));
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let Some(d) = ev.delta else { return Ok(()) };
                if let Some(t) = d.text {
                    res.text.push_str(&t);
                    out.push_back(Event::Text(t));
                }
                if let Some(t) = d.thinking {
                    if let Some(Open::Thinking(i)) = self.open {
                        res.thinking[i].text.push_str(&t);
                    }
                    res.reasoning.push_str(&t);
                    out.push_back(Event::Reasoning(t));
                }
                if let (Some(sig), Some(Open::Thinking(i))) = (d.signature, self.open) {
                    res.thinking[i].signature.push_str(&sig);
                }
                if let (Some(j), Some(Open::Tool(i))) = (d.partial_json, self.open) {
                    res.tool_calls[i].arguments.push_str(&j);
                }
            }
            "content_block_stop" => {
                if let Some(Open::Tool(i)) = self.open.take() {
                    let call = &mut res.tool_calls[i];
                    if call.arguments.trim().is_empty() {
                        call.arguments = self.start_input.take().unwrap_or_else(|| "{}".to_owned());
                    }
                    out.push_back(Event::ToolCall(call.clone()));
                }
            }
            "message_delta" => {
                if let Some(r) = ev.delta.and_then(|d| d.stop_reason) {
                    res.finish_reason = FinishReason::parse(&r);
                }
                if let Some(u) = ev.usage {
                    u.merge_into(&mut res.usage);
                }
            }
            "error" => return Err(ApiError::from_value(ev.error.as_ref().unwrap_or(&Value::Null)).into()),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body() {
        let mut asst = Message::assistant("let me check");
        asst.tool_calls.push(ToolCall { id: "t1".into(), name: "f".into(), arguments: r#"{"x":1}"#.into() });
        asst.tool_calls.push(ToolCall { id: "t2".into(), name: "f".into(), arguments: String::new() });
        asst.thinking.push(Thinking { text: "hm".into(), signature: "sig".into(), redacted: false });
        asst.thinking.push(Thinking { text: String::new(), signature: "enc".into(), redacted: true });
        asst.thinking.push(Thinking { text: "unsigned".into(), ..Default::default() });
        let req = Request::new("claude")
            .system("a")
            .user("hi")
            .image(Image::base64("image/png", "AAAA"))
            .message(asst)
            .tool_result("t1", "one")
            .message(Message::tool("t2", "two").image(Image::url("https://x/p.png")))
            .system("b")
            .tool(Tool::new("f", "", json!({"type": "object"})))
            .tool_choice(ToolChoice::Required)
            .stop("X");
        let v: Value = serde_json::to_value(ChatBody { req: &req, stream: false }).unwrap();
        assert_eq!(
            v,
            json!({
                "model": "claude",
                "max_tokens": 4096,
                "system": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}],
                "messages": [
                    {"role": "user", "content": [
                        {"type": "text", "text": "hi"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}]},
                    {"role": "assistant", "content": [
                        {"type": "thinking", "thinking": "hm", "signature": "sig"},
                        {"type": "redacted_thinking", "data": "enc"},
                        {"type": "text", "text": "let me check"},
                        {"type": "tool_use", "id": "t1", "name": "f", "input": {"x": 1}},
                        {"type": "tool_use", "id": "t2", "name": "f", "input": {}}]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "t1", "content": "one"},
                        {"type": "tool_result", "tool_use_id": "t2", "content": [
                            {"type": "text", "text": "two"},
                            {"type": "image", "source": {"type": "url", "url": "https://x/p.png"}}]}]}
                ],
                "tools": [{"name": "f", "input_schema": {"type": "object"}}],
                "tool_choice": {"type": "any"},
                "stop_sequences": ["X"]
            })
        );
    }

    #[test]
    fn cache_breakpoints() {
        let one_hour = json!({"type": "ephemeral", "ttl": "1h"});
        let five = json!({"type": "ephemeral"});
        let tools = |r: Request| r.tool(Tool::new("a", "", json!({}))).tool(Tool::new("b", "", json!({})));
        let req = tools(Request::new("c")).user("q1").assistant("a1").user("q2").cache(Cache::OneHour);
        let v = serde_json::to_value(ChatBody { req: &req, stream: false }).unwrap();
        assert_eq!(v["tools"][0].get("cache_control"), None);
        assert_eq!(v["tools"][1]["cache_control"], one_hour);
        assert_eq!(v["messages"][0]["content"], "q1");
        assert_eq!(v["messages"][2]["content"], json!([{"type": "text", "text": "q2", "cache_control": one_hour}]));

        let req = tools(Request::new("c"))
            .system("s")
            .message(Message::user("doc").cache())
            .user("q")
            .cache(Cache::FiveMinutes);
        let v = serde_json::to_value(ChatBody { req: &req, stream: false }).unwrap();
        assert_eq!(v["tools"][1].get("cache_control"), None);
        assert_eq!(v["system"][0]["cache_control"], five);
        assert_eq!(v["messages"][0]["content"][0]["cache_control"], five);
        assert_eq!(v["messages"][1]["content"][0]["cache_control"], five);

        let req = Request::new("c").user("q").tool_result("t", "r").cache(Cache::FiveMinutes);
        let v = serde_json::to_value(ChatBody { req: &req, stream: false }).unwrap();
        assert_eq!(v["messages"][1]["content"][0]["cache_control"], five);
        let off = serde_json::to_string(&ChatBody { req: &Request::new("c").user("q"), stream: false }).unwrap();
        assert!(!off.contains("cache_control"));
    }

    #[test]
    fn response() {
        let r = parse_chat(br#"{"id":"m1","model":"c","content":[{"type":"thinking","thinking":"hmm","signature":"s"},{"type":"redacted_thinking","data":"x"},{"type":"text","text":"ok"},{"type":"tool_use","id":"t","name":"f","input":{"a":[1,2]}}],"stop_reason":"tool_use","usage":{"input_tokens":5,"cache_read_input_tokens":10,"cache_creation_input_tokens":1,"output_tokens":2}}"#).unwrap();
        assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("ok", "hmm"));
        assert_eq!(r.thinking[0], Thinking { text: "hmm".into(), signature: "s".into(), redacted: false });
        assert!(r.thinking[1].redacted);
        assert_eq!(r.tool_calls[0].arguments, r#"{"a":[1,2]}"#);
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        let u =
            Usage { input_tokens: 16, output_tokens: 2, cached_tokens: 10, cache_write_tokens: 1, reasoning_tokens: 0 };
        assert_eq!(r.usage, u);
        let msg = r.into_message();
        assert_eq!(msg.thinking.len(), 2);

        let e =
            parse_chat(br#"{"type":"error","error":{"type":"overloaded_error","message":"busy"},"request_id":"r1"}"#);
        assert_eq!(e.unwrap_err().request_id(), Some("r1"));
    }

    #[test]
    fn tool_input_in_start_block() {
        let mut s = StreamState::default();
        let mut q = VecDeque::new();
        for ev in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"f","input":{"a":1}}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ] {
            s.data(ev.as_bytes(), &mut q).unwrap();
        }
        assert_eq!(s.res.tool_calls[0].arguments, r#"{"a":1}"#);
    }
}
