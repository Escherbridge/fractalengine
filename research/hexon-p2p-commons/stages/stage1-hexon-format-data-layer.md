---
type: research-findings
stage: 1
date: 2026-07-11
---

# Stage 1 Findings: Hexon Format & Data Layer as Digital Twin Substrate

## 0. Summary of the biggest surprise

The delta-hexon spec (`conductor/tracks/hexon_delta_format_20260710/spec.md`) asserts as
**verified ground truth**: *"Per-op ed25519 signature: Exists (`OpLogEntry.sig`)"*. This is
**false in practice** — confirmed HIGH confidence. Every call site across the workspace that
constructs an `OpLogEntry` hardcodes `sig: "00".repeat(64)` (64 zero bytes, hex-encoded) — a
placeholder, not a real signature:

- `fe-database/src/handlers/transform.rs:44`
- `fe-database/src/handlers/entity_property.rs:52,131`
- `fe-database/src/queries.rs:20,56,91`
- `fe-database/src/space_manager.rs:48,84,174,234`
- `fe-database/src/role_manager.rs:111` (`// placeholder signature`), `:147`
- `fe-auth/src/revocation.rs:18`

Eleven of eleven construction sites use the same stub value; zero use a real `SigningKey`. The
`sig: String` field on `OpLogEntry` (`fe-database/src/types.rs:39`) exists structurally, and
`write_op_log()` (`fe-database/src/op_log.rs:112-122`) populates `lamport_clock`/`hlc_timestamp`
via the HLC — but it never touches `sig`, and no caller ever signs. **"Sovereign authorship"
(each op cryptographically attributable to its originating node) does not exist today.** This
is the load-bearing assumption for the entire hexon-delta P2P vision (§ "Signature chain" in the
spec) and it needs to be built, not "packaged."

## 1. fe-hexon: format, signing, verification cost, streaming

### 1.1 On-disk format: plain ZIP, not content-addressed at the archive level

**Confidence: HIGH.** Two independent, parallel hexon implementations exist in this workspace,
using different ZIP crate majors and incompatible signature schemes:

| | `fe-format` (Phase 6.5, `.hexon` scene/terrain archives) | `fe-hexon` (Phase 8, `.fecrate` publisher packages) |
|---|---|---|
| ZIP crate | `zip = "2"` (`fe-format/Cargo.toml:10`) | `zip = "0.6"` (`fe-hexon/Cargo.toml:16`) |
| Container | `manifest.json`, `entries.json`, `license.json`, `entities/*.json`, `assets/{hash}`, `terrain/*` — plain ZIP, Deflate for JSON, **Stored (uncompressed)** for tile images (`archive.rs:209-213`, already-compressed PNG/JPG re-deflating is wasted CPU, correctly avoided) | Same shape + `signature.txt`, `icon.png`, `preview/*`, `README.md` |
| Signature scope | Canonical JSON (sorted keys recursively, `signature` field zeroed) then signed (`fe-format/src/signature.rs:59-71`) | **Raw manifest JSON bytes signed directly**, no canonicalization (`fe-hexon/src/signature.rs:9-15`) |
| Hash | `blake3::hash` per-asset only (`asset_hash`); no whole-archive content address | Same — `asset_hash`/`manifest_hash` via blake3 (`fe-hexon/src/signature.rs:40-48`) |

**Implication:** the archive itself is not content-addressed — only *entries inside it* are
(by `asset_hash`). Two hexons with identical assets but different `created_at`/`updated_at`
timestamps in the manifest produce entirely different ZIP bytes and no dedup benefit at the
archive level. This matters for the delta-hexon vision: if delta hexons are also whole-ZIP
archives, checkpoint compaction (spec §"Time-travel checkpoints") gets no free dedup from
re-exporting overlapping ranges.

The **two incompatible signing schemes** (canonical-JSON vs raw-bytes) are a real fragmentation
risk if delta hexons are meant to reuse "existing registry/distribution machinery... unchanged"
(spec, "Content-addressed P2P distribution" section) — unchanged *which* one? `fe-hexon`'s
`p2p/fetch.rs:105-129` (`verify_fetched_manifest`) explicitly signs/verifies over raw bytes and
even comments on the ambiguity: *"the current signature scheme signs the full manifest JSON as
provided during publishing"* (fetch.rs:113-115) — i.e., the implementer was aware two schemes
exist and chose raw-bytes for P2P fetch, diverging from `fe-format`'s canonical-JSON scheme used
by `TilesetBuilder` (`fe-terrain/src/tiles/builder.rs:176`, which calls `fe_format::sign_manifest`).

