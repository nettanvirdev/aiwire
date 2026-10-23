//! OpenAI Chat Completions wire format.

use std::{borrow::Cow, collections::VecDeque};

use serde::{Deserialize, Serialize, Serializer, ser::Error as _, ser::SerializeMap};
use serde_json::{Value, value::RawValue};

use crate::{Result, error::ApiError, types::*};

pub(crate) struct ChatBody<'a> {
    pub req: &'a Request,
    pub stream: bool,
    pub max_tokens_field: &'a str,
    pub send_reasoning: bool,
}

impl Serialize for ChatBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.req;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        let msgs = r.messages.iter().map(|msg| Msg { msg, send_reasoning: self.send_reasoning });
        m.serialize_entry("messages", &Seq(msgs))?;
        if !r.tools.is_empty() {
            m.serialize_entry("tools", &Tools(&r.tools))?;
        }
        if let Some(c) = &r.tool_choice {
            m.serialize_entry("tool_choice", &ToolChoiceOut(c))?;
        }
        if let Some(n) = r.max_tokens {
            m.serialize_entry(self.max_tokens_field, &n)?;
        }
        if let Some(t) = r.temperature {
            m.serialize_entry("temperature", &t)?;
        }
        if let Some(p) = r.top_p {
            m.serialize_entry("top_p", &p)?;
        }
        if !r.stop.is_empty() {
            m.serialize_entry("stop", &r.stop)?;
        }
        if self.stream {
            m.serialize_entry("stream", &true)?;
            m.serialize_entry("stream_options", &StreamOptions { include_usage: true })?;
        }
        for (k, v) in &r.extra {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// Serializes an iterator as a JSON array without collecting it.
pub(crate) struct Seq<I>(pub I);

impl<I: Iterator<Item = T> + Clone, T: Serialize> Serialize for Seq<I> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.clone())
    }
}

#[derive(Clone, Copy)]
struct Msg<'a> {
    msg: &'a Message,
    send_reasoning: bool,
}

impl Serialize for Msg<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.msg;
        let mut m = s.serialize_map(None)?;
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        m.serialize_entry("role", role)?;
        match msg.single_text() {
            Some("") if !msg.tool_calls.is_empty() => m.serialize_entry("content", &())?,
            Some(t) => m.serialize_entry("content", t)?,
            None => {
                if msg.role == Role::Tool && msg.content.iter().any(|p| matches!(p, Part::Image(_))) {
                    return Err(S::Error::custom("images in tool results need the Anthropic protocol"));
                }
                m.serialize_entry("content", &Seq(msg.content.iter().map(PartOut)))?;
            }
        }
        if self.send_reasoning && msg.role == Role::Assistant {
            let text = msg.thinking.iter().filter(|t| !t.redacted).map(|t| t.text.as_str());
            if text.clone().any(|t| !t.is_empty()) {
                m.serialize_entry("reasoning_content", &text.collect::<String>())?;
            }
        }
        if !msg.tool_calls.is_empty() {
            let calls = msg.tool_calls.iter().map(|c| CallOut {
                id: &c.id,
                kind: "function",
                function: FnOut { name: &c.name, arguments: if c.arguments.is_empty() { "{}" } else { &c.arguments } },
            });
            m.serialize_entry("tool_calls", &Seq(calls))?;
        }
        if let Some(id) = &msg.tool_call_id {
            m.serialize_entry("tool_call_id", id)?;
        }
        m.end()
    }
}

struct PartOut<'a>(&'a Part);

impl Serialize for PartOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Part::Text(text) => TextPart { kind: "text", text }.serialize(s),
            Part::Image(i) => ImagePart { kind: "image_url", image_url: Url { url: &i.url } }.serialize(s),
            Part::Raw(v) => v.serialize(s),
        }
    }
}

#[derive(Serialize)]
struct TextPart<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct ImagePart<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    image_url: Url<'a>,
}

#[derive(Serialize)]
struct Url<'a> {
    url: &'a str,
}

