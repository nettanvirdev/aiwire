# 1. Installation

aiwire is async and runs on [Tokio](https://tokio.rs). Add both:

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread
```

Or in `Cargo.toml`:

```toml
[dependencies]
aiwire = "0.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Why Tokio: aiwire's methods are `async`, and its HTTP layer runs on Tokio. `rt-multi-thread` is
the runtime; `macros` gives you `#[tokio::main]`. If your app already uses Tokio (axum,
actix-web, most Rust servers), skip `cargo add tokio`: aiwire runs on the runtime you have.

Minimum Rust version: 1.85.

## Features

| Feature | Default | What it does |
|---|---|---|
| `rustls` | yes | TLS with rustls. |
| `native-tls` | no | TLS with the OS stack (SChannel, Secure Transport, OpenSSL) instead. |
| `mcp` | no | [MCP client](06-mcp.md) for stdio and HTTP servers, with OAuth. |
| `tracing` | no | [Logs](10-errors.md#logging-with-tracing) every request, retry and stream through `tracing`. |

Enable MCP:

```bash
cargo add aiwire --features mcp
```

Use native TLS instead of rustls:

```toml
aiwire = { version = "0.2", default-features = false, features = ["native-tls"] }
```

## API keys

Each built-in provider reads its key from a conventional env var when you don't pass one,
e.g. `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GROQ_API_KEY`. The full list is in
[Providers](03-providers.md).

```bash
export GROQ_API_KEY=gsk_...
```

On Windows PowerShell: `$env:GROQ_API_KEY = "gsk_..."`.

Next: [Your first request](02-quickstart.md).
