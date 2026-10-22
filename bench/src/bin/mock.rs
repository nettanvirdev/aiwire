//! Canned OpenAI-compatible server for benchmarks: no tokens, no money, no network.
//! Answers `/chat/completions` (plain or SSE) and `/embeddings` with fixed bodies.
//! A bare-bones HTTP/1.1 keep-alive responder, not a general server.
//!
//! Usage: mock [port] [stream_chunks] [embed_inputs] [embed_dim]

use std::sync::Arc;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct Canned {
    chat: Vec<u8>,
    stream: Vec<u8>,
    embed: Vec<u8>,
}

fn arg(i: usize, default: usize) -> usize {
    std::env::args().nth(i).and_then(|a| a.parse().ok()).unwrap_or(default)
}

fn http(content_type: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
        .into_bytes()
}

fn canned(chunks: usize, inputs: usize, dim: usize) -> Canned {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(8);
    let chat = format!(
        r#"{{"id":"chatcmpl-bench","object":"chat.completion","created":1761000000,"model":"bench-model","choices":[{{"index":0,"message":{{"role":"assistant","content":"{text}","refusal":null}},"logprobs":null,"finish_reason":"stop"}}],"usage":{{"prompt_tokens":24,"completion_tokens":80,"total_tokens":104}},"system_fingerprint":"fp_bench"}}"#
    );

    let mut stream =
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
    let mut event = |data: &str| {
        let ev = format!("data: {data}\n\n");
        stream.extend_from_slice(format!("{:x}\r\n{ev}\r\n", ev.len()).as_bytes());
    };
    let head = r#"{"id":"chatcmpl-bench","object":"chat.completion.chunk","created":1761000000,"model":"bench-model","system_fingerprint":"fp_bench","choices":[{"index":0,"delta":"#;
    for i in 0..chunks {
        event(&format!(r#"{head}{{"content":" token{i}"}},"logprobs":null,"finish_reason":null}}]}}"#));
    }
    event(&format!(r#"{head}{{}},"logprobs":null,"finish_reason":"stop"}}]}}"#));
    event(
        r#"{"id":"chatcmpl-bench","object":"chat.completion.chunk","created":1761000000,"model":"bench-model","choices":[],"usage":{"prompt_tokens":24,"completion_tokens":200,"total_tokens":224}}"#,
    );
    event("[DONE]");
    stream.extend_from_slice(b"0\r\n\r\n");

    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut data = Vec::with_capacity(inputs);
    for i in 0..inputs {
        let v: Vec<String> = (0..dim)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                format!("{:.10}", (seed % 2_000_000) as f64 / 1e8 - 0.01)
            })
            .collect();
        data.push(format!(r#"{{"object":"embedding","index":{i},"embedding":[{}]}}"#, v.join(",")));
    }
    let embed = format!(
        r#"{{"object":"list","data":[{}],"model":"bench-embed","usage":{{"prompt_tokens":128,"total_tokens":128}}}}"#,
        data.join(",")
    );

    Canned { chat: http("application/json", &chat), stream, embed: http("application/json", &embed) }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `"stream": true` in any JSON spacing.
fn wants_stream(body: &[u8]) -> bool {
    find(body, b"\"stream\"").is_some_and(|i| {
        let rest = &body[i + 8..];
        let rest: Vec<u8> = rest.iter().copied().skip_while(|b| matches!(b, b' ' | b':')).take(4).collect();
        rest == b"true"
    })
}

async fn serve(mut sock: TcpStream, canned: Arc<Canned>) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(16 * 1024);
    loop {
        let head_end = loop {
            if let Some(p) = find(&buf, b"\r\n\r\n") {
                break p + 4;
            }
            if sock.read_buf(&mut buf).await? == 0 {
                return Ok(());
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let len: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
        while buf.len() < head_end + len {
            if sock.read_buf(&mut buf).await? == 0 {
                return Ok(());
            }
        }
        let body = &buf[head_end..head_end + len];
        let res = if path.ends_with("/embeddings") {
            &canned.embed
        } else if wants_stream(body) {
            &canned.stream
        } else {
            &canned.chat
        };
        sock.write_all(res).await?;
        buf.drain(..head_end + len);
    }
}

#[tokio::main]
async fn main() {
    let port = arg(1, 8787);
    let canned = Arc::new(canned(arg(2, 200), arg(3, 16), arg(4, 1536)));
    let listener = TcpListener::bind(("127.0.0.1", port as u16)).await.expect("bind");
    println!("mock listening on http://127.0.0.1:{port}/v1");
    loop {
        let (sock, _) = listener.accept().await.expect("accept");
        sock.set_nodelay(true).ok();
        tokio::spawn(serve(sock, canned.clone()));
    }
}
