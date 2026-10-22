# Benchmark results

Machine: Windows-11-10.0.26200-SP0, 32 logical CPUs, Intel64 Family 6 Model 183 Stepping 1, GenuineIntel  
Python 3.14.7 + openai 2.37.0; Node v26.8.1 + openai 7.42.0  
Server: local mock (bench/src/bin/mock.rs), plain HTTP/1.1 on 127.0.0.1. Workloads (requests, concurrency): {'chat': (20000, 64), 'stream': (3000, 64), 'embed': (1000, 16)}.

| workload | SDK | req/s | p50 ms | p99 ms | peak memory MB | CPU s |
|---|---|--:|--:|--:|--:|--:|
| chat | aiwire (Rust, 1 thread) | 28910 | 2.23 | 3.47 | 11.1 | 0.70 |
| chat | aiwire (Rust, 4 threads) | 86393 | 0.70 | 1.45 | 13.2 | 0.73 |
| chat | openai-python | 345 | 126.26 | 886.06 | 100.5 | 58.92 |
| chat | openai-node | 3830 | 15.93 | 35.07 | 277.0 | 7.59 |
| stream | aiwire (Rust, 1 thread) | 2987 | 18.40 | 29.70 | 18.6 | 1.00 |
| stream | aiwire (Rust, 4 threads) | 10597 | 5.34 | 17.04 | 18.7 | 1.16 |
| stream | openai-python | 39 | 1652.15 | 1903.64 | 169.9 | 77.69 |
| stream | openai-node | 429 | 142.49 | 226.61 | 193.1 | 8.25 |
| embed | aiwire (Rust, 1 thread) | 1028 | 13.71 | 21.25 | 14.1 | 0.97 |
| embed | aiwire (Rust, 4 threads) | 4836 | 3.13 | 6.22 | 15.8 | 0.78 |
| embed | openai-python | 20 | 703.47 | 2487.69 | 133.6 | 51.80 |
| embed | openai-node | 463 | 32.38 | 66.99 | 296.5 | 2.91 |
