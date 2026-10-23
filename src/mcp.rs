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
//!
//! Requests run concurrently: `Mcp` is `Send + Sync`, share it behind an `Arc` and call tools in
//! parallel. Remote servers that need a login use [`Mcp::http_oauth`].

use std::{
    collections::HashMap,
    ffi::OsStr,
    future::Future,
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    StatusCode, Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::{Error, Result, Tool, ToolCall, error, error::ApiError, stream::data_lines};

pub mod oauth;
pub use oauth::{OAuth, Tokens};

const PROTOCOL_VERSION: &str = "2025-11-25";
const SESSION: &str = "mcp-session-id";

/// A connected MCP server. Dropping it stops a stdio server and ends an HTTP session.
pub struct Mcp {
    transport: Transport,
    next_id: AtomicU64,
    timeout: Duration,
}

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Value>>>>;

#[allow(clippy::large_enum_variant)] // one per server
enum Transport {
    Stdio { stdin: Arc<Mutex<ChildStdin>>, pending: Pending, reader: JoinHandle<()>, _child: Child },
    Http(Http),
}

struct Http {
    http: reqwest::Client,
    url: Url,
    /// Session id, protocol version, auth: changes during the connection.
    headers: RwLock<HeaderMap>,
    auth: Option<oauth::Auth>,
}

fn mcp_err(e: impl std::fmt::Display) -> Error {
    Error::Mcp(e.to_string())
}

impl Mcp {
    /// Spawn a local server speaking MCP over stdin/stdout. Its stderr is inherited.
    ///
    /// On Windows, launchers like `npx` need their full name (`npx.cmd`).
    pub async fn stdio<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.args(args);
        Self::command(cmd).await
    }

    /// Spawn a local server from a prepared [`Command`] (env vars, working directory, …).
    /// stdin and stdout are piped; the process is killed when the `Mcp` is dropped.
    pub async fn command(mut cmd: Command) -> Result<Self> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| mcp_err(format!("spawn: {e}")))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        let pending = Pending::default();
        let reader = tokio::spawn(read_loop(stdout, stdin.clone(), pending.clone()));
        Self::init(Transport::Stdio { stdin, pending, reader, _child: child }).await
    }

    /// Connect to a remote server over Streamable HTTP, with an optional bearer token.
    pub async fn http(url: &str, bearer_token: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Some(t) = bearer_token {
            headers.insert(AUTHORIZATION, oauth::bearer(t)?);
        }
        Self::http_with(url, headers, None).await
    }

    /// Connect to a remote server over Streamable HTTP with custom headers (API keys, …).
    pub async fn http_headers(url: &str, headers: HeaderMap) -> Result<Self> {
        Self::http_with(url, headers, None).await
    }

    /// Connect to a server that needs an OAuth login. Without saved [`Tokens`] this discovers
    /// the authorization server, registers a client if needed, and runs the browser login
    /// (or your [`OAuth::on_authorize`] callback). Tokens refresh automatically; save
    /// [`Mcp::tokens`] to skip the login next time.
    pub async fn http_oauth(url: &str, oauth: OAuth) -> Result<Self> {
        Self::http_with(url, HeaderMap::new(), Some(oauth::Auth::new(oauth))).await
    }

    async fn http_with(url: &str, mut headers: HeaderMap, auth: Option<oauth::Auth>) -> Result<Self> {
        let url = Url::parse(url).map_err(|e| Error::Config(format!("bad MCP url: {e}")))?;
        headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/event-stream"));
        let http = reqwest::Client::builder()
            .user_agent(concat!("aiwire/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        if let Some(a) = &auth {
            if let Some(v) = a.header(&http, &url).await? {
                headers.insert(AUTHORIZATION, v);
            }
        }
        Self::init(Transport::Http(Http { http, url, headers: RwLock::new(headers), auth })).await
    }

    async fn init(transport: Transport) -> Result<Self> {
        let mcp = Self { transport, next_id: AtomicU64::new(0), timeout: Duration::from_secs(60) };
        mcp.handshake().await?;
        Ok(mcp)
    }

    fn handshake(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let info = json!({"name": "aiwire", "version": env!("CARGO_PKG_VERSION")});
            let params = json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": info});
            let init = self.request("initialize", params).await?;
            if let Transport::Http(h) = &self.transport {
                if let Some(v) = init["protocolVersion"].as_str().and_then(|v| HeaderValue::try_from(v).ok()) {
                    h.headers.write().expect("not poisoned").insert("mcp-protocol-version", v);
                }
            }
            self.notify("notifications/initialized", Value::Null).await
        })
    }

    /// Per-request timeout. Default 60 seconds. On timeout the request is cancelled on the
    /// server and fails with [`Error::Timeout`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Current OAuth tokens (after [`Mcp::http_oauth`]), to save for the next run.
    pub async fn tokens(&self) -> Option<Tokens> {
        match &self.transport {
            Transport::Http(Http { auth: Some(a), .. }) => a.tokens().await,
            _ => None,
        }
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let reply = match tokio::time::timeout(self.timeout, self.roundtrip(id, &msg)).await {
            Ok(r) => r?,
            Err(_) => {
                if let Transport::Stdio { pending, .. } = &self.transport {
                    pending.lock().expect("not poisoned").remove(&id);
                }
                let _ = self.notify("notifications/cancelled", json!({"requestId": id, "reason": "timed out"})).await;
                return Err(Error::Timeout(self.timeout));
            }
        };
        if let Some(e) = reply.get("error") {
            return Err(Error::Mcp(format!("{method}: {}", error::value_message(e))));
        }
        let Value::Object(mut m) = reply else { return Err(mcp_err("reply is not an object")) };
        Ok(m.remove("result").unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if !params.is_null() {
            msg["params"] = params;
        }
        match &self.transport {
            Transport::Stdio { stdin, .. } => write_line(stdin, &msg).await,
            Transport::Http(h) => self.post(h, &msg, None).await.map(drop),
        }
    }

    async fn roundtrip(&self, id: u64, msg: &Value) -> Result<Value> {
        match &self.transport {
            Transport::Stdio { stdin, pending, .. } => {
                let (tx, rx) = oneshot::channel();
                pending.lock().expect("not poisoned").insert(id, tx);
                write_line(stdin, msg).await?;
                rx.await.map_err(|_| mcp_err("server closed its stdout"))
            }
            Transport::Http(h) => self.post(h, msg, Some(&json!(id))).await?.ok_or_else(|| mcp_err("no reply")),
        }
    }

    /// POSTs one message; returns the reply to `id` (from JSON or an event stream).
    async fn post(&self, h: &Http, msg: &Value, id: Option<&Value>) -> Result<Option<Value>> {
        let mut retried = false;
        loop {
            let headers = h.headers.read().expect("not poisoned").clone();
            let res = h
                .http
                .post(h.url.clone())
                .headers(headers.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(msg)?)
                .send()
                .await?;
            let status = res.status();
            if !retried && status == StatusCode::UNAUTHORIZED {
                if let Some(a) = &h.auth {
                    let challenge =
                        res.headers().get(WWW_AUTHENTICATE).and_then(|v| v.to_str().ok()).map(str::to_owned);
                    let v = a.reauthorize(&h.http, &h.url, challenge.as_deref()).await?;
                    h.headers.write().expect("not poisoned").insert(AUTHORIZATION, v);
                    retried = true;
                    continue;
                }
            }
            // An expired session: start a new one and send again (the spec's required recovery).
            if !retried
                && status == StatusCode::NOT_FOUND
                && headers.contains_key(SESSION)
                && msg["method"] != "initialize"
            {
                h.headers.write().expect("not poisoned").remove(SESSION);
                self.handshake().await?;
                retried = true;
                continue;
            }
            if !status.is_success() {
                let resp_headers = res.headers().clone();
                let mut e = ApiError::from_body(status.as_u16(), &res.bytes().await.unwrap_or_default());
                e.take_headers(&resp_headers);
                e.provider = "mcp";
                e.url = h.url.to_string();
                e.attempts = 1 + u32::from(retried);
                return Err(e.into());
            }
            if let Some(s) = res.headers().get(SESSION) {
                h.headers.write().expect("not poisoned").insert(SESSION, s.clone());
            }
            let Some(id) = id else { return Ok(None) };
            let sse = res.headers().get(CONTENT_TYPE).is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
            if !sse {
                let body = res.bytes().await?;
                let v: Value = serde_json::from_slice(&body)
                    .map_err(|e| mcp_err(format!("bad reply: {e}: {}", String::from_utf8_lossy(&body))))?;
                // A batch reply is an array; pick ours.
                let v = match v {
                    Value::Array(a) => a.into_iter().find(|m| m.get("id") == Some(id)).unwrap_or(Value::Null),
                    v => v,
                };
                return Ok(Some(v));
            }
            return self.read_sse(h, res, id).await.map(Some);
        }
    }

    /// Reads an event stream until the reply to `id`, answering server requests on the way.
    async fn read_sse(&self, h: &Http, mut res: reqwest::Response, id: &Value) -> Result<Value> {
        let mut buf = Vec::new();
        let mut found = None;
        let mut asks = Vec::new();
        while found.is_none() {
            let Some(chunk) = res.chunk().await? else { break };
            buf.extend_from_slice(&chunk);
            let used = data_lines(&buf, &mut |d| {
                let Ok(v) = serde_json::from_slice::<Value>(d) else { return Ok(()) };
                match (v.get("method"), v.get("id")) {
                    (None, Some(rid)) if rid == id => found = Some(v),
                    (Some(m), Some(rid)) => asks.push(server_reply(m, rid)),
                    _ => {}
                }
                Ok(())
            })?;
            buf.drain(..used);
            for reply in asks.drain(..) {
                // Answer through a fresh POST; ignore its (empty) body.
                Box::pin(self.post(h, &reply, None)).await?;
            }
        }
        found.ok_or_else(|| mcp_err("event stream ended without a reply"))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        match &self.transport {
            Transport::Stdio { reader, .. } => reader.abort(),
            Transport::Http(h) => {
                // End the session politely; best effort, needs a running runtime.
                let headers = h.headers.read().map(|h| h.clone()).unwrap_or_default();
                if headers.contains_key(SESSION) {
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        let req = h.http.delete(h.url.clone()).headers(headers);
                        rt.spawn(async move {
                            let _ = req.send().await;
                        });
                    }
                }
            }
        }
    }
}