#[derive(Serialize)]
struct CallOut<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FnOut<'a>,
}

#[derive(Serialize)]
struct FnOut<'a> {
    name: &'a str,
    arguments: &'a str,
}

struct Tools<'a>(&'a [Tool]);

impl Serialize for Tools<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(|t| ToolOut {
            kind: "function",
            function: FnDef { name: &t.name, description: &t.description, parameters: Some(&t.parameters) },
        }))
    }
}

#[derive(Serialize)]
struct ToolOut<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FnDef<'a>,
}

#[derive(Serialize)]
struct FnDef<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<&'a RawValue>,
}

struct ToolChoiceOut<'a>(&'a ToolChoice);

impl Serialize for ToolChoiceOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            ToolChoice::Auto => s.serialize_str("auto"),
            ToolChoice::None => s.serialize_str("none"),
            ToolChoice::Required => s.serialize_str("required"),
            ToolChoice::Tool(name) => {
                ToolOut { kind: "function", function: FnDef { name, description: "", parameters: None } }.serialize(s)
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct OaUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    prompt_tokens_details: Option<PromptDetails>,
    completion_tokens_details: Option<CompletionDetails>,
    /// DeepSeek reports cache hits here.
    prompt_cache_hit_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct PromptDetails {
    cached_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct CompletionDetails {
    reasoning_tokens: Option<u32>,
}

impl From<OaUsage> for Usage {
    fn from(u: OaUsage) -> Self {
        Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
            cached_tokens: u
                .prompt_tokens_details
                .and_then(|d| d.cached_tokens)
                .or(u.prompt_cache_hit_tokens)
                .unwrap_or(0),
            cache_write_tokens: 0,
            reasoning_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens).unwrap_or(0),
        }
    }
}

/// `content` is a string almost everywhere; Mistral's reasoning models send a list of parts.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Deserialize)]
struct ContentPart {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    thinking: Option<Value>,
}

impl Content {
    /// Splits into (text, reasoning).
    fn split(self) -> (String, String) {
        match self {
            Content::Text(t) => (t, String::new()),
            Content::Parts(parts) => {
                let (mut text, mut reasoning) = (String::new(), String::new());
                for p in parts {
                    match (p.kind.as_deref(), p.text, p.thinking) {
                        (Some("thinking"), _, Some(th)) => collect_text(&th, &mut reasoning),
                        (_, Some(t), _) => text.push_str(&t),
                        _ => {}
                    }
                }
                (text, reasoning)
            }
        }
    }
}

fn collect_text(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => out.push_str(s),
        Value::Array(a) => a.iter().for_each(|x| collect_text(x, out)),
        Value::Object(o) => o.get("text").into_iter().for_each(|x| collect_text(x, out)),
        _ => {}
    }
}

#[derive(Deserialize)]
struct ChatResp {
    id: Option<String>,
    model: Option<String>,
    choices: Option<Vec<RespChoice>>,
    usage: Option<OaUsage>,
    error: Option<Value>,
    base_resp: Option<BaseResp>,
}

/// MiniMax reports some failures as HTTP 200 with a non-zero status here.
#[derive(Deserialize)]
pub(crate) struct BaseResp {
    status_code: Option<i64>,
    status_msg: Option<String>,
}

impl BaseResp {
    pub(crate) fn check(&self) -> Result<()> {
        match self.status_code {
            Some(c) if c != 0 => Err(ApiError {
                kind: Some(c.to_string()),
                message: self.status_msg.clone().unwrap_or_default(),
                ..ApiError::default()
            }
            .into()),
            _ => Ok(()),
        }
    }
}

#[derive(Deserialize)]
struct RespChoice {
    index: Option<usize>,
    message: Option<RespMsg>,
    finish_reason: Option<String>,
    logprobs: Option<Logprobs>,
}

#[derive(Deserialize)]
struct RespMsg {
    content: Option<Content>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    refusal: Option<String>,
    tool_calls: Option<Vec<RespCall>>,
}

