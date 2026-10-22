//! Anthropic Messages wire format.

use std::{borrow::Cow, collections::VecDeque};

use serde::{Deserialize, Serialize, Serializer, ser::Error as _, ser::SerializeMap, ser::SerializeSeq};
use serde_json::{Value, value::RawValue};

use crate::{Error, Result, error, openai::Seq, types::*};

/// The Anthropic protocol requires `max_tokens`; every model accepts this much.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 4096;

pub(crate) struct ChatBody<'a> {
    pub req: &'a Request,
    pub stream: bool,
}

impl Serialize for ChatBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.req;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        m.serialize_entry("max_tokens", &r.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS))?;
        let system = r.messages.iter().filter(|m| m.role == Role::System);
        if system.clone().next().is_some() {
            m.serialize_entry("system", &Seq(system.map(|m| TextBlock { kind: "text", text: &m.content })))?;
        }
        m.serialize_entry("messages", &Msgs(&r.messages))?;
        if !r.tools.is_empty() {
            let tools = r.tools.iter().map(|t| ToolDef {
                name: &t.name,
                description: &t.description,
                input_schema: &t.parameters,
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
    text: &'a str,
}

#[derive(Serialize)]
struct ToolDef<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    input_schema: &'a RawValue,
}

#[derive(Serialize)]
struct ChoiceOut<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

/// Skips system messages (sent top-level) and merges runs of tool results into one user turn.
struct Msgs<'a>(&'a [Message]);

impl Serialize for Msgs<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msgs = self.0;
        let mut seq = s.serialize_seq(None)?;
        let mut i = 0;
        while i < msgs.len() {
            match msgs[i].role {
                Role::System => i += 1,
                Role::Tool => {
                    let end = msgs[i..].iter().position(|m| m.role != Role::Tool).map_or(msgs.len(), |k| i + k);
                    seq.serialize_element(&ToolResults(&msgs[i..end]))?;
                    i = end;
                }
                _ => {
                    seq.serialize_element(&Msg(&msgs[i]))?;
                    i += 1;
                }
            }
        }
        seq.end()
    }
}

struct Msg<'a>(&'a Message);

impl Serialize for Msg<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.0;
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("role", if msg.role == Role::Assistant { "assistant" } else { "user" })?;
        if msg.tool_calls.is_empty() {
            m.serialize_entry("content", &msg.content)?;
        } else {
            m.serialize_entry("content", &AssistantBlocks(msg))?;
        }
        m.end()
    }
}

struct AssistantBlocks<'a>(&'a Message);

impl Serialize for AssistantBlocks<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.0;
        let mut seq = s.serialize_seq(None)?;
        if !msg.content.is_empty() {
            seq.serialize_element(&TextBlock { kind: "text", text: &msg.content })?;
        }
        for c in &msg.tool_calls {
            let args = if c.arguments.trim().is_empty() { "{}" } else { &c.arguments };
            let input: &RawValue = serde_json::from_str(args).map_err(S::Error::custom)?;
            seq.serialize_element(&ToolUse { kind: "tool_use", id: &c.id, name: &c.name, input })?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct ToolUse<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    name: &'a str,
    input: &'a RawValue,
}

struct ToolResults<'a>(&'a [Message]);

impl Serialize for ToolResults<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let blocks = self.0.iter().map(|m| ToolResult {
            kind: "tool_result",
            tool_use_id: m.tool_call_id.as_deref().unwrap_or_default(),
            content: &m.content,
        });
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("role", "user")?;
        m.serialize_entry("content", &Seq(blocks))?;
        m.end()
    }
}

#[derive(Serialize)]
struct ToolResult<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    tool_use_id: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
}

impl AUsage {
    /// Anthropic reports uncached input separately; `Usage::input_tokens` counts all of it.
    fn merge_into(&self, u: &mut Usage) {
        if let Some(n) = self.input_tokens {
            let read = self.cache_read_input_tokens.unwrap_or(0);
            u.input_tokens = n + read + self.cache_creation_input_tokens.unwrap_or(0);
            u.cached_tokens = read;
        }
        if let Some(n) = self.output_tokens {
            u.output_tokens = n;
        }
    }
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    thinking: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct ChatResp {
    id: Option<String>,
    model: Option<String>,
    content: Option<Vec<Block>>,
    stop_reason: Option<String>,
    usage: Option<AUsage>,
}

pub(crate) fn parse_chat(body: &[u8]) -> Result<Response> {
    let r: ChatResp = serde_json::from_slice(body)?;
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
            "thinking" => out.reasoning.push_str(b.thinking.as_deref().unwrap_or_default()),
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
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

/// Folds Messages stream events into a [`Response`].
#[derive(Default)]
pub(crate) struct StreamState {
    pub res: Response,
    /// Index into `res.tool_calls` of the tool_use block being streamed.
    open_tool: Option<usize>,
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
                if let Some(b) = ev.content_block.filter(|b| b.kind == "tool_use") {
                    self.open_tool = Some(res.tool_calls.len());
                    res.tool_calls.push(ToolCall {
                        id: b.id.unwrap_or_default(),
                        name: b.name.unwrap_or_default(),
                        arguments: String::new(),
                    });
                }
            }
            "content_block_delta" => {
                if let Some(d) = ev.delta {
                    if let Some(t) = d.text {
                        res.text.push_str(&t);
                        out.push_back(Event::Text(t));
                    }
                    if let Some(t) = d.thinking {
                        res.reasoning.push_str(&t);
                        out.push_back(Event::Reasoning(t));
                    }
                    if let (Some(j), Some(i)) = (d.partial_json, self.open_tool) {
                        res.tool_calls[i].arguments.push_str(&j);
                    }
                }
            }
            "content_block_stop" => {
                if let Some(i) = self.open_tool.take() {
                    let call = &mut res.tool_calls[i];
                    if call.arguments.is_empty() {
                        call.arguments.push_str("{}");
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
            "error" => {
                let message = ev.error.as_ref().map_or_else(|| "unknown".to_owned(), error::value_message);
                return Err(Error::Api { status: 0, message });
            }
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
        let req = Request::new("claude")
            .system("a")
            .user("hi")
            .message(asst)
            .tool_result("t1", "one")
            .tool_result("t2", "two")
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
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "let me check"},
                        {"type": "tool_use", "id": "t1", "name": "f", "input": {"x": 1}},
                        {"type": "tool_use", "id": "t2", "name": "f", "input": {}}]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "t1", "content": "one"},
                        {"type": "tool_result", "tool_use_id": "t2", "content": "two"}]}
                ],
                "tools": [{"name": "f", "input_schema": {"type": "object"}}],
                "tool_choice": {"type": "any"},
                "stop_sequences": ["X"]
            })
        );
    }

    #[test]
    fn response() {
        let r = parse_chat(br#"{"id":"m1","model":"c","content":[{"type":"thinking","thinking":"hmm","signature":"s"},{"type":"text","text":"ok"},{"type":"tool_use","id":"t","name":"f","input":{"a":[1,2]}}],"stop_reason":"tool_use","usage":{"input_tokens":5,"cache_read_input_tokens":10,"output_tokens":2}}"#).unwrap();
        assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("ok", "hmm"));
        assert_eq!(r.tool_calls[0].arguments, r#"{"a":[1,2]}"#);
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.usage, Usage { input_tokens: 15, output_tokens: 2, cached_tokens: 10 });
    }
}
