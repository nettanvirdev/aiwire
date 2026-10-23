---
name: aiwire
description: Write Rust code that calls LLMs with the aiwire crate (OpenAI Chat Completions, Anthropic Messages and OpenAI Responses compatible APIs) - chat, streaming, tool calling, images, reasoning/thinking, prompt caching, MCP tools (with OAuth), embeddings, providers, retries, timeouts and debuggable errors. Use when code depends on aiwire, or the user wants to call OpenAI, Anthropic, Groq, Cerebras, OpenRouter, MiniMax, DeepSeek, Gemini, Ollama or any OpenAI/Anthropic-compatible endpoint from Rust.
---

# aiwire

aiwire is an async Rust client for any OpenAI-compatible or Anthropic-compatible LLM API.
One `Client`, three wire formats, 21 built-in providers, any model string.
Crate: https://crates.io/crates/aiwire · Docs: https://docs.rs/aiwire · Repo: https://github.com/nettanvirdev/aiwire

Everything below matches aiwire 0.2. Every complete program here compiles.

## Rules when writing aiwire code

1. Models are plain strings. Never invent model constants; use the id the provider documents.
2. Build one `Client` and reuse it (`clone()` is cheap and shares the connection pool and concurrency limit). Never build one per request.
3. Use `Client::from_env(Provider::X)` unless the user gives a key; it reads the provider's env var.
4. Pass `&req` to `chat`/`stream` when the request is reused (tool loops, multi-turn).
5. In a tool loop, push `res.into_message()` before adding tool results. Never rebuild the assistant turn by hand: `into_message()` carries the reasoning that thinking models need back.
6. `Event`, `FinishReason`, `Part`, `Error` and `Protocol` are `#[non_exhaustive]`: `match` needs a `_ => {}` arm.
7. Text parameters take `impl Into<Cow<'static, str>>`: string literals or owned `String`. A borrowed `&String` or non-static `&str` needs `.to_string()` / `.clone()`.
8. Provider-specific options go through `.extra(key, value)`; don't hand-build JSON requests with reqwest.
9. Don't add a dependency on `reqwest`, `serde_json` or `futures` just for aiwire; `aiwire::json!` is re-exported and `stream.next()` needs no import.
10. To debug a failure, print the error with `{e}` (one line) or `{e:#?}` (every field: status, kind, request id, url, headers, body, attempts).

