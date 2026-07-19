---
type: Decision Record
title: P2P Streaming Round — transfer granularity, scene-driven residency, registry parity, integrity, iroh timing (D-73…D-77)
timestamp: 2026-07-18T00:00:00Z
status: RATIFIED 2026-07-18 — user directive locked D-73…D-77 to their staged defaults ("no new primitive; leverage pointers to multiple hexons") and added D-78 (application settings area); see "## Ratification (2026-07-18)"
tags: [decision-record, p2p, streaming, hexon, iroh, render-budget, camera]
---

# Decision Record: P2P Asset Streaming (2026-07-18)

Verbatim user directive (2026-07-18): *"start on p2p hardening — maybe we can
create a decision round on how fine grained p2p can be and how we can handle
streaming asset data instead of forcing full downloads as I'll predict these can
be big. Ideally we can create an engine that allows us to stream in parts of
multiple hexons based on what's actually in the scene and work based on render
distance and entity caps."*

Source: six-agent exploration sweep 2026-07-18 (fe-sync/fe-network transfer
paths, hexon format internals, fe-terrain tile lifecycle, render-budget
guardrails, camera stack, iroh 0.35 capability research). Full memo retained in
track `p2p_asset_streaming_20260718`. Every option table cites repo evidence;
recommended defaults follow the house rule: staged, **not settled until the user
ratifies**.

## Ratification (2026-07-18)

**User directive (2026-07-18):** *"much better — no need to create a new
primitive if we can leverage pointers to multiple hexons instead."* Plus three
questions this section answers (P2P mechanics, renderer effect, limit
enforcement) and one addition: *"We may need a dedicated application settings
area as we approach this."*

**Locked:** D-73 = **A4**, D-74 = **residency ledger driving a pointer-set (no
materialized artifact)**, D-75 = **C1**, D-76 = **D2**, D-77 = **E2**. D-78
(application settings) added below.

### The "scene" is a pointer-set, not an object

The active scene is **not** a materialized "scene hexon." It is an **ephemeral
set of references** `{(hexon_uri, bundle_hash), …}` that the residency ledger
(D-74) recomputes every frame from render-distance rings + camera-forward
weighting + entity caps. Because every tile bundle under A4 is a
**content-addressed iroh blob** (its blake3 hash *is* its identity), the scene
is fully described by a list of hashes drawn across N hexons — no bytes are
copied, no artifact is persisted, and there is therefore **no cache-invalidation
or staleness problem** (the failure mode a materialized scene-hexon would have
introduced). If a user ever wants to *share* their exact working set, they share
the hash-list — an optional export, never a required primitive.

### (1) How pointers work peer-to-peer

- Each **source hexon** is published as a **HashSeq**: a small root manifest → an
  ordered list of tile-bundle hashes + the `ChunkIndex`. Resolving "hexon U, tile
  (z,x,y)" is two steps: fetch U's chunk-index once (the cheap **header phase**,
  already what `index_one` does), then request the one bundle blob by hash.
- The residency ledger drives **demand**: it emits the hash-set it needs this
  frame; `FetchStrategy` (semaphore, peer priority, sig verify — `fetch.rs:39-129`)
  pulls only the not-yet-resident hashes, over **iroh** (P2P) or the **registry
  member-granular HTTP route** (D-75) behind the one `PartialHexonFetch` trait.
- **Cross-hexon dedup is free:** a base-terrain bundle shared by two hexons has
  one hash, so it is fetched once and reused for both — the swarm-level win the
  pointer model unlocks. A peer already standing where you are can serve exactly
  the bundles you're missing, by hash.

### (2) How it affects the renderer

Today's path is **eager-everything** (`registry.load_all()` loads every installed
tileset; petal binding re-loads the active hexon → double residency; terrain
chunks spawn on a camera-height LOD ring; nodes/stamps spawn to hard caps gated
by the boolean `mesh_budget.exceeded`). Under the pointer model:

