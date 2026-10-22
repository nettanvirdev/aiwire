# 10. Errors

Every call returns `aiwire::Result<T>`; the error type is `aiwire::Error`:

| Variant | Meaning | Typical fix |
|---|---|---|
| `Config(String)` | bad setup: protocol not offered, missing key, bad URL, no embeddings | fix the builder call |
| `Http(reqwest::Error)` | network failure (DNS, TLS, reset, timeout) | retry later; check proxy |
| `Api { status, message }` | the server said no; `status` is 0 for an error inside a stream | read `message` |
| `Json(serde_json::Error)` | unexpected response shape, or invalid tool-call arguments | report it |
| `Mcp(String)` | MCP server failure | check the server |

```rust
use aiwire::Error;

match client.chat(&req).await {
    Ok(res) => println!("{}", res.text),
    Err(Error::Api { status: 401, .. }) => eprintln!("bad API key"),
    Err(Error::Api { status: 429, message }) => eprintln!("rate limited after retries: {message}"),
    Err(e) => eprintln!("{e}"),
}
```

`err.status()` gives the HTTP status for `Api` and `Http` errors.

429 and 5xx are retried automatically (see [configuration](09-configuration.md)); you only
see them after the retries ran out.

Next: [Benchmarks](11-benchmarks.md).