## Install

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread   # skip if the app already uses Tokio
cargo add aiwire --features mcp                      # MCP servers (stdio, HTTP, OAuth)
cargo add aiwire --features tracing                  # request/retry/stream logs
```

```toml
[dependencies]
aiwire = "0.2"                       # features: rustls (default), native-tls, mcp, tracing
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# native TLS instead of rustls:
# aiwire = { version = "0.2", default-features = false, features = ["native-tls"] }
```

Minimum Rust: 1.85. aiwire is async and runs on Tokio (its HTTP layer is reqwest).

## Imports

Everything is at the crate root except MCP:

```rust,ignore
use aiwire::{
    ApiError, Cache, ChatStream, Choice, Client, ClientBuilder, EmbedRequest, Embeddings, Error, Event,
    FinishReason, Image, Logprob, Message, Part, Protocol, Provider, Request, Response, Result, Role, Text,
    Thinking, Tool, ToolCall, ToolChoice, Usage, json,
};
use aiwire::mcp::{Mcp, OAuth, Tokens}; // feature "mcp"
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
    let _responses = Client::builder().provider(Provider::OPENAI).protocol(Protocol::Responses).api_key("sk-...").build()?;

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
        .stream_idle_timeout(Duration::from_secs(60))    // default 300 s of silence ends a stream
        .max_concurrent_requests(8)                      // default unlimited
        .build()?;
    Ok(())
}
```

Bring your own `reqwest::Client` (proxy, custom certificates, shared pool) with
`.http_client(reqwest::Client)`; that requires `reqwest = "0.13"` in your own `Cargo.toml`.
`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` are honoured by the default client. The default
client follows 307/308 redirects and reports 301/302 as an error naming the new URL (following
them would turn the POST into a GET).

`build()` fails with `Error::Config` when: the provider doesn't offer the chosen protocol, no
provider and no `base_url` is set, the provider needs a key and neither `.api_key()` nor its env
var is set, a header is invalid, or the URL doesn't parse.

## Protocols

| `Protocol` | Endpoint | Notes |
|---|---|---|
| `OpenAi` | `{base}/chat/completions` | default for everything but Anthropic |
| `Anthropic` | `{base}/v1/messages` | system hoisted, tool results merged, `max_tokens` default 4096 |
| `Responses` | `{base}/responses` | stateless (full history each call); `max_tokens` → `max_output_tokens`; tools sent with `strict: false`; `.stop()` is an error |

Embeddings always use the OpenAI `{base}/embeddings` format.

## Providers

`Provider::X` constants. Default protocol is OpenAI except `ANTHROPIC`.

| Constant | OpenAI | Anthropic | Responses | Embeddings | Key env var |
|---|:-:|:-:|:-:|:-:|---|
| `OPENAI` | ✓ | | ✓ | ✓ | `OPENAI_API_KEY` |
| `ANTHROPIC` | ✓ (compat) | ✓ default | | | `ANTHROPIC_API_KEY` |
| `GROQ` | ✓ | | ✓ | | `GROQ_API_KEY` |
| `CEREBRAS` | ✓ | | | | `CEREBRAS_API_KEY` |
| `OPENROUTER` | ✓ | ✓ | ✓ | ✓ | `OPENROUTER_API_KEY` |
| `MINIMAX` | ✓ | ✓ | | | `MINIMAX_API_KEY` |
| `ATLAS_CLOUD` | ✓ | ✓ | | | `ATLASCLOUD_API_KEY` |
| `GMI_CLOUD` | ✓ | ✓ | | | `GMI_API_KEY` |
| `DEEPSEEK` | ✓ | ✓ | | | `DEEPSEEK_API_KEY` |
| `MOONSHOT` | ✓ | ✓ | | | `MOONSHOT_API_KEY` |
| `ZAI` | ✓ | ✓ | | | `ZAI_API_KEY` |
| `TOGETHER` | ✓ | | | ✓ | `TOGETHER_API_KEY` |
| `FIREWORKS` | ✓ | ✓ | | ✓ | `FIREWORKS_API_KEY` |
| `MISTRAL` | ✓ | | | ✓ | `MISTRAL_API_KEY` |
| `XAI` | ✓ | | ✓ | | `XAI_API_KEY` |
| `GEMINI` | ✓ | | | ✓ | `GEMINI_API_KEY` |
| `DEEPINFRA` | ✓ | ✓ | | ✓ | `DEEPINFRA_API_KEY` |
| `HUGGINGFACE` | ✓ | ✓ | | | `HF_TOKEN` |
| `NEBIUS` | ✓ | | | ✓ | `NEBIUS_API_KEY` |
| `SAMBANOVA` | ✓ | ✓ | | | `SAMBANOVA_API_KEY` |
| `OLLAMA` | ✓ | ✓ | ✓ | ✓ | none (`http://localhost:11434`) |

Helpers: `Provider::ALL` (slice), `Provider::by_name("groq")` (case-insensitive),
`p.supports(Protocol::Anthropic)`, `p.url(Protocol::OpenAi)`.

A custom provider is a plain struct (all fields public):

```rust
use aiwire::{Client, Protocol, Provider};

const MY_HOST: Provider = Provider {
    name: "myhost",
    openai_url: Some("https://api.myhost.ai/v1"),  // `/chat/completions`, `/responses`, `/embeddings` appended
    anthropic_url: None,                            // `/v1/messages` appended
    protocol: Protocol::OpenAi,                     // default protocol
    api_key_env: "MYHOST_API_KEY",                  // "" = no key needed
    embeddings: true,
    anthropic_bearer: false,                        // Anthropic endpoint wants Bearer, not x-api-key
    max_tokens_field: "max_tokens",                 // OpenAI itself uses "max_completion_tokens"
    responses: false,                               // serves /responses
    send_reasoning: false,                          // wants `reasoning_content` back (DeepSeek-style)
};

fn custom() -> aiwire::Result<Client> {
    Client::from_env(MY_HOST)
}
```

