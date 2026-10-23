# 2. Your first request

```rust
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::GROQ)?; // reads GROQ_API_KEY

    let res = client
        .chat(Request::new("llama-3.3-70b-versatile")
            .system("Answer in one sentence.")
            .user("Why is the sky blue?"))
        .await?;

    println!("{}", res.text);
    println!("{:?}", res.usage); // input/output/cached tokens
    Ok(())
}
```

That's the whole API shape:

- **`Client`** knows *where* to send requests and in which wire format.
- **`Request`** says *what* to ask. The model is any string; there is no fixed model list.
- **`Response`** holds `text`, `reasoning`, `tool_calls`, `finish_reason`, `usage` and the
  provider's `request_id`.

## Three ways to make a client

```rust
use aiwire::{Client, Protocol, Provider};

// 1. Provider + key
let client = Client::new(Provider::OPENAI, "sk-...");

// 2. Provider + env var
let client = Client::from_env(Provider::ANTHROPIC)?;

// 3. Anything else: builder
let client = Client::builder()
    .provider(Provider::OPENROUTER)
    .protocol(Protocol::Anthropic) // OpenRouter speaks both
    .api_key("sk-or-...")
    .build()?;
```

Build one client and reuse it: it holds the connection pool. `Client` is cheap to `clone()`
and safe to share across tasks.

## Multi-turn conversations

```rust
let mut req = Request::new("gpt-5-mini").user("My name is Ana.");
let res = client.chat(&req).await?;     // pass &req to keep using it
req.messages.push(res.into_message());  // add the assistant turn
req = req.user("What is my name?");
let res = client.chat(&req).await?;
```

`chat` accepts `Request` or `&Request`.

## Messages

| Method | Role |
|---|---|
| `.system(text)` | instructions |
| `.user(text)` | user |
| `.assistant(text)` | earlier model answer |
| `.tool_result(call_id, text)` | result of a tool call |
| `.image(Image)` | adds an image to the last user message ([Images](12-images.md)) |
| `.message(Message)` | any prebuilt message |

Text can be a `&'static str` (stored without copying) or a `String`.

Next: [Providers and protocols](03-providers.md).
