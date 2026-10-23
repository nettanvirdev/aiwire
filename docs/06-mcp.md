# 6. MCP servers

The [Model Context Protocol](https://modelcontextprotocol.io) lets you plug ready-made tool
servers into any model. aiwire's MCP client lists a server's tools as ordinary `Tool`s and runs
the model's calls against it, so it works with every provider and protocol.

Enable the feature:

```bash
cargo add aiwire --features mcp
```

## Connect

```rust
use aiwire::mcp::Mcp;

// Local server over stdin/stdout (the process is killed when `mcp` is dropped)
let mcp = Mcp::stdio("uvx", ["mcp-server-time"]).await?;

// Same, with env vars or a working directory
let mut cmd = tokio::process::Command::new("npx.cmd");
cmd.args(["-y", "@modelcontextprotocol/server-filesystem", "."]).env("DEBUG", "0");
let mcp = Mcp::command(cmd).await?;

// Remote server over Streamable HTTP, optional bearer token
let mcp = Mcp::http("https://example.com/mcp", Some("token")).await?;

// Remote server with other headers (API keys, …)
let mut headers = reqwest::header::HeaderMap::new();
headers.insert("x-api-key", "secret".parse().unwrap());
let mcp = Mcp::http_headers("https://example.com/mcp", headers).await?;
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

## Parallel calls

Requests to one server run concurrently: replies are matched by id. Run a turn's tool calls
together (`Mcp` is `Send + Sync`; share it across tasks behind an `Arc`):

```rust
let outputs = futures::future::join_all(calls.iter().map(|c| mcp.call(c))).await;
for (call, out) in calls.iter().zip(outputs) {
    req = req.tool_result(call.id.clone(), out?);
}
```

## Timeouts

Each request gets 60 seconds by default. On timeout aiwire tells the server to cancel it and
returns `Error::Timeout`:

```rust
use std::time::Duration;
let mcp = Mcp::stdio("uvx", ["some-slow-server"]).await?.with_timeout(Duration::from_secs(300));
```

## OAuth login

Servers that need a user login (most hosted MCP servers) work with `http_oauth`. aiwire
discovers the authorization server, registers a client if you do not have one, runs the PKCE
login in the browser, and refreshes the token when it expires:

```rust
use aiwire::mcp::{Mcp, OAuth};

let mcp = Mcp::http_oauth(
    "https://mcp.example.com/mcp",
    OAuth::new("http://127.0.0.1:8765/callback"), // a free local port for the redirect
)
.await?;

// Save the tokens (they are secrets) to skip the login next time:
let tokens = mcp.tokens().await;
// ...later: OAuth::new(redirect).tokens(saved)
```

| `OAuth` method | Use |
|---|---|
| `.client(id, secret)` | a client you registered with the provider beforehand |
| `.scope("read write")` | scopes to ask for (default: what the server advertises) |
| `.client_name("My App")` | name shown on the consent screen |
| `.tokens(saved)` | reuse tokens from an earlier run |
| `.on_authorize(f)` | run the login yourself (web apps, headless): `f` gets the URL to show and returns the URL the browser came back to |

The default login prints the URL, opens the system browser, and listens on the loopback
redirect URI for the answer (5 minutes at most). `Tokens` implements `serde::Serialize` and
`Deserialize` so you can store it.

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

## Behaviour worth knowing

- Server pings are answered. Other server-to-client requests (sampling, elicitation, roots)
  are declined with "method not supported".
- An expired HTTP session (404) is re-initialized and the request sent again, as the spec asks.
- Dropping an HTTP `Mcp` ends its session with a `DELETE`; dropping a stdio one kills the process.

Next: [Embeddings](07-embeddings.md).