## Requests and messages

```rust
use aiwire::{Cache, Image, Message, Request, Role, ToolChoice, json};

fn requests() {
    let req = Request::new("gpt-5-mini")             // model: any string
        .system("You are terse.")                    // Role::System
        .user("Hi")                                  // Role::User
        .assistant("Hello!")                         // Role::Assistant (earlier answer)
        .user(format!("Now count to {}", 3))         // owned String works
        .image(Image::url("https://example.com/cat.png")) // appended to the last user message
        .message(Message::user("raw message"))       // any prebuilt Message
        .max_tokens(512)
        .temperature(0.2)
        .top_p(0.9)
        .stop("END")                                 // repeatable
        .tool_choice(ToolChoice::Auto)
        .cache(Cache::FiveMinutes)                   // Anthropic prompt-cache breakpoints
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
`top_p: Option<f32>`, `stop: Vec<Text>`, `cache: Cache`, `extra: serde_json::Map<String, Value>`.

`Message { role: Role, content: Vec<Part>, tool_calls: Vec<ToolCall>, tool_call_id: Option<String>, thinking: Vec<Thinking>, cache: bool }`.
Constructors `Message::system/user/assistant(text)`, `Message::tool(call_id, text)`; builders
`.image(Image)`, `.part(impl Into<Part>)`, `.cache()`; `msg.text()` joins the text parts.
`Part`: `Text(Text)`, `Image(Image)`, `Raw(serde_json::Value)` (sent as-is, in the protocol's own format).
`Role`: `System`, `User`, `Assistant`, `Tool`. `Text` = `Cow<'static, str>`.

How common parameters map:

| aiwire | OpenAI protocol | Anthropic protocol | Responses protocol |
|---|---|---|---|
| `.max_tokens(n)` | `max_tokens` (`max_completion_tokens` for `Provider::OPENAI`) | `max_tokens`, **default 4096** | `max_output_tokens` |
| `.temperature`, `.top_p` | same | same | same |
| `.stop(s)` | `stop` | `stop_sequences` | not supported (error) |
| system messages | `system` role messages | hoisted to top-level `system` blocks | `system` role input items |
| tool results | `tool` role messages | merged into one `user` turn of `tool_result` blocks | `function_call_output` items |
| `ToolChoice::Required` | `"required"` | `{"type":"any"}` | `"required"` |
| images | `image_url` parts | `image` blocks (base64 or url source) | `input_image` parts |
| `.cache(..)` | ignored (provider caches itself) | `cache_control` breakpoints | ignored |

`extra` examples:

```rust
use aiwire::{Request, json};

fn extras() {
    let _openai = Request::new("gpt-5").extra("reasoning_effort", "high").extra("response_format", json!({
        "type": "json_schema",
        "json_schema": {"name": "answer", "schema": {"type": "object", "properties": {"a": {"type": "string"}}}}
    }));
    let _claude_thinking = Request::new("claude-sonnet-5-5").max_tokens(16_000).extra("thinking", json!({"type": "adaptive"}));
    let _openrouter = Request::new("meta-llama/llama-4-scout").extra("provider", json!({"sort": "throughput"}));
    let _several = Request::new("gpt-4.1-mini").extra("n", 3).extra("logprobs", true).extra("top_logprobs", 2);
}
```

## Responses

`chat` returns `Response`:

| Field | Type | Notes |
|---|---|---|
| `text` | `String` | answer text (a refusal's text, with `finish_reason == ContentFilter`) |
| `reasoning` | `String` | `reasoning_content`/`reasoning`, Anthropic `thinking`, Responses reasoning summaries; empty if none |
| `thinking` | `Vec<Thinking>` | signed Anthropic thinking blocks (`text`, `signature`, `redacted`), kept for replay |
| `tool_calls` | `Vec<ToolCall>` | `ToolCall { id, name, arguments }`, `arguments` is raw JSON |
| `finish_reason` | `FinishReason` | `Stop`, `Length`, `ToolCalls`, `ContentFilter`, `Other(String)`, `Unknown` |
| `usage` | `Usage` | `input_tokens` (cached included), `output_tokens`, `cached_tokens`, `cache_write_tokens`, `reasoning_tokens` |
| `logprobs` | `Vec<Logprob>` | `Logprob { token, logprob, top: Vec<(String, f32)> }` when requested |
| `other_choices` | `Vec<Choice>` | choices 2..n with `.extra("n", k)` (OpenAI protocol) |
| `request_id` | `Option<String>` | provider request id from the response headers |
| `id`, `model` | `String` | |

`res.into_message()` turns it into the assistant `Message` for the next turn (text, tool calls and reasoning).

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
  thinking, finish reason, usage, request id). Usage is included on every protocol.
