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

## Streaming with tools

Each finished call arrives as `Event::ToolCall(call)`; `stream.response().await?.tool_calls`
has them all. See [Streaming](04-streaming.md).

Next: [MCP servers](06-mcp.md).
