# 1. Installation

aiwire is async and runs on [Tokio](https://tokio.rs). Add both:

```bash
cargo add aiwire
cargo add tokio --features macros,rt-multi-thread
```

Or in `Cargo.toml`:

```toml
[dependencies]
aiwire = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Minimum Rust version: 1.85.

## Features

| Feature | Default | What it does |
|---|---|---|
| `rustls` | yes | TLS with rustls. |
| `native-tls` | no | TLS with the OS stack (SChannel, Secure Transport, OpenSSL) instead. |
| `mcp` | no | [MCP client](06-mcp.md) for stdio and HTTP servers. |

Enable MCP:

```bash
cargo add aiwire --features mcp
```

Use native TLS instead of rustls:

```toml
aiwire = { version = "0.1", default-features = false, features = ["native-tls"] }
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
