//! OpenAI Chat Completions wire format.

use std::{borrow::Cow, collections::VecDeque};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use serde_json::{Value, value::RawValue};

use crate::{Error, Result, error, types::*};

pub(crate) struct ChatBody<'a> {
    pub req: &'a Request,
    pub stream: bool,
    pub max_tokens_field: &'a str,
}

impl Serialize for ChatBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.req;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        m.serialize_entry("messages", &Msgs(&r.messages))?;
        if !r.tools.is_empty() {
            m.serialize_entry("tools", &Tools(&r.tools))?;
        }
        if let Some(c) = &r.tool_choice {
            m.serialize_entry("tool_choice", &Choice(c))?;
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

struct Msgs<'a>(&'a [Message]);

impl Serialize for Msgs<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(Msg))
    }
}

struct Msg<'a>(&'a Message);

impl Serialize for Msg<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.0;
        let mut m = s.serialize_map(None)?;
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        m.serialize_entry("role", role)?;
        if msg.content.is_empty() && !msg.tool_calls.is_empty() {
            m.serialize_entry("content", &())?;
        } else {
            m.serialize_entry("content", &msg.content)?;
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

/// Serializes an iterator as a JSON array without collecting it.
pub(crate) struct Seq<I>(pub I);

impl<I: Iterator<Item = T> + Clone, T: Serialize> Serialize for Seq<I> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.clone())
    }
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

struct Choice<'a>(&'a ToolChoice);

impl Serialize for Choice<'_> {
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
    prompt_tokens_details: Option<Details>,
}

#[derive(Deserialize)]
struct Details {
    cached_tokens: Option<u32>,
}

impl From<OaUsage> for Usage {
    fn from(u: OaUsage) -> Self {
        Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
            cached_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens).unwrap_or(0),
        }
    }
}

#[derive(Deserialize)]
struct ChatResp {
    id: Option<String>,
    model: Option<String>,
    choices: Option<Vec<RespChoice>>,
    usage: Option<OaUsage>,
}

#[derive(Deserialize)]
struct RespChoice {
    message: Option<RespMsg>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct RespMsg {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
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

pub(crate) fn parse_chat(body: &[u8]) -> Result<Response> {
    let r: ChatResp = serde_json::from_slice(body)?;
    let mut out = Response {
        id: r.id.unwrap_or_default(),
        model: r.model.unwrap_or_default(),
        usage: r.usage.map(Usage::from).unwrap_or_default(),
        ..Response::default()
    };
    if let Some(c) = r.choices.and_then(|c| c.into_iter().next()) {
        out.finish_reason = c.finish_reason.as_deref().map(FinishReason::parse).unwrap_or_default();
        if let Some(m) = c.message {
            out.text = m.content.unwrap_or_default();
            out.reasoning = m.reasoning_content.or(m.reasoning).unwrap_or_default();
            out.tool_calls = m
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
    }
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
    delta: Option<Delta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
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

/// Folds `chat.completion.chunk` events into a [`Response`].
#[derive(Default)]
pub(crate) struct StreamState {
    pub res: Response,
    emitted: usize,
}

impl StreamState {
    pub fn data(&mut self, data: &[u8], out: &mut VecDeque<Event>) -> Result<()> {
        let c: Chunk = serde_json::from_slice(data)?;
        if let Some(e) = c.error {
            return Err(Error::Api { status: 0, message: error::value_message(&e) });
        }
        let res = &mut self.res;
        let mut finished = false;
        if let (true, Some(id)) = (res.id.is_empty(), c.id) {
            res.id = id.into_owned();
        }
        if let (true, Some(m)) = (res.model.is_empty(), c.model) {
            res.model = m.into_owned();
        }
        if let Some(u) = c.usage {
            res.usage = u.into();
        }
        for ch in c.choices.into_iter().flatten() {
            if let Some(d) = ch.delta {
                if let Some(t) = d.content.filter(|t| !t.is_empty()) {
                    res.text.push_str(&t);
                    out.push_back(Event::Text(t));
                }
                if let Some(t) = d.reasoning_content.or(d.reasoning).filter(|t| !t.is_empty()) {
                    res.reasoning.push_str(&t);
                    out.push_back(Event::Reasoning(t));
                }
                for tc in d.tool_calls.into_iter().flatten() {
                    let calls = &mut res.tool_calls;
                    let i = match tc.index {
                        Some(i) => i,
                        None if tc.id.is_some() => calls.len(),
                        None => calls.len().saturating_sub(1),
                    };
                    if i >= calls.len() {
                        calls.resize_with(i + 1, ToolCall::default);
                    }
                    let call = &mut calls[i];
                    if let Some(id) = tc.id {
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
            if let Some(f) = ch.finish_reason {
                res.finish_reason = FinishReason::parse(&f);
                finished = true;
            }
        }
        if finished {
            self.finish(out);
        }
        Ok(())
    }

    /// Emit tool calls not yet emitted.
    pub fn finish(&mut self, out: &mut VecDeque<Event>) {
        for c in &self.res.tool_calls[self.emitted..] {
            out.push_back(Event::ToolCall(c.clone()));
        }
        self.emitted = self.res.tool_calls.len();
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
        let req = Request::new("m")
            .system("sys")
            .user("hi")
            .message(asst)
            .tool_result("c1", "42")
            .tool(Tool::new("f", "does f", json!({"type": "object"})))
            .tool_choice(ToolChoice::Tool("f".into()))
            .max_tokens(10)
            .temperature(0.5)
            .stop("END")
            .extra("seed", 7);
        let v: Value =
            serde_json::to_value(ChatBody { req: &req, stream: true, max_tokens_field: "max_tokens" }).unwrap();
        assert_eq!(
            v,
            json!({
                "model": "m",
                "messages": [
                    {"role": "system", "content": "sys"},
                    {"role": "user", "content": "hi"},
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
    }

    #[test]
    fn response() {
        let r = parse_chat(br#"{"id":"x","model":"m","choices":[{"message":{"content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#).unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls[0].args::<Value>().unwrap(), json!({"a": 1}));
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (3, 4));
    }
}
