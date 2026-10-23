# 3. Providers and protocols

aiwire speaks three wire formats:

| `Protocol` | Endpoint | Used by |
|---|---|---|
| `Protocol::OpenAi` | `{base}/chat/completions`, `{base}/embeddings` | almost everyone |
| `Protocol::Anthropic` | `{base}/v1/messages` | Anthropic and a growing list of others |
| `Protocol::Responses` | `{base}/responses` | OpenAI's newer API; also Groq, xAI, OpenRouter, Ollama |

A `Provider` is a small table entry: name, base URL per protocol, key env var, auth style and
whether it serves embeddings. If you ask a provider for a protocol it does not speak,
`build()` returns `Error::Config`:

```rust
let err = Client::builder()
    .provider(Provider::GROQ)
    .protocol(Protocol::Anthropic)
    .build()
    .unwrap_err(); // "groq does not offer the Anthropic protocol"
```

## Built-in providers

| Constant | OpenAI | Anthropic | Responses | Embeddings | Key env var |
|---|:-:|:-:|:-:|:-:|---|
| `Provider::OPENAI` | ✓ | | ✓ | ✓ | `OPENAI_API_KEY` |
| `Provider::ANTHROPIC` | ✓ (compat) | ✓ default | | | `ANTHROPIC_API_KEY` |
| `Provider::GROQ` | ✓ | | ✓ | | `GROQ_API_KEY` |
| `Provider::CEREBRAS` | ✓ | | | | `CEREBRAS_API_KEY` |
| `Provider::OPENROUTER` | ✓ | ✓ | ✓ | ✓ | `OPENROUTER_API_KEY` |
| `Provider::MINIMAX` | ✓ | ✓ | | | `MINIMAX_API_KEY` |
| `Provider::ATLAS_CLOUD` | ✓ | ✓ | | | `ATLASCLOUD_API_KEY` |
| `Provider::GMI_CLOUD` | ✓ | ✓ | | | `GMI_API_KEY` |
| `Provider::DEEPSEEK` | ✓ | ✓ | | | `DEEPSEEK_API_KEY` |
| `Provider::MOONSHOT` | ✓ | ✓ | | | `MOONSHOT_API_KEY` |
| `Provider::ZAI` | ✓ | ✓ | | | `ZAI_API_KEY` |
| `Provider::TOGETHER` | ✓ | | | ✓ | `TOGETHER_API_KEY` |
| `Provider::FIREWORKS` | ✓ | ✓ | | ✓ | `FIREWORKS_API_KEY` |
| `Provider::MISTRAL` | ✓ | | | ✓ | `MISTRAL_API_KEY` |
| `Provider::XAI` | ✓ | | ✓ | | `XAI_API_KEY` |
| `Provider::GEMINI` | ✓ | | | ✓ | `GEMINI_API_KEY` |
| `Provider::DEEPINFRA` | ✓ | ✓ | | ✓ | `DEEPINFRA_API_KEY` |
| `Provider::HUGGINGFACE` | ✓ | ✓ | | | `HF_TOKEN` |
| `Provider::NEBIUS` | ✓ | | | ✓ | `NEBIUS_API_KEY` |
| `Provider::SAMBANOVA` | ✓ | ✓ | | | `SAMBANOVA_API_KEY` |
| `Provider::OLLAMA` | ✓ | ✓ | ✓ | ✓ | none (localhost) |

DeepSeek, Moonshot and Z.ai get their reasoning sent back as `reasoning_content` in tool loops
(their thinking models reject the follow-up request otherwise); see
[Reasoning and caching](13-reasoning-and-caching.md).

URLs were checked against each provider's docs on 2026-10-22. The default protocol is OpenAI,
except for Anthropic. Look a provider up by name with `Provider::by_name("groq")`, or list
them all with `Provider::ALL`.

## Any other server (vLLM, LM Studio, LiteLLM, a gateway…)

Skip the provider and give a base URL:

```rust
let client = Client::builder()
    .base_url("http://localhost:8000/v1")
    .protocol(Protocol::OpenAi) // the default for a bare base_url
    .api_key("anything")        // optional
    .build()?;
```

For an unknown Anthropic-compatible server, aiwire sends both `x-api-key` and
`Authorization: Bearer`, so either auth style works.

## Proxies in front of a known provider

`base_url` overrides the provider URL but keeps everything else (auth style, protocol check,
key env var):

```rust
let client = Client::builder()
    .provider(Provider::OPENAI)
    .base_url("https://my-gateway.example.com/openai/v1")
    .build()?;
```

## Your own provider entry

Every `Provider` field is public, so you can add one without waiting for a release:

```rust
use aiwire::{Protocol, Provider};

const MY_HOST: Provider = Provider {
    name: "myhost",
    openai_url: Some("https://api.myhost.ai/v1"),
    anthropic_url: None,
    protocol: Protocol::OpenAi,
    api_key_env: "MYHOST_API_KEY",
    embeddings: true,
    anthropic_bearer: false,
    max_tokens_field: "max_tokens",
    responses: false,
    send_reasoning: false,
};
let client = Client::from_env(MY_HOST)?;
```

## The Responses API

`Protocol::Responses` talks to `/responses` statelessly: the whole conversation goes up every
time, exactly like Chat Completions, so the rest of your code does not change.

```rust
let client = Client::builder().provider(Provider::OPENAI).protocol(Protocol::Responses).build()?;
let res = client.chat(Request::new("gpt-5-mini").user("hi").max_tokens(500)).await?;
```

`max_tokens` becomes `max_output_tokens`, tools are sent with `strict: false`, tool calls and
results become `function_call` / `function_call_output` items, and reasoning summaries arrive
as `res.reasoning` / `Event::Reasoning`. Stop sequences are not part of that API, so
`.stop(...)` is an error there. Server-side state (`previous_response_id`, `store`) and options
like `reasoning` go through `.extra(...)`.

Next: [Streaming](04-streaming.md).
