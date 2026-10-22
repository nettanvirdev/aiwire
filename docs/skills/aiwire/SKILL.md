---
name: aiwire
description: Write Rust code that calls LLMs with the aiwire crate (OpenAI- and Anthropic-compatible APIs) - chat, streaming, tool calling, MCP tools, embeddings, providers, retries and errors. Use when code depends on aiwire, or the user wants to call OpenAI, Anthropic, Groq, Cerebras, OpenRouter, MiniMax, DeepSeek, Gemini, Ollama or any OpenAI/Anthropic-compatible endpoint from Rust.
---

# aiwire

aiwire is an async Rust client for any OpenAI-compatible or Anthropic-compatible LLM API.
One `Client`, two wire formats, 21 built-in providers, any model string.
Crate: https://crates.io/crates/aiwire · Docs: https://docs.rs/aiwire · Repo: https://github.com/nettanvirdev/aiwire

Everything below matches aiwire 0.1. Every complete program here compiles.

## Rules when writing aiwire code

1. Models are plain strings. Never invent model constants; use the id the provider documents.
2. Build one `Client` and reuse it (`clone()` is cheap and shares the connection pool). Never build one per request.
3. Use `Client::from_env(Provider::X)` unless the user gives a key; it reads the provider's env var.
4. Pass `&req` to `chat`/`stream` when the request is reused (tool loops, multi-turn).
5. In a tool loop, push `res.into_message()` before adding tool results.
6. `Event` and `FinishReason` are `#[non_exhaustive]`: `match` needs a `_ => {}` arm.
7. Text parameters take `impl Into<Cow<'static, str>>`: string literals or owned `String`. A borrowed `&String` or non-static `&str` needs `.to_string()` / `.clone()`.
8. Provider-specific options go through `.extra(key, value)`; don't hand-build JSON requests with reqwest.
9. Don't add a dependency on `reqwest`, `serde_json` or `futures` just for aiwire; `aiwire::json!` is re-exported and `stream.next()` needs no import.

## Install

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread   # skip if the app already uses Tokio
cargo add aiwire --features mcp                      # only for MCP servers
```

```toml
[dependencies]
aiwire = "0.1"                       # features: rustls (default), native-tls, mcp
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# native TLS instead of rustls:
# aiwire = { version = "0.1", default-features = false, features = ["native-tls"] }
```

Minimum Rust: 1.85. aiwire is async and runs on Tokio (its HTTP layer is reqwest).

## Imports

Everything is at the crate root except MCP:

```rust,ignore
use aiwire::{
    ChatStream, Client, ClientBuilder, EmbedRequest, Embeddings, Error, Event, FinishReason, Message,
    Protocol, Provider, Request, Response, Result, Role, Text, Tool, ToolCall, ToolChoice, Usage, json,
};
use aiwire::mcp::Mcp; // feature "mcp"
```

## Minimal chat

```rust
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::GROQ)?; // reads GROQ_API_KEY
    let res = client
        .chat(Request::new("llama-3.3-70b-versatile").system("Answer in one sentence.").user("Why is the sky blue?"))
        .await?;
    println!("{}", res.text);
    println!("in={} out={} cached={}", res.usage.input_tokens, res.usage.output_tokens, res.usage.cached_tokens);
    Ok(())
}
```

## Creating a client

```rust
use std::time::Duration;

use aiwire::{Client, Protocol, Provider};

