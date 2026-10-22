# aiwire

[![crates.io](https://img.shields.io/crates/v/aiwire.svg)](https://crates.io/crates/aiwire)
[![docs.rs](https://img.shields.io/docsrs/aiwire)](https://docs.rs/aiwire)
[![CI](https://github.com/nettanvirdev/aiwire/actions/workflows/ci.yml/badge.svg)](https://github.com/nettanvirdev/aiwire/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/nettanvirdev/aiwire/blob/main/LICENSE)

Fast, lean async Rust client for any **OpenAI-compatible** or **Anthropic-compatible** LLM API.
Chat, streaming, tool calling, MCP and embeddings, with one small API.

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
- **Two wire formats, one API**: switch between OpenAI and Anthropic protocols with one line.
- **Lean**: 7 direct dependencies, no `async-trait`, no `Value` trees on the hot path,
  embeddings parsed straight into one `Vec<f32>`.
- **Production defaults**: retries with backoff and `retry-after`, timeouts, connection
  pooling, HTTP/2, rustls.

## Install

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread
```

Features: `rustls` (default), `native-tls`, `mcp`.

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

| Provider | OpenAI | Anthropic | Embeddings |
|---|:-:|:-:|:-:|
| OpenAI | ✓ | | ✓ |
| Anthropic | ✓ | ✓ | |
| Groq, Cerebras, xAI | ✓ | | |
| OpenRouter, Fireworks, DeepInfra, Ollama | ✓ | ✓ | ✓ |
| MiniMax, Atlas Cloud, GMI Cloud, DeepSeek, Moonshot, Z.ai, Hugging Face, SambaNova | ✓ | ✓ | |
| Together, Mistral, Gemini, Nebius | ✓ | | ✓ |

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

## MCP

```rust
let mcp = aiwire::mcp::Mcp::stdio("uvx", ["mcp-server-time"]).await?; // feature "mcp"
let req = Request::new("gpt-5-mini").user("Time in Tokyo?").tools(mcp.tools().await?);
// ... then answer each tool call with mcp.call(&call).await?
```

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
| chat, req/s | **28,910** | **86,393** | 3,830 | 345 |
| streaming (200 chunks), req/s | **2,987** | **10,597** | 429 | 39 |
| embeddings (16 × 1536), req/s | **1,028** | **4,836** | 463 | 20 |
| peak memory | **11–19 MB** | **13–19 MB** | 193–297 MB | 100–170 MB |
| CPU time for 20,000 chats | **0.70 s** | **0.73 s** | 7.6 s | 58.9 s |

On a single thread, aiwire handles ~7× the streaming throughput of openai-node with ~10× less
memory. Full table with p50/p99 latency: [bench/RESULTS.md](https://github.com/nettanvirdev/aiwire/blob/main/bench/RESULTS.md). Absolute numbers
vary between runs; the ratios hold. A release binary using aiwire is ~5 MB; the crate itself
is 40 KB.

All SDKs hit the same local mock server: no API keys, no tokens, no money. Reproduce with
`python bench/run.py` ([how](https://github.com/nettanvirdev/aiwire/blob/main/docs/11-benchmarks.md)).

## Docs

Step-by-step guide in [docs/](https://github.com/nettanvirdev/aiwire/blob/main/docs/README.md): installation, providers, streaming, tools, MCP,
embeddings, parameters, configuration, errors.

Examples: `cargo run --example chat` (also `stream`, `tools`, `embed`, `mcp --features mcp`).

## License

MIT, see [LICENSE](https://github.com/nettanvirdev/aiwire/blob/main/LICENSE).
