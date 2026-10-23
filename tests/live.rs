//! Live tests against a real OpenAI- and Anthropic-compatible server. Skipped by default;
//! they cost tokens. Run with:
//!
//! ```sh
//! AIWIRE_LIVE_OPENAI_URL=https://host/v1 AIWIRE_LIVE_ANTHROPIC_URL=https://host \
//! AIWIRE_LIVE_KEY=... AIWIRE_LIVE_MODEL=... cargo test --all-features --test live -- --ignored
//! ```
//!
//! Leave either URL unset to skip that protocol. The checks avoid exact wording; models vary.

use std::time::Duration;

use aiwire::{Cache, Client, Error, Event, FinishReason, Image, Message, Protocol, Request, Tool, ToolChoice, json};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn model() -> String {
    env("AIWIRE_LIVE_MODEL").expect("set AIWIRE_LIVE_MODEL")
}

/// One client per configured protocol.
fn clients() -> Vec<(Protocol, Client)> {
    let key = env("AIWIRE_LIVE_KEY").expect("set AIWIRE_LIVE_KEY");
    let mut out = Vec::new();
    for (var, protocol) in
        [("AIWIRE_LIVE_OPENAI_URL", Protocol::OpenAi), ("AIWIRE_LIVE_ANTHROPIC_URL", Protocol::Anthropic)]
    {
        if let Some(url) = env(var) {
            let c = Client::builder().base_url(url).protocol(protocol).api_key(&key).build().unwrap();
            out.push((protocol, c));
        }
    }
    assert!(!out.is_empty(), "set AIWIRE_LIVE_OPENAI_URL and/or AIWIRE_LIVE_ANTHROPIC_URL");
    out
}

fn weather() -> Tool {
    Tool::new(
        "get_weather",
        "Current weather for a city.",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    )
}