- `stream.request_id()` is available as soon as the stream starts.
- `ChatStream` also implements `futures_core::Stream<Item = aiwire::Result<Event>>`.
- An error event mid-stream yields `Err(Error::Api(e))` with `e.status == 0` and `e.kind` set
  (e.g. `overloaded_error`); retries happen only before the stream starts.
- No bytes for `stream_idle_timeout` (default 5 min, including the wait for headers) ends the
  stream with `Err(Error::Timeout(d))`.

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
        req.messages.push(res.into_message()); // assistant turn with its tool calls (and reasoning), FIRST
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
  Some gateways ignore forced choices; newest Claude models reject `Required`/`Tool` with a 400.
- Parallel calls: answer every entry in `res.tool_calls`.
- Same code on every protocol; aiwire translates tool definitions, calls and results.
- Thinking models: `into_message()` keeps reasoning; aiwire replays signed thinking blocks on
  Anthropic and `reasoning_content` for DeepSeek/Moonshot/Z.ai (their APIs 400 without it).
- Images in a tool result (Anthropic only): `Message::tool(id, "see image").image(img)`.
- Streaming: each finished call arrives as `Event::ToolCall`; `stream.response().await?.tool_calls` has them all.

## Images and other inputs

```rust
use aiwire::{Client, Image, Message, Part, Provider, Request, json};

async fn vision(client: &Client, png: &[u8]) -> aiwire::Result<String> {
    let req = Request::new("gpt-5-mini")
        .user("Compare these images.")
        .image(Image::bytes("image/png", png))                 // raw bytes
        .image(Image::url("https://example.com/b.jpg"));        // provider downloads it
    let _from_file = Image::file("photo.jpg")?; // png/jpg/gif/webp; a missing file is Error::Config
    let _b64 = Image::base64("image/png", "iVBORw0KGgo...");
    let _mixed = Message::user("Before:").image(Image::url("https://example.com/a.png")).part("After:");
    // Anything else (PDF, audio, ...): a raw block in the protocol's own format
    let _pdf = Message::user("Summarize").part(Part::Raw(json!({
        "type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0..."}
    })));
    let _ = Client::from_env(Provider::OPENAI);
    Ok(client.chat(req).await?.text)
}
```


## Reasoning and prompt caching

```rust
use aiwire::{Cache, Client, Message, Request, json};

async fn reasoning_and_cache(client: &Client, manual: String) -> aiwire::Result<()> {
    // Reasoning: switch it on per provider via extra; read it back.
    let res = client.chat(Request::new("claude-sonnet-5-5").user("Plan a trip").extra("thinking", json!({"type": "adaptive"}))).await?;
    println!("{} / {} reasoning tokens", res.reasoning, res.usage.reasoning_tokens);

    // Anthropic prompt caching: breakpoints after tools/system and after the last message.
    let req = Request::new("claude-sonnet-5-5").system(manual.clone()).user("Q1").cache(Cache::FiveMinutes); // or OneHour
    let res = client.chat(&req).await?;
    println!("cached {} written {}", res.usage.cached_tokens, res.usage.cache_write_tokens);

    // Explicit breakpoint after a shared prefix:
    let _req = Request::new("claude-sonnet-5-5").message(Message::user(manual).cache()).user("Question");
    Ok(())
}
```

OpenAI-style providers cache automatically (`usage.cached_tokens` shows hits); `.cache()` only
affects the Anthropic protocol. Anthropic allows 4 breakpoints and ignores short prefixes.

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

