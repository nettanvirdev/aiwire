//! [Model Context Protocol](https://modelcontextprotocol.io) client: give any model the tools of any MCP server.
//!
//! ```no_run
//! # async fn run(client: aiwire::Client) -> aiwire::Result<()> {
//! use aiwire::{Request, mcp::Mcp};
//!
//! let mcp = Mcp::stdio("uvx", ["mcp-server-time"]).await?;
//! let mut req = Request::new("gpt-5-mini").user("What time is it in Tokyo?").tools(mcp.tools().await?);
//! loop {
//!     let res = client.chat(&req).await?;
//!     if res.tool_calls.is_empty() {
//!         println!("{}", res.text);
//!         break;
//!     }
//!     let calls = res.tool_calls.clone();
//!     req.messages.push(res.into_message());
//!     for call in &calls {
//!         req = req.tool_result(call.id.clone(), mcp.call(call).await?);
//!     }
//! }
//! # Ok(()) }
//! ```

use std::{ffi::OsStr, process::Stdio};

use reqwest::{
    Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use crate::{Error, Result, Tool, ToolCall, error, stream::data_lines};

const PROTOCOL_VERSION: &str = "2025-11-25";

/// A connected MCP server.
// ponytail: one request in flight per server (a Mutex); add id-routed concurrency if tool calls get slow.
pub struct Mcp {
    conn: Mutex<Conn>,
}

struct Conn {
    transport: Transport,
    next_id: u64,
}

#[allow(clippy::large_enum_variant)] // one per server
enum Transport {
    Stdio { _child: Child, stdin: ChildStdin, stdout: BufReader<ChildStdout>, line: String },
    Http { http: reqwest::Client, url: Url, headers: HeaderMap },
}

fn mcp_err(e: impl std::fmt::Display) -> Error {
    Error::Mcp(e.to_string())
}

impl Mcp {
    /// Spawn a local server speaking MCP over stdin/stdout. Its stderr is inherited.
    /// The process is killed when this value is dropped.
    ///
    /// On Windows, launchers like `npx` need their full name (`npx.cmd`).
    pub async fn stdio<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| mcp_err(format!("spawn: {e}")))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Self::init(Transport::Stdio { _child: child, stdin, stdout, line: String::new() }).await
    }

    /// Connect to a remote server over Streamable HTTP, with an optional bearer token.
    pub async fn http(url: &str, bearer_token: Option<&str>) -> Result<Self> {
        let url = Url::parse(url).map_err(|e| Error::Config(format!("bad MCP url: {e}")))?;
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/event-stream"));
        if let Some(t) = bearer_token {
            let mut v = HeaderValue::try_from(format!("Bearer {t}")).map_err(mcp_err)?;
            v.set_sensitive(true);
            headers.insert(AUTHORIZATION, v);
        }
        Self::init(Transport::Http { http: reqwest::Client::new(), url, headers }).await
    }

    async fn init(transport: Transport) -> Result<Self> {
        let mut conn = Conn { transport, next_id: 0 };
        let info = json!({"name": "aiwire", "version": env!("CARGO_PKG_VERSION")});
        let init = conn
            .request("initialize", json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": info}))
            .await?;
        if let Transport::Http { headers, .. } = &mut conn.transport {
            if let Some(v) = init["protocolVersion"].as_str().and_then(|v| HeaderValue::try_from(v).ok()) {
                headers.insert("mcp-protocol-version", v);
            }
        }
        conn.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// The server's tools, ready for [`Request::tools`](crate::Request::tools).
    pub async fn tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.map_or_else(|| json!({}), |c| json!({ "cursor": c }));
            let mut page = self.request("tools/list", params).await?;
            for mut t in page["tools"].as_array_mut().map(std::mem::take).unwrap_or_default() {
                let name = t["name"].as_str().unwrap_or_default().to_owned();
                let desc = t["description"].as_str().unwrap_or_default().to_owned();
                tools.push(Tool::new(name, desc, t["inputSchema"].take()));
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// Run a tool call; returns its text content. A tool-reported error comes back as `Ok`
    /// text starting with `Error:` so the model can see it and recover.
    pub async fn call(&self, call: &ToolCall) -> Result<String> {
        let args: Value = call.args()?;
        let r = self.request("tools/call", json!({"name": call.name, "arguments": args})).await?;
        let mut text = String::new();
        for c in r["content"].as_array().into_iter().flatten() {
            if !text.is_empty() {
                text.push('\n');
            }
            match c["text"].as_str() {
                Some(t) if c["type"] == "text" => text.push_str(t),
                _ => text.push_str(&c.to_string()),
            }
        }
        if text.is_empty() {
            if let Some(s) = r.get("structuredContent") {
                text = s.to_string();
            }
        }
        if r["isError"] == true {
            text.insert_str(0, "Error: ");
        }
        Ok(text)
    }

    /// Any JSON-RPC method (`resources/list`, `prompts/get`, …); returns its `result`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.conn.lock().await.request(method, params).await
    }
}

impl Conn {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let reply = self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        let reply = match reply {
            Some(r) => r,
            None => self.read_reply(id).await?,
        };
        if let Some(e) = reply.get("error") {
            return Err(Error::Mcp(format!("{method}: {}", error::value_message(e))));
        }
        let Value::Object(mut m) = reply else { return Err(mcp_err("reply is not an object")) };
        Ok(m.remove("result").unwrap_or(Value::Null))
    }

    /// Writes a message. Over HTTP, returns the reply carried by the response.
    async fn send(&mut self, msg: &Value) -> Result<Option<Value>> {
        match &mut self.transport {
            Transport::Stdio { stdin, .. } => {
                let mut line = serde_json::to_vec(msg)?;
                line.push(b'\n');
                stdin.write_all(&line).await.map_err(mcp_err)?;
                stdin.flush().await.map_err(mcp_err)?;
                Ok(None)
            }
            Transport::Http { http, url, headers } => {
                let res = http
                    .post(url.clone())
                    .headers(headers.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(serde_json::to_vec(msg)?)
                    .send()
                    .await?;
                let status = res.status();
                if !status.is_success() {
                    return Err(Error::Api { status: status.as_u16(), message: error::message(&res.bytes().await?) });
                }
                if let Some(s) = res.headers().get("mcp-session-id") {
                    headers.insert("mcp-session-id", s.clone());
                }
                let Some(id) = msg.get("id") else { return Ok(Some(Value::Null)) };
                let sse =
                    res.headers().get(CONTENT_TYPE).is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
                let body = res.bytes().await?;
                if !sse {
                    return Ok(Some(serde_json::from_slice(&body)?));
                }
                let mut found = None;
                let mut body = body.to_vec();
                body.push(b'\n');
                data_lines(&body, &mut |d| {
                    if let Ok(v) = serde_json::from_slice::<Value>(d) {
                        if v.get("id") == Some(id) && v.get("method").is_none() {
                            found = Some(v);
                        }
                    }
                    Ok(())
                })?;
                found.map(Some).ok_or_else(|| mcp_err("no reply in event stream"))
            }
        }
    }

    /// Stdio: reads lines until the reply to `id`, answering server pings on the way.
    async fn read_reply(&mut self, id: u64) -> Result<Value> {
        loop {
            let msg = {
                let Transport::Stdio { stdout, line, .. } = &mut self.transport else { unreachable!() };
                line.clear();
                if stdout.read_line(line).await.map_err(mcp_err)? == 0 {
                    return Err(mcp_err("server closed stdout"));
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(v) => v,
                    Err(_) => continue, // log noise on stdout
                }
            };
            match (msg.get("method"), msg.get("id")) {
                (Some(m), Some(rid)) => {
                    let reply = if m == "ping" {
                        json!({"jsonrpc": "2.0", "id": rid, "result": {}})
                    } else {
                        json!({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "not supported"}})
                    };
                    self.send(&reply).await?;
                }
                (None, Some(rid)) if rid.as_u64() == Some(id) => return Ok(msg),
                _ => {}
            }
        }
    }
}
