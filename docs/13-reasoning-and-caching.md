# 13. Reasoning and prompt caching

## Reasoning

Reasoning models return their thinking next to the answer:

```rust
let res = client.chat(&req).await?;
println!("thought: {}", res.reasoning);   // empty if the provider hides it
println!("answer: {}", res.text);
println!("{} reasoning tokens", res.usage.reasoning_tokens);
```

When streaming it arrives as `Event::Reasoning`. How to turn it on is provider-specific, so it
goes through `extra`:

```rust
use aiwire::{Request, json};

Request::new("claude-sonnet-5-5").extra("thinking", json!({"type": "adaptive"}));        // Anthropic
Request::new("gpt-5").extra("reasoning_effort", "high");                                    // OpenAI chat
Request::new("gpt-5").extra("reasoning", json!({"effort": "high", "summary": "auto"}));     // Responses
```

### Tool loops keep the reasoning

Some APIs need the model's earlier reasoning sent back on the next request:

- **Anthropic**: with thinking on, the assistant turn that called tools must include its signed
  `thinking` blocks unchanged, or the request fails.
- **DeepSeek, Moonshot (Kimi), Z.ai (GLM)**: with tools, assistant turns must include
  `reasoning_content`, or DeepSeek answers HTTP 400.

`res.into_message()` keeps it (in `message.thinking`) and aiwire sends it in the right shape
for each protocol and provider. Unsigned reasoning is never sent to Anthropic, and other
OpenAI-style providers never get the extra field (strict servers such as Groq reject it). For
your own provider entry, set `send_reasoning: true` if it wants `reasoning_content` back.

## Prompt caching

Most providers cache long prompt prefixes automatically (OpenAI, DeepSeek, Gemini, Groq…);
`res.usage.cached_tokens` shows the hits. On the Anthropic protocol caching needs
`cache_control` breakpoints, and aiwire places them for you:

```rust
use aiwire::{Cache, Request};

let req = Request::new("claude-sonnet-5-5")
    .system(long_instructions)    // tools + system prompt get cached
    .user(question)
    .cache(Cache::FiveMinutes);   // or Cache::OneHour
```

With `cache` on, aiwire marks the end of the system prompt (or of the tools, when there is no
system prompt) and the end of the last message. In a multi-turn conversation each request
reuses everything the previous one cached. For a large shared prefix followed by a varying
question, mark the end of the shared part yourself:

```rust
use aiwire::{Message, Request};

let req = Request::new("claude-sonnet-5-5")
    .message(Message::user(big_document).cache())   // breakpoint here
    .user("Question about the document");
```

Anthropic allows at most 4 breakpoints per request, and prefixes shorter than about 1–4K
tokens (depending on the model) are not cached. Usage shows the result:

| `Usage` field | Meaning |
|---|---|
| `input_tokens` | all prompt tokens, cached ones included |
| `cached_tokens` | read from the cache (cheap) |
| `cache_write_tokens` | written to the cache by this request |

On the OpenAI protocols `cache` does nothing; providers that take a cache key (OpenAI's
`prompt_cache_key`) get it through `.extra(...)`.

Next: [back to the index](README.md).