fn clients() -> aiwire::Result<()> {
    // Provider + explicit key (infallible; panics only if TLS can't initialize)
    let _openai = Client::new(Provider::OPENAI, "sk-...");

    // Provider + env var (OPENAI_API_KEY, ANTHROPIC_API_KEY, GROQ_API_KEY, ...)
    let _claude = Client::from_env(Provider::ANTHROPIC)?;

    // Pick the protocol. Errors if the provider doesn't speak it.
    let _minimax = Client::builder().provider(Provider::MINIMAX).protocol(Protocol::Anthropic).api_key("key").build()?;

    // Any OpenAI-compatible server (vLLM, LM Studio, LiteLLM, llama.cpp ...)
    let _local = Client::builder().base_url("http://localhost:8000/v1").build()?;

    // Any Anthropic-compatible server (sends both x-api-key and Bearer auth)
    let _gw = Client::builder().base_url("https://gw.example.com").protocol(Protocol::Anthropic).api_key("k").build()?;

    // Known provider behind a proxy/gateway: keeps its auth style and protocol check
    let _proxied = Client::builder()
        .provider(Provider::OPENAI)
        .base_url("https://my-gateway.example.com/openai/v1")
        .api_key("sk-...")
        .build()?;

    // Every option
    let _full = Client::builder()
        .provider(Provider::OPENROUTER)
        .protocol(Protocol::OpenAi)
        .api_key("sk-or-...")
        .header("HTTP-Referer", "https://myapp.example") // extra header on every request
        .max_retries(3)                                  // default 2
        .timeout(Duration::from_secs(120))               // default 600 s; non-streaming only
        .build()?;
    Ok(())
}
```

Bring your own `reqwest::Client` (proxy, custom certificates, shared pool) with
`.http_client(reqwest::Client)`; that requires `reqwest = "0.13"` in your own `Cargo.toml`.
`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` are honoured by the default client.

`build()` fails with `Error::Config` when: the provider doesn't offer the chosen protocol, no
provider and no `base_url` is set, the provider needs a key and neither `.api_key()` nor its env
var is set, a header is invalid, or the URL doesn't parse.

## Providers

`Provider::X` constants. Default protocol is OpenAI except `ANTHROPIC`.

| Constant | OpenAI | Anthropic | Embeddings | Key env var |
|---|:-:|:-:|:-:|---|
| `OPENAI` | ✓ | | ✓ | `OPENAI_API_KEY` |
| `ANTHROPIC` | ✓ (compat) | ✓ default | | `ANTHROPIC_API_KEY` |
| `GROQ` | ✓ | | | `GROQ_API_KEY` |
| `CEREBRAS` | ✓ | | | `CEREBRAS_API_KEY` |
| `OPENROUTER` | ✓ | ✓ | ✓ | `OPENROUTER_API_KEY` |
| `MINIMAX` | ✓ | ✓ | | `MINIMAX_API_KEY` |
| `ATLAS_CLOUD` | ✓ | ✓ | | `ATLASCLOUD_API_KEY` |
| `GMI_CLOUD` | ✓ | ✓ | | `GMI_API_KEY` |
| `DEEPSEEK` | ✓ | ✓ | | `DEEPSEEK_API_KEY` |
| `MOONSHOT` | ✓ | ✓ | | `MOONSHOT_API_KEY` |
| `ZAI` | ✓ | ✓ | | `ZAI_API_KEY` |
| `TOGETHER` | ✓ | | ✓ | `TOGETHER_API_KEY` |
| `FIREWORKS` | ✓ | ✓ | ✓ | `FIREWORKS_API_KEY` |
| `MISTRAL` | ✓ | | ✓ | `MISTRAL_API_KEY` |
| `XAI` | ✓ | | | `XAI_API_KEY` |
| `GEMINI` | ✓ | | ✓ | `GEMINI_API_KEY` |
| `DEEPINFRA` | ✓ | ✓ | ✓ | `DEEPINFRA_API_KEY` |
| `HUGGINGFACE` | ✓ | ✓ | | `HF_TOKEN` |
| `NEBIUS` | ✓ | | ✓ | `NEBIUS_API_KEY` |
| `SAMBANOVA` | ✓ | ✓ | | `SAMBANOVA_API_KEY` |
| `OLLAMA` | ✓ | ✓ | ✓ | none (`http://localhost:11434`) |

Helpers: `Provider::ALL` (slice), `Provider::by_name("groq")` (case-insensitive),
`p.supports(Protocol::Anthropic)`, `p.url(Protocol::OpenAi)`.