/// A 32x32 solid red PNG.
fn red_png() -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let c = crc(&out[start..]);
        out.extend(c.to_be_bytes());
    }
    let (w, h) = (32u32, 32u32);
    let row: Vec<u8> = std::iter::once(0).chain([255, 0, 0].repeat(w as usize)).collect();
    let raw = row.repeat(h as usize);
    // Stored (uncompressed) deflate blocks inside a zlib stream.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(u8::from(last));
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn chat_and_stream() {
    for (p, c) in clients() {
        let req = Request::new(model()).system("Answer in one short sentence.").user("What is the capital of France?");
        let res = c.chat(&req).await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
        assert!(res.text.contains("Paris"), "{p:?}: {res:?}");
        assert_eq!(res.finish_reason, FinishReason::Stop, "{p:?}");
        assert!(res.usage.output_tokens > 0 && !res.id.is_empty(), "{p:?}: {res:?}");

        let mut s = c.stream(&req).await.unwrap();
        let mut text = String::new();
        while let Some(ev) = s.next().await {
            if let Event::Text(t) = ev.unwrap_or_else(|e| panic!("{p:?}: {e:#?}")) {
                text.push_str(&t);
            }
        }
        let res = s.response().await.unwrap();
        assert_eq!(text, res.text, "{p:?}");
        assert!(res.text.contains("Paris") && res.usage.output_tokens > 0, "{p:?}: {res:?}");
        assert_eq!(res.finish_reason, FinishReason::Stop, "{p:?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn parameters() {
    for (p, c) in clients() {
        let res = c
            .chat(
                Request::new(model())
                    .user("Write a 300 word story about a cat.")
                    .max_tokens(20)
                    .temperature(0.2)
                    .top_p(0.9),
            )
            .await
            .unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
        assert_eq!(res.finish_reason, FinishReason::Length, "{p:?}: {res:?}");

        // Multi-turn memory.
        let req = Request::new(model())
            .user("My name is Zorblax. Reply with just OK.")
            .assistant("OK")
            .user("What is my name? Reply with just the name.");
        let res = c.chat(req).await.unwrap();
        assert!(res.text.contains("Zorblax"), "{p:?}: {}", res.text);

        // Provider-specific JSON passes through.
        let res = c.chat(Request::new(model()).user("Say hi").max_tokens(200).extra("seed", 1)).await;
        assert!(res.is_ok(), "{p:?}: {res:?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn tool_loop() {
    for (p, c) in clients() {
        for stream in [false, true] {
            let mut req = Request::new(model())
                .system("Use the get_weather tool for every city asked about, all calls at once.")
                .user("What's the weather in Paris and in Tokyo?")
                .tool(weather())
                .max_tokens(2000);
            let mut rounds = 0;
            let answer = loop {
                rounds += 1;
                assert!(rounds <= 4, "{p:?}: tool loop does not end");
                let res = if stream {
                    let mut s = c.stream(&req).await.unwrap();
                    let mut calls = Vec::new();
                    while let Some(ev) = s.next().await {
                        if let Event::ToolCall(call) = ev.unwrap_or_else(|e| panic!("{p:?}: {e:#?}")) {
                            calls.push(call);
                        }
                    }
                    let res = s.response().await.unwrap();
                    assert_eq!(calls, res.tool_calls, "{p:?}: streamed calls differ from the response");
                    res
                } else {
                    c.chat(&req).await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"))
                };
                if res.tool_calls.is_empty() {
                    break res.text;
                }
                assert_eq!(res.finish_reason, FinishReason::ToolCalls, "{p:?}");
                let calls = res.tool_calls.clone();
                req.messages.push(res.into_message());
                for call in &calls {
                    let args: serde_json::Value = call.args().unwrap();
                    let city = args["city"].as_str().unwrap_or_default().to_owned();
                    let temp = if city.contains("Paris") { "17" } else { "29" };
                    req = req.tool_result(call.id.clone(), format!("{city}: {temp} C, sunny"));
                }
            };
            assert!(rounds >= 2, "{p:?}: model never called the tool");
            assert!(answer.contains("17") && answer.contains("29"), "{p:?} stream={stream}: {answer}");
        }
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn tool_choice_forced() {
    for (p, c) in clients() {
        let req = Request::new(model())
            .user("Hello there!")
            .tool(weather())
            .tool_choice(ToolChoice::Tool("get_weather".into()))
            .max_tokens(1000);
        let res = c.chat(req).await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
        // Some gateways ignore forced tool choice; the request shape is covered by unit tests.
        if res.tool_calls.first().map(|t| t.name.as_str()) != Some("get_weather") {
            eprintln!("{p:?}: server ignored the forced tool choice: {:?}", res.text);
        }
        let req = Request::new(model()).user("Weather in Rome?").tool(weather()).tool_choice(ToolChoice::None);
        let res = c.chat(req).await.unwrap();
        assert!(res.tool_calls.is_empty(), "{p:?}: {res:?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn thinking_replayed_in_tool_loop() {
    for (p, c) in clients().into_iter().filter(|(p, _)| *p == Protocol::Anthropic) {
        for stream_first in [false, true] {
            let mut req = Request::new(model())
                .user(
                    "First work out 37*43 step by step, then call get_weather for Oslo. Finally tell me both results.",
                )
                .tool(weather())
                .max_tokens(4000)
                .extra("thinking", json!({"type": "enabled", "budget_tokens": 1024}));
            // The model decides whether to think; ask until it does.
            let mut tries = 0;
            let res = loop {
                tries += 1;
                let res =
                    if stream_first { c.stream(&req).await.unwrap().response().await } else { c.chat(&req).await };
                let res = res.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
                if (!res.thinking.is_empty() && !res.tool_calls.is_empty()) || tries == 5 {
                    break res;
                }
            };
            assert!(!res.thinking.is_empty(), "{p:?}: no thinking blocks in {tries} tries: {res:?}");
            assert!(res.thinking.iter().all(|t| !t.signature.is_empty()), "{p:?}: unsigned thinking: {res:?}");
            assert!(!res.tool_calls.is_empty(), "{p:?}: {res:?}");
            let calls = res.tool_calls.clone();
            req.messages.push(res.into_message());
            for call in &calls {
                req = req.tool_result(call.id.clone(), "Oslo: -3 C, snow");
            }
            let res = c.stream(&req).await.unwrap().response().await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
            assert!(res.text.replace([',', ' '], "").contains("1591"), "{p:?}: {}", res.text);
        }
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn long_multibyte_stream() {
    for (p, c) in clients() {
        let req = Request::new(model())
            .user("Write the numbers 1 to 150 separated by spaces, each followed by the emoji 🦀, then the word FIN. Nothing else.")
            .max_tokens(4000);
        let mut s = c.stream(&req).await.unwrap();
        let (mut text, mut events) = (String::new(), 0);
        while let Some(ev) = s.next().await {
            if let Event::Text(t) = ev.unwrap_or_else(|e| panic!("{p:?}: {e:#?}")) {
                text.push_str(&t);
                events += 1;
            }
        }
        let res = s.response().await.unwrap();
        assert_eq!(text, res.text);
        assert!(events > 5, "{p:?}: only {events} text events");
        assert!(text.contains("150") && text.contains("🦀") && text.contains("FIN"), "{p:?}: {text}");
    }
}

#[cfg(feature = "mcp")]
#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars and uvx"]
async fn mcp_agent_loop() {
    use aiwire::mcp::Mcp;
    let mcp = Mcp::stdio(if cfg!(windows) { "uvx.exe" } else { "uvx" }, ["mcp-server-time"])
        .await
        .expect("uvx mcp-server-time");
    let tools = mcp.tools().await.unwrap();
    for (p, c) in clients() {
        let mut req = Request::new(model())
            .user("What time is it now in Asia/Tokyo? Use the tools, then answer with the HH:MM time.")
            .tools(tools.clone())
            .max_tokens(3000);
        let mut used = false;
        for _ in 0..4 {
            let res = c.chat(&req).await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
            if res.tool_calls.is_empty() {
                assert!(used, "{p:?}: model answered without the MCP tool: {}", res.text);
                assert!(res.text.contains(':'), "{p:?}: {}", res.text);
                break;
            }
            used = true;
            let calls = res.tool_calls.clone();
            req.messages.push(res.into_message());
            for call in &calls {
                let out = mcp.call(call).await.unwrap();
                req = req.tool_result(call.id.clone(), out);
            }
        }
        assert!(used, "{p:?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn image_input() {
    let png = red_png();
    for (p, c) in clients() {
        let req = Request::new(model())
            .user("Is this image red, green or blue? Answer with one word.")
            .image(Image::bytes("image/png", &png));
        // Vision answers on a solid square are noisy (seen with plain curl too): best of 3.
        for stream in [false, true] {
            let mut answers = Vec::new();
            for _ in 0..3 {
                let res = if stream { c.stream(&req).await.unwrap().response().await } else { c.chat(&req).await };
                let text = res.unwrap_or_else(|e| panic!("{p:?}: {e:#?}")).text.to_lowercase();
                if text.contains("red") {
                    break;
                }
                answers.push(text);
            }
            assert!(answers.len() < 3, "{p:?} stream={stream}: never saw red: {answers:?}");
        }
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn prompt_cache() {
    let manual = "Rule: always be polite and precise. ".repeat(400);
    for (p, c) in clients() {
        let req = Request::new(model())
            .system(manual.clone())
            .user("Reply with OK.")
            .max_tokens(300)
            .cache(Cache::FiveMinutes);
        let first = c.chat(&req).await.unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
        let second = c.chat(&req).await.unwrap();
        eprintln!("{p:?}: first {:?}, second {:?}", first.usage, second.usage);
        if p == Protocol::Anthropic {
            assert!(second.usage.input_tokens > 1000, "{p:?}");
            assert!(second.usage.cached_tokens > 0, "{p:?}: nothing served from cache: {:?}", second.usage);
        }
        let msg =
            Request::new(model()).message(Message::user(manual.clone()).cache()).user("Reply with OK.").max_tokens(300);
        assert!(c.chat(msg).await.is_ok(), "{p:?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn errors_are_debuggable() {
    let key_vars = ["AIWIRE_LIVE_OPENAI_URL", "AIWIRE_LIVE_ANTHROPIC_URL"];
    for (var, protocol) in key_vars.iter().zip([Protocol::OpenAi, Protocol::Anthropic]) {
        let Some(url) = env(var) else { continue };
        let c =
            Client::builder().base_url(&url).protocol(protocol).api_key("wrong-key").max_retries(0).build().unwrap();
        let err = c.chat(Request::new(model()).user("hi")).await.unwrap_err();
        eprintln!("{protocol:?}: {err}\n{err:#?}");
        let api = err.api().unwrap_or_else(|| panic!("{protocol:?}: {err:#?}"));
        assert!(matches!(api.status, 401 | 403), "{protocol:?}: {api:#?}");
        assert!(!api.message.is_empty() && !api.body.is_empty() && !api.url.is_empty(), "{api:#?}");
        assert!(!err.is_retryable());

        let bad =
            Client::builder().base_url(format!("{url}/nope")).protocol(protocol).api_key("k").max_retries(0).build();
        let err = bad.unwrap().chat(Request::new(model()).user("hi")).await.unwrap_err();
        eprintln!("{protocol:?}: {err}");
        assert_eq!(err.status(), Some(404), "{err:#?}");
    }
}

#[tokio::test]
#[ignore = "live: needs AIWIRE_LIVE_* env vars"]
async fn concurrent_and_timeouts() {
    for (p, _) in clients() {
        let url =
            env(if p == Protocol::OpenAi { "AIWIRE_LIVE_OPENAI_URL" } else { "AIWIRE_LIVE_ANTHROPIC_URL" }).unwrap();
        let c = Client::builder()
            .base_url(url)
            .protocol(p)
            .api_key(env("AIWIRE_LIVE_KEY").unwrap())
            .max_concurrent_requests(3)
            .stream_idle_timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        let tasks: Vec<_> = (1..=6)
            .map(|i| {
                let c = c.clone();
                tokio::spawn(async move {
                    let req = Request::new(model()).user(format!("What is {i} + {i}? Reply with just the number."));
                    let res = c.stream(req).await?.response().await?;
                    Ok::<_, Error>((i, res.text))
                })
            })
            .collect();
        for t in tasks {
            let (i, text) = t.await.unwrap().unwrap_or_else(|e| panic!("{p:?}: {e:#?}"));
            assert!(text.contains(&(i * 2).to_string()), "{p:?}: {i}+{i} -> {text}");
        }
    }
}