- The **residency ledger becomes the first system** in the VerseManagerPlugin
  chain and hands each spawner a **distance-ranked allowance** instead of the
  boolean gate. This is a **drop-in**: every spawner already takes a numeric
  budget (`spawn_allowance`, `take_stamp_budget`) and merely consumes
  `if exceeded {0} else {CAP}` today — we replace the constant with a ranked
  allowance derived from the (now-consumed) `render_distance`.
- **Tile bytes go lazy:** instead of `load_all()` + full `HexonTileSource`
  residency, tiles are fetched on demand through the existing-but-never-called
  async `fetch_tile` seam (`composite.rs:255-334`) as the ledger requests them,
  and evicted on exit-ring (the ×1.5 despawn-hysteresis pattern already exists).
- The renderer only ever sees what the ledger admitted, and spawns are gated
  **before** the spike (NFR-1: Bevy render buffers never shrink after one spiked
  frame — prevention, not recovery).

### (3) How limits are known and enforced (over-consumption prevention)

Two layers, kept distinct:

- **Hard backstops (already exist — keep as the safety net, never remove):** the
  2 M-instance `mesh_instance_watchdog` (warn + gate), `MAX_PETAL_NODES` 10 k,
  `MAX_STAMPS` 4096/track + 65 536/petal, terrain chunk cap 256. These are the
  "never crash" floor and stay as-is.
- **Soft budget (new — the ledger):** a **distance-ranked allowance** derived
  from a *configured* budget: `render_distance` (finally given a consumer),
  entity caps, and — phase 2 — a **closed-loop GPU-byte horizon**. "How do we
  *know*" is answered by promoting the **DIAG-15M census**
  (`fe-runtime/src/diag15m.rs`, today debug-log-only) to a **shared resource**:
  it already measures render-world instance/view/buffer bytes, so the ledger can
  read actual GPU pressure and shrink the radius as buffer bytes approach the
  2 GiB binding limit — the exact mechanism that would have prevented the 2.76 GB
  `create_bind_group` crash.

All three of `render_distance`, the entity caps, and the budget ceiling become
**user-tunable via D-78**.

## State of the world (evidence baseline, 2026-07-18)

- The only working large-asset transfer is a **whole-file HTTPS GET** of the
  entire `.hexon` (`fe-hexon/src/remote.rs:96-106`); the registry `/download`
  route has no Range support (`fe-hexon-registry/src/routes.rs:256-275`) and
  publish already caps at 1 GiB.
- P2P transfer is aspirational: iroh 0.35 gives a bound QUIC endpoint + BBR
  (`fe-sync/src/endpoint.rs:18-41`) and **outbound-only** gossip JSON broadcasts;
  **no ALPN/protocol router is registered anywhere**, iroh-docs is mock-backed,
  iroh-blobs is pinned but consumed by no crate, fe-network's libp2p swarm is
  Kademlia-only with debug-logged events.
- A complete **chunked tileset protocol already exists in types + UI wiring**
  (`RequestTilesetMeta`/`RequestChunk`/`ChunkReceived`,
  `fe-sync/src/messages.rs:52-57,128-140`; size-capped chunk packaging
  `fe-terrain/src/tiles/builder.rs:292-340`; sequential UI driver
  `fe-ui/src/terrain_map/events.rs:110-175`) — but the sync-thread handlers are
  stubs emitting `ChunkFailed` (`sync_thread.rs:727-759`). The round is
  therefore "ratify granularity + fill the stubs", not "invent streaming".
- Tile residency today is **eager-everything**: `registry.load_all()` reads every
  installed tileset fully into RAM at startup, and each petal assignment loads
  the active hexon **a second time** (`petal_binding.rs:151` vs
  `registry.rs:75-100`) — memory budgets are off by 2× until deduplicated.
