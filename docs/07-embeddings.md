# 7. Embeddings

```rust
use aiwire::{Client, EmbedRequest, Provider};

let client = Client::from_env(Provider::OPENAI)?;
let e = client
    .embed(EmbedRequest::new("text-embedding-3-small").inputs(["cats", "dogs"]))
    .await?;

println!("{} vectors of {} dims", e.len(), e.dim());
let first: &[f32] = e.get(0).unwrap();
for v in e.iter() { /* v: &[f32], in input order */ }
```

## Request options

```rust
EmbedRequest::new("text-embedding-3-large")
    .input("one text")
    .inputs(vec!["more".to_string(), "texts".to_string()])
    .dimensions(256)          // shorten, for models that support it
    .extra("user", "u-123");  // any other field
```

## Memory layout

All vectors live in one contiguous `Vec<f32>`, filled straight from the JSON parser: one
allocation per batch instead of one per vector.

| Method | Returns |
|---|---|
| `len()`, `dim()` | count, vector length |
| `get(i)` | `Option<&[f32]>` |
| `iter()` | `&[f32]` per vector |
| `as_flat()` / `into_flat()` | the whole buffer, row-major, ready for ndarray or a vector DB |

## Which providers

Embeddings use the OpenAI `/embeddings` format. Providers marked ✓ in the
[provider table](03-providers.md) support it. A client built on the Anthropic protocol for a
provider like OpenRouter still embeds through that provider's OpenAI endpoint. Calling
`embed` on a provider without embeddings returns `Error::Config` without sending anything.

Next: [Parameters](08-parameters.md).
