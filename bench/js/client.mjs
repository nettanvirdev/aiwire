// openai-node side of the benchmark. Same workload as bench/src/bin/client.rs.
// Usage: node client.mjs <base_url> <chat|stream|embed> <requests> <concurrency>
import OpenAI from "openai";

const [url, mode, nStr, concStr] = process.argv.slice(2);
const n = Number(nStr), conc = Number(concStr);
const client = new OpenAI({ baseURL: url, apiKey: "bench", maxRetries: 0 });
const messages = [
  { role: "system", content: "You are a helpful assistant." },
  { role: "user", content: "Write one sentence about foxes." },
];
const inputs = Array.from({ length: 16 }, (_, i) => `document number ${i} about foxes and dogs`);

async function one() {
  if (mode === "chat") {
    const r = await client.chat.completions.create({ model: "bench-model", messages, max_tokens: 256 });
    return r.choices[0].message.content.length;
  }
  if (mode === "stream") {
    let len = 0;
    const stream = await client.chat.completions.create({
      model: "bench-model", messages, max_tokens: 256, stream: true, stream_options: { include_usage: true },
    });
    for await (const chunk of stream) len += chunk.choices[0]?.delta?.content?.length ?? 0;
    return len;
  }
  const r = await client.embeddings.create({ model: "bench-embed", input: inputs, encoding_format: "float" });
  return r.data.length;
}

let next = 0, check = 0;
const lat = [];
async function worker() {
  while (next++ < n) {
    const t = performance.now();
    const got = await one();
    check += got;
    lat.push(performance.now() - t);
  }
}

const start = performance.now();
await Promise.all(Array.from({ length: conc }, worker));
const secs = (performance.now() - start) / 1000;
lat.sort((a, b) => a - b);
const pct = (p) => lat[Math.min(Math.floor(lat.length * p), lat.length - 1)];
console.log(JSON.stringify({ secs, p50_ms: pct(0.5), p99_ms: pct(0.99), check }));
// Stay alive so the driver can read peak memory and CPU time.
process.stdin.once("data", () => process.exit(0));