A custom provider is a plain struct (all fields public):

```rust
use aiwire::{Client, Protocol, Provider};

const MY_HOST: Provider = Provider {
    name: "myhost",
    openai_url: Some("https://api.myhost.ai/v1"),  // `/chat/completions`, `/embeddings` appended
    anthropic_url: None,                            // `/v1/messages` appended
    protocol: Protocol::OpenAi,                     // default protocol
    api_key_env: "MYHOST_API_KEY",                  // "" = no key needed
    embeddings: true,
    anthropic_bearer: false,                        // Anthropic endpoint wants Bearer, not x-api-key
    max_tokens_field: "max_tokens",                 // OpenAI itself uses "max_completion_tokens"
};

fn custom() -> aiwire::Result<Client> {
    Client::from_env(MY_HOST)
}
```

## Requests and messages

```rust
use aiwire::{Message, Request, Role, ToolChoice, json};

fn requests() {
    let req = Request::new("gpt-5-mini")             // model: any string
        .system("You are terse.")                    // Role::System
        .user("Hi")                                  // Role::User
        .assistant("Hello!")                         // Role::Assistant (earlier answer)
        .user(format!("Now count to {}", 3))         // owned String works
        .message(Message::user("raw message"))       // any prebuilt Message
        .max_tokens(512)
        .temperature(0.2)
        .top_p(0.9)
        .stop("END")                                 // repeatable
        .tool_choice(ToolChoice::Auto)
        .extra("seed", 42)                           // any other top-level JSON field
        .extra("response_format", json!({"type": "json_object"}));

    // All fields are public
    let mut req = req;
    req.temperature = None;
    req.messages.retain(|m| m.role != Role::System);
    let _ = req;
}
```

`Request` fields: `model: Text`, `messages: Vec<Message>`, `tools: Vec<Tool>`,
`tool_choice: Option<ToolChoice>`, `max_tokens: Option<u32>`, `temperature: Option<f32>`,
`top_p: Option<f32>`, `stop: Vec<Text>`, `extra: serde_json::Map<String, Value>`.

`Message { role: Role, content: Text, tool_calls: Vec<ToolCall>, tool_call_id: Option<String> }`;
constructors `Message::system/user/assistant(text)`, `Message::tool(call_id, text)`.
`Role`: `System`, `User`, `Assistant`, `Tool`. `Text` = `Cow<'static, str>`.

How common parameters map:

| aiwire | OpenAI protocol | Anthropic protocol |
|---|---|---|
| `.max_tokens(n)` | `max_tokens` (`max_completion_tokens` for `Provider::OPENAI`) | `max_tokens`, **default 4096** (required by the API) |
| `.temperature`, `.top_p` | same | same |
| `.stop(s)` | `stop` | `stop_sequences` |
| system messages | `system` role messages | hoisted to top-level `system` blocks |
| tool results | `tool` role messages | merged into one `user` turn of `tool_result` blocks |
| `ToolChoice::Required` | `"required"` | `{"type":"any"}` |

`extra` examples:

```rust
use aiwire::{Request, json};

fn extras() {
    let _openai = Request::new("gpt-5").extra("reasoning_effort", "high").extra("response_format", json!({
        "type": "json_schema",
        "json_schema": {"name": "answer", "schema": {"type": "object", "properties": {"a": {"type": "string"}}}}
    }));
    let _claude_thinking = Request::new("claude-sonnet-5-5")
        .max_tokens(16_000)
        .extra("thinking", json!({"type": "enabled", "budget_tokens": 8_000}));
    let _openrouter = Request::new("meta-llama/llama-4-scout").extra("provider", json!({"sort": "throughput"}));
}
```

## Responses

`chat` returns `Response`:

