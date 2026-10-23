# aiwire

[![crates.io](https://img.shields.io/crates/v/aiwire.svg)](https://crates.io/crates/aiwire)
[![docs.rs](https://img.shields.io/docsrs/aiwire)](https://docs.rs/aiwire)
[![CI](https://github.com/nettanvirdev/aiwire/actions/workflows/ci.yml/badge.svg)](https://github.com/nettanvirdev/aiwire/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/nettanvirdev/aiwire/blob/main/LICENSE)

Fast, lean async Rust client for any **OpenAI-compatible** or **Anthropic-compatible** LLM API.
Chat, streaming, tool calling, images, reasoning, prompt caching, MCP and embeddings, with one
small API.

```rust
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::GROQ)?; // reads GROQ_API_KEY
    let res = client.chat(Request::new("llama-3.3-70b-versatile").user("Say hi")).await?;
    println!("{}", res.text);
    Ok(())
}
```

- **Any model**: the model is a plain string. No hardcoded model list to wait on.
- **Any host**: 21 built-in providers, or any `base_url`. Asking a provider for a protocol it
  doesn't speak is an error, not a confusing 404.
- **Three wire formats, one API**: OpenAI Chat Completions, Anthropic Messages and OpenAI
  Responses; switch with one line.
- **Lean**: 7 direct dependencies, no `async-trait`, no `Value` trees on the hot path,
  embeddings parsed straight into one `Vec<f32>`.
- **Production defaults**: retries with backoff and `retry-after`, stream idle timeouts, an
  optional concurrency limit, connection pooling, HTTP/2, rustls.
- **Debuggable**: errors carry status, provider error type, request id, rate-limit headers and
  the raw body; the `tracing` feature logs every request, retry and stream.
- **Tested live** against real OpenAI- and Anthropic-compatible servers (`tests/live.rs`).

## Install

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread
```

Already using Tokio (axum, actix-web, most Rust servers)? Skip the second line: aiwire runs on the runtime you already have.

Features: `rustls` (default), `native-tls`, `mcp` (MCP client with OAuth), `tracing` (logs).

## Streaming

```rust
let mut stream = client.stream(Request::new("llama-3.3-70b-versatile").user("Count to 5")).await?;
while let Some(event) = stream.next().await {
    if let aiwire::Event::Text(t) = event? {
        print!("{t}");
    }
}
```

## Pick a provider and protocol

```rust
use aiwire::{Client, Protocol, Provider};

let client = Client::new(Provider::ANTHROPIC, key);            // Anthropic Messages
let client = Client::builder()
    .provider(Provider::MINIMAX)
    .protocol(Protocol::Anthropic)                                // MiniMax speaks both
    .api_key(key)
    .build()?;
let client = Client::builder()                                    // anything else
    .base_url("http://localhost:8000/v1")
    .build()?;
```

| Provider | OpenAI | Anthropic | Responses | Embeddings |
|---|:-:|:-:|:-:|:-:|
| OpenAI | ✓ | | ✓ | ✓ |
| Anthropic | ✓ | ✓ | | |
| Groq, xAI | ✓ | | ✓ | |
| Cerebras | ✓ | | | |
| OpenRouter, Ollama | ✓ | ✓ | ✓ | ✓ |
| Fireworks, DeepInfra | ✓ | ✓ | | ✓ |
| MiniMax, Atlas Cloud, GMI Cloud, DeepSeek, Moonshot, Z.ai, Hugging Face, SambaNova | ✓ | ✓ | | |
| Together, Mistral, Gemini, Nebius | ✓ | | | ✓ |

## Tools

```rust
use aiwire::{Tool, json};

let weather = Tool::new("get_weather", "Weather for a city",
    json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}));
let mut req = Request::new("openai/gpt-5-mini").user("Rain in Dhaka?").tool(weather);

loop {
    let res = client.chat(&req).await?;
    if res.tool_calls.is_empty() { println!("{}", res.text); break; }
    let calls = res.tool_calls.clone();
    req.messages.push(res.into_message());
    for call in calls {
        req = req.tool_result(call.id, "31°C, light rain");
    }
}
```

Thinking models work in tool loops: `into_message()` keeps the reasoning and aiwire sends it
back the way each API requires (signed thinking blocks on Anthropic, `reasoning_content` for
DeepSeek, Moonshot and Z.ai).

## Images

```rust
use aiwire::Image;

let req = Request::new("gpt-5-mini").user("What is in this photo?").image(Image::file("cat.jpg")?);
```

`Image::url`, `Image::bytes` and `Image::base64` work too, on every protocol.

## Prompt caching

```rust
let req = Request::new("claude-sonnet-5-5").system(long_prompt).user(q).cache(aiwire::Cache::FiveMinutes);
println!("{} cached", client.chat(&req).await?.usage.cached_tokens);
```

On the Anthropic protocol aiwire places the `cache_control` breakpoints; OpenAI-style
providers cache on their own.

## Errors you can debug

```rust
match client.chat(&req).await {
    Err(aiwire::Error::Api(e)) => eprintln!("{e:#?}"), // status, kind, request id, headers, body
    other => { other?; }
}
```

```text
openai error 429 rate_limit_exceeded: Rate limit reached (https://api.openai.com/v1/chat/completions, request id req_42, 3 attempts, retry after 7s)
```

## MCP

```rust
let mcp = aiwire::mcp::Mcp::stdio("uvx", ["mcp-server-time"]).await?; // feature "mcp"
let req = Request::new("gpt-5-mini").user("Time in Tokyo?").tools(mcp.tools().await?);
// ... then answer each tool call with mcp.call(&call).await?
```

Stdio and Streamable HTTP, concurrent requests, timeouts, and OAuth login for hosted servers
(`Mcp::http_oauth`).

## Embeddings

```rust
let e = client.embed(aiwire::EmbedRequest::new("text-embedding-3-small").inputs(["cats", "dogs"])).await?;
let v: &[f32] = e.get(0).unwrap();
```

## Benchmarks

Same workloads, same local mock server, Windows 11, 32 logical CPUs
(Python 3.14 + openai 2.37, Node 26 + openai 7.42). Higher req/s is better; lower memory and CPU are better.

| | aiwire, 1 thread | aiwire, 4 threads | openai-node | openai-python |
|---|--:|--:|--:|--:|
| chat, req/s | **46,147** | **100,705** | 6,177 | 593 |
| streaming (200 chunks), req/s | **3,486** | **12,942** | 610 | 61 |
| embeddings (16 × 1536), req/s | **1,573** | **5,784** | 663 | 29 |
| peak memory | **11–18 MB** | **12–18 MB** | 221–294 MB | 97–162 MB |
| CPU time for 20,000 chats | **0.44 s** | **0.44 s** | 4.8 s | 34.8 s |

On a single thread, aiwire handles ~6× the streaming throughput of openai-node with ~13× less
memory. Full table with p50/p99 latency: [bench/RESULTS.md](https://github.com/nettanvirdev/aiwire/blob/main/bench/RESULTS.md). Absolute numbers
vary between runs; the ratios hold. A release binary using aiwire is ~5 MB; the crate itself
is ~70 KB compressed.

All SDKs hit the same local mock server: no API keys, no tokens, no money. Reproduce with
`python bench/run.py` ([how](https://github.com/nettanvirdev/aiwire/blob/main/docs/11-benchmarks.md)).

## Docs

Step-by-step guide in [docs/](https://github.com/nettanvirdev/aiwire/blob/main/docs/README.md): installation, providers, streaming, tools, MCP,
embeddings, parameters, configuration, errors and debugging, images, reasoning and caching.

Examples: `cargo run --example chat` (also `stream`, `tools`, `vision`, `embed`, `mcp --features mcp`).

## License

MIT, see [LICENSE](https://github.com/nettanvirdev/aiwire/blob/main/LICENSE).
