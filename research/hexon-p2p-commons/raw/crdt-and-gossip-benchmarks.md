# Source: multiple — CRDT benchmarks (Loro/Yjs/Automerge), gossipsub scaling, iroh-gossip, Willow, redb

Fetch date: 2026-07-11
Query intent: CRDT scale numbers, gossip/pubsub scaling numbers, embedded KV backend numbers

## Automerge (automerge.org blog + BigGo aggregation of Automerge 3.0 announcement)
- Automerge 2.0 -> 3.0: memory for a Moby-Dick-sized document (~1.6M-char equivalent test doc) dropped from 700MB (v2.0) to 1.3MB (v3.0) — a "10x" figure quoted in secondary coverage, though the raw 700MB->1.3MB numbers are actually ~538x; treat "10x" as the vendor's own framing, the raw numbers as the harder data.
- Benchmark methodology (crdt-benchmarks suite, dmonad/zxch3n forks): tracks time-to-replay, update-message size, encoded-doc size, encode/parse time, memory-to-hold-decoded-doc.
- A 10-million-character replay benchmark (100x replay of a real editing trace) requires a single transaction in Automerge or it exhausts memory — i.e., naive incremental replay of very large op-logs is memory-fragile even in recent Automerge.

## Loro vs Yjs (loro.dev/docs/performance, PkgPulse guide, Yjs community discussion)
- B4 benchmark (real-world editing trace, 260K-character document): Loro leads Yjs and Automerge in every category except initial WASM load time.
- Loro's encoding format: 2-5x smaller documents than Yjs/Automerge for equivalent content.
- Loro WASM bundle: 320kB, ~50ms one-time init cost — a real latency tax for browser/short-lived-process use cases.
- Ecosystem maturity: Yjs ~920k weekly downloads / 17k GitHub stars (production default); Loro ~12k downloads (fastest in benchmarks, much younger ecosystem).
- Takeaway framed by source: "Pick Yjs unless you need Automerge's history model or Loro's raw performance."

## Willow protocol (willowprotocol.org/specs/rbsr, arXiv 2212.13567)
- Willow uses 3D range-based set reconciliation (RBSR): recursively partitions an ordered universe into ranges, yields "logarithmically many rounds and communication within a logarithmic factor of optimal" — an asymptotic/theoretical guarantee, not a measured wall-clock number.
- Explicit caveat in the spec itself: RBSR is only efficient "if the backend can summarize an arbitrary range, split by relative cardinality, and enumerate small residual parts without repeated scanning" — i.e., the storage backend design determines whether the asymptotic win is realized. This directly implicates redb/SurrealKV choice for fractalengine.
- Comparable production use: Negentropy (an RBSR implementation) is used in Nostr (NIP-77) via strfry, nostria-relay, @nostr-dev-kit/sync — proof RBSR works in a real deployed gossip-relay network, but Nostr relays are servers, not residential/mobile P2P nodes.
- No 2026-dated iroh-willow production numbers found; iroh-willow remains an unreleased/WIP crate (per n0-computer/iroh-willow repo and HN dev comments).

## Gossipsub / libp2p pubsub scaling (research.protocol.ai gossipsub v1.1 eval report; Logos/Vac research)
- Published GossipSub v1.1 benchmark: 1000 honest nodes (100 publishers/miners + 900 lurkers/full nodes), baseline non-adversarial conditions.
- Topology: average out-degree 6 -> ideal-case full-network reach in 4 hops.
- One cited scenario: a 1024-byte message reached the entire 1000-node network within 350ms.
- Newer proposals (PPPT — push-pull phase transition, GossipSub v1.4, v2.0) evaluated against v1.2 using nim-libp2p + Shadow network simulator — i.e. simulation-based, not live-network measurement.

## iroh-gossip (docs.iroh.computer/connecting/gossip, iroh-gossip crate docs)
- Built on HyParView (membership) + PlumTree (broadcast tree) — same academic lineage as many epidemic-broadcast systems.
- Vendor characterization: "scales to a few thousand peers" (no specific number, no published benchmark found for iroh-gossip specifically — this is a qualitative claim, not measured).
- Caveat: each additional gossip *topic* subscription opens more connections and grows the local routing table — multi-topic (e.g., one gossip topic per Petal) has a compounding connection-count cost that is NOT quantified anywhere published.

## redb (GitHub cberner/redb, lib.rs)
- Vendor-run micro-benchmark (Ryzen 5900X, Samsung 980 PRO NVMe, 5M keys, single-host/single-thread): redb is 1.6-3x faster than LMDB on writes, 2-3x faster than RocksDB on reads; redb is SLOWER than LMDB on bulk-load and random-read workloads.
- Explicit caveat from the source itself: these are single-host, single-thread micro-benchmarks, "not large-scale, write-heavy, concurrent scenarios" — i.e., NOT representative of a P2P node under concurrent sync + local-write load, which is exactly fractalengine's use case for the iroh-docs backend.

## Confidence summary
- Loro/Yjs/Automerge numbers: HIGH (multiple independent benchmark suites converge)
- Willow RBSR asymptotics: HIGH (formal spec + peer-reviewed arXiv paper) but wall-clock numbers: LOW (none published)
- Gossipsub 1000-node/350ms: MEDIUM (protocol Labs official eval report, but is Protocol Labs' own PoC test, not third-party)
- iroh-gossip "few thousand peers": LOW (vendor qualitative claim, no benchmark)
- redb numbers: MEDIUM (reproducible, vendor-run, but explicitly not concurrent/scale-representative)