- Render-side guardrails exist (MAX_STAMPS=4096/track, 65,536/petal,
  MAX_PETAL_NODES=10,000, app-wide Mesh3d watchdog at 2,000,000, DIAG-15M
  census) but the FR-6 "render horizon" (camera-radius materialization +
  hysteresis) is **specced with zero code**
  (`runtime_instance_guardrails_20260717/plan.md:39-43`).

---

## D-73 — P2P transfer granularity

| Option | Summary | Verdict drivers |
|---|---|---|
| A1 | Whole-hexon blob over iroh-blobs | Simplest, but forces 10 GB-class tilesets fully resident; defeats streaming; 0.35 can't range-split one blob across peers |
| A2 | Per-tile blobs via HashSeq collection | Works fully on iroh 0.35 today; per-tile dedup free; but per-child metadata cost and many tiles are <16 KiB (bundle-worthy) |
| A3 | One blob + bao ranged reads, PMTiles-style spatial layout | Fewest blobs; proven prior art; but 0.35 partial-blob resume degrades and the RangeSpec API goes private in 0.90+ — guaranteed rewrite |
| **A4 (staged default)** | **HashSeq of size-tuned tile BUNDLES (16 KiB–tens of MB), reusing the existing `package_chunked`/ChunkIndex pipeline** | Only option that (a) reuses the fully-wired chunk protocol, (b) works on 0.35 semantics today, (c) survives the 0.90+ API rewrite (hash + HashSeq semantics are stable). Adopt PMTiles-style spatial ordering **within** bundles so a later A3 move on 1.0 blobs is a layout change, not a redesign. Refine to A2 per-tile for hot zoom levels only if over-fetch proves painful. |

Prerequisite noted, not decided here: fe-hexon's p2p module currently targets
the never-built **libp2p** path while live P2P direction is iroh
(`fe-hexon/src/p2p/mod.rs:3-5`); D-71 (2026-07-17) ratified KEEP fe-network, so
the staged reading is: fe-network remains the swarm seam, but hexon transfer
rides **iroh** — flag if you want the opposite.

## D-74 — Streaming engine shape (scene-driven residency)

