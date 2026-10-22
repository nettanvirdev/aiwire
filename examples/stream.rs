//! cargo run --example stream   (needs ANTHROPIC_API_KEY)
use std::io::Write;

use aiwire::{Client, Event, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::ANTHROPIC)?;
    let mut stream = client.stream(Request::new("claude-haiku-5-5").user("Write a haiku about Rust.")).await?;
    while let Some(event) = stream.next().await {
        if let Event::Text(t) = event? {
            print!("{t}");
            std::io::stdout().flush().ok();
        }
    }
    println!();
    Ok(())
}