/// Our answer to a request the server sent us.
fn server_reply(method: &Value, id: &Value) -> Value {
    if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported by client"}})
    }
}

async fn write_line(stdin: &Mutex<ChildStdin>, msg: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin.write_all(&line).await.map_err(|e| mcp_err(format!("write to server: {e}")))?;
    stdin.flush().await.map_err(|e| mcp_err(format!("write to server: {e}")))
}

/// Routes replies to waiting requests and answers server requests. Ends at EOF, which fails
/// every pending request (their senders drop).
async fn read_loop(mut stdout: BufReader<ChildStdout>, stdin: Arc<Mutex<ChildStdin>>, pending: Pending) {
    let mut line = String::new();
    loop {
        line.clear();
        match stdout.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue }; // log noise on stdout
        match (msg.get("method"), msg.get("id")) {
            (Some(m), Some(rid)) => {
                let _ = write_line(&stdin, &server_reply(m, rid)).await;
            }
            (None, Some(rid)) => {
                let tx = rid.as_u64().and_then(|id| pending.lock().expect("not poisoned").remove(&id));
                if let Some(tx) = tx {
                    let _ = tx.send(msg);
                }
            }
            _ => {} // notifications: progress, logging, list_changed
        }
    }
    pending.lock().expect("not poisoned").clear();
}