#[derive(Deserialize)]
struct RespCall {
    id: Option<String>,
    function: RespFn,
}

#[derive(Deserialize)]
struct RespFn {
    name: String,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct Logprobs {
    content: Option<Vec<TokenLp>>,
}

#[derive(Deserialize)]
struct TokenLp {
    token: String,
    logprob: f32,
    #[serde(default)]
    top_logprobs: Vec<TopLp>,
}

#[derive(Deserialize)]
struct TopLp {
    token: String,
    logprob: f32,
}

impl Logprobs {
    fn append_to(self, out: &mut Vec<Logprob>) {
        out.extend(self.content.into_iter().flatten().map(|t| Logprob {
            token: t.token,
            logprob: t.logprob,
            top: t.top_logprobs.into_iter().map(|x| (x.token, x.logprob)).collect(),
        }));
    }
}

/// Puts choice `i` into `res` (index 0) or `res.other_choices`.
fn into_response(res: &mut Response, mut choices: Vec<Indexed>) {
    choices.sort_by_key(|c| c.0);
    let mut it = choices.into_iter().map(|c| c.1);
    if let Some(first) = it.next() {
        res.set_first(first);
    }
    res.other_choices = it.collect();
}

type Indexed = (usize, Choice);

pub(crate) fn parse_chat(body: &[u8]) -> Result<Response> {
    let r: ChatResp = serde_json::from_slice(body)?;
    if let Some(e) = r.error.filter(|e| !e.is_null()) {
        return Err(ApiError::from_value(&e).into());
    }
    if let Some(b) = &r.base_resp {
        b.check()?;
    }
    let mut out = Response {
        id: r.id.unwrap_or_default(),
        model: r.model.unwrap_or_default(),
        usage: r.usage.map(Usage::from).unwrap_or_default(),
        ..Response::default()
    };
    let choices = r.choices.unwrap_or_default().into_iter().enumerate().map(|(pos, c)| {
        let mut ch = Choice {
            finish_reason: c.finish_reason.as_deref().map(FinishReason::parse).unwrap_or_default(),
            ..Default::default()
        };
        if let Some(lp) = c.logprobs {
            lp.append_to(&mut ch.logprobs);
        }
        if let Some(m) = c.message {
            let (text, reasoning) = m.content.map(Content::split).unwrap_or_default();
            ch.text = text;
            ch.reasoning = m.reasoning_content.or(m.reasoning).unwrap_or(reasoning);
            if ch.text.is_empty() {
                if let Some(r) = m.refusal {
                    ch.text = r;
                    ch.finish_reason = FinishReason::ContentFilter;
                }
            }
            ch.tool_calls = m
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(|t| ToolCall {
                    id: t.id.unwrap_or_default(),
                    name: t.function.name,
                    arguments: t.function.arguments.unwrap_or_default(),
                })
                .collect();
        }
        (c.index.unwrap_or(pos), ch)
    });
    into_response(&mut out, choices.collect());
    Ok(out)
}

#[derive(Deserialize)]
struct Chunk<'a> {
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    choices: Option<Vec<ChunkChoice>>,
    usage: Option<OaUsage>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    index: Option<usize>,
    delta: Option<Delta>,
    finish_reason: Option<String>,
    logprobs: Option<Logprobs>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<Content>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    refusal: Option<String>,
    tool_calls: Option<Vec<DeltaCall>>,
}

#[derive(Deserialize)]
struct DeltaCall {
    index: Option<usize>,
    id: Option<String>,
    function: Option<DeltaFn>,
}

#[derive(Deserialize)]
struct DeltaFn {
    name: Option<String>,
    arguments: Option<String>,
}

/// Folds `chat.completion.chunk` events into a [`Response`]. Events are emitted for the
/// first choice; other choices (`n > 1`) are only assembled.
#[derive(Default)]
pub(crate) struct StreamState {
    pub res: Response,
    choices: Vec<Indexed>,
    /// Tool calls of choice 0 already emitted as events.
    emitted: usize,
}

