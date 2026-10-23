# 4. Streaming

Call `stream` instead of `chat`, then read events:

```rust
use aiwire::{Client, Event, Provider, Request};
use std::io::Write;

let client = Client::from_env(Provider::ANTHROPIC)?;
let mut stream = client
    .stream(Request::new("claude-haiku-5-5").user("Write a haiku about Rust."))
    .await?;

while let Some(event) = stream.next().await {
    match event? {
        Event::Text(t) => { print!("{t}"); std::io::stdout().flush().ok(); }
        Event::Reasoning(t) => eprint!("{t}"),       // thinking, if the model sends it
        Event::ToolCall(call) => println!("\n{call:?}"), // complete, with all its arguments
        _ => {}
    }
}
```

No extra imports are needed for `next()`.

## Events

| Event | When |
|---|---|
| `Event::Text(String)` | each text delta |
| `Event::Reasoning(String)` | each reasoning / thinking delta |
| `Event::ToolCall(ToolCall)` | once per tool call, after its arguments are complete |

Tool-call argument fragments are joined for you; you never see partial JSON.

## The full response at the end

`response()` drains whatever is left and returns the assembled `Response`
(text, tool calls, finish reason, usage). You can call it after reading some events, or
right away if you only want streaming for its lower time-to-first-byte on the wire:

```rust
let mut stream = client.stream(&req).await?;
while let Some(ev) = stream.next().await {
    if let Event::Text(t) = ev? { print!("{t}"); }
}
let res = stream.response().await?;
println!("\n{} tokens out", res.usage.output_tokens);
```

Usage is included in streams on both protocols (aiwire sets `stream_options.include_usage`
on OpenAI).

## Using it as a `futures::Stream`

`ChatStream` implements `futures_core::Stream`, so combinators work too:

```rust
use futures::StreamExt;
let texts: Vec<_> = client.stream(&req).await?
    .filter_map(|e| async move { match e { Ok(aiwire::Event::Text(t)) => Some(t), _ => None } })
    .collect().await;
```

## Errors mid-stream

If the server sends an error event (e.g. Anthropic `overloaded_error`), `next()` yields
`Err(Error::Api(e))` with `e.status == 0` and the provider's error type in `e.kind`. Retries
only happen before the stream starts; `err.is_retryable()` tells you whether starting over is
worth it. Malformed data from the server ends the stream with `kind = "invalid_response"` and
the offending line in `e.body`.

## Stalled streams

A stream that goes quiet ends with `Error::Timeout` after 5 minutes without a single byte
(also while waiting for the response headers). Lower it to fail fast, or raise it for models
that think silently for a long time before the first token:

```rust
let client = Client::builder()
    .provider(Provider::OPENAI)
    .stream_idle_timeout(std::time::Duration::from_secs(60))
    .build()?;
```

Servers that send keep-alive comments or pings never trip it while they are alive.

## Request id

`stream.request_id()` (and `res.request_id` on the final response) is the provider's id for
this call; include it when reporting a problem to them.

## Several choices

With `.extra("n", 3)` on the OpenAI protocol, events are emitted for the first choice and the
others are assembled into `res.other_choices`.

## Memory

The SSE parser reads lines straight out of the network buffer and only copies a line that is
split across two network reads. Each event is parsed into small typed structs; unknown fields
are skipped without allocating.

Next: [Tools](05-tools.md).
