//! End to end against a local fake server: real HTTP, no API keys.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use aiwire::{Client, EmbedRequest, Error, Event, FinishReason, Protocol, Provider, Request};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Req {
    method: String,
    path: String,
    /// Raw header block, lowercased.
    headers: String,
    body: String,
    /// `http://host:port` of this server.
    base: String,
    n: usize,
}

#[derive(Default)]
struct Reply {
    status: u16,
    ctype: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay_ms: u64,
    /// Send the body, then keep the connection open without finishing it.
    stall: bool,
    /// Never answer at all.
    silent: bool,
}

fn reply(status: u16, ctype: &'static str, body: impl Into<String>) -> Reply {
    Reply { status, ctype, body: body.into(), ..Reply::default() }
}

type Handler = Arc<dyn Fn(&Req) -> Reply + Send + Sync>;

/// Serves `handler`'s reply to every request. Returns the base URL and the peak number of
/// requests handled at once.
async fn server_stats(handler: Handler) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let (inflight, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let base2 = base.clone();
    let peak2 = peak.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (handler, count, base) = (handler.clone(), count.clone(), base2.clone());
            let (inflight, peak) = (inflight.clone(), peak2.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                loop {
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4) else {
                        if sock.read_buf(&mut buf).await.unwrap_or(0) == 0 {
                            return;
                        }
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    while buf.len() < end + len {
                        sock.read_buf(&mut buf).await.unwrap();
                    }
                    let mut words = head.split_whitespace();
                    let req = Req {
                        method: words.next().unwrap().to_uppercase(),
                        path: words.next().unwrap().to_owned(),
                        body: String::from_utf8_lossy(&buf[end..end + len]).into_owned(),
                        headers: head.clone(),
                        base: base.clone(),
                        n: count.fetch_add(1, Ordering::SeqCst),
                    };
                    buf.drain(..end + len);
                    let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let r = handler(&req);
                    if r.delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(r.delay_ms)).await;
                    }
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    if r.silent {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                    let mut extra = String::new();
                    for (k, v) in &r.headers {
                        extra.push_str(&format!("{k}: {v}\r\n"));
                    }
                    let len = r.body.len() + if r.stall { 1000 } else { 0 };
                    let res = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {len}\r\n{extra}\r\n{}",
                        r.status, r.ctype, r.body
                    );
                    sock.write_all(res.as_bytes()).await.unwrap();
                    if r.stall {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                }
            });
        }
    });
    (base, peak)
}