### 1.2 Verification cost profile: whole-archive, no streaming

**Confidence: HIGH.** Both `HexonArchive::import` (`fe-format/src/archive.rs:255-458`) and
`HexonPackage::open` (`fe-hexon/src/package.rs:120-239`) fully deserialize `manifest.json`,
`entries.json`, `license.json` and then iterate **every** archive entry into `Vec<u8>` /
`HashMap<String, Vec<u8>>` in memory before returning. There is no lazy-entry API, no partial
read, no "verify manifest only, defer asset bytes" mode.

Concretely: `HexonPackage::open` reads `manifest_raw` bytes, verifies the ed25519 signature
against those exact bytes (`package.rs:126-134, 222-223`), **then unconditionally proceeds to
read every asset/preview/icon file into memory regardless of whether the signature validated**
— `signature_verified` is stored on the returned struct but the read loop for assets/previews
(`package.rs:172-220`) is not gated on it. A malformed-but-decompressible ZIP with a bad
signature still gets fully unzipped into memory before the caller can reject it. The rejection
only happens later, at `HexonRegistry::install()` (`fe-hexon/src/registry.rs:127-130`), which
does check `package.signature_verified` before storing blobs — so the *installation* is
signature-gated, but the *decompression/decoding cost* is paid regardless.

**Mechanism:** ZIP central directory requires the reader to have the full byte range (or at
minimum the end-of-central-directory record + full data for each `by_name` lookup);
`zip::ZipArchive::new(Cursor::new(bytes))` (`package.rs:122`, `archive.rs:257`) takes an
in-memory `Cursor` over the **entire** already-downloaded byte buffer. There is no
`AsyncRead`/streaming ZIP reader in either crate — **a hexon must be fully downloaded before
any of its content (including the manifest) can be parsed**, because the ZIP central directory
that indexes `manifest.json`'s offset lives at the **end** of the file. This is a fundamental
ZIP-format constraint, not a code oversight, but it directly blocks any "verify + stream
first N assets while the rest downloads" pattern — a real limitation for large terrain-tileset
hexons (§3).

**Candidate mitigation:** either (a) place the manifest + entries index as the **first** entry
and use a streaming ZIP reader that can start emitting entries before EOCD is reached (fragile,
most zip crates don't support this safely), or (b) abandon ZIP-as-container for large hexons in
favor of `iroh-blobs`' native `HashSeq` (already used in prior P2P research, `p2p-mycelium`
findings §2) as the manifest, with individual assets as separately-addressable blobs fetched
on demand — this is effectively what `research/p2p-mycelium/findings.md` already recommended
for the DB-row case ("hash-reference DB, blobs in iroh-blobs store") but has not been applied
to hexon *archives* themselves.

### 1.3 Registry install flow: blob-level dedup, DB write, handler dispatch

**Confidence: HIGH.** `HexonRegistry::install()` (`fe-hexon/src/registry.rs:120-191`):
1. Trust `package.signature_verified` (no re-verification — re-verification is explicitly
   avoided because re-serializing would diverge from the raw bytes actually signed,
   `registry.rs:125-126` comment — a correct call given the raw-bytes signing scheme, but it
   means the registry *must* trust whatever `HexonPackage::open` computed upstream).
2. Per-entry blob store via `FsBlobStore::store()` (`registry.rs:50-60`) — content-addressed by
   `asset_hash`, skips write if `path.exists()` (blake3-collision-free dedup, correctly cheap:
   one `Path::exists()` syscall per asset, no re-hash-and-compare).
3. DB persistence via `crossbeam::channel` `DbCommand` (fire-and-forget, non-blocking).
4. Type-specific post-install handlers (`fe-hexon/src/handlers/*.rs` — terrain, model, material,
   skybox, sound, gpx_collection) run synchronously in the install call, and failures are only
   logged (`registry.rs:181-186`), not propagated — a hexon can "install successfully" while its
   type-specific side effects (e.g., terrain auto-config) silently no-op. This matches the known
   Phase 8.4 review gap already in MEMORY.md ("terrain crate auto-config not wired").

