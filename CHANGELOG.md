# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor bump
(0.x → 0.y) may break the API; patch releases never do.

## [Unreleased]

## [0.2.0] - 2026-10-23

Tested live against a real OpenAI- and Anthropic-compatible server (`tests/live.rs`, opt-in).

### Added
- Image input on every protocol: `Image::url`, `Image::file`, `Image::bytes`, `Image::base64`,
  `Request::image`, `Message::image`; images in Anthropic tool results; `Part::Raw` for any
  other content block (PDF, audio, …).
- OpenAI Responses API as `Protocol::Responses` (OpenAI, Groq, xAI, OpenRouter, Ollama).
- Reasoning kept in tool loops: signed Anthropic `thinking` / `redacted_thinking` blocks are
  replayed; `reasoning_content` is sent back to DeepSeek, Moonshot and Z.ai
  (`Provider::send_reasoning`).
- Anthropic prompt caching: `Request::cache(Cache::FiveMinutes | Cache::OneHour)` places
  `cache_control` breakpoints; `Message::cache()` for explicit ones.
- `Usage::cache_write_tokens` and `Usage::reasoning_tokens`.
- `n > 1` choices (`Response::other_choices`) and logprobs (`Response::logprobs`) on the OpenAI
  protocol.
- Stream idle timeout (`ClientBuilder::stream_idle_timeout`, default 5 min), which also bounds
  the wait for response headers; `Error::Timeout`.
- Client-side concurrency limit: `ClientBuilder::max_concurrent_requests`.
- Debuggable errors: `Error::Api(Box<ApiError>)` with status, provider error kind, message,
  request id, provider, URL, retry-after, rate-limit headers, raw body and attempt count;
  `Error::is_retryable`, `Error::api`, `Error::request_id`; hints for wrong base URLs and
  redirects. `Response::request_id`, `ChatStream::request_id`.
- `tracing` feature: logs requests, responses, retries, failures and stream ends.
- MCP: concurrent requests (replies routed by id), per-request timeouts with cancellation,
  `Mcp::command`, `Mcp::http_headers`, OAuth 2.1 login (`Mcp::http_oauth`: discovery, dynamic
  client registration, PKCE, refresh, saved `Tokens`), automatic session re-initialization,
  session `DELETE` on drop.

### Changed
- **Breaking:** `Message::content` is now `Vec<Part>`; use `Message::text()` to read it.
  `Message` gained `thinking` and `cache`.
- **Breaking:** `Error::Api { status, message }` became `Error::Api(Box<ApiError>)`.
  Unparseable responses are `Error::Api` with `kind == "invalid_response"` and the body, not
  `Error::Json`.
- **Breaking:** `Provider` gained `responses` and `send_reasoning`; `Usage` gained two fields;
  `Response` gained `thinking`, `logprobs`, `other_choices` and `request_id`.
- `Image::file` errors are `Error::Config` naming the path.
- Errors returned with HTTP 200 (`{"error": …}`, MiniMax `base_resp`) are reported as errors.
- Anthropic streams take tool input sent whole in `content_block_start`.
- OpenAI responses whose `content` is a list of parts (Mistral reasoning models) parse.
- 301/302 redirects are reported instead of followed (following turned POST into GET).
- `x-should-retry` response header is honoured.

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

[Unreleased]: https://github.com/nettanvirdev/aiwire/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/nettanvirdev/aiwire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nettanvirdev/aiwire/releases/tag/v0.1.0
