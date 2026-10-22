//! Fast, lean async client for any OpenAI- or Anthropic-compatible LLM API.
//!
//! - One [`Client`], two wire formats ([`Protocol::OpenAi`], [`Protocol::Anthropic`]).
//! - Built-in [`Provider`] table (OpenAI, Anthropic, Groq, Cerebras, OpenRouter, MiniMax, …);
//!   asking a provider for a protocol it does not speak is an error.
//! - Model names are plain strings, never a fixed list.
//! - Chat, streaming, tool calls, embeddings, and MCP tools (feature `mcp`).
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
mod stream;
mod types;

pub use client::{Client, ClientBuilder};
pub use embed::{EmbedRequest, Embeddings};
pub use error::{Error, Result};
pub use provider::{Protocol, Provider};
pub use serde_json::json;
pub use stream::ChatStream;
pub use types::{Event, FinishReason, Message, Request, Response, Role, Text, Tool, ToolCall, ToolChoice, Usage};