async fn server(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> String {
    server_stats(Arc::new(handler)).await.0
}

const OK: &str = r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#;

#[tokio::test]
async fn retries_then_reports_everything() {
    let url = server(|r| {
        assert_eq!(r.path, "/v1/chat/completions");
        assert!(r.body.contains(r#""model":"m""#));
        match r.n {
            0 => Reply {
                headers: vec![("retry-after-ms", "1".into())],
                ..reply(429, "application/json", r#"{"error":{"message":"slow down"}}"#)
            },
            _ => reply(200, "application/json", OK),
        }
    })
    .await;
    let client = Client::builder().base_url(format!("{url}/v1")).build().unwrap();
    let res = client.chat(Request::new("m").user("x")).await.unwrap();
    assert_eq!(res.text, "hi");

    let url = server(|_| Reply {
        headers: vec![
            ("x-request-id", "req_42".into()),
            ("x-ratelimit-remaining-requests", "0".into()),
            ("retry-after", "7".into()),
        ],
        ..reply(
            429,
            "application/json",
            r#"{"error":{"message":"slow down","type":"requests","code":"rate_limit_exceeded"}}"#,
        )
    })
    .await;
    let no_retry = Client::builder().base_url(format!("{url}/v1")).max_retries(0).build().unwrap();
    let err = no_retry.chat(Request::new("m").user("x")).await.unwrap_err();
    let api = err.api().unwrap();
    assert_eq!(
        (api.status, api.message.as_str(), api.kind.as_deref()),
        (429, "slow down", Some("rate_limit_exceeded"))
    );
    assert_eq!((api.request_id.as_deref(), api.provider, api.attempts), (Some("req_42"), "custom", 1));
    assert_eq!(api.header("x-ratelimit-remaining-requests"), Some("0"));
    assert_eq!(api.retry_after, Some(Duration::from_secs(7)));
    assert!(api.url.ends_with("/v1/chat/completions") && api.body.contains("slow down"));
    assert!(err.is_retryable());
    let shown = err.to_string();
    assert!(shown.starts_with("custom error 429 rate_limit_exceeded: slow down (http://"), "{shown}");
    assert!(shown.contains("request id req_42"), "{shown}");
}

#[tokio::test]
async fn x_should_retry_false_stops_retries() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let url = server(move |_| {
        h.fetch_add(1, Ordering::SeqCst);
        Reply { headers: vec![("x-should-retry", "false".into())], ..reply(500, "text/plain", "nope") }
    })
    .await;
    let client = Client::builder().base_url(url).build().unwrap();
    let err = client.chat(Request::new("m").user("x")).await.unwrap_err();
    assert_eq!((err.status(), hits.load(Ordering::SeqCst)), (Some(500), 1));
}

#[tokio::test]
async fn errors_in_200_bodies() {
    let url = server(|r| match r.n {
        0 => reply(200, "application/json", r#"{"error":{"message":"upstream exploded","type":"server_error"}}"#),
        _ => reply(200, "text/html", "<html>gateway page</html>"),
    })
    .await;
    let client = Client::builder().base_url(url).build().unwrap();
    let err = client.chat(Request::new("m").user("x")).await.unwrap_err();
    assert_eq!(err.api().unwrap().message, "upstream exploded");
    let err = client.chat(Request::new("m").user("x")).await.unwrap_err();
    let api = err.api().unwrap();
    assert_eq!(
        (api.status, api.kind.as_deref(), api.body.as_str()),
        (200, Some("invalid_response"), "<html>gateway page</html>")
    );
}

#[tokio::test]
async fn redirects_are_reported_not_followed() {
    let url = server(|r| Reply {
        headers: vec![("location", format!("https{}", &r.base[4..]))],
        ..reply(301, "text/plain", "Moved Permanently")
    })
    .await;
    let client = Client::builder().base_url(&url).build().unwrap();
    let err = client.chat(Request::new("m").user("x")).await.unwrap_err();
    assert_eq!(err.status(), Some(301));
    assert!(err.to_string().contains("redirects to https://127.0.0.1"), "{err}");
}

#[tokio::test]
async fn anthropic_stream() {
    let url = server(|r| {
        assert_eq!(r.path, "/v1/messages");
        assert!(r.body.contains(r#""stream":true"#) && r.body.contains(r#""max_tokens":4096"#));
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":3}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        );
        Reply { headers: vec![("request-id", "req_a".into())], ..reply(200, "text/event-stream", sse) }
    })
    .await;
    let client = Client::builder().base_url(url).protocol(Protocol::Anthropic).api_key("k").build().unwrap();
    let mut s = client.stream(Request::new("c").user("x")).await.unwrap();
    assert_eq!(s.request_id(), Some("req_a"));
    assert_eq!(s.next().await.unwrap().unwrap(), Event::Text("Hel".into()));
    let res = s.response().await.unwrap();
    assert_eq!((res.text.as_str(), res.request_id.as_deref()), ("Hello", Some("req_a")));
    assert_eq!((res.usage.input_tokens, res.usage.output_tokens), (3, 2));
}

#[tokio::test]
async fn stalled_stream_times_out() {
    let url = server(|_| Reply {
        stall: true,
        ..reply(200, "text/event-stream", "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n")
    })
    .await;
    let client = Client::builder().base_url(url).stream_idle_timeout(Duration::from_millis(300)).build().unwrap();
    let mut s = client.stream(Request::new("m").user("x")).await.unwrap();
    assert_eq!(s.next().await.unwrap().unwrap(), Event::Text("Hi".into()));
    let t = Instant::now();
    let err = s.next().await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(s.next().await.is_none());
}

#[tokio::test]
async fn silent_server_times_out_before_headers() {
    let url = server(|_| Reply { silent: true, ..Reply::default() }).await;
    let client =
        Client::builder().base_url(url).max_retries(0).stream_idle_timeout(Duration::from_millis(200)).build().unwrap();
    let err = client.stream(Request::new("m").user("x")).await.err().unwrap();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert!(err.is_retryable());
}

#[tokio::test]
async fn concurrency_limit() {
    let (url, peak) = server_stats(Arc::new(|_| Reply { delay_ms: 100, ..reply(200, "application/json", OK) })).await;
    let client = Client::builder().base_url(url).max_concurrent_requests(2).build().unwrap();
    let calls = (0..6).map(|_| {
        let c = client.clone();
        tokio::spawn(async move { c.chat(Request::new("m").user("x")).await.unwrap() })
    });
    for c in calls.collect::<Vec<_>>() {
        c.await.unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn responses_protocol() {
    let url = server(|r| {
        assert_eq!(r.path, "/v1/responses");
        assert!(r.body.contains(r#""input":[{"role":"user","content":"x"}]"#), "{}", r.body);
        let sse = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"m\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"c\",\"name\":\"f\",\"arguments\":\"{}\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n",
        );
        reply(200, "text/event-stream", sse)
    })
    .await;
    let client = Client::builder().base_url(format!("{url}/v1")).protocol(Protocol::Responses).build().unwrap();
    let res = client.stream(Request::new("m").user("x")).await.unwrap().response().await.unwrap();
    assert_eq!((res.id.as_str(), res.text.as_str(), res.tool_calls[0].name.as_str()), ("resp_1", "Hi", "f"));
    assert_eq!((res.finish_reason, res.usage.input_tokens), (FinishReason::ToolCalls, 4));
}

#[tokio::test]
async fn embeddings() {
    let url = server(|r| {
        assert_eq!(r.path, "/v1/embeddings");
        assert!(r.body.contains(r#""input":["a","b"]"#));
        reply(200, "application/json", r#"{"data":[{"index":0,"embedding":[0.5,1]},{"index":1,"embedding":[2,3]}]}"#)
    })
    .await;
    let client = Client::builder().base_url(format!("{url}/v1")).build().unwrap();
    let e = client.embed(EmbedRequest::new("e").inputs(["a", "b"])).await.unwrap();
    assert_eq!(e.get(1), Some(&[2.0, 3.0][..]));

    let groq = Client::new(Provider::GROQ, "k");
    assert!(matches!(groq.embed(EmbedRequest::new("e").input("a")).await, Err(Error::Config(_))));
}

#[cfg(feature = "mcp")]
mod mcp {
    use std::sync::Mutex;

    use aiwire::{
        ToolCall,
        mcp::{Mcp, OAuth, Tokens, oauth::pkce_challenge},
    };
    use serde_json::{Value, json};

    use super::*;

    /// JSON-RPC answers of a tiny MCP server with one `echo` tool.
    fn answer(body: &str) -> Option<Value> {
        let v: Value = serde_json::from_str(body).unwrap();
        let result = match v["method"].as_str().unwrap_or_default() {
            "initialize" => {
                json!({"protocolVersion": "2025-11-25", "capabilities": {}, "serverInfo": {"name": "t", "version": "1"}})
            }
            "tools/list" => {
                json!({"tools": [{"name": "echo", "description": "echoes", "inputSchema": {"type": "object"}}]})
            }
            "tools/call" => json!({"content": [{"type": "text", "text": v["params"]["arguments"]["s"]}]}),
            _ => return None, // notifications, replies to our ping
        };
        Some(json!({"jsonrpc": "2.0", "id": v["id"], "result": result}))
    }

    fn rpc(r: &Req) -> Reply {
        match answer(&r.body) {
            // Reply as SSE, with a server ping first, to exercise that path.
            Some(msg) => {
                let ping = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"});
                let sse = format!("event: message\ndata: {ping}\n\nevent: message\ndata: {msg}\n\n");
                Reply { headers: vec![("mcp-session-id", "s1".into())], ..reply(200, "text/event-stream", sse) }
            }
            None => reply(202, "application/json", ""),
        }
    }

    #[tokio::test]
    async fn over_http_with_session_recovery() {
        let inits = Arc::new(AtomicUsize::new(0));
        let i = inits.clone();
        let url = server(move |r| {
            if r.body.contains("\"initialize\"") {
                i.fetch_add(1, Ordering::SeqCst);
            }
            // The first tools/call finds the session expired.
            if r.body.contains("tools/call") && i.load(Ordering::SeqCst) == 1 {
                return reply(404, "text/plain", "session expired");
            }
            rpc(r)
        })
        .await;
        let mcp = Mcp::http(&format!("{url}/mcp"), Some("tok")).await.unwrap();
        let tools = mcp.tools().await.unwrap();
        assert_eq!(tools[0].name, "echo");
        let call = ToolCall { id: "1".into(), name: "echo".into(), arguments: r#"{"s":"pong"}"#.into() };
        assert_eq!(mcp.call(&call).await.unwrap(), "pong");
        assert_eq!(inits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn oauth_login_and_refresh() {
        let challenge = Arc::new(Mutex::new(String::new()));
        let c = challenge.clone();
        let url = server(move |r| {
            let form = |k: &str| {
                r.body.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).unwrap_or_default().to_owned()
            };
            match (r.method.as_str(), r.path.as_str()) {
                ("POST", "/mcp") if r.headers.contains("authorization: bearer good") => rpc(r),
                ("POST", "/mcp") => Reply {
                    headers: vec![(
                        "www-authenticate",
                        format!(r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource/mcp""#, r.base),
                    )],
                    ..reply(401, "application/json", r#"{"error":"invalid_token"}"#)
                },
                ("GET", "/.well-known/oauth-protected-resource/mcp") => reply(
                    200,
                    "application/json",
                    json!({"resource": format!("{}/mcp", r.base), "authorization_servers": [r.base]}).to_string(),
                ),
                ("GET", "/.well-known/oauth-authorization-server") => reply(
                    200,
                    "application/json",
                    json!({
                        "authorization_endpoint": format!("{}/authorize", r.base),
                        "token_endpoint": format!("{}/token", r.base),
                        "registration_endpoint": format!("{}/register", r.base),
                    })
                    .to_string(),
                ),
                ("POST", "/register") => reply(201, "application/json", r#"{"client_id":"cid"}"#),
                ("POST", "/token") => {
                    assert_eq!(form("client_id"), "cid");
                    let ok = match form("grant_type").as_str() {
                        "authorization_code" => {
                            form("code") == "c1" && pkce_challenge(&form("code_verifier")) == *c.lock().unwrap()
                        }
                        "refresh_token" => form("refresh_token") == "r1",
                        _ => false,
                    };
                    match ok {
                        true => reply(
                            200,
                            "application/json",
                            r#"{"access_token":"good","refresh_token":"r1","expires_in":3600,"token_type":"Bearer"}"#,
                        ),
                        false => reply(400, "application/json", r#"{"error":"invalid_grant"}"#),
                    }
                }
                _ => reply(404, "text/plain", "not found"),
            }
        })
        .await;

        let c = challenge.clone();
        let oauth = OAuth::new("http://127.0.0.1:9/cb").on_authorize(move |auth_url| {
            let c = c.clone();
            async move {
                let u = reqwest::Url::parse(&auth_url).unwrap();
                let q = |k: &str| u.query_pairs().find(|(n, _)| n == k).unwrap().1.into_owned();
                assert_eq!((q("client_id").as_str(), q("code_challenge_method").as_str()), ("cid", "S256"));
                *c.lock().unwrap() = q("code_challenge");
                Ok(format!("{}?code=c1&state={}", q("redirect_uri"), q("state")))
            }
        });
        let mcp = Mcp::http_oauth(&format!("{url}/mcp"), oauth).await.unwrap();
        assert_eq!(mcp.tools().await.unwrap()[0].name, "echo");
        let tokens = mcp.tokens().await.unwrap();
        assert_eq!((tokens.access_token.as_str(), tokens.client_id.as_str()), ("good", "cid"));

        // Saved tokens with an expired access token: refreshed, no login.
        let saved = Tokens { access_token: "stale".into(), ..tokens };
        let oauth = OAuth::new("http://127.0.0.1:9/cb")
            .tokens(saved)
            .on_authorize(|_| async { panic!("should refresh, not log in") });
        let mcp = Mcp::http_oauth(&format!("{url}/mcp"), oauth).await.unwrap();
        assert_eq!(mcp.tools().await.unwrap().len(), 1);
    }

    /// Runs as a stdio MCP server when spawned by `stdio_concurrency_and_timeout`.
    #[test]
    fn stdio_child_server() {
        use std::io::{BufRead, Write};
        if std::env::var_os("AIWIRE_MCP_CHILD").is_none() {
            return;
        }
        let out = Arc::new(Mutex::new(std::io::stdout()));
        let say = |out: &Mutex<std::io::Stdout>, v: Value| {
            let mut o = out.lock().unwrap();
            writeln!(o, "\n{v}").unwrap();
            o.flush().unwrap();
        };
        for line in std::io::stdin().lock().lines() {
            let line = line.unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            match v["method"].as_str() {
                Some("initialize") => {
                    say(&out, json!({"jsonrpc": "2.0", "id": "srv-ping", "method": "ping"}));
                    say(&out, answer(&line).unwrap());
                }
                Some("tools/call") => {
                    let (out, ms) = (out.clone(), v["params"]["arguments"]["ms"].as_u64().unwrap());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(ms));
                        let r = json!({"jsonrpc": "2.0", "id": v["id"], "result": {"content": [{"type": "text", "text": ms.to_string()}]}});
                        say(&out, r);
                    });
                }
                Some(_) => {
                    if let Some(a) = answer(&line) {
                        say(&out, a);
                    }
                }
                None => {} // our ping answered
            }
        }
    }

    #[tokio::test]
    async fn stdio_concurrency_and_timeout() {
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["mcp::stdio_child_server", "--exact", "--nocapture", "--test-threads=1"])
            .env("AIWIRE_MCP_CHILD", "1");
        let mcp = Arc::new(Mcp::command(cmd).await.unwrap().with_timeout(Duration::from_secs(3)));
        let sleep =
            |ms: u64| ToolCall { id: ms.to_string(), name: "sleep".into(), arguments: format!(r#"{{"ms":{ms}}}"#) };
        let t = Instant::now();
        let (slow_call, fast_call) = (sleep(1000), sleep(10));
        let (slow, fast) = tokio::join!(mcp.call(&slow_call), async {
            let r = mcp.call(&fast_call).await;
            (r, t.elapsed())
        });
        assert_eq!(slow.unwrap(), "1000");
        assert_eq!(fast.0.unwrap(), "10");
        assert!(fast.1 < Duration::from_millis(800), "fast call waited for the slow one: {:?}", fast.1);
        assert!(t.elapsed() < Duration::from_millis(2500), "calls ran one after another: {:?}", t.elapsed());
        let err = mcp.call(&sleep(10_000)).await.unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err}");
        assert_eq!(mcp.tools().await.unwrap()[0].name, "echo");
    }
}
