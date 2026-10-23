//! OpenAI Responses wire format (`/responses`), used statelessly: the whole conversation is
//! sent every time, as with Chat Completions.

use std::{borrow::Cow, collections::VecDeque};

use serde::{Deserialize, Serialize, Serializer, ser::Error as _, ser::SerializeMap, ser::SerializeSeq};
use serde_json::{Value, value::RawValue};

use crate::{Result, error::ApiError, openai::Seq, types::*};

pub(crate) struct ChatBody<'a> {
    pub req: &'a Request,
    pub stream: bool,
}

impl Serialize for ChatBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.req;
        if !r.stop.is_empty() {
            return Err(S::Error::custom("the Responses API has no stop sequences; use the OpenAI protocol"));
        }
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        m.serialize_entry("input", &Input(&r.messages))?;
        if !r.tools.is_empty() {
            let tools = r.tools.iter().map(|t| ToolDef {
                kind: "function",
                name: &t.name,
                description: &t.description,
                parameters: &t.parameters,
                // Responses may default to strict schemas, which reject most hand-written ones.
                strict: false,
            });
            m.serialize_entry("tools", &Seq(tools))?;
        }
        if let Some(c) = &r.tool_choice {
            match c {
                ToolChoice::Auto => m.serialize_entry("tool_choice", "auto")?,
                ToolChoice::None => m.serialize_entry("tool_choice", "none")?,
                ToolChoice::Required => m.serialize_entry("tool_choice", "required")?,
                ToolChoice::Tool(name) => {
                    m.serialize_entry("tool_choice", &NamedFn { kind: "function", name })?;
                }
            }
        }
        if let Some(n) = r.max_tokens {
            m.serialize_entry("max_output_tokens", &n)?;
        }
        if let Some(t) = r.temperature {
            m.serialize_entry("temperature", &t)?;
        }
        if let Some(p) = r.top_p {
            m.serialize_entry("top_p", &p)?;
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
struct ToolDef<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    parameters: &'a RawValue,
    strict: bool,
}

#[derive(Serialize)]
struct NamedFn<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'a str,
}

/// Messages as input items: tool calls and results become `function_call` /
/// `function_call_output` items.
struct Input<'a>(&'a [Message]);

impl Serialize for Input<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(None)?;
        for msg in self.0 {
            match msg.role {
                Role::Tool => seq.serialize_element(&CallOutput(msg))?,
                Role::Assistant => {
                    if !msg.content.is_empty() {
                        seq.serialize_element(&Item(msg))?;
                    }
                    for c in &msg.tool_calls {
                        let args = if c.arguments.is_empty() { "{}" } else { &c.arguments };
                        let call = FnCall { kind: "function_call", call_id: &c.id, name: &c.name, arguments: args };
                        seq.serialize_element(&call)?;
                    }
                }
                Role::System | Role::User => seq.serialize_element(&Item(msg))?,
            }
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct FnCall<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    call_id: &'a str,
    name: &'a str,
    arguments: &'a str,
}

struct Item<'a>(&'a Message);

impl Serialize for Item<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.0;
        let mut m = s.serialize_map(Some(2))?;
        let role = match msg.role {
            Role::System => "system",
            Role::Assistant => "assistant",
            _ => "user",
        };
        m.serialize_entry("role", role)?;
        match msg.single_text() {
            Some(t) => m.serialize_entry("content", t)?,
            None if msg.role == Role::Assistant => m.serialize_entry("content", &msg.text())?,
            None => m.serialize_entry("content", &Seq(msg.content.iter().map(PartOut)))?,
        }
        m.end()
    }
}

struct CallOutput<'a>(&'a Message);

impl Serialize for CallOutput<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let msg = self.0;
        let mut m = s.serialize_map(Some(3))?;
        m.serialize_entry("type", "function_call_output")?;
        m.serialize_entry("call_id", msg.tool_call_id.as_deref().unwrap_or_default())?;
        match msg.single_text() {
            Some(t) => m.serialize_entry("output", t)?,
            None => m.serialize_entry("output", &Seq(msg.content.iter().map(PartOut)))?,
        }
        m.end()
    }
}

#[derive(Clone, Copy)]
struct PartOut<'a>(&'a Part);

impl Serialize for PartOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Part::Text(t) => Typed { kind: "input_text", text: Some(t), image_url: None }.serialize(s),
            Part::Image(i) => Typed { kind: "input_image", text: None, image_url: Some(&i.url) }.serialize(s),
            Part::Raw(v) => v.serialize(s),
        }
    }
}

