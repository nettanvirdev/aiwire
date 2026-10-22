//! cargo run --example embed   (needs OPENAI_API_KEY)
use aiwire::{Client, EmbedRequest, Provider};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENAI)?;
    let e = client.embed(EmbedRequest::new("text-embedding-3-small").inputs(["cats", "dogs"])).await?;
    for (i, v) in e.iter().enumerate() {
        println!("{i}: {} dims, first {:?}", v.len(), &v[..3]);
    }
    Ok(())
}
