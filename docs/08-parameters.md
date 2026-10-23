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
| `.cache(Cache::FiveMinutes)` | (automatic on the provider) | `cache_control` breakpoints |

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

## Several answers and log probabilities (OpenAI protocol)

```rust
let res = client.chat(Request::new("gpt-4.1-mini").user("Name a color").extra("n", 3)).await?;
println!("{}", res.text);                       // first choice
for c in &res.other_choices { println!("{}", c.text); }

let res = client
    .chat(Request::new("gpt-4.1-mini").user("Hi").extra("logprobs", true).extra("top_logprobs", 3))
    .await?;
for lp in &res.logprobs {
    println!("{:?} {:.3} alternatives: {:?}", lp.token, lp.logprob, lp.top);
}
```

## Reasoning output

If a model returns reasoning (`reasoning_content` / `reasoning` on OpenAI-style APIs,
`thinking` blocks on Anthropic, reasoning summaries on Responses), it is in `res.reasoning`,
or `Event::Reasoning` when streaming. `res.usage.reasoning_tokens` counts it when the provider
reports it. More in [Reasoning and caching](13-reasoning-and-caching.md).

## Request fields are public

`Request` is a plain struct, so you can also edit it directly:

```rust
let mut req = Request::new("m").user("hi");
req.temperature = Some(0.2);
req.messages.retain(|m| m.role != aiwire::Role::System);
```

Next: [Client configuration](09-configuration.md).
