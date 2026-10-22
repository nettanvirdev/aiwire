use std::borrow::Cow;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, value::RawValue};

use crate::Result;

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

/// One conversation turn.
#[derive(Clone, Debug)]
pub struct Message {
    /// Author.
    pub role: Role,
    /// Text content.
    pub content: Text,
    /// Tool calls the assistant made in this turn.
    pub tool_calls: Vec<ToolCall>,
    /// For [`Role::Tool`]: the call this result answers.
    pub tool_call_id: Option<String>,
}

impl Message {
    fn new(role: Role, content: impl Into<Text>) -> Self {
        Self { role, content: content.into(), tool_calls: Vec::new(), tool_call_id: None }
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
    /// Extra top-level JSON fields, sent as-is (e.g. `seed`, `reasoning_effort`, `thinking`).
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
            "stop" | "end_turn" | "stop_sequence" | "pause_turn" => Self::Stop,
            "length" | "max_tokens" | "model_context_window_exceeded" => Self::Length,
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
    /// Generated tokens.
    pub output_tokens: u32,
    /// Prompt tokens served from cache.
    pub cached_tokens: u32,
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
    /// Tool calls to run.
    pub tool_calls: Vec<ToolCall>,
    /// Why generation stopped.
    pub finish_reason: FinishReason,
    /// Token counts.
    pub usage: Usage,
}

impl Response {
    /// The assistant turn, ready to push onto [`Request::messages`] for the next round.
    pub fn into_message(self) -> Message {
        Message { role: Role::Assistant, content: self.text.into(), tool_calls: self.tool_calls, tool_call_id: None }
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
