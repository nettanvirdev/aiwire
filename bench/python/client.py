"""openai-python side of the benchmark. Same workload as bench/src/bin/client.rs.

Usage: python client.py <base_url> <chat|stream|embed> <requests> <concurrency>
"""

import asyncio
import json
import sys
import time

from openai import AsyncOpenAI

MESSAGES = [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "Write one sentence about foxes."},
]
INPUTS = [f"document number {i} about foxes and dogs" for i in range(16)]


async def one(client, mode):
    if mode == "chat":
        r = await client.chat.completions.create(model="bench-model", messages=MESSAGES, max_tokens=256)
        return len(r.choices[0].message.content)
    if mode == "stream":
        n = 0
        stream = await client.chat.completions.create(
            model="bench-model", messages=MESSAGES, max_tokens=256, stream=True,
            stream_options={"include_usage": True},
        )
        async for chunk in stream:
            if chunk.choices and chunk.choices[0].delta.content:
                n += len(chunk.choices[0].delta.content)
        return n
    r = await client.embeddings.create(model="bench-embed", input=INPUTS, encoding_format="float")
    return len(r.data)


async def main():
    url, mode, n, conc = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    client = AsyncOpenAI(base_url=url, api_key="bench", max_retries=0)
    next_i, lat, check = 0, [], 0

    async def worker():
        nonlocal next_i, check
        while next_i < n:
            next_i += 1
            t = time.perf_counter()
            got = await one(client, mode)
            check += got
            lat.append((time.perf_counter() - t) * 1e3)

    start = time.perf_counter()
    await asyncio.gather(*(worker() for _ in range(conc)))
    secs = time.perf_counter() - start
    lat.sort()
    pct = lambda p: lat[min(int(len(lat) * p), len(lat) - 1)]
    print(json.dumps({"secs": secs, "p50_ms": pct(0.5), "p99_ms": pct(0.99), "check": check}), flush=True)


asyncio.run(main())
sys.stdin.readline()  # stay alive so the driver can read peak memory and CPU time