## 2. fe-format: chunking, compression, versioning

**Confidence: HIGH** (all directly observed in `fe-format/src/*.rs`).

- **Chunking:** exists only at the *tileset* level (`ChunkIndex` in `manifest.rs:72-87`,
  produced by `TilesetBuilder::package_chunked`, `fe-terrain/src/tiles/builder.rs:200-300`),
  splitting by a `max_chunk_size_mb` byte budget over the flat tile list — not by spatial
  locality within a chunk beyond whatever order `enumerate_coords()` produces (row-major over
  zoom levels, `builder.rs:73-88`). No chunking primitive exists for scene-type or delta hexons.
- **Compression:** Deflate (via `zip` crate) for JSON manifests/entries; **Stored (no
  compression)** for elevation/satellite tile images since they're pre-compressed PNG/JPG
  (`archive.rs:209-213` — correct engineering choice, avoids wasted CPU re-deflating opaque
  bytes). No zstd anywhere in `fe-format` or `fe-hexon` — **the delta-format spec's claim that
  "chacha20poly1305 is already a key dep for paid hexons" and that "zstd would be a new,
  uncontroversial addition"** is aspirational; grep of both `Cargo.toml`s shows neither zstd nor
  chacha20poly1305 as a dependency today (only `zip`, `blake3`, `ed25519-dalek`, `bs58`). The
  "paid hexon encryption" story referenced by the spec is not yet implemented in `fe-format` or
  `fe-hexon` (License types exist — `LicenseType::Paid` — but no encrypted-blob code path was
  found).
