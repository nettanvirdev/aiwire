# 5. Tools (function calling)

Same code on both protocols.

## Define a tool

`Tool::new(name, description, json_schema)`. `json!` is re-exported:

```rust
use aiwire::{Tool, json};

let weather = Tool::new(
    "get_weather",
    "Current weather for a city",
    json!({
        "type": "object",
        "properties": { "city": { "type": "string" } },
        "required": ["city"]
    }),
);
```

The schema is serialized once, when the tool is created, not on every request.

## The loop

```rust
use aiwire::{Client, Provider, Request};

let client = Client::from_env(Provider::OPENROUTER)?;
let mut req = Request::new("openai/gpt-5-mini").user("Is it raining in Dhaka?").tool(weather);

loop {
    let res = client.chat(&req).await?;
    if res.tool_calls.is_empty() {
        println!("{}", res.text);
        break;
    }
    let calls = res.tool_calls.clone();
    req.messages.push(res.into_message()); // the assistant turn with its tool calls
    for call in calls {
        #[derive(serde::Deserialize)]
        struct Args { city: String }
        let args: Args = call.args()?;        // parse arguments into your type
        let result = format!("{}: 31°C, light rain", args.city);
        req = req.tool_result(call.id, result);
    }
}
```

- `res.tool_calls`: `Vec<ToolCall { id, name, arguments }>`. `arguments` is the raw JSON string.
- `call.args::<T>()`: parse into any `serde::Deserialize` type (or `serde_json::Value`).
- `req.tool_result(id, text)`: send the answer back. On the Anthropic protocol, consecutive
  results are merged into one user turn as the API requires.

## Tool choice

```rust
use aiwire::ToolChoice;

req.tool_choice(ToolChoice::Auto)                 // model decides (default)
req.tool_choice(ToolChoice::Required)             // must call some tool
req.tool_choice(ToolChoice::None)                 // no tools this turn
req.tool_choice(ToolChoice::Tool("get_weather".into())) // must call this one
```

Parallel tool calls work: you get every call in `tool_calls` and answer each one.

## Thinking models in tool loops

`res.into_message()` keeps the model's reasoning on the assistant turn and aiwire sends it back
the way each protocol needs: signed `thinking` blocks on Anthropic (required when thinking is
on), `reasoning_content` for DeepSeek, Moonshot and Z.ai. Always push `res.into_message()`
rather than rebuilding the assistant message by hand. See
[Reasoning and caching](13-reasoning-and-caching.md).

## Images in tool results

On the Anthropic protocol a tool result can carry images:

```rust
use aiwire::{Image, Message};
req = req.message(Message::tool(call.id.clone(), "screenshot attached").image(Image::file("shot.png")?));
```

The OpenAI protocols only accept text there and return `Error::Config` if you try.

## Streaming with tools

Each finished call arrives as `Event::ToolCall(call)`; `stream.response().await?.tool_calls`
has them all. See [Streaming](04-streaming.md).

Next: [MCP servers](06-mcp.md).
