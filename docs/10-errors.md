# 10. Errors and debugging

Every call returns `aiwire::Result<T>`; the error type is `aiwire::Error`:

| Variant | Meaning | Typical fix |
|---|---|---|
| `Config(String)` | bad setup, or a request the protocol cannot express | fix the builder call or request |
| `Http(reqwest::Error)` | network failure (DNS, TLS, reset, whole-request timeout) | retry later; check proxy |
| `Api(Box<ApiError>)` | the server said no, or sent something that is not the expected JSON | read the details below |
| `Timeout(Duration)` | no bytes from the server for that long (stalled stream, silent server, MCP request) | retry; raise the timeout |
| `Json(serde_json::Error)` | JSON you supplied did not parse (e.g. `call.args::<T>()`) | fix the type or arguments |
| `Mcp(String)` | MCP server failure | check the server |

## Everything about a failed call

`ApiError` carries what you need to fix the problem or report it to the provider:

| Field | Example |
|---|---|
| `status` | `429` (0 when the error came inside an open stream, 200 for an unreadable success body) |
| `kind` | provider error type or code: `rate_limit_error`, `invalid_api_key`, `overloaded_error`, `invalid_response` |
| `message` | the provider's message |
| `request_id` | from the headers or body: quote it to the provider |
| `provider` | `openai`, `groq`, … or `custom` for a bare base URL |
| `url` | the endpoint that was called |
| `retry_after` | how long the server asked you to wait |
| `headers` | rate-limit headers (`x-ratelimit-*`, `anthropic-ratelimit-*`), request ids, `retry-after`, `location` |
| `body` | the raw response body (first 8 KiB) |
| `attempts` | how many tries were made, retries included |

`Display` puts the important parts on one line, ready for a log:

```text
openai error 429 rate_limit_exceeded: Rate limit reached for gpt-5-mini (https://api.openai.com/v1/chat/completions, request id req_42, 3 attempts, retry after 7s)
```

and `{:#?}` prints every field. A wrong base URL or a redirect gets a hint:

```text
custom error 404: 404 page not found (https://host/v2/chat/completions); check the base URL (aiwire appends /chat/completions, /v1/messages, /responses or /embeddings)
custom error 301: Moved Permanently (http://host/v1/chat/completions); the server redirects to https://host/v1/chat/completions: use that as the base URL
```

## Handling errors

```rust
use aiwire::Error;

match client.chat(&req).await {
    Ok(res) => println!("{}", res.text),
    Err(Error::Api(e)) if e.status == 401 => eprintln!("bad API key: {}", e.message),
    Err(Error::Api(e)) if e.status == 429 => eprintln!("rate limited; wait {:?}", e.retry_after),
    Err(e) if e.is_retryable() => eprintln!("temporary problem, try again: {e}"),
    Err(e) => eprintln!("{e}"),
}
```

- `err.status()`: HTTP status of `Api` and `Http` errors.
- `err.api()`: the `ApiError`, if any. `err.request_id()`: its request id.
- `err.is_retryable()`: network failures, timeouts, 408/409/429/5xx and overload errors
  (inside streams too).

429 and 5xx are retried automatically before you see them (see
[configuration](09-configuration.md)); `attempts` shows how many tries were made. Some
providers answer HTTP 200 with an error in the body; those become `Api` errors too.

## Logging with `tracing`

Turn on the `tracing` feature and every request, retry, response and stream end is logged:

```bash
cargo add aiwire --features tracing
```

| Level | Message | Fields |
|---|---|---|
| `debug` | `aiwire request` | provider, url, attempt, bytes, stream |
| `debug` | `aiwire response` | provider, status, request_id, elapsed_ms |
| `debug` | `aiwire chat finished` | request_id, input/output/cached tokens, finish_reason, elapsed_ms |
| `debug` | `aiwire stream finished` | provider, request_id, input/output/cached tokens, elapsed_ms |
| `warn` | `aiwire retrying` | attempt, wait_ms, reason |
| `warn` | `aiwire request failed`, `aiwire stream failed` | the full error |
| `trace` | `aiwire request body` | the JSON sent (contains your prompts) |

With `tracing-subscriber`, `RUST_LOG=aiwire=debug` shows them. API keys never appear: they
travel in headers marked sensitive.

Next: [Benchmarks](11-benchmarks.md).