**Drivers:** B1 radial render-distance rings (ring math exists + tested,
`lod_ring.rs`; `PetalManifest.render_distance` default 500.0 already persisted
and user-editable but consumed by nothing) — plus B2 frustum/camera-forward
weighting (halves wasted prefetch; needs hysteresis so rotation doesn't thrash).
B3 closed-loop GPU-budget horizon (shrink radius as DIAG-15M buffer bytes
approach limits) is **phase 2**, gated on promoting the census from debug
scaffold to shared resource.

**Location (staged default):** a **residency ledger** resource written as the
first system in fe-ui's chained VerseManagerPlugin group — the single choke
point where every spawner already accepts a budget (`spawn_allowance`,
`take_stamp_budget` are pure + tested); it replaces the boolean
`mesh_budget.exceeded` gate with a distance-ranked allowance (extend
`MeshInstanceBudget`, not a parallel resource). Tile-**byte** streaming stays in
fe-terrain (lazy offset-index `HexonTileSource` + async dispatch of the
existing-but-never-called `fetch_tile` seam, `composite.rs:255-334`);
node/stamp residency lives in fe-ui. No new crate yet (premature given the
fe-ui↛fe-terrain boundary).

**Eviction:** despawn-on-exit-ring with hysteresis (the ×1.5 despawn-radius
pattern already exists, `terrain_plugin.rs:212-270`); count caps stay as the
safety net. **Hard constraint: prevention, not recovery** — Bevy render buffers
never shrink after one spiked frame, so the horizon must gate spawns *before*
the spike.

**Mandated prior art before design freeze:** archived specs
`render_distance_lod_20260407` (3-tier LOD, 1000 m hard limit) and
`relay_data_horizon_20260407` (data/sync half of the dual-interest model) — the
latter was not covered by any exploration report and is an open input.

## D-75 — Registry/HTTP parity (partial fetch over the hosted path)

| Option | Summary |
|---|---|
| **C1 (staged default)** | **Member-granular routes** (`GET /{uri}/chunk/{seq}`, `/{uri}/meta`) — 90% present: `read_zip_bytes` already serves single members lazily (`index.rs:32-43`) and an unused per-entry asset route exists (`routes.rs:210-254`); unify hosted + P2P behind one `PartialHexonFetch` trait so fe-hexon's transport-agnostic FetchStrategy (semaphore, peer priority, sig verify — `fetch.rs:39-129`) is the shared policy layer |
| C2 | HTTP Range on `/download` (tiles are Stored/uncompressed so byte-ranges work) — cheap later bonus for third-party tooling, but two partial-fetch abstractions if primary |
| C3 | No parity — hosted stays whole-file; rejected: the 1 GiB publish cap already strains and hosted users get none of the streaming benefit |

Client side regardless of option: split `HexonArchive::import` into a header
phase (manifest+entries+tileset_meta — exactly what `index_one` does) + on-demand
member reads; `TilesetMeta` is already fetchable without tile bytes
(`{id}.meta.json`), satisfying metadata-before-bytes.

## D-76 — Integrity granularity for partial fetch

Current gaps: Ed25519 covers the **manifest JSON only**, in two mutually
incompatible schemes (fe-format canonical-JSON vs fe-hexon raw-bytes
signature.txt); per-asset blake3 hashes exist in entries.json but are **never
verified** on install or publish; tiles are covered by neither; `ChunkReceived`
carries no per-chunk hash; the registry accepts unsigned packages.

| Option | Summary |
|---|---|
| D1 | Trust bao transport verification only — rejected as sole mechanism: bao proves bytes-match-hash, not hash-is-authorized (nothing binds chunk hashes to the publisher's signature), and does nothing for the HTTP path |
| **D2 (staged default)** | **Merkle-extend the signature root**: (1) per-chunk blake3 into ChunkIndex/TilesetMetaReceived; (2) signed manifest commits to entries.json digest + chunk-index digest; (3) enforce asset_hash at install AND registry publish (and verify signatures at publish). bao stays as transport-layer verification; the signed manifest becomes the authorization root over both HTTP and iroh |
| D3 | D2 + per-chunk signatures — overkill; a hash committed under one signed manifest is equivalent trust with far less key traffic |

Prerequisite: collapse the two format stacks — staged survivor is **fe-format
v1.0.0 canonical-JSON** with a compat verifier for legacy packages. The P2P
announce/fetch code is currently built on the LEGACY manifest
(`announce.rs:86`) and must migrate. Owned by `hexon_unification_20260716`;
D-76 adds the integrity requirements to that unification, it does not fork it.

## D-77 — iroh upgrade timing vs relay EOL 2026-12-31

Facts: 0.35 public relays hard-EOL 2026-12-31 (~5.5 months); relay wire protocol
broke in 0.91 so the whole stack jumps together; 0.90+ blobs is not yet labeled
production-quality; the repo's streaming surface is **greenfield** (iroh-blobs
wired to nothing), so there is no legacy transfer code to preserve. Today the
relay failure mode is **quiet**: default n0 relay binding, no relay_url config
wired, offline detection only checks bind failure — post-EOL, gossip queues
silently forever.

| Option | Summary |
|---|---|
| E1 | Block streaming on the 1.0 upgrade — stalls months on a not-yet-production dep; risks compound in one track |
| **E2 (staged default)** | **Build on 0.35 behind a transport trait**, designing only to hash+ranges+HashSeq semantics (stable across the break); keep RangeSpec types out of public seams; schedule the 1.0 jump as its own track (`iroh_1_0_upgrade` exists) with a hard internal deadline well before 2026-12-31, bumping iroh-quinn-proto in lockstep |
| E3 | Build on 0.35 raw — rejected: RangeSpec goes private in 0.90+, guaranteed rewrite of exactly the code that matters |

**Decision-independent hardening (no ratification needed, scheduled in the
track):** wire `RelayConfig` (Custom/Disabled) into `SyncEndpoint::new` and
surface relay health in `SyncStatus` so the EOL fails **loudly**; the docker
relay-container pattern already exists for self-hosting. (fractalengine-relay
is the app's headless server, NOT an iroh relay.)

## D-78 — Application settings surface (new, ratified 2026-07-18)

**User directive:** *"We may need a dedicated application settings area as we
approach this."* Confirmed by evidence: **no settings/preferences surface exists
anywhere** — no `AppSettings`/`AppConfig` resource, no egui settings window, no
persisted user config; the only "settings" dialog is per-entity
(`dialogs/entity_settings.rs`). The archived `render_distance_lod_20260407` track
fully **designed** an `AppSettings` (RON at `~/.config/fractalengine/settings.ron`,
fields `render_distance`/`camera_sensitivity`/`camera_zoom_speed` + a
`SettingsPanel` egui window) but it was **never built** — resurrect that design as
the home for the ledger's knobs.

**Staged default (ratified):** a new `AppSettings` Bevy `Resource` in fe-ui with
a RON/TOML persistence layer under the platform config dir, surfaced as a new
`ActiveDialog::Settings` egui window. It exposes: **render distance**,
**entity/mesh budget ceiling** (`MeshInstanceBudget.ceiling` is already a runtime
field — the cheapest first knob), **stamp caps**, **tile source mode**
(Offline/Online/Hybrid), **camera sensitivity/zoom/easing**, and **P2P
relay/peer config** (folds in D-77's `RelayConfig`). The hardcoded `const` limits
(`MAX_MESH_INSTANCES`, `MAX_PETAL_NODES`, `MAX_STAMPS_PER_PETAL`, terrain
`max_chunks`) are routed through the resource with the current constants as
defaults. `PetalManifest.render_distance` (per-petal, already persisted +
editable but consumed by nothing today) becomes the **per-petal override**;
`AppSettings.render_distance` is the **global default**.

**Ownership:** decision-independent of the transfer work; scheduled as its own
phase in `p2p_asset_streaming` (FR-7). Consumed by both the residency ledger
(this track) and `terrain_editor_overhaul_20260718` (camera + measurement prefs).

---

## Risks the round must keep in view

1. **Quiet relay death** post-EOL (highest urgency; hardening item above).
2. **Two P2P stacks, disjoint identity** — libp2p fresh keypair per start vs
   iroh deterministic NodeId from fe-identity; D-71 keeps fe-network, so the
   swarm-seam vs transfer-transport split must be explicit in D-73.
3. **Format-stack unification is on the critical path** of D-76 (owned by
   hexon_unification, not this round).
4. **Render buffers never shrink** — any streaming bug that over-spawns once is
   unrecoverable in-process; the Mesh3d watchdog is structurally blind to view
   fan-out (glb-embedded shadow lights could recreate the 2.76 GB crash today).
5. **Double residency** (2× active tileset in RAM) skews all memory math until
   `Arc<HexonTileSource>` dedup lands.
6. **Sync thread is current-thread tokio** — all new transfer I/O must
   spawn_blocking or it stalls gossip/replication.
7. **Chunk pipeline is scaffolding, not behavior** — `package_chunked` has no
   production callers; the download tracker has no cross-restart resume.
8. **Bundle sizing is theory** — 16 KiB–tens-of-MB economics come from iroh
   DESIGN.md, not from measured hexon tile-size distributions; histogram first.
9. **`get_tile_sync` main-thread latency** is tolerable only because bytes are
   pre-resident; lazy loading must prove it doesn't hitch frames or the async
   path becomes mandatory.
10. **Offline-provenance separation** — Offline mode deliberately skips the
    online-origin disk cache; a unified streaming cache must preserve this.
