# 8. Parameters and provider-specific options

## Common parameters

These map to the right field on each protocol:

| Method | OpenAI field | Anthropic field |
|---|---|---|
| `.max_tokens(n)` | `max_tokens` (`max_completion_tokens` on OpenAI itself) | `max_tokens` (defaults to 4096, required there) |
| `.temperature(t)` | `temperature` | `temperature` |
| `.top_p(p)` | `top_p` | `top_p` |
| `.stop(s)` (repeatable) | `stop` | `stop_sequences` |
| `.tool(...)`, `.tools(...)` | `tools` | `tools` |
| `.tool_choice(...)` | `tool_choice` | `tool_choice` |

## Everything else: `extra`

Any other top-level field goes through `.extra(key, value)` and is sent as-is. This covers
every provider-specific option without waiting for aiwire to model it:

```rust
use aiwire::json;

// OpenAI-style
Request::new("gpt-5")
    .extra("reasoning_effort", "high")
    .extra("seed", 42)
    .extra("response_format", json!({
        "type": "json_schema",
        "json_schema": { "name": "answer", "schema": { "type": "object" } }
    }));

// Anthropic extended thinking
Request::new("claude-sonnet-5-5")
    .max_tokens(16_000)
    .extra("thinking", json!({ "type": "enabled", "budget_tokens": 8_000 }));

// OpenRouter routing
Request::new("meta-llama/llama-4-scout")
    .extra("provider", json!({ "sort": "throughput" }));
```

An `extra` key with the same name as a common parameter is sent after it; most servers keep
the last value.

## Reasoning output

If a model returns reasoning (`reasoning_content` / `reasoning` on OpenAI-style APIs,
`thinking` blocks on Anthropic), it is in `res.reasoning`, or `Event::Reasoning` when
streaming.

## Request fields are public

`Request` is a plain struct, so you can also edit it directly:

```rust
let mut req = Request::new("m").user("hi");
req.temperature = Some(0.2);
req.messages.retain(|m| m.role != aiwire::Role::System);
```

Next: [Client configuration](09-configuration.md).
