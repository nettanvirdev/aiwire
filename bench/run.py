"""Benchmark aiwire vs openai-python vs openai-node against a local mock server.

No API keys, no tokens, no network: every SDK talks to bench/src/bin/mock.rs on 127.0.0.1.
Measures throughput, latency, peak memory (RSS / working set) and total CPU time per SDK.

Setup:  pip install openai psutil   &&   (cd bench/js && npm install)
Run:    python bench/run.py [--quick]
"""

import json
import os
import platform
import subprocess
import sys
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "bench"
PORT = 8787
URL = f"http://127.0.0.1:{PORT}/v1"
QUICK = "--quick" in sys.argv

# mode -> (requests, concurrency)
WORKLOADS = {
    "chat": (2_000 if QUICK else 20_000, 64),
    "stream": (300 if QUICK else 3_000, 64),  # 200 SSE chunks per response
    "embed": (100 if QUICK else 1_000, 16),  # 16 x 1536 floats per response (~460 KB JSON)
}


def exe(name):
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    return str(target / "release" / (name + (".exe" if os.name == "nt" else "")))


def peak_rss(p):
    mem = p.memory_info()
    if hasattr(mem, "peak_wset"):  # Windows
        return mem.peak_wset
    try:  # Linux
        for line in Path(f"/proc/{p.pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1]) * 1024
    except OSError:
        pass
    return mem.rss  # macOS: current, not peak


def run(cmd, tail, mode):
    n, conc = WORKLOADS[mode]
    proc = subprocess.Popen(
        cmd + [URL, mode, str(n), str(conc)] + tail, cwd=BENCH, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    line = proc.stdout.readline()
    if not line:
        proc.wait()
        raise RuntimeError(f"{cmd} {mode} failed")
    r = json.loads(line)
    p = psutil.Process(proc.pid)
    # Node/npm launchers may wrap the real process; measure the busiest descendant too.
    procs = [p] + p.children(recursive=True)
    r["peak_mb"] = max(peak_rss(q) for q in procs) / 2**20
    r["cpu_s"] = sum(q.cpu_times().user + q.cpu_times().system for q in procs)
    r["rps"] = n / r["secs"]
    proc.stdin.write("\n")
    proc.stdin.close()
    proc.wait()
    return r


def version(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, cwd=BENCH).stdout.strip()
    except OSError:
        return "?"


def main():
    subprocess.run(["cargo", "build", "--release", "-p", "aiwire-bench"], cwd=ROOT, check=True)
    mock = subprocess.Popen([exe("mock"), str(PORT)], stdout=subprocess.PIPE, text=True)
    mock.stdout.readline()

    sdks = {  # name -> (command, args after the workload args)
        "aiwire (Rust, 1 thread)": ([exe("client")], ["1"]),
        "aiwire (Rust, 4 threads)": ([exe("client")], ["4"]),
        "openai-python": ([sys.executable, "python/client.py"], []),
        "openai-node": (["node", "js/client.mjs"], []),
    }
    rows = []
    try:
        for mode in WORKLOADS:
            for name, (cmd, tail) in sdks.items():
                r = run(cmd, tail, mode)
                rows.append((mode, name, r))
                print(f"{mode:6} {name:26} {r['rps']:9.0f} req/s  p50 {r['p50_ms']:7.2f} ms  "
                      f"p99 {r['p99_ms']:7.2f} ms  peak {r['peak_mb']:6.1f} MB  cpu {r['cpu_s']:6.2f} s  check {r['check']}", flush=True)
    finally:
        mock.kill()

    py_openai = version([sys.executable, "-c", "import openai; print(openai.__version__)"])
    node_openai = version(["node", "-p", "require('./js/node_modules/openai/package.json').version"])
    out = [
        "# Benchmark results",
        "",
        f"Machine: {platform.platform()}, {psutil.cpu_count()} logical CPUs, {platform.processor() or platform.machine()}  ",
        f"Python {platform.python_version()} + openai {py_openai}; Node {version(['node', '--version'])} + openai {node_openai}  ",
        "Server: local mock (bench/src/bin/mock.rs), plain HTTP/1.1 on 127.0.0.1. "
        f"Workloads (requests, concurrency): {WORKLOADS}.",
        "",
        "| workload | SDK | req/s | p50 ms | p99 ms | peak memory MB | CPU s |",
        "|---|---|--:|--:|--:|--:|--:|",
    ]
    for mode, name, r in rows:
        out.append(f"| {mode} | {name} | {r['rps']:.0f} | {r['p50_ms']:.2f} | {r['p99_ms']:.2f} | "
                   f"{r['peak_mb']:.1f} | {r['cpu_s']:.2f} |")
    (BENCH / "RESULTS.md").write_text("\n".join(out) + "\n", encoding="utf8")
    print("\nwrote bench/RESULTS.md")


if __name__ == "__main__":
    main()
