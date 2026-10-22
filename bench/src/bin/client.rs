//! aiwire side of the benchmark. Same workload as bench/python/client.py and bench/js/client.mjs.
//!
//! Usage: client <base_url> <chat|stream|embed> <requests> <concurrency> [threads]
//! threads = 1 runs a single-threaded runtime, like Python asyncio and Node.

use std::{
    io::BufRead,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use aiwire::{Client, EmbedRequest, Event, Protocol, Request};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (url, mode) = (a[1].clone(), a[2].clone());
    let n: usize = a[3].parse().unwrap();
    let conc: usize = a[4].parse().unwrap();
    let threads: usize = a.get(5).map_or(1, |t| t.parse().unwrap());

    let rt = if threads == 1 {
        tokio::runtime::Builder::new_current_thread().enable_all().build()
    } else {
        tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build()
    }
    .unwrap();

    let (secs, mut lat, check) = rt.block_on(async {
        let client =
            Client::builder().base_url(url).protocol(Protocol::OpenAi).api_key("bench").max_retries(0).build().unwrap();
        let next = Arc::new(AtomicUsize::new(0));
        let start = Instant::now();
        let workers: Vec<_> = (0..conc)
            .map(|_| {
                let (client, next, mode) = (client.clone(), next.clone(), mode.clone());
                tokio::spawn(async move {
                    let mut lat = Vec::new();
                    let mut check = 0usize;
                    while next.fetch_add(1, Ordering::Relaxed) < n {
                        let t = Instant::now();
                        check += match mode.as_str() {
                            "chat" => client.chat(chat_req()).await.unwrap().text.len(),
                            "stream" => {
                                let mut s = client.stream(chat_req()).await.unwrap();
                                let mut len = 0;
                                while let Some(ev) = s.next().await {
                                    if let Event::Text(t) = ev.unwrap() {
                                        len += t.len();
                                    }
                                }
                                len
                            }
                            "embed" => client.embed(embed_req()).await.unwrap().len(),
                            m => panic!("unknown mode {m}"),
                        };
                        lat.push(t.elapsed().as_secs_f64() * 1e3);
                    }
                    (lat, check)
                })
            })
            .collect();
        let (mut lat, mut check) = (Vec::with_capacity(n), 0);
        for w in workers {
            let (l, c) = w.await.unwrap();
            lat.extend(l);
            check += c;
        }
        (start.elapsed().as_secs_f64(), lat, check)
    });

    lat.sort_by(f64::total_cmp);
    let pct = |p: f64| lat[((lat.len() as f64 * p) as usize).min(lat.len() - 1)];
    println!(r#"{{"secs":{secs:.4},"p50_ms":{:.3},"p99_ms":{:.3},"check":{check}}}"#, pct(0.5), pct(0.99));
    // Stay alive so the driver can read peak memory and CPU time.
    std::io::stdin().lock().lines().next();
}

fn chat_req() -> Request {
    Request::new("bench-model")
        .system("You are a helpful assistant.")
        .user("Write one sentence about foxes.")
        .max_tokens(256)
}

fn embed_req() -> EmbedRequest {
    EmbedRequest::new("bench-embed").inputs((0..16).map(|i| format!("document number {i} about foxes and dogs")))
}