- Connect: `Mcp::stdio(program, args)`, `Mcp::command(tokio::process::Command)` (env, cwd),
  `Mcp::http(url, Option<&str> bearer)`, `Mcp::http_headers(url, HeaderMap)`, `Mcp::http_oauth(url, OAuth)`.
- `mcp.tools()` → `Vec<Tool>` (follows pagination).
- `mcp.call(&ToolCall)` → `String` text output. A tool-side error returns `Ok("Error: ...")` so the
  model can recover; transport failures are `Err(Error::Mcp(..))` / `Err(Error::Api(..))`.
- `mcp.request(method, params)` → `serde_json::Value` result for any JSON-RPC method.
- Requests run concurrently (replies routed by id); `Mcp` is `Send + Sync`, share it with `Arc`.
- `.with_timeout(Duration)`: per-request timeout, default 60 s; on timeout the server gets
  `notifications/cancelled` and you get `Error::Timeout`.
- Server pings are answered; other server requests are declined. Expired HTTP sessions (404) are
  re-initialized automatically.

OAuth (hosted MCP servers that need a login):

```rust
use aiwire::mcp::{Mcp, OAuth, Tokens};

async fn oauth(saved: Option<Tokens>) -> aiwire::Result<Option<Tokens>> {
    let mut auth = OAuth::new("http://127.0.0.1:8765/callback") // loopback redirect for the browser login
        .client_name("My App");                                   // .client(id, secret), .scope("a b") optional
    if let Some(t) = saved {
        auth = auth.tokens(t); // skip the login; refreshed automatically when expired
    }
    let mcp = Mcp::http_oauth("https://mcp.example.com/mcp", auth).await?;
    println!("{} tools", mcp.tools().await?.len());
    Ok(mcp.tokens().await) // Serialize + Deserialize; store like a password
}
```

Discovery (RFC 9728 / 8414), dynamic client registration, PKCE S256 and refresh are automatic.
The default login opens the system browser and waits on the redirect URI; for web apps use
`.on_authorize(|auth_url| async move { /* show url, return the redirect URL or code */ Ok(redirected) })`.

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
use aiwire::{Client, Provider, Request};

async fn many(prompts: Vec<String>) -> aiwire::Result<Vec<aiwire::Result<String>>> {
    // At most 4 requests in flight across all clones; the rest wait. Streams hold a slot until done.
    let client = Client::builder().provider(Provider::OPENAI).max_concurrent_requests(4).build()?;
    let handles: Vec<_> = prompts
        .into_iter()
        .map(|p| {
            let client = client.clone(); // shares the pool and the limit
            tokio::spawn(async move { client.chat(Request::new("gpt-5-mini").user(p)).await.map(|r| r.text) })
        })
        .collect();
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.expect("task panicked"));
    }
    Ok(out)
}
```

429s and 5xx are retried with backoff automatically (`x-should-retry` is honoured).

## Errors and debugging

```rust
use aiwire::{Client, Error, Request};