#[derive(Serialize)]
struct Typed<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<&'a str>,
}

#[derive(Deserialize, Default)]
struct RResp {
    id: Option<String>,
    model: Option<String>,
    status: Option<String>,
    #[serde(default)]
    output: Vec<OutItem>,
    usage: Option<RUsage>,
    incomplete_details: Option<Incomplete>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct Incomplete {
    reason: Option<String>,
}

#[derive(Deserialize)]
struct RUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    input_tokens_details: Option<InDetails>,
    output_tokens_details: Option<OutDetails>,
}

#[derive(Deserialize)]
struct InDetails {
    cached_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct OutDetails {
    reasoning_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct OutItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<OutPart>,
    #[serde(default)]
    summary: Vec<OutPart>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OutPart {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    refusal: Option<String>,
}

impl OutItem {
    fn into_call(self) -> ToolCall {
        ToolCall {
            id: self.call_id.unwrap_or_default(),
            name: self.name.unwrap_or_default(),
            arguments: self.arguments.unwrap_or_default(),
        }
    }
}

impl RResp {
    fn check(&self) -> Result<()> {
        match &self.error {
            Some(e) if !e.is_null() => Err(ApiError::from_value(e).into()),
            _ if self.status.as_deref() == Some("failed") => {
                Err(ApiError { message: "response failed".into(), ..ApiError::default() }.into())
            }
            _ => Ok(()),
        }
    }

    /// Usage and finish reason; output too when `with_output` (the non-streaming case).
    fn apply(self, res: &mut Response, with_output: bool) {
        if let (true, Some(id)) = (res.id.is_empty(), self.id) {
            res.id = id;
        }
        if let (true, Some(m)) = (res.model.is_empty(), self.model) {
            res.model = m;
        }
        if let Some(u) = self.usage {
            res.usage = Usage {
                input_tokens: u.input_tokens.unwrap_or(0),
                output_tokens: u.output_tokens.unwrap_or(0),
                cached_tokens: u.input_tokens_details.and_then(|d| d.cached_tokens).unwrap_or(0),
                cache_write_tokens: 0,
                reasoning_tokens: u.output_tokens_details.and_then(|d| d.reasoning_tokens).unwrap_or(0),
            };
        }
        let mut refused = false;
        for item in self.output {
            match item.kind.as_str() {
                "function_call" if with_output => res.tool_calls.push(item.into_call()),
                "message" if with_output => {
                    for p in item.content {
                        match (p.kind.as_str(), p.text, p.refusal) {
                            ("refusal", _, Some(r)) => {
                                res.text.push_str(&r);
                                refused = true;
                            }
                            (_, Some(t), _) => res.text.push_str(&t),
                            _ => {}
                        }
                    }
                }
                "reasoning" if with_output => {
                    for p in item.summary.into_iter().chain(item.content) {
                        res.reasoning.push_str(p.text.as_deref().unwrap_or_default());
                    }
                }
                "message" => refused |= item.content.iter().any(|p| p.kind == "refusal"),
                _ => {}
            }
        }
        res.finish_reason = match (self.status.as_deref(), self.incomplete_details.and_then(|d| d.reason)) {
            _ if refused => FinishReason::ContentFilter,
            (Some("incomplete"), Some(r)) => FinishReason::parse(&r),
            _ if !res.tool_calls.is_empty() => FinishReason::ToolCalls,
            (Some(s), _) => FinishReason::parse(s),
            _ => FinishReason::Unknown,
        };
    }
}

pub(crate) fn parse_chat(body: &[u8]) -> Result<Response> {
    let r: RResp = serde_json::from_slice(body)?;
    r.check()?;
    let mut out = Response::default();
    r.apply(&mut out, true);
    Ok(out)
}

#[derive(Deserialize)]
struct Ev<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    delta: Option<String>,
    item: Option<OutItem>,
    response: Option<RResp>,
}

/// Folds Responses stream events into a [`Response`].
#[derive(Default)]
pub(crate) struct StreamState {
    pub res: Response,
}

impl StreamState {
    pub fn data(&mut self, data: &[u8], out: &mut VecDeque<Event>) -> Result<()> {
        let ev: Ev = serde_json::from_slice(data)?;
        let res = &mut self.res;
        match &*ev.kind {
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(t) = ev.delta.filter(|t| !t.is_empty()) {
                    res.text.push_str(&t);
                    out.push_back(Event::Text(t));
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(t) = ev.delta.filter(|t| !t.is_empty()) {
                    res.reasoning.push_str(&t);
                    out.push_back(Event::Reasoning(t));
                }
            }
            "response.output_item.done" => {
                if let Some(item) = ev.item.filter(|i| i.kind == "function_call") {
                    let call = item.into_call();
                    out.push_back(Event::ToolCall(call.clone()));
                    res.tool_calls.push(call);
                }
            }
            "response.created"
            | "response.in_progress"
            | "response.completed"
            | "response.incomplete"
            | "response.failed" => {
                let r = ev.response.unwrap_or_default();
                r.check()?;
                r.apply(res, false);
            }
            "error" => return Err(ApiError::from_value(&serde_json::from_slice::<Value>(data)?).into()),
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
        let mut asst = Message::assistant("checking");
        asst.tool_calls.push(ToolCall { id: "c1".into(), name: "f".into(), arguments: String::new() });
        let req = Request::new("m")
            .system("sys")
            .user("hi")
            .image(Image::url("https://x/a.png"))
            .message(asst)
            .tool_result("c1", "42")
            .tool(Tool::new("f", "does f", json!({"type": "object"})))
            .tool_choice(ToolChoice::Tool("f".into()))
            .max_tokens(10);
        let v = serde_json::to_value(ChatBody { req: &req, stream: true }).unwrap();
        assert_eq!(
            v,
            json!({
                "model": "m",
                "input": [
                    {"role": "system", "content": "sys"},
                    {"role": "user", "content": [
                        {"type": "input_text", "text": "hi"},
                        {"type": "input_image", "image_url": "https://x/a.png"}]},
                    {"role": "assistant", "content": "checking"},
                    {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
                    {"type": "function_call_output", "call_id": "c1", "output": "42"}
                ],
                "tools": [{"type": "function", "name": "f", "description": "does f", "parameters": {"type": "object"}, "strict": false}],
                "tool_choice": {"type": "function", "name": "f"},
                "max_output_tokens": 10,
                "stream": true
            })
        );
        assert!(serde_json::to_value(ChatBody { req: &Request::new("m").stop("x"), stream: false }).is_err());
    }

    #[test]
    fn response() {
        let r = parse_chat(br#"{"id":"resp_1","model":"m","status":"completed","output":[
            {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"think"}]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi","annotations":[]}]},
            {"type":"function_call","call_id":"c","name":"f","arguments":"{\"a\":1}"}],
            "usage":{"input_tokens":5,"output_tokens":3,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":1}}}"#).unwrap();
        assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("Hi", "think"));
        assert_eq!(r.tool_calls[0].name, "f");
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!((r.usage.cached_tokens, r.usage.reasoning_tokens), (2, 1));
        let r =
            parse_chat(br#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[]}"#);
        assert_eq!(r.unwrap().finish_reason, FinishReason::Length);
        let e = parse_chat(br#"{"status":"failed","error":{"code":"server_error","message":"boom"},"output":[]}"#);
        assert!(e.unwrap_err().to_string().contains("boom"));
    }

    #[test]
    fn stream() {
        let mut s = StreamState::default();
        let mut q = VecDeque::new();
        for ev in [
            r#"{"type":"response.created","response":{"id":"r","model":"m","status":"in_progress","output":[]}}"#,
            r#"{"type":"response.reasoning_summary_text.delta","delta":"hm"}"#,
            r#"{"type":"response.output_text.delta","delta":"Hel"}"#,
            r#"{"type":"response.output_text.delta","delta":"lo"}"#,
            r#"{"type":"response.function_call_arguments.delta","delta":"{}"}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"c","name":"f","arguments":"{}"}}"#,
            r#"{"type":"response.completed","response":{"status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":2}}}"#,
        ] {
            s.data(ev.as_bytes(), &mut q).unwrap();
        }
        assert_eq!(q.len(), 4);
        assert_eq!((s.res.id.as_str(), s.res.text.as_str()), ("r", "Hello"));
        assert_eq!((s.res.finish_reason.clone(), s.res.usage.output_tokens), (FinishReason::ToolCalls, 2));
        let err = s.data(br#"{"type":"error","code":"rate_limit_exceeded","message":"slow"}"#, &mut q).unwrap_err();
        assert!(err.is_retryable() || err.to_string().contains("slow"));
    }
}
