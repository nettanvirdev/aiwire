# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor bump
(0.x → 0.y) may break the API; patch releases never do.

## [Unreleased]

## [0.1.0] - 2026-10-22

### Added
- `Client` speaking OpenAI Chat Completions and Anthropic Messages.
- Provider table with protocol checks: OpenAI, Anthropic, Groq, Cerebras, OpenRouter, MiniMax,
  Atlas Cloud, GMI Cloud, DeepSeek, Moonshot, Z.ai, Together, Fireworks, Mistral, xAI, Gemini,
  DeepInfra, Hugging Face, Nebius, SambaNova, Ollama.
- Streaming with `ChatStream` (`next()`, `futures_core::Stream`, `response()`).
- Tool calling, tool choice, `extra` passthrough for provider-specific fields.
- Embeddings into one contiguous buffer.
- MCP client over stdio and Streamable HTTP (feature `mcp`).
- Retries with backoff and `retry-after`, timeouts, custom headers, bring-your-own `reqwest::Client`.

[Unreleased]: https://github.com/nettanvirdev/aiwire/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nettanvirdev/aiwire/releases/tag/v0.1.0
