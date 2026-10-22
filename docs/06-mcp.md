# 6. MCP servers

The [Model Context Protocol](https://modelcontextprotocol.io) lets you plug ready-made tool
servers into any model. aiwire's MCP client lists a server's tools as ordinary `Tool`s and runs
the model's calls against it, so it works with every provider and both protocols.

Enable the feature:

```bash
cargo add aiwire --features mcp
```

## Connect

```rust
use aiwire::mcp::Mcp;

// Local server over stdin/stdout (the process is killed when `mcp` is dropped)
let mcp = Mcp::stdio("uvx", ["mcp-server-time"]).await?;

// Remote server over Streamable HTTP, optional bearer token
let mcp = Mcp::http("https://example.com/mcp", Some("token")).await?;
```

On Windows, Node launchers need their full name: `Mcp::stdio("npx.cmd", ["-y", "..."])`.

## Use its tools

```rust
use aiwire::{Client, Provider, Request};

let client = Client::from_env(Provider::OPENAI)?;
let mut req = Request::new("gpt-5-mini")
    .user("What time is it in Tokyo?")
    .tools(mcp.tools().await?);

loop {
    let res = client.chat(&req).await?;
    if res.tool_calls.is_empty() {
        println!("{}", res.text);
        break;
    }
    let calls = res.tool_calls.clone();
    req.messages.push(res.into_message());
    for call in &calls {
        req = req.tool_result(call.id.clone(), mcp.call(call).await?);
    }
}
```

- `mcp.tools()` follows pagination and returns `Vec<Tool>`.
- `mcp.call(&tool_call)` returns the tool's text output. If the tool reports an error, you get
  `Ok("Error: ...")` so the model can see it and try again; transport failures are `Err`.
- Mix MCP tools with your own: `.tools(mcp.tools().await?).tool(my_tool)`. Route each call by
  name.

## Several servers

```rust
let time = Mcp::stdio("uvx", ["mcp-server-time"]).await?;
let fetch = Mcp::stdio("uvx", ["mcp-server-fetch"]).await?;
let time_tools = time.tools().await?;
let req = req.tools(time_tools.iter().cloned()).tools(fetch.tools().await?);
// when running a call:
let server = if time_tools.iter().any(|t| t.name == call.name) { &time } else { &fetch };
let out = server.call(&call).await?;
```

## Anything else in MCP

`mcp.request(method, params)` sends any JSON-RPC request and returns its `result`:

```rust
let resources = mcp.request("resources/list", aiwire::json!({})).await?;
```

## Limits

- One request at a time per server connection.
- Server-to-client requests other than `ping` (sampling, elicitation, roots) are declined.

Next: [Embeddings](07-embeddings.md).
