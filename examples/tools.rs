//! Tool-calling loop. cargo run --example tools   (needs OPENROUTER_API_KEY)
use aiwire::{Client, Provider, Request, Tool, json};

#[tokio::main]
async fn main() -> aiwire::Result<()> {
    let client = Client::from_env(Provider::OPENROUTER)?;
    let weather = Tool::new(
        "get_weather",
        "Current weather for a city",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    );
    let mut req = Request::new("openai/gpt-5-mini").user("Is it raining in Dhaka?").tool(weather);

    loop {
        let res = client.chat(&req).await?;
        if res.tool_calls.is_empty() {
            println!("{}", res.text);
            return Ok(());
        }
        let calls = res.tool_calls.clone();
        req.messages.push(res.into_message());
        for call in calls {
            #[derive(serde::Deserialize)]
            struct Args {
                city: String,
            }
            let args: Args = call.args()?;
            req = req.tool_result(call.id, format!("{}: 31°C, light rain", args.city));
        }
    }
}