impl StreamState {
    pub fn data(&mut self, data: &[u8], out: &mut VecDeque<Event>) -> Result<()> {
        let c: Chunk = serde_json::from_slice(data)?;
        if let Some(e) = c.error.filter(|e| !e.is_null()) {
            return Err(ApiError::from_value(&e).into());
        }
        let res = &mut self.res;
        if let (true, Some(id)) = (res.id.is_empty(), c.id) {
            res.id = id.into_owned();
        }
        if let (true, Some(m)) = (res.model.is_empty(), c.model) {
            res.model = m.into_owned();
        }
        if let Some(u) = c.usage {
            res.usage = u.into();
        }
        let mut first_finished = false;
        for (pos, ch) in c.choices.into_iter().flatten().enumerate() {
            let index = ch.index.unwrap_or(pos);
            let slot = match self.choices.iter().position(|c| c.0 == index) {
                Some(i) => i,
                None => {
                    self.choices.push((index, Default::default()));
                    self.choices.len() - 1
                }
            };
            let main = index == 0;
            let acc = &mut self.choices[slot].1;
            if let Some(lp) = ch.logprobs {
                lp.append_to(&mut acc.logprobs);
            }
            if let Some(d) = ch.delta {
                let (text, reasoning) = d.content.map(Content::split).unwrap_or_default();
                for (t, is_text) in [(text, true), (d.reasoning_content.or(d.reasoning).unwrap_or(reasoning), false)] {
                    if t.is_empty() {
                        continue;
                    }
                    let (buf, ev): (_, fn(String) -> Event) =
                        if is_text { (&mut acc.text, Event::Text) } else { (&mut acc.reasoning, Event::Reasoning) };
                    buf.push_str(&t);
                    if main {
                        out.push_back(ev(t));
                    }
                }
                if let Some(r) = d.refusal.filter(|r| !r.is_empty()) {
                    acc.text.push_str(&r);
                    acc.finish_reason = FinishReason::ContentFilter;
                    if main {
                        out.push_back(Event::Text(r));
                    }
                }
                for tc in d.tool_calls.into_iter().flatten() {
                    let calls = &mut acc.tool_calls;
                    let i = match tc.index {
                        Some(i) => i,
                        None if tc.id.is_some() => calls.len(),
                        None => calls.len().saturating_sub(1),
                    };
                    if i >= calls.len() {
                        calls.resize_with(i + 1, ToolCall::default);
                    }
                    let call = &mut calls[i];
                    if let Some(id) = tc.id.filter(|id| !id.is_empty()) {
                        call.id = id;
                    }
                    if let Some(f) = tc.function {
                        if let (true, Some(n)) = (call.name.is_empty(), f.name) {
                            call.name = n;
                        }
                        if let Some(a) = f.arguments {
                            call.arguments.push_str(&a);
                        }
                    }
                }
            }
            if let Some(f) = ch.finish_reason.filter(|f| !f.is_empty()) {
                if acc.finish_reason != FinishReason::ContentFilter {
                    acc.finish_reason = FinishReason::parse(&f);
                }
                first_finished |= main;
            }
        }
        if first_finished {
            self.finish(out);
        }
        Ok(())
    }

    /// Emit tool calls of the first choice not yet emitted.
    pub fn finish(&mut self, out: &mut VecDeque<Event>) {
        let Some((_, c)) = self.choices.iter().find(|c| c.0 == 0) else { return };
        for call in &c.tool_calls[self.emitted.min(c.tool_calls.len())..] {
            out.push_back(Event::ToolCall(call.clone()));
        }
        self.emitted = c.tool_calls.len();
    }