- **Versioning/schema evolution:** `HexonManifest.schema_version: String` is a free-text field
  (`"1.0.0"`), not enforced by any migration/compat-check code in `fe-format`. `serde(default)`
  is used liberally on manifest fields (e.g., `tags`, `dependencies`, `platforms` —
  `manifest.rs:130,142,144`), which gives additive-field forward compatibility for free via
  serde, but there is **no explicit version-gate or migration function** — an old reader given a
  `schema_version: "2.0.0"` archive with a structurally incompatible field would either silently
  ignore unknown fields (if using `#[serde(default)]`/permissive deserialization) or hard-fail
  with a generic serde error, not a clear "unsupported version" message. **Confidence: MEDIUM**
  on the failure-mode characterization (didn't find an explicit test exercising this).

## 3. fe-terrain: tile packing, LOD, mesh cost, scale story

**Confidence: HIGH** for load path; **MEDIUM** for at-scale extrapolation (no benchmark found).

- **`HexonTileSource::from_archive`** (`fe-terrain/src/tiles/hexon_source.rs:30-44`) loads
  **all** elevation + satellite tiles into two `HashMap<String, Vec<u8>>`s, fully in memory,
  synchronously, at load time. There is no lazy/on-demand per-tile decompression from the
  archive — the whole tileset's tile bytes live in RAM for the process lifetime once loaded
  (`HexonStore::load_tileset`, `store.rs:207-214`, calls this directly). For a
  continent/US-region hexon (per MEMORY.md: gis-tile-etl builds "real US-region hexons"), tile
  count at even modest zoom ranges (e.g., z5-z14 over a large bbox) can run into the tens of
  thousands of 256×256 PNGs — each held as raw compressed bytes (not decoded pixels, so this is
  bounded by file size, not decoded-bitmap size, which is a mitigating factor) but still an
  unbounded process-lifetime RAM commitment with **no eviction** once a `HexonTileSource` is
  constructed. Contrast with `DiskTileCache` (used by `CompositeTileSource`, `composite.rs:19`)
  which is presumably bounded/on-disk — but hexon-sourced tiles bypass that path entirely
  (`covers()` check first, `composite.rs:73,93,120,133,163,202`, hexon sources take priority and
  are never evicted).
- **LOD:** `TileLodManager::compute_tile_requests` (`fe-terrain/src/tiles/lod.rs:29-70`) is a
  **recursive quadtree subdivision** driven by a linear distance→zoom mapping
  (`subdivide()`, `lod.rs:72-114`) — reasonable in shape, but `haversine_deg` (`lod.rs:118-122`)
  is actually **not** haversine (no trig, no great-circle correction) — it's flat-earth
  Euclidean distance in degree-space (`(dlat²+dlon²).sqrt()`), which under-weights east-west
  distance in high latitudes (a degree of longitude shrinks toward the poles) — a real accuracy
  bug in LOD tile selection for temperate/high-latitude regions, though it only affects
  *sort order/prioritization*, not correctness of what eventually loads. **Confidence: HIGH**
  that this is not true haversine; **MEDIUM** on real-world visual impact magnitude.
- **Mesh generation:** `terrain_mesh()` (`fe-terrain/src/mesh/terrain.rs:12-90`) is O(w·h) for
  vertices/UVs and O(w·h) for the normal-averaging pass, with **unweighted** face-normal
  accumulation (each shared vertex just sums face normals equally regardless of triangle area,
  `terrain.rs:64-78`) — correct topologically but a minor shading-quality tradeoff vs
  area-weighted normals; irrelevant for perf. No indexed LOD mesh simplification found wired
  into this path (there's a separate `fe-terrain/src/simplify.rs` at the crate root — not read
  in depth this stage, flagged for stage 2 if mesh-simplification-under-churn matters for the
  digital-twin story).
- **`CompositeTileSource`** (`fe-terrain/src/tiles/composite.rs`) fallback order is
  hexon→disk-cache→online, correctly offline-first, but note: `get_tile_sync` on `Offline` mode
  **skips the disk cache entirely** ("may contain online tiles" — `composite.rs:71`) — meaning
  a peer with no matching hexon source but tiles previously fetched-and-cached from that exact
  region will still show as unavailable in strict offline mode. This is intentional (comment
  documents the rationale) but worth flagging as an availability tradeoff for churn/partition
  scenarios per the research objective's §2 "availability under churn."

## 4. fe-database / fe-entity-store: op_log, write amplification, geometry

### 4.1 HLC + lamport clock: real and correct

**Confidence: HIGH.** `fe-database/src/op_log.rs:29-149` implements a genuine hybrid logical
clock: 48-bit wall-ms + 16-bit counter packed into `u64`, monotonic across restarts by
initializing from `max_persisted` lamport value (`init_hlc`, `op_log.rs:45-67`), with
counter-overflow handling that advances `wall_ms` rather than wrapping (`next_hlc_timestamp`,
`op_log.rs:88-100`, exercised by `counter_overflow_advances_wall` test). This is solid,
well-tested infrastructure. The gap is entirely in `sig` (§0), not in HLC correctness.

### 4.2 write_op_log is used by exactly 6 modules, all handlers, not universal

**Confidence: HIGH.** Callers: `fe-auth/src/revocation.rs`,
`fe-database/src/handlers/{transform,entity_property}.rs`, `fe-database/src/queries.rs`,
`fe-database/src/space_manager.rs`, `fe-database/src/role_manager.rs`. This is **not** "every
mutation already goes through `write_op_log()`" as the delta-format spec's "ground truth"
section states — it's specific handlers that were retrofitted to also emit an op_log row
alongside their primary write. Structural/RBAC/space mutations and transform updates are
covered; there is no evidence (this stage) that every raw `UPDATE`/`CREATE` path in
`fe-database` goes through this — e.g., generic property batch operations, seed data
(`handlers/seed.rs`), or the query-builder path (`fe-query/src/builder/`) were not confirmed to
emit op_log entries. **This matters directly for the delta-hexon vision**: if the delta hexon
is "just" an export of `op_log`, any write path that bypasses `write_op_log` is **invisible to
replay** — a silent gap between live state and what a delta hexon can reconstruct.

### 4.3 write amplification per transform update

**Confidence: HIGH**, mechanism traced end-to-end. A single node transform edit
(`update_node_transform_handler`, `fe-database/src/handlers/transform.rs:17-49`) does:
1. A `SELECT` to fetch old transform (`query_current_transform`, awaited before the write).
2. A `write_op_log` call → `Repo::<OpLog>::create_raw` → one SurrealDB `CREATE` on the
   append-only `op_log` table (own row, own HLC tick).
3. The actual `UPDATE node SET position=..., elevation=..., rotation=..., scale=...,
   edit_seq=edit_seq+1` (`transform.rs:54-60`) with an explicit `<geometry<point>>` cast
   (documented gotcha per `AGENTS.md §geometry-inserts` and MEMORY.md's Genesis-seed fix).

So **every transform edit is 1 read + 2 writes** (op_log insert + node update), not 1 write.
For high-frequency IoT/sensor position streams (the exact "digital twin" workload this stage
was asked to assess), this is a 3x-operation multiplier per update, all synchronous within the
DB thread's single connection, before considering SurrealKV's own append-only segment growth
underneath. The `write_op_log` failure is explicitly best-effort/non-fatal
(`transform.rs:46-49`, "Don't fail the transform — op_log is best-effort for now") — a
reasonable degradation choice, but it means the op_log (and therefore any future delta hexon
built from it) can silently under-represent the true edit history under DB pressure.

### 4.4 EntityStore in-memory cache: O(n) clone-on-write per update (real bottleneck)

**Confidence: HIGH**, and this is the most concrete perf bug found this stage, separate from the
signature issue. `fe-entity-store/src/lib.rs`:
- `EntitySnapshot.node_log: Vec<NodeLogEntry>` (`lib.rs:19-26`) is an **unbounded, append-only,
  per-node** log held **inside** the snapshot struct that lives in the hot `papaya::HashMap`
  cache.
- `EntityStore::upsert()` (`lib.rs:152-167`) takes a full `EntitySnapshot` by value and
  `insert()`s it — replacing the whole entry.
- Every mutation path — `append_log` (`lib.rs:197-209`) and `apply_scene_change`'s
  `NodeTransform` arm (`lib.rs:260-284`) — calls `self.get(node_id)` first, which **clones the
  entire snapshot including the full `node_log` Vec** (`EntityStore::get`, `lib.rs:134-137`,
  `guard.get(node_id).cloned()`), mutates the clone by pushing one new `NodeLogEntry`, then
  `upsert()`s the whole thing back.
- **Mechanism:** for a node with N prior log entries, a single new transform update is O(N) —
  clone N entries, push the (N+1)th, reinsert all N+1. For a digital-twin node under continuous
  IoT position streaming (the stage's explicit focus area), N grows without bound (no
  compaction/truncation of `node_log` was found anywhere in this file), so update cost grows
  **linearly and unboundedly over the node's lifetime** — a classic append-only-log-without-
  compaction anti-pattern, and it lives in the *hot path* in-memory cache, not just the durable
  op_log. This is strictly worse than the DB-side write amplification (§4.3) because it's
  O(N) per update rather than O(1).
- **Candidate mitigation:** cap `node_log` length in the hot cache (ring buffer / keep-last-K)
  and treat the *durable* op_log (SurrealDB) as the source of truth for full history — the hot
  cache only needs recent entries for conflict/undo purposes, not the entire lifetime log. This
  directly serves the "high-frequency property updates" concern in the research objective §6.

### 4.5 Time-travel query: works, but whole-record VERSION scan

**Confidence: MEDIUM.** `query_petal_at_time` (`op_log.rs:124-137`) uses SurrealDB's native
`SELECT * FROM $record VERSION d'{ts}'` — this delegates entirely to SurrealKV's versioned
storage, so the *query* mechanism is real (not a spec-only claim), but no benchmark or query
plan was inspected this stage to characterize its cost at scale (SurrealKV's append-only segment
scan behavior for `VERSION` queries is unknown — flagged for stage 2/3 if external SurrealDB
docs cover this).

## 5. The hexon delta vision vs. what exists

Restating the delta-format spec's "what exists" table against this stage's findings, only where
they diverge from the spec's claims:

| Spec's claim | This stage's finding |
|---|---|
| "Per-op ed25519 signature: Exists (`OpLogEntry.sig`)" | **False** — field exists, always set to a 64-zero-byte placeholder at all 11 call sites (§0). |
| "Every mutation already goes through `write_op_log()`" | **Partially false** — 6 specific handler modules call it; no evidence of universal coverage (§4.2). |
| "`chacha20poly1305` is already a key dep for paid hexons" | **Not found** — absent from both `fe-hexon/Cargo.toml` and `fe-format/Cargo.toml` (§2). |
| "Reuse `fe-hexon`'s existing registry/distribution machinery... unchanged" | Two incompatible signature schemes already coexist between `fe-format` and `fe-hexon` (§1.1) — "unchanged" needs to specify *which* scheme a delta hexon adopts. |
| Content addressing (blake3) — "exists, reuse unchanged" | True, but only at the *asset* level, not the *archive* level (§1.1) — a delta hexon that's a whole ZIP gets no dedup benefit from re-exported overlapping ranges. |

## 6. Performance issues (ranked, with mechanism)

1. **EntityStore hot-cache O(N) clone-on-write per node update** (§4.4) — unbounded `node_log`
   growth inside the in-memory snapshot, cloned in full on every mutation. Directly blocks
   high-frequency IoT/sensor digital-twin workloads from being cheap. **HIGH confidence.**
2. **3x-operation write amplification per transform edit** (1 SELECT + 2 writes) (§4.3), with
   op_log writes best-effort/non-fatal, meaning replay-completeness is not guaranteed under DB
   load. **HIGH confidence.**
3. **Whole-archive, whole-tileset in-memory loading with no streaming or eviction** (§1.2, §3) —
   both hexon verification (must fully unzip before signature check gates blob storage) and
   `HexonTileSource` (all tiles resident in RAM, no lazy load, no eviction) scale linearly with
   archive/tileset size and don't degrade gracefully for continent-scale terrain hexons.
   **HIGH confidence** on mechanism, **MEDIUM** on real-world magnitude (no benchmark run).
4. **No real per-op signing** (§0) means any "sovereign authorship" or tamper-evidence claim for
   the current op_log is currently vacuous — this is a security/trust gap as much as a
   performance one, but it blocks the entire delta-hexon signature-chain proposal from having
   real grounding today. **HIGH confidence.**

## 7. Format-level limitations (ranked)

1. **ZIP-as-container requires full download before any content (including the manifest) is
   parseable**, because the central directory is at EOF (§1.2). This is a structural constraint
   of the format choice, not fixable by better code within the current container — it blocks
   partial-read/streaming hexon consumption, which matters for the P2P distribution story
   (large tilesets, delta-hexon ranges under churn).
2. **No archive-level content addressing** and **two incompatible manifest signature schemes**
   coexisting in the workspace (§1.1) — a real fragmentation risk if the delta-hexon format is
   meant to compose with both existing paths "unchanged."

## 8. Candidate mitigations (ranked)

1. **Cap/ring-buffer the in-memory `node_log`** in `fe-entity-store`, decoupling hot-cache
   recency from durable full-history (which the SurrealDB op_log/VERSION mechanism already
   handles) — directly fixes the O(N) clone bottleneck (§4.4) with a small, local, low-risk
   change.
2. **Adopt `iroh-blobs`' `HashSeq`/manifest-of-hashes pattern instead of ZIP** for any
   *new* large/streamable hexon type (tilesets, delta hexons) — keeps ZIP for small
   scene/model hexons where whole-archive semantics are fine, but lets large or
   incrementally-growing hexons (terrain tilesets, delta ranges) be fetched/verified
   per-blob rather than requiring the whole container up front. This directly answers the
   research objective's "can a hexon be partially read/streamed" question: **not today, and
   not fixable within ZIP** — it needs a different container for the types where it matters.

## Contradictions and surprises documented verbatim

- Delta-format spec: *"Per-op ed25519 signature | Exists (`OpLogEntry.sig`)"* vs. code:
  `sig: "00".repeat(64)` at every construction site, including one explicitly commented
  `// placeholder signature` (`role_manager.rs:111`) — the spec's own citation
  (`fe-database/src/op_log.rs`) does not itself construct or sign any `OpLogEntry`; it only
  consumes one already constructed by the caller with a fake signature.
- `fe-hexon/src/p2p/fetch.rs:113-115` comment acknowledges the dual-signature-scheme ambiguity
  in its own words: *"the current signature scheme signs the full manifest JSON as provided
  during publishing"* — written as if there's only one scheme, while `fe-format` (used by
  terrain tilesets, i.e. the exact hexon type the digital-twin story cares about most) uses a
  different, canonical-JSON scheme.
