//! End to end against a local fake server: real HTTP, no API keys.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aiwire::{Client, EmbedRequest, Error, Event, Protocol, Provider, Request};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type Handler = Arc<dyn Fn(&str, &str, usize) -> (u16, &'static str, String) + Send + Sync>;

/// Serves one response per request: `handler(path, body, request_number)`.
async fn server(handler: Handler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (handler, count) = (handler.clone(), count.clone());
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
                    let path = head.split_whitespace().nth(1).unwrap().to_owned();
                    let body = String::from_utf8_lossy(&buf[end..end + len]).into_owned();
                    let (status, ctype, out) = handler(&path, &body, count.fetch_add(1, Ordering::SeqCst));
                    let res = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nretry-after-ms: 1\r\nmcp-session-id: s1\r\n\r\n{out}",
                        out.len()
                    );
                    sock.write_all(res.as_bytes()).await.unwrap();
                    buf.drain(..end + len);
                }
            });
        }
    });
    url
}

#[tokio::test]
async fn openai_chat_retries_then_succeeds() {
    let url = server(Arc::new(|path, body, n| {
        assert_eq!(path, "/v1/chat/completions");
        assert!(body.contains(r#""model":"m""#));
        match n {
            0 => (429, "application/json", r#"{"error":{"message":"slow down"}}"#.into()),
            _ => {
                (200, "application/json", r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#.into())
            }
        }
    }))
    .await;
    let client = Client::builder().base_url(format!("{url}/v1")).build().unwrap();
    assert_eq!(client.chat(Request::new("m").user("x")).await.unwrap().text, "hi");

    let url =
        server(Arc::new(|_, _, _| (429, "application/json", r#"{"error":{"message":"slow down"}}"#.into()))).await;
    let no_retry = Client::builder().base_url(format!("{url}/v1")).max_retries(0).build().unwrap();
    match no_retry.chat(Request::new("m").user("x")).await {
        Err(Error::Api { status: 429, message }) => assert_eq!(message, "slow down"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn anthropic_stream() {
    let url = server(Arc::new(|path, body, _| {
        assert_eq!(path, "/v1/messages");
        assert!(body.contains(r#""stream":true"#) && body.contains(r#""max_tokens":4096"#));
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":3}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        );
        (200, "text/event-stream", sse.into())
    }))
    .await;
    let client = Client::builder().base_url(url).protocol(Protocol::Anthropic).api_key("k").build().unwrap();
    let mut s = client.stream(Request::new("c").user("x")).await.unwrap();
    assert_eq!(s.next().await.unwrap().unwrap(), Event::Text("Hel".into()));
    let res = s.response().await.unwrap();
    assert_eq!(res.text, "Hello");
    assert_eq!((res.usage.input_tokens, res.usage.output_tokens), (3, 2));
}

#[tokio::test]
async fn embeddings() {
    let url = server(Arc::new(|path, body, _| {
        assert_eq!(path, "/v1/embeddings");
        assert!(body.contains(r#""input":["a","b"]"#));
        (200, "application/json", r#"{"data":[{"index":0,"embedding":[0.5,1]},{"index":1,"embedding":[2,3]}]}"#.into())
    }))
    .await;
    let client = Client::builder().base_url(format!("{url}/v1")).build().unwrap();
    let e = client.embed(EmbedRequest::new("e").inputs(["a", "b"])).await.unwrap();
    assert_eq!(e.get(1), Some(&[2.0, 3.0][..]));

    let groq = Client::new(Provider::GROQ, "k");
    assert!(matches!(groq.embed(EmbedRequest::new("e").input("a")).await, Err(Error::Config(_))));
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn mcp_over_http() {
    use aiwire::{ToolCall, mcp::Mcp};
    let url = server(Arc::new(|_, body, _| {
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        let id = v["id"].clone();
        let result = match v["method"].as_str().unwrap() {
            "initialize" => serde_json::json!({"protocolVersion": "2025-11-25", "capabilities": {}, "serverInfo": {"name": "t", "version": "1"}}),
            "notifications/initialized" => return (202, "application/json", String::new()),
            "tools/list" => serde_json::json!({"tools": [{"name": "echo", "description": "echoes", "inputSchema": {"type": "object"}}]}),
            "tools/call" => serde_json::json!({"content": [{"type": "text", "text": v["params"]["arguments"]["s"]}]}),
            m => panic!("{m}"),
        };
        // Reply as SSE to exercise that path.
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        (200, "text/event-stream", format!("event: message\ndata: {msg}\n\n"))
    }))
    .await;
    let mcp = Mcp::http(&format!("{url}/mcp"), Some("tok")).await.unwrap();
    let tools = mcp.tools().await.unwrap();
    assert_eq!(tools[0].name, "echo");
    let call = ToolCall { id: "1".into(), name: "echo".into(), arguments: r#"{"s":"pong"}"#.into() };
    assert_eq!(mcp.call(&call).await.unwrap(), "pong");
}
