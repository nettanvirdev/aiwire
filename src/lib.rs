//! Fast, lean async client for any OpenAI- or Anthropic-compatible LLM API.
//!
//! - One [`Client`], three wire formats ([`Protocol::OpenAi`], [`Protocol::Anthropic`],
//!   [`Protocol::Responses`]).
//! - Built-in [`Provider`] table (OpenAI, Anthropic, Groq, Cerebras, OpenRouter, MiniMax, …);
//!   asking a provider for a protocol it does not speak is an error.
//! - Model names are plain strings, never a fixed list.
//! - Chat, streaming, tool calls, images, reasoning, prompt caching, embeddings, and MCP tools
//!   (feature `mcp`). Errors carry status, provider error type, request id and rate-limit headers;
//!   feature `tracing` logs every request, retry and stream.
//!
//! ```no_run
//! use aiwire::{Client, Event, Provider, Request};
//!
//! # async fn run() -> aiwire::Result<()> {
//! let client = Client::from_env(Provider::GROQ)?; // reads GROQ_API_KEY
//!
//! let res = client.chat(Request::new("llama-3.3-70b-versatile").user("Say hi")).await?;
//! println!("{}", res.text);
//!
//! let mut stream = client.stream(Request::new("llama-3.3-70b-versatile").user("Count to 5")).await?;
//! while let Some(event) = stream.next().await {
//!     if let Event::Text(t) = event? {
//!         print!("{t}");
//!     }
//! }
//! # Ok(()) }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod anthropic;
mod client;
mod embed;
mod error;
#[cfg(feature = "mcp")]
pub mod mcp;
mod openai;
mod provider;
mod responses;
mod stream;
mod types;

pub use client::{Client, ClientBuilder};
pub use embed::{EmbedRequest, Embeddings};
pub use error::{ApiError, Error, Result};
pub use provider::{Protocol, Provider};
pub use serde_json::json;
pub use stream::ChatStream;
pub use types::{
    Cache, Choice, Event, FinishReason, Image, Logprob, Message, Part, Request, Response, Role, Text, Thinking, Tool,
    ToolCall, ToolChoice, Usage,
};
