# FractalEngine — Deep Research Report
**Asymmetric Research Squad + Crucible Synthesis**
Date: 2026-03-21 | Confidence: HIGH

---

## Verdict: Conditionally Sound Strategy

The vision is architecturally coherent but execution-fragile. The single-binary design corrects High Fidelity's fatal multi-process failure. The iroh convergence (4 independent sources agree) is the strongest signal. SurrealKV over RocksDB is clearly correct. The overlay WebView approach is pragmatic. Federation over global decentralization is right for v1.

**The danger:** This stacks five unproven integrations (Bevy + iroh + SurrealDB + wry + libp2p) with zero production precedent for the combination. The 4ms frame budget margin (12.7ms used / 16.67ms total) is the canary — any integration slower than modeled collapses the real-time guarantee.

**The missing critical work:** WebView security threat model, content moderation protocol, and GLTF-only upload declaration must exist BEFORE any code is written.

---

## Top 5 Convergences (High Confidence)

1. **iroh replaces raw libp2p as the data layer** — ANALOGICAL, ARCHAEOLOGICAL, CONTRARIAN, and JOURNALISTIC all independently arrive at iroh-blobs + iroh-gossip + iroh-docs over raw libp2p for the data stack. Raw libp2p has an async runtime conflict AND slowing release cadence.

2. **wry WebView MUST be an overlay window, not in-scene** — CONTRARIAN identifies the wgpu+wry GPU swap chain conflict as CRITICAL (Windows forbids two DXGI swap chains per HWND). SYSTEMS confirms V1 = overlay. JOURNALISTIC confirms bevy_wry is abandoned. No viable path to in-scene rendering today.

3. **SurrealDB backend = SurrealKV, not RocksDB** — CONTRARIAN: RocksDB compaction pauses (5–80ms) exceed the 16ms frame budget. JOURNALISTIC: SurrealKV is pure Rust, no C++ build deps. SYSTEMS: async bridge already adds 1-frame latency, leaving zero room for compaction stalls.

4. **Tokio isolation on dedicated OS threads is non-negotiable** — All four runtime consumers must be isolated: Bevy (smol) on T1, libp2p+iroh (Tokio) on T2, SurrealDB (Tokio) on T3. Cross-thread via typed mpsc channels only. Two Tokio runtimes calling into each other panics the process.

5. **did:key wraps existing ed25519 keypair trivially** — HISTORICAL: W3C did:key is exactly `did:key:z6Mk<multibase_pub>`. FUTURIST: Add `sub: did:key:<pub>` to JWT now. The v1→v2 DID upgrade is a single wrapping operation with zero architectural change.

---

## Top 5 Conflicts (Require Resolution)

1. **iroh still uses Tokio** — iroh is built on iroh-net which uses Tokio internally. The "iroh fixes the runtime conflict" assumption may be wrong — the thread isolation architecture (T2) is still mandatory regardless of library choice.

2. **Federation vs DHT discovery** — HISTORICAL validates federation. NEGATIVE SPACE flags DHT ≠ search. These are architecturally opposed discovery models. Resolution required: define a Petal Metadata Schema + Discovery Index protocol.

3. **Content-addressed assets vs upload conversion** — GLTF-only upload means BLAKE3 hash is of the final GLB. If conversion happens, the hash must be of the converted output. Pipeline ordering must be specified.

4. **TeaTime stateless relay vs iroh-docs persistent store** — ARCHAEOLOGICAL recommends both without specifying which handles authoritative world state vs ephemeral event relay. Resolution: iroh-docs = op-log transport; SurrealDB = local materialization; iroh relay = stateless event timestamper.

5. **WASM client by Q4 2026 vs unportable stack** — SurrealDB has no WASM target. iroh has no WASM target. WASM is single-threaded. The timeline is asserted without feasibility validation. Flag as aspirational, not committed.

---

## Critical Pre-Work (Before Any Code)

1. **Thread topology prototype** — Prove the three-thread architecture (Bevy + iroh/libp2p + SurrealDB) doesn't panic or stall under synthetic load. This is THE foundational decision.
2. **WebView Threat Model document** — Cover SSRF, CSP, localhost blocking, JS bridge lockdown, credential harvesting. No WebView code without this.
3. **GLTF-only upload declaration** — No FBX support. No conversion. Document the Blender export workflow. One spec decision eliminates a whole class of complexity.
4. **Content moderation protocol** — Hash-blocklist hook at asset ingest, signed report packets via gossip. Legal prerequisite, not optional.
5. **wry overlay prototype** — Standalone Bevy + wry test proving per-frame overlay repositioning works on all three platforms before committing to this architecture.

---

## Definitive Technology Selections