| Field | Type | Notes |
|---|---|---|
| `text` | `String` | answer text |
| `reasoning` | `String` | `reasoning_content`/`reasoning` (OpenAI-style) or `thinking` blocks (Anthropic); empty if none |
| `tool_calls` | `Vec<ToolCall>` | `ToolCall { id, name, arguments }`, `arguments` is raw JSON |
| `finish_reason` | `FinishReason` | `Stop`, `Length`, `ToolCalls`, `ContentFilter`, `Other(String)`, `Unknown` |
| `usage` | `Usage` | `input_tokens` (cached included), `output_tokens`, `cached_tokens` |
| `id`, `model` | `String` | |

`res.into_message()` turns it into the assistant `Message` for the next turn.

## Multi-turn conversation

```rust
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENAI)?;
    let mut req = Request::new("gpt-5-mini").user("My name is Ana.");
    let res = client.chat(&req).await?;
    req.messages.push(res.into_message());
    req = req.user("What is my name?");
    println!("{}", client.chat(&req).await?.text);
    Ok(())
}
```

## Streaming

```rust
use std::io::Write;

use aiwire::{Client, Event, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::ANTHROPIC)?;
    let mut stream = client.stream(Request::new("claude-haiku-5-5").user("Write a haiku about Rust.")).await?;
    while let Some(event) = stream.next().await {
        match event? {
            Event::Text(t) => {
                print!("{t}");
                std::io::stdout().flush().ok();
            }
            Event::Reasoning(t) => eprint!("{t}"),
            Event::ToolCall(call) => println!("\ntool call: {} {}", call.name, call.arguments),
            _ => {}
        }
    }
    println!();
    Ok(())
}
```

- `Event::Text` / `Event::Reasoning`: deltas. `Event::ToolCall`: once per call, arguments complete.
- `stream.response().await?` drains the rest and returns the assembled `Response` (text, tool calls,
  finish reason, usage). Usage is included on both protocols.
- `ChatStream` also implements `futures_core::Stream<Item = aiwire::Result<Event>>`.
- An error event mid-stream yields `Err(Error::Api { status: 0, .. })`. Retries happen only before
  the stream starts. Streams have no total timeout.

Collect text and get usage at the end:

```rust
use aiwire::{Client, Event, Request};

async fn stream_then_usage(client: &Client, req: &Request) -> aiwire::Result<String> {
    let mut stream = client.stream(req).await?;
    let mut out = String::new();
    while let Some(ev) = stream.next().await {
        if let Event::Text(t) = ev? {
            out.push_str(&t);
        }
    }
    let res = stream.response().await?;
    println!("{} output tokens, finish: {:?}", res.usage.output_tokens, res.finish_reason);
    Ok(out)
}
```

## Tool calling

```rust
use aiwire::{Client, Provider, Request, Tool, json};
use serde::Deserialize;

#[derive(Deserialize)]
struct WeatherArgs {
    city: String,
}

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENROUTER)?;
    let weather = Tool::new(
        "get_weather",
        "Current weather for a city",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    );
    let mut req = Request::new("openai/gpt-5-mini").user("Is it raining in Dhaka?").tool(weather);

    loop {
        let res = client.chat(&req).await?;
        if res.tool_calls.is_empty() {
            println!("{}", res.text);
            return Ok(());
        }
        let calls = res.tool_calls.clone();
        req.messages.push(res.into_message()); // assistant turn with its tool calls, FIRST
        for call in calls {
            let output = match call.name.as_str() {
                "get_weather" => {
                    let args: WeatherArgs = call.args()?; // parse arguments into your type
                    format!("{}: 31°C, light rain", args.city)
                }
                other => format!("unknown tool {other}"),
            };
            req = req.tool_result(call.id, output);
        }
    }
}
```

