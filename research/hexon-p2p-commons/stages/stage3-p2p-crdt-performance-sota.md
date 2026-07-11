---
type: research-findings
stage: 3
date: 2026-07-11
---

# Stage 3: State of the Art — P2P Content Distribution & CRDT Replication Performance at "Federated 3D Commons" Scale

## Scope and method

External web research (WebSearch/WebFetch), raw captures saved under `research/hexon-p2p-commons/raw/`. Goal: close p2p-mycelium §8 unknowns with numbers, or explicitly mark them still-open. Every claim below carries a confidence rating and a source. Contradictions between sources are preserved, not smoothed.

## 1. iroh performance: what's actually measured

### 1.1 iroh reached 1.0 on June 15, 2026 — mid-research-cycle context shift
**Confidence: HIGH.** Source: [iroh 1.0 blog](https://www.iroh.computer/blog/v1); raw: `raw/iroh-1-0-blog-v1.md`.

This *supersedes* the p2p-mycelium (April 2026) framing that "0.35 is the last production-quality version, 0.99 is current dev." iroh shipped stable 1.0 after 65 pre-release versions. Wire-protocol stability is now a formal commitment (breaking wire changes require a major version). New official bindings: Python, Node.js, Kotlin, Swift. **Design implication:** fractalengine's iroh 0.35 pin needs a deliberate upgrade decision — 0.35 relay support runs only until Dec 31, 2026. The "isolate iroh behind a trait boundary" mitigation from the April research was the right call and is now on a concrete clock.

Adoption figure (self-reported by n0): "more than 200 million endpoints created in the last 30 days" on public relays. **Confidence: MEDIUM** — this counts endpoint *creations*, not concurrent active peers or sustained connections; it's a marketing-adjacent vanity metric, not a load-bearing capacity number.

### 1.2 Real measured throughput: 40% of link capacity, and congestion control choice is a 30x lever
**Confidence: HIGH** (primary source, reproducible numbers). Source: [n0-computer/iroh#4286](https://github.com/n0-computer/iroh/issues/4286); raw: `raw/iroh-blobs-throughput-issue-4286.md`.

On a LAN with ~110 MB/s iperf3-measured capacity (May 2026, iroh 0.98):
- **BBR congestion control: 42-50 MB/s (~40% of link capacity)** — this is the *best* iroh-blobs single-stream result found in this research.
- **CUBIC congestion control: 1.29-1.70 MB/s (~1-1.5% of capacity)** — CUBIC is ~30x slower than BBR on the identical path.
- In-memory storage was *slower* than filesystem storage in this test — the bottleneck is congestion-control/windowing, not disk I/O.

**Design-constraining number:** if fractalengine's transport defaults to CUBIC (many QUIC stacks do, or OS-level defaults leak through), single-stream hexon/blob transfer could run at ~1-2% of available bandwidth, not 40%. This is a concrete, actionable finding: **verify and explicitly configure BBR** in fe-sync's iroh transport config; do not assume defaults are tuned for throughput.

### 1.3 NAT traversal: iroh's own claim (90-95%) is not independently verified at the scale the published academic data covers
**Confidence: mixed — see contradiction below.** Sources: [iroh vs libp2p blog](https://www.iroh.computer/blog/comparing-iroh-and-libp2p); Trautwein et al. papers ([arXiv 2604.12484](https://arxiv.org/abs/2604.12484), [arXiv 2510.27500](https://arxiv.org/pdf/2510.27500)); raw: `raw/nat-traversal-measurement-papers.md`.

**The strongest independently-measured NAT traversal number available:** a peer-reviewed-track study (ACM IMC '26) measured **4.4 million+ real hole-punch attempts across 85,000+ networks in 167 countries** for libp2p's DCUtR (used by IPFS) and found a **conditional success rate of 70% ± 7.1%** (conditional on relay-reservation and public-address-discovery both succeeding first). Notably, the paper found **TCP and QUIC perform statistically identically (~70%)** — directly contradicting the common belief ("tribal knowledge") that UDP hole-punches better than TCP.

**The contradiction:** iroh's own marketing/blog material quotes libp2p's ~70% figure for comparison, then claims iroh itself achieves ~90-95% — but no independently-measured, comparably-scaled study of iroh's own hole-punch success rate was found anywhere in this research. The 90-95% number traces back only to n0/iroh's own posts and self-monitoring (`perf.iroh.com`), with no disclosed methodology at comparable scale (4M+ attempts, 85k+ networks). **Treat iroh's 90-95% as vendor-claimed/LOW confidence; treat libp2p DCUtR's ~70% as HIGH confidence** and as the most defensible baseline planning number for "what fraction of peer pairs get a direct QUIC connection in the wild" until iroh publishes comparable independent data.

**Design implication:** fractalengine should plan for **relay-dependent connectivity in roughly 5-30% of peer pairings** (using the 70-95% success band as the honest range), not treat direct P2P as the default case. Relay bandwidth/cost planning (see §3) should be sized against the pessimistic end of that band, not the optimistic one.

## 2. IPFS/Bitswap: the discovery-broadcast problem, and why content-addressed-with-known-provider sidesteps it

**Confidence: HIGH.** Sources: arXiv 2208.05877, NSF-hosted IPFS accessibility study, ipfs/go-bitswap#166, Protocol Labs multi-path Bitswap paper; raw: `raw/ipfs-bitswap-analysis.md`.

- Majority of IPFS content retrievals take **at least 4x as long as the equivalent HTTPS request** — the published "cost of decentralization."
- Root cause: Bitswap's discovery step **queries ALL connected neighbors** for wanted content ("broadcast WANT"), which both leaks interest and is the primary source of message amplification.
- Retrieval latency is markedly worse for **low-popularity content** specifically — the long tail degrades hardest, which is precisely the shape of a federated commons (many small, rarely-touched hexons/petals, not a few viral objects).
- Protocol Labs' own fix direction was **multi-path retrieval** (pull chunks from several peers concurrently) — the BitTorrent-style lever, not a Bitswap-protocol fix per se.

**Why this matters for fractalengine specifically:** iroh-blobs' content-addressing (BLAKE3 hash + `BlobTicket` naming a known provider) *structurally avoids* Bitswap's "ask everyone" broadcast-discovery amplification — but **only because discovery is pushed up to the application layer** (the hexon registry / relay, per the existing architecture) rather than solved by the P2P transport itself. This is a validation of the existing hexon-registry-as-federation-seam design, not a new problem — but it also means **the registry becomes the single load-bearing discovery path**, and its own availability/scaling properties (not researched in this stage — recommend as a stage 4 or follow-up item) directly determines whether fractalengine avoids IPFS's discovery-latency failure mode or reproduces it at the registry layer instead.

## 3. Relay costs and CGNAT/IPv6 reality

**Confidence: MEDIUM.** Sources: iroh relay docs, CGNAT/IPv6 trend pieces (APNIC-adjacent, jazenetworks, coronium.io), Cornell NAT64 study; raw: `raw/cgnat-ipv6-mobile-browser.md`.

- No published per-GB relay bandwidth cost was found for iroh's own hosted relay tiers (pricing page exists but wasn't itself quantified in fetched sources) — **this remains an open gap**, not closed by this research pass. Recommend: fractalengine should benchmark or directly price iroh Services relay tiers rather than assume a number.
- Aggregate IPv6 trend claims (2025-dated projections): global ~50-60%, **mobile networks 95%+**, major ISPs 70%+, enterprise 35-40%. Mobile carriers increasingly run IPv6-only cores with 464XLAT — meaning **mobile NAT reachability may now be structurally better than residential broadband CGNAT** in many markets, a mild positive surprise not anticipated in the April research.
- Where NAT64/464XLAT is in the path, a Cornell study found **23.13% longer routes and 17.47% higher RTT** than native IPv4 — a concrete, quantified latency tax for the (still common) case where translation is involved.
- Overall: don't assume IPv6-labeled networks are NAT-free in practice; deployment is uneven and often retains "IPv4-era" operational patterns even where IPv6 is nominally available.

## 4. CRDT performance at scale: Loro/Yjs/Automerge, Willow, and the churn gap

**Confidence: HIGH for library benchmarks; MEDIUM for churn/scale extrapolation.** Sources: automerge.org, loro.dev/docs/performance, PkgPulse 2026 guide, willowprotocol.org RBSR spec, ScienceDirect PS-CRDTs paper; raw: `raw/crdt-and-gossip-benchmarks.md`, `raw/crdt-churn-volatile-networks.md`.

- **Automerge 3.0 memory improvement is real and large**: a Moby-Dick-sized test document dropped from 700MB (v2.0) to 1.3MB (v3.0) in-memory — this is the single most dramatic number found in the CRDT space this stage, and directly de-risks "CRDT doc size explodes memory" as a fractalengine concern **if** the equivalent library/approach is used. (fractalengine uses iroh-docs, not Automerge, but the general finding — recent-generation CRDT engines have solved the naive memory-blowup problem via compressed runtime representations — is transferable context.)
- **Loro beats Yjs and Automerge in nearly every benchmark category** (2-5x smaller documents) except one-time WASM init cost (~50ms, 320kB bundle) — relevant if fractalengine ever embeds a JS/WASM CRDT engine (e.g., a web-based hexon viewer), less relevant to the native Rust iroh-docs path.
- **Naive large-doc replay is still memory-fragile even in modern engines**: a 10M-character replay benchmark requires being done in a single Automerge transaction or it exhausts memory — a cautionary note against naively replaying a hexon's full append-only op-log on every load without snapshotting/compaction.
- **Willow's RBSR gives an asymptotic guarantee (log-many rounds), not a measured wall-clock number** — and the spec itself states this guarantee is **conditional on the storage backend supporting efficient range-summarization, cardinality-based splitting, and residual enumeration without repeated scanning.** This is a direct, actionable constraint on fractalengine's storage choice: **redb (iroh-docs' backend) and SurrealKV's suitability for RBSR-style access patterns has NOT been benchmarked by anyone, in this research or otherwise.** This is a genuine unresolved risk, not just an unknown — it's a structural precondition of the reconciliation algorithm's efficiency that nobody has tested for these specific backends.
- **CRDT convergence-time-vs-peer-count remains under-studied in the literature generally** — multiple independent sources confirm convergence *time* (as opposed to correctness or traffic volume) is "usually overlooked" in CRDT papers.
- **The best available churn data tops out at 100 nodes**: PS-CRDTs paper (100-node model) found a 25% churn rate produces only ~4% traffic increase — a mild, non-alarming number, but it does NOT extend to the "thousands of peers" scale iroh-gossip vendor-claims to support. **The gap between 100 nodes (measured, mild churn cost) and 1,000s of nodes (only vendor-claimed as reachable) is real and unclosed.**

## 5. Gossip/pubsub scaling

**Confidence: MEDIUM (official PoC eval, not third-party) for gossipsub; LOW (vendor qualitative claim only) for iroh-gossip specifically.** Sources: Protocol Labs GossipSub v1.1 evaluation report, Logos/Vac research on scaling proposals, iroh-gossip docs; raw: `raw/crdt-and-gossip-benchmarks.md`.

- Published GossipSub v1.1 benchmark: **1000 nodes (100 publishers + 900 lurkers)**, average out-degree 6, full-network reach in 4 hops ideal-case; one cited scenario reached the entire 1000-node network in **350ms** for a 1024-byte message.
- iroh-gossip (HyParView membership + PlumTree broadcast) is vendor-characterized as scaling "to a few thousand peers" — **no specific number, no published benchmark was found**, purely a qualitative vendor claim.
- **Un-quantified compounding cost**: each additional gossip *topic* subscription increases connection count and local routing-table size — directly relevant since fractalengine's design implies one gossip topic per Verse (and potentially finer-grained per-Petal topics). **Nobody has published what N topics x M peers does to a single node's connection budget.** This is a genuine, currently-open scaling risk for any design that multiplies gossip topics per scope level.

## 6. Browser/WASM and mobile: hard architectural ceilings, not just performance numbers

**Confidence: HIGH for browser limitation (unambiguous official docs); LOW/unresolved for mobile battery.** Sources: docs.iroh.computer/deployment/wasm-browser-support, iroh "Iroh & the Web" blog, GitHub #2671/#2799; raw: `raw/cgnat-ipv6-mobile-browser.md`.

- **Browser iroh is relay-only, by construction, not by current limitation-to-be-fixed-later**: "Browser sandboxes don't support sending UDP packets to IP addresses from inside the browser" — this rules out direct P2P from any browser-hosted fractalengine client entirely under the current transport model. All browser-based peers pay 100% relay cost, always, with no partial-direct fallback.
- WebTransport reached cross-browser "Baseline" status as of ~March 2026 (Safari 26.4 was the last holdout) — the *precondition* for iroh to build a non-relay-only browser mode now exists industry-wide, but as of the fetched sources **iroh has not yet shipped this.** Track this actively; it changes the browser-peer cost model materially whenever it ships.
- **Mobile battery/background-execution data: not found.** No published measurement of iOS/Android background-execution limits killing long-running iroh/QUIC connections, nor real battery-drain numbers for a QUIC P2P stack backgrounded on mobile. **This unknown from p2p-mycelium §8 is NOT closed** — it remains the weakest-evidenced area of the whole research question.

## 7. Embedded DB under replication-like load

**Confidence: MEDIUM.** Sources: SurrealDB official benchmarks page, surrealkv README, redb GitHub/lib.rs; raw: `raw/surrealdb-benchmarks-page.md`, `raw/crdt-and-gossip-benchmarks.md`.

- SurrealDB's own published numbers (28 May 2026, Threadripper 9970X + 128GB RAM + 4TB NVMe, 15M records, 128 clients x 48 concurrent) show strong single-record CRUD throughput (~280-300k ops/s, beating Redis on create/update/delete) — but this is **top-tier dedicated workstation hardware**, not remotely representative of a typical P2P peer laptop/desktop, and **no RocksDB/LMDB-vs-SurrealKV backend comparison was published**, only SurrealDB-vs-other-databases.
- SurrealKV's architecture migrated from an all-in-memory VART index (explicitly "unsuitable for datasets larger than available RAM," with write amplification from "each update created new versions") to an LSM-tree with score-based leveled compaction — but **no quantified before/after write-amplification numbers exist anywhere published.**
- SurrealKV is still explicitly labeled **beta**, targeted at versioning/versioned-query use cases, not positioned as a production RocksDB/LMDB replacement; Windows support is "basic" with known thread-safety TODOs.
- redb (iroh-docs' backend): vendor micro-benchmark shows 1.6-3x faster writes than LMDB, 2-3x faster reads than RocksDB — **but explicitly on single-host, single-thread workloads**, with the vendor's own caveat that this is "not large-scale, write-heavy, concurrent scenarios." **fractalengine's actual workload (concurrent local writes + incoming P2P sync applies) is precisely the untested case.**

## Summary: p2p-mycelium §8 unknowns — closed vs still open

| # | Unknown | Status |
|---|---|---|
| 1 | iroh-docs throughput at >100 peers / >100k entries | **Still open.** No published benchmark found at this scale for iroh-docs specifically. Closest analog (redb micro-benchmarks) explicitly disclaims concurrent/scale scenarios. |
| 2 | SurrealDB schema evolution while replicating | Not addressed this stage (out of scope for external research; codebase-only question). |
| 3 | Peer's iroh-docs storage growth / GC story | **Still open**, but bounded: redb has no published GC-under-churn numbers; SurrealKV's own migration story (VART→LSM) shows the team is actively fighting an analogous problem in their own embedded store, suggesting this is an industry-wide unsolved edge, not a fractalengine-specific gap. |
| 4 | Private Petals within public Verses | Not addressed this stage (governance/trust-boundary question, deferred to stage 4). |
| 5 | Mobile P2P battle-testing | **Still open, essentially unresearched.** No battery/background-execution data found anywhere. Weakest-evidenced area in this whole report. |
| NEW | iroh NAT traversal real success rate | **Partially closed**, with an important contradiction surfaced: HIGH-confidence external data (70%, libp2p DCUtR, 4.4M attempts) vs LOW-confidence vendor claim (90-95%, iroh, no comparable independent study). Plan against the pessimistic end. |
| NEW | Willow/RBSR backend suitability (redb, SurrealKV) | **New unknown surfaced, not closed.** The spec itself makes efficiency conditional on backend range-summarization support; nobody has tested redb or SurrealKV against this requirement. |
| NEW | Gossip topic-count scaling (topics x peers) | **New unknown surfaced, not closed.** Directly relevant to per-Verse/per-Petal gossip topic design; no published data on connection-budget cost of N topics. |