| Component | Choice | Why |
|---|---|---|
| P2P data | iroh-blobs + iroh-gossip + iroh-docs | 4-source convergence; higher-level than raw libp2p |
| P2P discovery | libp2p Kademlia DHT | De facto standard; iroh uses it internally too |
| Database backend | SurrealDB 3.0 + SurrealKV | Pure Rust; no C++ deps; no GC pauses |
| Networking (transport) | bevy_quinnet (QUIC/ICE) | Best NAT traversal; QUIC for P2P |
| Networking (replication) | lightyear 0.26 | If high-level replication needed |
| In-game UI | bevy_egui 0.39 | Only mature option; Bevy 0.18 compatible |
| WebView | wry 0.54 (custom thin plugin) | bevy_wry is abandoned; build own wrapper |
| Identity | ed25519-dalek 2.2 + JWT `sub: did:key` | Trivial did:key upgrade path |
| Asset format | GLTF/GLB only | Bevy-native; embed textures (avoid bug #18267) |
| Asset distribution | iroh-blobs (BLAKE3 content-addressed) | Verified P2P distribution |
| Sync primitive | Lamport clocks + iroh-docs reconciliation | Leaderless, offline-capable |
| Auth persistence | OS keychain (keyring crate) | Never expose raw key material |
| RBAC enforcement | SurrealDB record-level DEFINE TABLE PERMISSIONS | Unforgeable from clients |

---

## Architectural Decisions That Age Well (2026–2028)

- Ed25519 keypair + JWT with `sub: did:key` — EU eIDAS wallets arrive Q4 2026; zero-cost upgrade
- GLTF/GLB with BLAKE3 content addressing — Gaussian splat exports will use GLTF extensions by 2027
- libp2p Kademlia DHT — WebTransport adds WASM peers additively
- SurrealDB embedded — CRDT offline sync ships ~Q2 2027; iroh-docs provides sync in the interim
- ECS architecture — XR activation = plugin add, no coordinate refactor needed

## Architectural Decisions That Become Debt

- wry overlay approach → Bevy native compositor ~2027 — Abstract behind `BrowserSurface` trait now
- Raw JSON scenes in SurrealDB → BSN scene format in Bevy 0.19 — Add `SceneSerializer` trait
- Hard Tokio runtime coupling → Bevy task executor changes in 0.19-0.20 — Use `AsyncComputeTaskPool`
- GLTF-only → Gaussian splatting mainstream 2027 — Design `AssetIngester` trait now

---

## Blind Spots Register (Summary)

| ID | Blind Spot | Risk | Action |
|---|---|---|---|
| BS-01 | Asset format conversion (FBX → GLTF) | HIGH | Declare GLTF-only; document Blender workflow |
| BS-02 | Content moderation without central authority | CRITICAL | Hash-blocklist at ingest; operator legal doc |
| BS-03 | Discovery UX (DHT ≠ search) | HIGH | Define Petal Metadata Schema + Discovery Index |
| BS-04 | Petal persistence across Node downtime | HIGH | Define persistence tiers: EPHEMERAL/CACHED/REPLICATED |
| BS-05 | Version conflict resolution | MEDIUM | Merkle root version hash in DHT; staleness handshake |
| BS-06 | Bandwidth economics (free rider problem) | HIGH | P2P swarming for assets; max_concurrent_visitors cap |
| BS-07 | DID key recovery | HIGH | OS keychain + social recovery (M-of-N) v2; guest mode v1 |
| BS-08 | Accessibility in 3D | MEDIUM | WCAG 2.1 AA for 2D UI; text layer per Model |
| BS-09 | WebView security (SSRF, XSS, phishing) | CRITICAL | Threat model doc; localhost block; typed IPC only |

---

## Recommended Track Architecture (10 Tracks)

1. **Seed Runtime** — Thread topology and channel skeleton (no deps)
2. **Root Identity** — Ed25519 keypair, OS keychain, JWT + did:key (deps: 1)
3. **Petal Soil** — SurrealDB schema, RBAC permissions, op-log (deps: 1, 2)
4. **Mycelium Network** — libp2p DHT + iroh data transport (deps: 1, 2)
5. **Bloom Renderer** — GLTF pipeline, content addressing, dead-reckoning (deps: 3, 4)
6. **Petal Gate** — Auth handshake, session cache, role enforcement (deps: 2, 3, 4)
7. **Canopy View** — wry WebView overlay + BrowserInteraction tabs (deps: 1, 5, 6)
8. **Fractal Mesh** — Multi-Node sync, Petal replication, offline cache (deps: 3, 4, 5)
9. **Gardener Console** — Node operator admin UI (deps: 3, 5, 6, 7)
10. **Thorns and Shields** — Security hardening + pre-launch documents (deps: 4, 5, 6, 7)

---

## Key Safety Rules (Recurring from Research)

1. `SURREAL_SYNC_DATA=true` — mandatory for crash safety (not default)
2. Never call `block_on()` from a Bevy system — always async task bridge
3. Never call `tokio::runtime::Handle::try_current()` from Bevy systems — runtimes must be isolated
4. BLAKE3 hash the final GLB bytes (not the source FBX) as the canonical asset ID
5. RBAC enforcement ONLY in SurrealDB permissions — never in Bevy system logic
6. WebView URL denylist: block all localhost + RFC1918 ranges unconditionally
7. All gossip payloads signed by sender's ed25519 key; unsigned messages rejected
8. JWT lifetime ≤ role cache TTL (recommended: JWT=300s, cache TTL=60s with re-validation)
9. Use `verify_strict()` not `verify()` from ed25519-dalek to prevent weak-key forgery
10. Keep Bevy pinned — budget 1-2 engineer-weeks per quarterly Bevy upgrade cycle
