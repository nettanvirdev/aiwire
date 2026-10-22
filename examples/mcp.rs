//! MCP tools with any model.
//! cargo run --example mcp --features mcp   (needs OPENAI_API_KEY and `uvx`)
use aiwire::{Client, Provider, Request, mcp::Mcp};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENAI)?;
    let mcp = Mcp::stdio("uvx", ["mcp-server-time"]).await?;
    let mut req = Request::new("gpt-5-mini").user("What time is it in Tokyo?").tools(mcp.tools().await?);

    loop {
        let res = client.chat(&req).await?;
        if res.tool_calls.is_empty() {
            println!("{}", res.text);
            return Ok(());
        }
        let calls = res.tool_calls.clone();
        req.messages.push(res.into_message());
        for call in &calls {
            req = req.tool_result(call.id.clone(), mcp.call(call).await?);
        }
    }
}
