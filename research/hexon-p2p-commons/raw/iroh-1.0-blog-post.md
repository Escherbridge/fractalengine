# Iroh 1.0 - Dial Keys, not IPs

URL: https://www.iroh.computer/blog/v1
Fetched: 2026-07-11
Query intent: Topic 1 - iroh 1.0 release status, what changed from 0.35, perf/scale stats

## Extracted content (via WebFetch summarization)

### Key Changes from 0.35

Wire & API Stability: "an iroh v1 endpoint will be able to communicate with another v1 endpoint,
regardless of minor version or language." Version 0.35.x will no longer receive updates; public
relay support for 0.35 ends December 31, 2026.

Technical Improvements:
- Custom QUIC multipath implementation enabling multiple routes within single connections
- QUIC NAT traversal for direct connections with encrypted details
- Local-first configurations for internet-free device discovery
- WebAssembly compilation and browser execution support
- Custom transport plugins (Bluetooth Low-Energy, LoRa, WiFi Aware, Tor)
- Language bindings: Python, Node.js, Kotlin, and Swift now officially supported

### Performance & Usage Statistics

Scale: The public relays handled "more than 200 million endpoints created, in the last 30 days
alone." The announcement notes iroh "running on millions of devices today."

Efficiency: "It's normal to see 95% of data transferred in a connection pass directly between
devices," reducing cloud egress and network hops.

### Missing Information

The page contains no data on connection setup latency, throughput benchmarks, iroh-docs/document
sync, storage/garbage collection, mobile-specific performance metrics, or formal benchmark
comparisons.

(Note: captured via WebFetch's summarizing prompt, not a raw HTML dump — WebFetch does not return
raw source text, only a processed answer to the prompt given. This is the fullest available capture
of this page's content through this tool.)
