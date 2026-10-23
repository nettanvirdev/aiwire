# 12. Images and other inputs

Messages hold a list of parts. Text is the common case; images work on every protocol.

```rust
use aiwire::{Client, Image, Provider, Request};

let client = Client::from_env(Provider::OPENAI)?;
let res = client
    .chat(Request::new("gpt-5-mini")
        .user("What is in this picture?")
        .image(Image::file("photo.jpg")?))   // added to the last user message
    .await?;
```

## Where an image can come from

| Constructor | Sends |
|---|---|
| `Image::url("https://…/cat.png")` | the URL; the provider downloads it |
| `Image::file("photo.jpg")?` | the file, base64-encoded; type from the extension (png, jpg/jpeg, gif, webp) |
| `Image::bytes("image/png", &bytes)` | bytes you already have |
| `Image::base64("image/png", &b64)` | base64 you already have |

Each protocol gets its own format: `image_url` parts on OpenAI, `image` blocks with a base64 or
URL source on Anthropic, `input_image` on Responses. Models without vision return an error
from the provider.

Some providers cannot download URLs from sites that block bots; send the bytes instead.

## Several images, any order

```rust
use aiwire::{Image, Message, Request};

let msg = Message::user("Compare these two:")
    .image(Image::file("before.png")?)
    .part("and")
    .image(Image::file("after.png")?);
let req = Request::new("claude-sonnet-5-5").message(msg);
```

`Request::image` appends to the last user message, or starts a new one if the last message is
not from the user.

## PDFs, audio and anything else: raw parts

`Part::Raw(json)` is sent as-is, so you can use any content block a provider supports, written
in that protocol's format:

```rust
use aiwire::{Message, Part, Request, json};

// Anthropic: a PDF document
let pdf = json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": b64}});
let req = Request::new("claude-sonnet-5-5").message(Message::user("Summarize this").part(Part::Raw(pdf)));

// OpenAI: a file part
let file = json!({"type": "file", "file": {"filename": "a.pdf", "file_data": format!("data:application/pdf;base64,{b64}")}});
let req = Request::new("gpt-5-mini").message(Message::user("Summarize this").part(Part::Raw(file)));
```

## Reading a message's text

`message.text()` joins its text parts; `Response::text` is already a `String`.

Next: [Reasoning and caching](13-reasoning-and-caching.md).
