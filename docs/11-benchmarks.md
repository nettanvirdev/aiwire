# 11. Benchmarks

`bench/` compares aiwire with the official OpenAI SDKs for Python and Node on the same
workloads. Nothing leaves your machine: every SDK talks to a local mock server
(`bench/src/bin/mock.rs`) that returns canned OpenAI-format responses, so the numbers measure
**SDK overhead** (HTTP, JSON, SSE parsing, allocation), not model speed.

Latest results: [bench/RESULTS.md](../bench/RESULTS.md).

## Workloads

| Name | What each request does | Requests | Concurrency |
|---|---|--:|--:|
| `chat` | non-streaming chat, ~360-char answer | 20,000 | 64 |
| `stream` | streaming chat, 200 SSE chunks | 3,000 | 64 |
| `embed` | 16 inputs → 16 × 1536 floats (~460 KB JSON) | 1,000 | 16 |

Measured per SDK process: requests/second, p50/p99 latency, **peak memory** (working set on
Windows, `VmHWM` on Linux) and **total CPU time**. aiwire runs twice: on one thread (like
Python asyncio and Node's event loop) and on four.

Every run prints a `check` value (total characters or vectors received); it must match across
SDKs, which proves each one really parsed every response.

## Run it

```bash
pip install openai psutil
cd bench/js && npm install && cd ../..
python bench/run.py           # full run, a few minutes
python bench/run.py --quick   # 10x smaller
```

The script builds the Rust side in release mode, starts the mock on port 8787, runs every
SDK, and writes `bench/RESULTS.md`.

## Reading the numbers

- Real APIs add network and model latency, so end-to-end speedups in production are smaller.
  What stays: CPU per request (more requests per core, smaller machines) and memory
  (more concurrent conversations per box).
- The mock answers instantly, which makes it a worst case for the SDKs: they are the
  bottleneck.
