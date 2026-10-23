//! Image input. cargo run --example vision -- photo.jpg   (needs OPENAI_API_KEY)
use aiwire::{Client, Error, Image, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| Error::Config("usage: vision <image file>".into()))?;
    let image = Image::file(&path)?;
    let client = Client::from_env(Provider::OPENAI)?;
    let res =
        client.chat(Request::new("gpt-5-mini").user("Describe this image in two sentences.").image(image)).await?;
    println!("{}", res.text);
    Ok(())
}
