# Contributing

Thanks for helping. Bug reports, provider updates and fixes are all welcome.

## Before you start

- Questions and ideas: [Discussions](https://github.com/nettanvirdev/aiwire/discussions).
- Bugs and feature requests: [issues](https://github.com/nettanvirdev/aiwire/issues/new/choose).
- For a bigger change, open an issue first so we can agree on the approach.

## Development

```bash
cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --all
```

Tests run against local fake servers, so you don't need API keys. Please don't add tests that
call a real provider.

## Adding or fixing a provider

Providers live in `src/provider.rs`. Every URL must come from the provider's official docs:
link the docs page in your PR, and add the provider to the tables in `README.md` and
`docs/03-providers.md`.

## Pull requests

- Keep each PR to one change, with a test that fails without it.
- Add a line under `[Unreleased]` in `CHANGELOG.md` for user-facing changes.
- The crate is lean on purpose: a new dependency needs a strong reason.

## Benchmarks

To check performance, run `python bench/run.py` before and after your change on the same
machine. See [docs/11-benchmarks.md](docs/11-benchmarks.md).

By contributing, you agree your work is released under the [MIT license](LICENSE).