(`serde` with `derive` is the user's own dependency for typed args; `call.args::<serde_json::Value>()` also works.)

- `Tool::new(name, description, json_schema_value)`; the schema is serialized once at creation.
- `.tool(t)` adds one, `.tools(iter)` adds many.
- `ToolChoice::Auto` (default), `ToolChoice::None`, `ToolChoice::Required`, `ToolChoice::Tool("name".into())`.
- Parallel calls: answer every entry in `res.tool_calls`.
- Same code on both protocols; aiwire translates tool definitions, calls and results.
- Streaming: each finished call arrives as `Event::ToolCall`; `stream.response().await?.tool_calls` has them all.

## MCP servers (feature `mcp`)

```rust
use aiwire::mcp::Mcp;
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENAI)?;
    // stdio server; killed when `mcp` is dropped. Windows: "npx.cmd" not "npx".
    let mcp = Mcp::stdio("uvx", ["mcp-server-time"]).await?;
    // remote: Mcp::http("https://example.com/mcp", Some("bearer-token")).await?
    let mut req = Request::new("gpt-5-mini").user("What time is it in Tokyo?").tools(mcp.tools().await?);

    loop {
        let res = client.chat(&req).await?;
        if res.tool_calls.is_empty() {
            println!("{}", res.text);
            return Ok(());
        }
        let calls = res.tool_calls.clone();
        req.messages.push(res.into_message());
        for call in &calls {
            req = req.tool_result(call.id.clone(), mcp.call(call).await?);
        }
    }
}
```

- `Mcp::stdio(program, args)`, `Mcp::http(url, Option<&str> bearer)`: connect and initialize.
- `mcp.tools()` → `Vec<Tool>` (follows pagination).
- `mcp.call(&ToolCall)` → `String` text output. A tool-side error returns `Ok("Error: ...")` so the
  model can recover; transport failures are `Err(Error::Mcp(..))`.
- `mcp.request(method, params)` → `serde_json::Value` result for any JSON-RPC method
  (`resources/list`, `prompts/get`, ...).
- One request at a time per connection; server requests other than `ping` are declined.

Several servers, routed by tool name:

```rust
use aiwire::mcp::Mcp;
use aiwire::{Request, ToolCall};

async fn two_servers(req: Request) -> aiwire::Result<(Request, Mcp, Mcp, Vec<String>)> {
    let time = Mcp::stdio("uvx", ["mcp-server-time"]).await?;
    let fetch = Mcp::stdio("uvx", ["mcp-server-fetch"]).await?;
    let time_tools = time.tools().await?;
    let names = time_tools.iter().map(|t| t.name.to_string()).collect();
    let req = req.tools(time_tools).tools(fetch.tools().await?);
    Ok((req, time, fetch, names))
}

async fn route(call: &ToolCall, time: &Mcp, fetch: &Mcp, time_names: &[String]) -> aiwire::Result<String> {
    let server = if time_names.contains(&call.name) { time } else { fetch };
    server.call(call).await
}
```

## Embeddings

```rust
use aiwire::{Client, EmbedRequest, Provider};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENAI)?;
    let e = client
        .embed(EmbedRequest::new("text-embedding-3-small").inputs(["cats", "dogs"]).dimensions(256))
        .await?;
    println!("{} vectors x {} dims, {} tokens", e.len(), e.dim(), e.usage.input_tokens);
    let first: &[f32] = e.get(0).expect("one vector per input");
    println!("{:?}", &first[..3]);
    for (i, v) in e.iter().enumerate() {
        println!("{i}: {}", v.len());
    }
    let flat: Vec<f32> = e.into_flat(); // row-major, len() * dim() floats
    println!("{}", flat.len());
    Ok(())
}
```

- `EmbedRequest::new(model)`, `.input(text)`, `.inputs(iter)`, `.dimensions(n)`, `.extra(k, v)`.
- `Embeddings`: `len()`, `is_empty()`, `dim()`, `get(i)`, `iter()`, `as_flat()`, `into_flat()`, `model`, `usage`.
  All vectors live in one contiguous `Vec<f32>`, in input order.
- Uses the OpenAI `/embeddings` format. Providers without embeddings return `Error::Config` without
  a network call. An Anthropic-protocol client for a provider that has embeddings (e.g. OpenRouter)
  embeds through that provider's OpenAI endpoint.

## Concurrency

```rust
use aiwire::{Client, Request};

async fn many(client: &Client, prompts: Vec<String>) -> Vec<aiwire::Result<String>> {
    let handles: Vec<_> = prompts
        .into_iter()
        .map(|p| {
            let client = client.clone(); // shares the pool
            tokio::spawn(async move { client.chat(Request::new("gpt-5-mini").user(p)).await.map(|r| r.text) })
        })
        .collect();
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.expect("task panicked"));
    }
    out
}
```

There is no client-side concurrency limit; bound it yourself (e.g. `tokio::sync::Semaphore`) to
respect provider rate limits. 429s are retried with backoff automatically.

## Errors

```rust
use aiwire::{Client, Error, Request};

async fn handle(client: &Client, req: &Request) {
    match client.chat(req).await {
        Ok(res) => println!("{}", res.text),
        Err(Error::Api { status: 401, .. }) => eprintln!("bad API key"),
        Err(Error::Api { status: 429, message }) => eprintln!("rate limited after retries: {message}"),
        Err(Error::Api { status, message }) => eprintln!("server error {status}: {message}"),
        Err(Error::Config(m)) => eprintln!("setup problem: {m}"),
        Err(e) => eprintln!("{e}"), // Http, Json, Mcp
    }
}
```

| Variant | Meaning |
|---|---|
| `Config(String)` | bad setup: protocol not offered, missing key, bad URL/header, no embeddings |
| `Http(reqwest::Error)` | network: DNS, TLS, reset, timeout |
| `Api { status, message }` | server error; `status == 0` means an error event inside a stream |
| `Json(serde_json::Error)` | unexpected response shape, or invalid tool-call arguments |
| `Mcp(String)` | MCP server failure |

`err.status()` → `Option<u16>` for `Api` and `Http`. `Error` is `#[non_exhaustive]`.

## Defaults and behaviour

| Setting | Default |
|---|---|
| Retries | 2, on connection errors/timeouts and HTTP 408, 409, 429, 5xx |
| Backoff | 0.5 s, 1 s, 2 s, 4 s, 8 s (±12% jitter); honours `retry-after` / `retry-after-ms` up to 60 s |
| Request timeout | 600 s, non-streaming only |
| Connect timeout | 10 s |
| Pool idle | 90 s; TCP nodelay; keepalive 30 s; HTTP/2 when negotiated |
| `anthropic-version` | `2023-06-01` (override with `.header("anthropic-version", "...")`; `2026-10-21` rejects `temperature`/`top_p`) |
| Anthropic `max_tokens` | 4096 when unset |
| OpenAI streams | `stream_options.include_usage = true` |

## Known limits (0.1)

- Text-only messages: no image/audio input or generation.
- No OpenAI Responses API; Chat Completions only.
- Anthropic thinking blocks are not replayed in tool loops (matters only for extended thinking + tools).
- No prompt-caching helpers; use `.extra(...)` or provider defaults.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `config: groq does not offer the Anthropic protocol` | use `Protocol::OpenAi`, or a provider with ✓ Anthropic |
| `config: missing API key: call .api_key() or set X` | set the env var named in the message |
| `config: this provider has no embeddings endpoint` | pick a provider with ✓ Embeddings |
| `api error 400 ... max_tokens` on OpenAI reasoning models | aiwire already sends `max_completion_tokens` for `Provider::OPENAI`; via a custom `base_url` use `.extra("max_completion_tokens", n)` instead of `.max_tokens` |
| `api error 400 ... temperature` on Anthropic | remove `.temperature`/`.top_p`, or keep `anthropic-version: 2023-06-01` |
| borrowed-value / `'static` errors on `.user(&s)` | pass `s.clone()` or `s.to_string()` |
| `Mcp::stdio("npx", ..)` fails on Windows | use `"npx.cmd"` |
| non-exhaustive pattern on `Event` | add `_ => {}` |
