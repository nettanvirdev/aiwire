# 9. Client configuration

Defaults are chosen so you rarely need this page.

```rust
use std::time::Duration;
use aiwire::{Client, Protocol, Provider};

let client = Client::builder()
    .provider(Provider::OPENAI)          // URL, auth style, key env var
    .protocol(Protocol::OpenAi)          // checked against the provider
    .base_url("https://gateway/v1")      // override the URL (proxy, gateway, local server)
    .api_key("sk-...")                   // default: the provider's env var
    .header("OpenAI-Organization", "org-...") // extra header on every request
    .max_retries(3)                      // default 2
    .timeout(Duration::from_secs(120))   // default 10 min, non-streaming only
    .stream_idle_timeout(Duration::from_secs(60)) // default 5 min of silence ends a stream
    .max_concurrent_requests(16)         // default unlimited
    .build()?;
```

## Defaults

| Setting | Default | Why |
|---|---|---|
| Retries | 2 | on connection errors, timeouts, 408, 409, 429, 5xx; `x-should-retry` is honoured |
| Backoff | 0.5 s, 1 s, 2 s … ±12% jitter | honours `retry-after` / `retry-after-ms` up to 60 s |
| Request timeout | 10 min | long reasoning answers (non-streaming) |
| Stream idle timeout | 5 min | a stalled stream or a server that never answers fails instead of hanging |
| Concurrency limit | none | set `max_concurrent_requests` to stay under provider rate limits |
| Redirects | 307/308 followed, 301/302 reported | a 301 would turn the POST into a GET; the error names the new URL |
| Connect timeout | 10 s | fail fast on dead hosts |
| Idle connections | kept 90 s | reuse TLS connections between calls |
| TCP | `nodelay`, keepalive 30 s | low latency, long streams survive NAT |
| HTTP/2 | negotiated, adaptive window | multiplexing, fast streams |
| `anthropic-version` | `2023-06-01` | widest compatibility; see below |

## Anthropic API version

The newest Anthropic API version (`2026-10-21`) rejects `temperature` / `top_p`. aiwire stays
on `2023-06-01` by default. To opt in:

```rust
Client::builder().provider(Provider::ANTHROPIC).header("anthropic-version", "2026-10-21").build()?;
```

## Your own HTTP client

For proxies, custom certificates, or sharing one pool with the rest of your app:

```rust
let http = reqwest::Client::builder()
    .proxy(reqwest::Proxy::all("http://proxy:3128")?)
    .build()?;
let client = Client::builder().provider(Provider::GROQ).http_client(http).build()?;
```

`HTTP_PROXY` / `HTTPS_PROXY` env vars are honoured by the default client too.

## High throughput

- Build one `Client` and `clone()` it into tasks: clones share the pool.
- Pass `&req` to avoid cloning big conversations.
- Use string literals for fixed prompts: they are stored without copying.
- Run requests concurrently with `tokio::spawn` or `futures::future::join_all`. To cap how many
  run at once (rate limits, a small local server), set `max_concurrent_requests(n)`: extra
  calls wait for a free slot, and a stream keeps its slot until it ends or is dropped.

Next: [Errors and debugging](10-errors.md).
