//! cargo run --example chat   (needs GROQ_API_KEY)
use aiwire::{Client, Provider, Request};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::GROQ)?;
    let res = client
        .chat(Request::new("llama-3.3-70b-versatile").system("Answer in one sentence.").user("Why is the sky blue?"))
        .await?;
    println!("{}\n\n{:?}", res.text, res.usage);
    Ok(())
}