async fn handle(client: &Client, req: &Request) {
    match client.chat(req).await {
        Ok(res) => println!("{} (request id {:?})", res.text, res.request_id),
        Err(Error::Api(e)) if e.status == 401 => eprintln!("bad API key: {}", e.message),
        Err(Error::Api(e)) if e.status == 429 => eprintln!("rate limited, wait {:?}: {e}", e.retry_after),
        Err(Error::Api(e)) => eprintln!("{e}\n{e:#?}"), // one line, then every field
        Err(Error::Timeout(d)) => eprintln!("no data for {d:?}"),
        Err(Error::Config(m)) => eprintln!("setup problem: {m}"),
        Err(e) if e.is_retryable() => eprintln!("temporary, try again: {e}"),
        Err(e) => eprintln!("{e}"), // Http, Json, Mcp
    }
}
```

| Variant | Meaning |
|---|---|
| `Config(String)` | bad setup, or a request the protocol can't express (e.g. images in OpenAI tool results, `.stop()` on Responses) |
| `Http(reqwest::Error)` | network: DNS, TLS, reset, whole-request timeout |
| `Api(Box<ApiError>)` | server error, error event in a stream, error inside an HTTP 200 body, or an unreadable body (`kind == "invalid_response"`) |
| `Timeout(Duration)` | no bytes for that long: stalled stream, silent server, MCP request |
| `Json(serde_json::Error)` | JSON you supplied did not parse (e.g. `call.args::<T>()`) |
| `Mcp(String)` | MCP server failure |

`ApiError` fields: `status` (0 = inside a stream), `kind` (provider error type/code),
`message`, `request_id`, `provider` (`"custom"` for a bare base URL), `url`, `retry_after`,
`headers` (rate-limit headers, request ids, `location`), `body` (raw, first 8 KiB), `attempts`;
`e.header("x-ratelimit-remaining-requests")`. Helpers on `Error`: `status()`, `api()`,
`request_id()`, `is_retryable()`. Display hints at wrong base URLs (404) and redirects (301/302).

Feature `tracing` logs `aiwire request` / `aiwire response` / `aiwire chat finished` /
`aiwire stream finished` (debug), `aiwire retrying` / `aiwire request failed` (warn) and the
request body (trace). Show with `RUST_LOG=aiwire=debug` and `tracing-subscriber`.

## Defaults and behaviour

| Setting | Default |
|---|---|
| Retries | 2, on connection errors/timeouts and HTTP 408, 409, 429, 5xx; `x-should-retry: false` stops them |
| Backoff | 0.5 s, 1 s, 2 s, 4 s, 8 s (±12% jitter); honours `retry-after` / `retry-after-ms` up to 60 s |
| Request timeout | 600 s, non-streaming only |
| Stream idle timeout | 300 s without a byte (headers included) → `Error::Timeout` |
| Concurrency limit | none (`max_concurrent_requests`) |
| Connect timeout | 10 s |
| Pool idle | 90 s; TCP nodelay; keepalive 30 s; HTTP/2 when negotiated |
| Redirects | 307/308 followed; 301/302 reported as errors |
| `anthropic-version` | `2023-06-01` (override with `.header("anthropic-version", "...")`; `2026-10-21` rejects `temperature`/`top_p`) |
| Anthropic `max_tokens` | 4096 when unset |
| OpenAI streams | `stream_options.include_usage = true` |
| MCP request timeout | 60 s |

## Known limits (0.2)

- No audio or image generation (out of scope); audio/PDF input only through `Part::Raw`.
- Responses API is used statelessly; reasoning items are not replayed between turns (use
  `.extra("previous_response_id", ...)` for server-side state).
- `n > 1` and logprobs are parsed on the OpenAI protocol only; stream events cover the first choice.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `config: groq does not offer the Anthropic protocol` | use `Protocol::OpenAi`, or a provider with ✓ Anthropic |
| `config: missing API key: call .api_key() or set X` | set the env var named in the message |
| `config: this provider has no embeddings endpoint` | pick a provider with ✓ Embeddings |
| `... error 404 ...; check the base URL` | OpenAI-style bases usually end in `/v1`; Anthropic bases do not (aiwire appends `/v1/messages`) |
| `... error 301 ...; the server redirects to https://...` | use the `https://` URL as the base URL |
| `... error 400 ... max_tokens` on OpenAI reasoning models | aiwire already sends `max_completion_tokens` for `Provider::OPENAI`; via a custom `base_url` use `.extra("max_completion_tokens", n)` instead of `.max_tokens` |
| `... error 400 ... temperature` on Anthropic | remove `.temperature`/`.top_p`, or keep `anthropic-version: 2023-06-01` |
| DeepSeek 400 "reasoning_content" in a tool loop | push `res.into_message()` (not a hand-built assistant message) and use `Provider::DEEPSEEK` |
| Anthropic 400 about `thinking` blocks in a tool loop | same: push `res.into_message()` unchanged |
| `timed out: no data from the server` | the stream stalled; raise `.stream_idle_timeout(...)` for slow silent models |
| borrowed-value / `'static` errors on `.user(&s)` | pass `s.clone()` or `s.to_string()` |
| `Mcp::stdio("npx", ..)` fails on Windows | use `"npx.cmd"` |
| non-exhaustive pattern on `Event` / `Error` | add `_ => {}` |