    pub fn into_response(mut self) -> Response {
        into_response(&mut self.res, self.choices);
        self.res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body() {
        let mut asst = Message::assistant("");
        asst.tool_calls.push(ToolCall { id: "c1".into(), name: "f".into(), arguments: String::new() });
        asst.thinking.push(Thinking { text: "hmm".into(), ..Default::default() });
        let req = Request::new("m")
            .system("sys")
            .user("hi")
            .image(Image::url("https://x/a.png"))
            .message(asst)
            .tool_result("c1", "42")
            .tool(Tool::new("f", "does f", json!({"type": "object"})))
            .tool_choice(ToolChoice::Tool("f".into()))
            .max_tokens(10)
            .temperature(0.5)
            .stop("END")
            .extra("seed", 7);
        let body = |send_reasoning| {
            serde_json::to_value(ChatBody { req: &req, stream: true, max_tokens_field: "max_tokens", send_reasoning })
                .unwrap()
        };
        assert_eq!(
            body(false),
            json!({
                "model": "m",
                "messages": [
                    {"role": "system", "content": "sys"},
                    {"role": "user", "content": [
                        {"type": "text", "text": "hi"},
                        {"type": "image_url", "image_url": {"url": "https://x/a.png"}}]},
                    {"role": "assistant", "content": null, "tool_calls": [
                        {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                    {"role": "tool", "content": "42", "tool_call_id": "c1"}
                ],
                "tools": [{"type": "function", "function": {"name": "f", "description": "does f", "parameters": {"type": "object"}}}],
                "tool_choice": {"type": "function", "function": {"name": "f"}},
                "max_tokens": 10,
                "temperature": 0.5,
                "stop": ["END"],
                "stream": true,
                "stream_options": {"include_usage": true},
                "seed": 7
            })
        );
        assert_eq!(body(true)["messages"][2]["reasoning_content"], "hmm");

        let bad = Request::new("m").message(Message::tool("c", "x").image(Image::url("https://x")));
        let err =
            serde_json::to_value(ChatBody { req: &bad, stream: false, max_tokens_field: "", send_reasoning: false });
        assert!(err.unwrap_err().to_string().contains("Anthropic"));
        let raw = Request::new("m").message(Message::user("").part(Part::Raw(json!({"type": "file", "file": {}}))));
        let v =
            serde_json::to_value(ChatBody { req: &raw, stream: false, max_tokens_field: "", send_reasoning: false });
        assert_eq!(v.unwrap()["messages"][0]["content"], json!([{"type": "file", "file": {}}]));
    }

    #[test]
    fn response() {
        let r = parse_chat(br#"{"id":"x","model":"m","choices":[{"message":{"content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"completion_tokens_details":{"reasoning_tokens":2}}}"#).unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls[0].args::<Value>().unwrap(), json!({"a": 1}));
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens, r.usage.reasoning_tokens), (3, 4, 2));
    }

    #[test]
    fn choices_logprobs_and_part_content() {
        let r = parse_chat(br#"{"choices":[
            {"index":1,"message":{"content":"B"},"finish_reason":"stop"},
            {"index":0,"message":{"content":[{"type":"thinking","thinking":[{"type":"text","text":"why"}]},{"type":"text","text":"A"}]},"finish_reason":"length",
             "logprobs":{"content":[{"token":"A","logprob":-0.5,"top_logprobs":[{"token":"A","logprob":-0.5},{"token":"B","logprob":-1.0}]}]}}]}"#).unwrap();
        assert_eq!((r.text.as_str(), r.reasoning.as_str(), &r.finish_reason), ("A", "why", &FinishReason::Length));
        assert_eq!(r.logprobs[0].top, [("A".to_owned(), -0.5), ("B".to_owned(), -1.0)]);
        assert_eq!(r.other_choices[0].text, "B");

        let r = parse_chat(br#"{"choices":[{"message":{"content":null,"refusal":"no"},"finish_reason":"stop"}]}"#);
        assert_eq!(r.unwrap().finish_reason, FinishReason::ContentFilter);
        let e = parse_chat(br#"{"error":{"message":"busy","type":"server_error"}}"#).unwrap_err();
        assert!(e.is_retryable(), "{e}");
        let e = parse_chat(br#"{"base_resp":{"status_code":1008,"status_msg":"insufficient balance"}}"#).unwrap_err();
        assert!(e.to_string().contains("insufficient balance"), "{e}");
    }
}
