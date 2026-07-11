# Source: n0-computer/iroh GitHub Issue #4286

URL: https://github.com/n0-computer/iroh/issues/4286
Fetch date: 2026-07-11
Query intent: iroh performance benchmarks throughput/latency

## Extracted content

Title: "iroh-blobs single-stream LAN throughput caps at ~40% of link capacity with BBR; CUBIC is ~30x slower than BBR on the same path"

- Issue opened: May 24, 2026, against iroh version 0.98
- Baseline LAN capacity: ~110 MB/s (measured via iperf3 UDP)
- BBR congestion control: 42-50 MB/s (~40% of link capacity)
  - BBR tuned windows (32M/64M): 48.16 MB/s
  - BBR vanilla config: 37 MB/s
- CUBIC congestion control: 1.29 MB/s (fs storage), 1.70 MB/s (vanilla settings) — ~1-1.5% of available bandwidth
- Key quote: "CUBIC is ~30x slower than BBR on the same path"
- In-memory storage performed *slower* than filesystem storage in this test, suggesting the bottleneck is congestion-control/windowing, not disk I/O.

## Confidence: HIGH (primary source, specific reproducible numbers, but single-reporter/single-LAN — not an official n0 benchmark)
