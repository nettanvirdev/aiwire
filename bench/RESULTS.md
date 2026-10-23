# Benchmark results

Machine: Windows-11-10.0.26200-SP0, 32 logical CPUs, Intel64 Family 6 Model 183 Stepping 1, GenuineIntel  
Python 3.14.7 + openai 2.37.0; Node v26.8.1 + openai 7.42.0  
Server: local mock (bench/src/bin/mock.rs), plain HTTP/1.1 on 127.0.0.1. Workloads (requests, concurrency): {'chat': (20000, 64), 'stream': (3000, 64), 'embed': (1000, 16)}.

| workload | SDK | req/s | p50 ms | p99 ms | peak memory MB | CPU s |
|---|---|--:|--:|--:|--:|--:|
| chat | aiwire (Rust, 1 thread) | 46147 | 1.19 | 2.65 | 10.8 | 0.44 |
| chat | aiwire (Rust, 4 threads) | 100705 | 0.59 | 1.36 | 12.0 | 0.44 |
| chat | openai-python | 593 | 72.67 | 523.20 | 96.7 | 34.78 |
| chat | openai-node | 6177 | 9.39 | 23.33 | 220.8 | 4.83 |
| stream | aiwire (Rust, 1 thread) | 3486 | 17.68 | 24.24 | 18.0 | 0.84 |
| stream | aiwire (Rust, 4 threads) | 12942 | 4.74 | 10.65 | 17.7 | 0.92 |
| stream | openai-python | 61 | 1026.75 | 1189.01 | 162.2 | 49.31 |
| stream | openai-node | 610 | 101.09 | 154.32 | 242.1 | 5.84 |
| embed | aiwire (Rust, 1 thread) | 1573 | 9.89 | 18.47 | 14.2 | 0.64 |
| embed | aiwire (Rust, 4 threads) | 5784 | 2.70 | 5.08 | 14.8 | 0.67 |
| embed | openai-python | 29 | 402.61 | 2577.93 | 127.2 | 34.30 |
| embed | openai-node | 663 | 23.04 | 46.75 | 293.5 | 1.95 |
