---
type: plan
track: p2p_asset_streaming_20260718
created: 2026-10-13
title: "Campaign plan — PAS-P5: partial hexon fetch (HTTP + P2P), archive reader split, signed integrity root"
ledger_rows: [PAS-P5]
depends_on: [PAS-P3, PAS-P4]
decisions: [D-75 C1, D-76 D2, D-73 A4, D-77 E2, "2026-10-13 R4/R5 (D-5/D-17 generic asset)"]
resource: ./plan.md
---

# PAS-P5 campaign plan (2026-10-13)

Planner output for the large-phases campaign (RUNBOOK §7 step 3: PAS-P3+P4 → **PAS-P5** →
CDL W4 → CDL SPEC-10). Written against `877e801`. Slices run strictly sequentially; each
executor gates with `cargo check -p <crates> --tests` + `cargo +1.99.0 clippy -p <crates>
--all-targets -- -D warnings` only. Flow tests below are written in the slice that owns
them and run once, in the campaign's final sweep (RUNBOOK §3 working rules).

---

## 1. Goal and acceptance

**Goal (spec FR-5 + FR-6, ratified D-75 = C1, D-76 = D2).** Any hexon, hosted or held by a
peer, can be fetched in parts: header first (manifest + catalogs, no tile bytes), then the
chunks or members the caller needs. Every part is checked against one Ed25519-signed
root, so the same trust holds over HTTP and iroh. fe-hexon's `FetchStrategy` becomes the
single policy layer over both transports.

**Acceptance — the ledger row (`doneness-ledger.md:120`) plus the ratified text:**

| # | Criterion | Source |
|---|---|---|
| A1 | `PartialHexonFetch` trait with an HTTP impl (registry routes) and a P2P impl over the PAS-P3 transport trait | D-75 C1, spec FR-5 |
| A2 | Registry routes: `GET /hexons/{uri}/meta` (header, no bytes) and `GET /hexons/{uri}/chunk/{seq}`; plus a member route for monolithic archives | D-75 C1 (`/{uri}/chunk/{seq}`, `/{uri}/meta`) |
| A3 | `FetchStrategy` is the shared policy layer (semaphore, source priority, signature and digest checks) | D-75 C1, decision record "Ratification (1)" |
| A4 | `HexonArchive::import` split into a header phase and on-demand member reads | D-75 "Client side regardless of option" |
| A5 | Per-chunk blake3 in the chunk index; the signed manifest commits to the entries.json digest and the chunk-index digest; bao stays transport-only | D-76 D2 (1)(2) |
| A6 | `asset_hash` enforced at install AND at registry publish; signatures verified at publish (mismatch tests) | D-76 D2 (3), ledger row |
| A7 | One conformance suite, and both the hosted and the P2P path pass it | ledger row, plan.md Phase 5 checkpoint |
| A8 | Manifest field changes go through fe-format v1.0.0 canonical JSON (the format that survives unification) and do not fork `hexon_unification_20260716` | D-76 "Prerequisite", spec "Out of scope" |
| A9 | No partial-fetch gate is built and left unwired: at least one production path consumes it (house rule from the 2026-07-31 security audit, "gate-built-not-wired") | memory `security-audit-20260731` |

---

## 2. Current code (verified at `877e801`)

**Format layer (fe-format, the format that survives unification):**
- `HexonManifest` has `signature: Option<String>` and no integrity fields:
  `fe-format/src/manifest.rs:199-236`. It has no `f64` fields, which matters for
  canonical-JSON signing (see risk R4).
- `ChunkIndex {tileset_id, chunk_seq, total_chunks, chunk_bounds}` describes one chunk and
  carries **no hash**: `fe-format/src/manifest.rs:133-142`. It sits on
  `TilesetMeta.chunk_index` (`:57`).
- Signing works on canonical JSON with `signature` blanked to `""`, and the key comes from
  `publisher_did` (self-certifying): `fe-format/src/signature.rs:53-94`, rationale in
  `fe-format/src/AGENTS.md` §signature.
- `HexonArchive::import` reads everything eagerly (every tile, asset and GPX into `Vec`s) under one
  decompression budget, and verifies **nothing** (no signature, no `asset_hash`):
  `fe-format/src/archive.rs:256-392`. `export_tileset` writes an empty `entries.json`, so tiles are
  in no catalog: `archive.rs:200-252`, comment at `:223`.
- fe-format pulls in bevy through `fe-runtime` (`fe-format/Cargo.toml:11`). The only use is
  `From<NodeDto> for ExportNode` (`fe-format/src/lib.rs:87`, consumed at `fe-api/src/format.rs:57`).
  This is why the registry keeps fe-format as a dev-dependency only
  (`fe-hexon-registry/Cargo.toml:24`) and **duplicates the limits on purpose**
  (`fe-hexon-registry/AGENTS.md` §Decompression limits, last bullet).

**Hosted registry (fe-hexon-registry):**
- Routes: search, manifest, entries, `entries/{id}/asset`, `download`, publish:
  `fe-hexon-registry/src/routes.rs:76-94`. There is no meta route or chunk route. The lazy
  single-member read already exists: `index.rs:34-46` `read_zip_bytes`.
- Publish validates zip structure, caps and required fields only. It checks **no signature and no
  `asset_hash`**, and anyone holding the token can publish a new version of someone else's
  `hexon_id`: `routes.rs:291-359`, `index.rs:51-78`.
- `/entries` returns **re-serialized** JSON (`routes.rs:200-208` → `serde_json::Value`), so a client
  cannot hash it against a committed digest. The header route must ship raw member text.

**fe-hexon (the legacy package stack plus policy):**
- `FetchStrategy` (semaphore + `P2pConfig` policy) and `verify_fetched_manifest` are on the
  **legacy** manifest/signature scheme and have **zero production consumers**:
  `fe-hexon/src/p2p/fetch.rs:39-129`. The only consumers are re-exports at `p2p/mod.rs:15` and tests.
  `P2pConfig.max_download_rate_bytes_per_sec` is parsed (`p2p/config.rs:25,55`) but read by nothing.
- `RemoteRegistryClient` has search/manifest/whole-file `download` only and **no production caller**
  (tests only): `fe-hexon/src/remote.rs:35-107`, feature `remote` (`fe-hexon/Cargo.toml` `[features]`).
- Legacy `HexonRegistry::install`: a mismatched blob is refused by `FsBlobStore::store`
  (`registry.rs:54-56`), but a **missing** blob for a catalogued entry only logs a `warn!` and the
  install succeeds: `fe-hexon/src/registry.rs:186-205`.

**Install sites (production path is fe-terrain, not fe-hexon — `fe-hexon/src/AGENTS.md` §blob-store):**
- `HexonStore::install_tileset` checks only the hexon type and verifies nothing:
  `fe-terrain/src/tiles/store.rs:125-178`. It also builds the on-disk path straight from the
  archive's `hexon_id` with no charset check: `store.rs:281-287` `base_dir.join(format!("{hexon_id}.hexon"))`.
  **A crafted `hexon_id` such as `../x` writes outside the store.** Today this is reachable through
  REST/MCP tileset install (Editor+, `fe-api/src/terrain.rs:717-740`). P5 adds network sources, which
  makes it reachable from untrusted peers, so S6 fixes it.
- The other import sites that inherit whatever `import` enforces: `fe-terrain/src/tiles/hexon_source.rs:37-59`,
  `fe-terrain/src/tiles/distribution.rs:161-200` (`assemble_chunks`, which flattens chunks into one
  archive), `fe-terrain/src/format.rs:75`, `fe-api/src/format.rs:163` (scene import),
  `fe-api/src/terrain.rs:427` (id sniff).

**Chunk pipeline (the PAS-P3 domain, listed here because P5 builds on it):**
- `package_chunked` builds N self-contained chunk `.hexon`s and signs each manifest. It writes **no
  root/outer index and no per-chunk hash**, and has no production caller:
  `fe-terrain/src/tiles/builder.rs:289-394` (signing at `:352-356`).
- The sync handlers are still stubs: `fe-sync/src/sync_thread.rs:2101`
  (`handle_request_tileset_meta`), `:2123` (`handle_request_chunk` → `ChunkFailed`), tracker `:2010`.
  Messages: `fe-sync/src/messages.rs:62-69` (commands), `:161-181` (events). `ChunkReceived` has no hash.
- iroh-blobs 0.103 **is now real and routed**: `FsStore` + `BlobsProtocol` accepted on
  `iroh_blobs::ALPN`, `fe-sync/src/docs_engine.rs:120-170`. The 0.35-era claim "wired to nothing"
  (decision record D-77 facts) is stale.
- Blob identity: `fe_runtime::blob_store::BlobHash = [u8; 32]` (`fe-runtime/src/blob_store.rs:8`),
  hex via `hash_to_hex` (`:56`). The same blake3 hex is the `asset.content_hash` column
  (`fe-database/src/schema.rs:228-243`, written at `handlers/crud.rs:382`), the hexon
  `AssetEntry.asset_hash` (`fe-format/src/entries.rs:27`), and the iroh-blobs content hash. **One
  identifier space**, which is the hook for the 2026-10-13 generic-asset model.

**Builder ordering hazard:** `append_baked_splats_to_archive` rewrites the whole zip *after*
export (`fe-terrain/src/splat/format.rs:112-150`; it reads with uncapped `read_to_end`). A seal applied
before this step would be invalidated, so sealing must be the last transform.

---

## 3. Design (the contract the slices implement)

### 3.1 Integrity root (D-76 D2), in fe-format

```text
manifest.json  (signed: canonical JSON, signature blanked — unchanged scheme)
 └─ integrity: { v: 1, algo: "blake3",
                 header: { "entries.json": h, "license.json": h,
                           "terrain/tileset_meta.json": h, "members.json": h,
                           "chunks/index.json": h, ... } }      ← header-phase members, by exact bytes
      ├─ members.json: { "<zip path>": h, ... }                  ← every body member (tiles, satellite,
      │                                                            splats, entities/*, terrain/tracks/*)
      ├─ assets/{h}    self-certifying: name == blake3(bytes) AND h is referenced by a
      │                (header-committed) entries.json AssetEntry.asset_hash
      └─ chunks/index.json: [{seq, blake3, size_bytes, bounds, min_zoom, max_zoom}]
            └─ chunks/{seq}.hexon   each itself a sealed hexon (recursive trust)
```

- **Digests are over exact stored bytes** (blake3 of the decompressed member). Nothing is
  re-serialized, so formatting can never break a digest. Only the manifest is canonicalized,
  exactly as it is today.
- **Coverage invariant:** every zip member except `manifest.json` must be covered, either by the
  `header` map, by `members.json`, or as a self-certifying `assets/{h}` that entries.json references.
  An uncovered member is refused. This stops members being smuggled in and catches a post-seal
  rewrite such as the splat append.
- **`seal(archive_bytes, &SigningKey) -> Vec<u8>`** is a post-processor. It copies every member
  through `ZipWriter::raw_copy_file` (no recompression), computes the digests, writes `members.json`,
  sets `manifest.integrity` and signs. It is always the last transform in every producer.
- **`IntegrityPolicy { RequireSealed { pin: Option<publisher_did> }, VerifyIfSealed }`.** Under the
  default policy, a sealed archive is fully enforced everywhere it is read. An unsealed (legacy)
  archive passes with `IntegrityStatus::Unsealed`. Network paths and registry publish use
  `RequireSealed`.
- **Chunking happens at publish time.** Only the publisher can produce a container, because only the
  publisher can sign `chunks/index.json`. Seeders serve a monolithic hexon as one unit and never
  re-chunk it.
- **The chunk `blake3` is the D-74 `bundle_hash`.** The pointer-set `{(hexon_uri, bundle_hash)}`
  that PAS-P4's ledger computes is made of signed, transport-neutral identifiers.

### 3.2 Header document (one shape for both transports), in fe-format

```rust
pub struct HeaderDoc {            // fe_format::partial::HeaderDoc — serde JSON
    pub v: u32,                   // 1
    pub uri: String,              // "{hexon_id}@{version}"
    pub manifest: String,         // RAW manifest.json text
    pub header_members: BTreeMap<String, String>, // zip path → RAW text (entries, license,
                                  // tileset_meta, members.json, chunks/index.json, …)
    pub archive_size: u64,
    pub layout: Layout,           // Monolithic | Container { chunk_count }
}
pub fn verify_header(doc: &HeaderDoc, policy: IntegrityPolicy) -> Result<VerifiedHeader, IntegrityError>;
```

Raw member text travels as a JSON string. serde_json's string escaping is lossless for UTF-8, and
the writer only emits UTF-8 JSON, so blake3 over the client's `String::as_bytes()` equals the
committed digest. The HTTP `/meta` route returns this document, and the P2P path stores it as
HashSeq child 0. Because both transports carry the same bytes, the conformance suite can compare
results directly.

### 3.3 Partial fetch trait and policy, in fe-hexon

```rust
// fe-hexon/src/partial.rs  (async-trait — dyn-safe so FetchStrategy can hold mixed sources)
#[async_trait]
pub trait PartialHexonFetch: Send + Sync {
    fn source(&self) -> FetchSource;                       // Peer{peer_ref, priority} | Registry{base_url}
    async fn fetch_header(&self, uri: &str) -> Result<HeaderDoc, PartialFetchError>;
    async fn fetch_chunk(&self, uri: &str, seq: u32) -> Result<Bytes, PartialFetchError>;
    async fn fetch_member(&self, uri: &str, path: &str) -> Result<Bytes, PartialFetchError>;
}
pub enum PartialFetchError { NotFound, OutOfRange, Integrity(IntegrityError), Transport(String), Timeout }
```

Transports return **unverified** bytes. All checks happen in `FetchStrategy`, so no transport can
skip them:

- `fetch_header(uri, sources, pin)` → `VerifiedHeader`
- `fetch_chunk(&VerifiedHeader, seq, sources)` → blake3 checked against the signed chunk index
- `fetch_member(&VerifiedHeader, path, sources)` → checked against the header map, `members.json`,
  or the asset self-certification
- `fetch_by_hash(hash, sources)` → the generic content primitive (§8 cross-cuts)

Each call holds a semaphore slot (`fetch.rs:73-81`). Sources are tried in order:
`PeerPriority` (`Lan < LowLatency < Remote`), then the registry. An integrity failure demotes that
source and the next one is tried. Pacing honours `max_download_rate_bytes_per_sec`, which closes the
dead config at `p2p/config.rs:25`. When a pin is set, a header signed by a different DID is refused.

### 3.4 What P5 depends on from PAS-P3 (contract C-P3)

P5 relies on the following *semantics*. If P3 lands different names, S5 adapts to them at the
start of the slice (grep first) and does not rebuild them.

1. **An iroh-type-free transport trait** (D-77: no RangeSpec/iroh types in public seams). P5 prefers
   it in **`fe-runtime`** (e.g. `fe_runtime::transport::BlobTransport`) so that fe-hexon can implement
   P2P fetch without depending on fe-sync:
   ```rust
   #[async_trait] pub trait BlobTransport: Send + Sync {
       async fn get_blob(&self, from: &PeerRef, hash: BlobHash) -> Result<Bytes, TransportError>; // bao-verified
       async fn get_hash_seq(&self, from: &PeerRef, root: BlobHash) -> Result<Vec<BlobHash>, TransportError>;
       async fn put_blob(&self, bytes: Bytes) -> Result<BlobHash, TransportError>;
       async fn put_hash_seq(&self, children: &[BlobHash]) -> Result<BlobHash, TransportError>;
       async fn has_blob(&self, hash: BlobHash) -> bool;
   }
   ```
   It must also ship an **in-memory implementation** (the ledger P3 acceptance "in-memory transport
   impl used by tests") that verifies hashes on `get_blob` exactly as bao does, plus the iroh impl
   over `iroh_blobs::api::Store` and the routed `BlobsProtocol` (`docs_engine.rs:160`).
   *If P3 puts the trait in fe-sync instead,* the P2P impl moves to `fe-sync/src/partial_fetch.rs`
   and fe-sync gains an `fe-hexon` dependency. There is no cycle: fe-hexon depends on
   fe-runtime/identity/policy/format only.
2. **A per-chunk record with a blake3 hash** (P3 acceptance: "per-chunk blake3"). P5's
   `chunks/index.json` row type *is* that record. If P3 has already named it (for example by
   extending `ChunkIndex` or adding a `ChunkManifest`), P5 reuses the name.
3. **Real `handle_request_tileset_meta` / `handle_request_chunk`**, with the advertisement
   (`fe-terrain/src/tiles/distribution.rs:29`) carrying a HashSeq **root hash**. S7a then makes
   the sync-driven download verify the signed header before any chunk is requested.
4. **The P4 lazy tile source.** If P4 built its own offset-index reader inside fe-terrain, S2
   *promotes* it into fe-format's `HexonArchiveReader` instead of adding a second reader
   (doctrine: "shared cores + separate guards = drift", RUNBOOK §5).

### 3.5 Reconciliation with the PAS-P3P4 plan (critic, 2026-10-13) — the earlier phase owns these

The P3P4 plan (`campaign-plan-p3p4-20261013.md` §10) now **fixes** the C-P3 contract. The
"if P3 lands X" hedges above are resolved as follows, and P5 adapts:

| Topic | Owned by P3P4 (lands first) | What P5 does |
|---|---|---|
| Transport trait | `fe_runtime::blob_transport::BlobTransport` (fe-runtime, iroh-free) with `add`, `add_hash_seq`, `set_tag`/`delete_tag`/`list_tags`, `fetch(provider, hash)`, `fetch_hash_seq(provider, root)`, `read_local(hash)`; `InMemoryTransport` beside it; `IrohBlobTransport` in fe-sync | Default placement A1 holds: the S5 P2P impl lives in **fe-hexon**. The `has_blob` used by the S5 test is `read_local(..).is_some()`. Do not rename the methods. |
| Chunk record | `fe_format::TilesetBundleIndex { bundles: Vec<BundleEntry{seq, blake3_hex, byte_len, …ranges}> }` + `ChunkIndex.blake3` | `chunks/index.json` **is** a serialized `TilesetBundleIndex`. S1 adds no `ChunkTable` row type. |
| Tile → chunk lookup | `TilesetBundleIndex::bundle_for_tile` | S2 drops `ChunkTable::chunk_for_tile` and its test. |
| Lazy archive reader | `fe_format::archive_index::HexonArchiveIndex` (P4 S5) | S2 **extends** it with header-doc, verified `read_member` and `chunk(seq)`. It adds no `reader.rs` (R2 resolved: one reader, one guard set). |
| HashSeq child 0 | opaque `TilesetBundleIndex` JSON; fe-sync never parses fe-format | S5 changes child 0 to the `HeaderDoc` JSON, which carries `chunks/index.json` as raw text. fe-sync still treats it as opaque bytes. |
| Who drives a download | the sync thread (`DownloadTileset`); fe-ui is a view (P3P4 S4) | S7a adds a **header-confirm phase** (see S7a), because fe-ui never sees the header before chunks are requested. |
| Path guard | `hexon_id` charset guard in `HexonStore` (P3P4 S1 step 6); `tileset_id` guard in `TilesetTransfer` (S2) | S6 keeps them, adding a regression row only. |
| Seeder re-bundling vs A3 "seeders never re-chunk" | P3P4 S4/S5-relay bundle any seeding tileset deterministically (unsigned) | **Rule (default; reversal moderate):** a sealed **container** re-seeds its existing signed chunks and is never re-bundled; a sealed **monolithic** archive seeds as `[HeaderDoc, whole archive]`; only legacy **unsealed** tilesets keep P3P4's deterministic bundling (installed `verified: false`). S5 implements this in the seeding path P3P4 built. |

---

## 4. Slices (strictly sequential; one executor each)

Order: **S1 → S2 → S3 → S4 → S6 → S5 → S7a → S7b → S8.** S6 runs before S5 so the P2P suite can
use real builder-sealed containers.

### S1 — fe-format integrity core (size M)

**Owned files:** `fe-format/Cargo.toml`, `fe-format/src/lib.rs`, `fe-format/src/manifest.rs`,
`fe-format/src/integrity.rs` (new), `fe-format/src/AGENTS.md`, `fe-api/Cargo.toml` (feature
opt-in only), `fe-format/examples/seal_hexon.rs` (new).

**Steps:**
1. Make `fe-runtime` optional behind a default feature `runtime`, and gate `From<NodeDto>`
   (`lib.rs:87`) on it. `fe-api` keeps the default. Add `blake3 = { workspace = true }`.
2. Add `HexonManifest.integrity: Option<IntegrityRoot>` with `#[serde(default,
   skip_serializing_if = "Option::is_none")]`. It is additive, and legacy manifests deserialize
   unchanged.
3. `integrity.rs`: `IntegrityRoot`, `MembersDoc` (**critic:** the chunk table is P3P4's
   `TilesetBundleIndex`/`BundleEntry`; there is no new `ChunkTable` type, §3.5),
   `IntegrityPolicy`, `IntegrityStatus {Sealed{publisher_did}, Unsealed}`, an `IntegrityError` enum
   with one variant per failure class (BadSignature, PublisherMismatch, DigestMismatch{path},
   Uncovered{path}, MissingMember{path}, AssetNotReferenced{hash}), `seal()`, `verify_archive()`.
4. Re-export from `lib.rs`. Add the example `seal_hexon <in> <out>`, which reads the key from
   `FE_SECRET_HEXON_SEAL_KEY` (hex). Keys never go in files (RUNBOOK §5 Secrets).

**Invariants:** the signature scheme is byte-identical to `signature.rs:53-94` (fe-format AGENTS
§signature). `HexonManifest` gains no float fields (R4). `TilesetMeta` producers keep using
`..Default::default()` (fe-format AGENTS §scale). The decompression caps apply to every read in
`seal`/`verify` (fe-format AGENTS §decompression-limits).

**Extra gate:** `cargo check -p fe-format --no-default-features`.

**Flow tests** (`fe-format/src/integrity.rs` tests, `fe-format/tests/integrity_flow.rs`):
- `sealed_tileset_verifies_and_names_its_publisher`: export_tileset → seal → `verify_archive(RequireSealed)`
  returns `Sealed{did}` equal to the signing key's DID.
- `tampering_is_refused_per_class` (**one table-driven test**): starting from one sealed archive, each
  row applies one mutation and asserts the exact `IntegrityError` variant. Rows: flip a byte in a tile,
  in entries.json, in tileset_meta.json, in members.json, in an `assets/{h}` blob; add an uncovered
  member (simulates the splat append after sealing); delete a member; re-sign the manifest with a
  different key while a publisher is pinned.
- `legacy_unsealed_archive_passes_verify_if_sealed_and_fails_require_sealed`.
- `sealing_preserves_every_member_byte`: `import(seal(x))` yields the same tiles, assets and meta as
  `import(x)`. This catches a raw-copy regression.

### S2 — Header/member reader split + chunk container (size M)

**Owned files:** `fe-format/src/archive.rs`, `fe-format/src/archive_index.rs` (**critic:**
extend P4's `HexonArchiveIndex`; no new `reader.rs`, §3.5), `fe-format/src/partial.rs`
(new: `HeaderDoc`, `verify_header`), `fe-format/src/AGENTS.md`, plus the P4 call sites of the index
in `fe-terrain/src/tiles/hexon_source.rs` (re-point only, if a signature changes).

**Steps:**
1. **(Critic:)** Extend `HexonArchiveIndex` (P4) into the reader described here rather than
   adding `HexonArchiveReader<R: Read + Seek>`. The bullets below define what it must gain.
   - `open` reads only the central directory plus the header members (manifest, entries, license,
     tileset_meta, terrain/config, schema, members.json, chunks/index.json). This mirrors `index_one`.
   - `header_doc()`, `member_names()`.
   - `read_member(path)`: capped, with a shared budget, and verified against `members.json`/assets
     when the archive is sealed.
   - `chunk(seq)` for containers.
2. Re-implement `HexonArchive::import` as reader + full walk, so it now enforces
   `VerifyIfSealed`. `HexonArchiveData` gains `integrity: IntegrityStatus`.
3. `HexonArchive::export_chunked(manifest, tileset_meta, chunks: &[Vec<u8>])` writes `chunks/index.json`
   and `chunks/{seq}.hexon` (Stored), then the caller seals.
4. ~~`ChunkTable::chunk_for_tile`~~ **(critic: dropped.** P3P4 S1 already ships
   `TilesetBundleIndex::bundle_for_tile` with its table test; reuse it.)

**Invariants:** keep the existing behaviour of `import` (its tests in `archive.rs:443-600` and
`lib.rs` tests stay green). No `HexonArchiveData` consumer changes shape except for the added field.

**Flow tests:**
- `header_phase_reads_no_tile_bytes`: a counting `Read+Seek` wrapper over a 4 MiB tileset; opening
  the reader plus `header_doc()` reads < 64 KiB.
- `lazy_member_read_refuses_only_the_tampered_member`: sealed archive with one corrupted tile; the
  header passes, the corrupted tile returns `DigestMismatch`, and a different tile still reads.
- `chunked_container_roundtrips_and_each_chunk_matches_its_signed_hash`.
- ~~`chunk_for_tile_table`~~ (critic: covered by P3P4 S1 `bundle_for_tile_table`).

### S3 — Registry: strict publish + meta/chunk/member routes (size M)

**Owned files:** `fe-hexon-registry/Cargo.toml`, `src/routes.rs`, `src/index.rs`, `src/limits.rs`,
`fe-hexon-registry/AGENTS.md`, `tests/registry_test.rs`, `tests/support/fixtures.rs`.

**Steps:**
1. Depend on `fe-format = { default-features = false }`. Replace the duplicated `limits.rs` with
   `fe_format::limits`. The reason for the duplication (bevy) is gone, and the same AGENTS doctrine
   applies.
2. Publish runs `verify_archive(RequireSealed)` after `validate_package`, so an `asset_hash` mismatch,
   a bad signature or an unsealed archive gets 400.
   **Publisher continuity:** if `hexon_id` is already indexed under a different `publisher_did`, the
   publish gets 409. This mirrors AP-3.4 (`fe-hexon/src/AGENTS.md` §authz "Publish ownership").
3. `HexonIndexEntry` gains `sealed: bool` and `layout`. Packages placed on disk by other means still
   index, and `/download` still serves them.
4. Add routes:
   - `GET /hexons/{uri}/meta` → `HeaderDoc` (built from `HexonArchiveReader` on `spawn_blocking`)
   - `GET /hexons/{uri}/chunk/{seq}` → raw bytes: `chunks/{seq}.hexon` for a container; for a
     monolithic archive, seq 0 is the whole file and anything else is 404
   - `GET /hexons/{uri}/member/{*path}` → one raw member; the path must be a member name in the
     central directory, so traversal is impossible by construction
5. Seal the fixtures with a deterministic test key.
6. **(Critic, security.)** `/chunk` and `/member` serve publisher-controlled bytes from the
   registry origin. They respond with `Content-Type: application/octet-stream`,
   `X-Content-Type-Options: nosniff` and `Content-Disposition: attachment`, matching the
   stored-XSS rule CDL-SPEC10 A4 applies to fe-api blobs.

**Invariants:** `/api/v1` stays token-gated (`fe-hexon-registry/AGENTS.md` §Auth model). Every read
stays decompression-capped (§Decompression limits). The filename charset rule stays
(§Publish validation).

**Extra gate:** `cargo tree -p fe-hexon-registry -e normal | grep -c bevy` → `0`.

**Flow tests** (in-process router, `tests/registry_test.rs`):
- `publish_outcomes` (table): sealed → 200 and then searchable; unsigned → 400; `asset_hash` ≠ blob
  → 400 (**the ledger's mismatch test**); same id with a different publisher → 409; zip bomb → 400
  (existing).
- `meta_route_header_verifies_client_side_without_tile_bytes`: response size ≪ archive, and
  `verify_header(RequireSealed)` passes.
- `chunk_route_serves_bytes_matching_the_signed_index` plus out-of-range → 404 and monolithic seq 0
  = whole file.
- `member_route_refuses_names_not_in_the_archive` (`../`, an absolute path, a nonexistent name).
- **(critic)** `an_html_member_is_served_as_an_octet_stream_attachment_with_nosniff`.

### S4 — fe-hexon partial-fetch layer + HTTP impl + conformance suite (size L)

**Owned files:** `fe-hexon/Cargo.toml`, `fe-hexon/src/lib.rs`, `fe-hexon/src/partial.rs` (new),
`fe-hexon/src/partial/conformance.rs` (new, feature `conformance`), `fe-hexon/src/p2p/fetch.rs`,
`fe-hexon/src/p2p/mod.rs`, `fe-hexon/src/remote.rs`, `fe-hexon/src/registry.rs` (install
missing-blob only), `fe-hexon/src/AGENTS.md`, `fe-hexon/tests/partial_fetch_http.rs` (new),
`fe-hexon/tests/support/*`.

**Steps:**
1. Add the trait and the error enum from §3.3. Add `async-trait` and `bytes` (workspace).
2. Rework `FetchStrategy` (§3.3): verified header/chunk/member/by-hash, source ordering, demotion
   on integrity failure, pin, pacing. Replace the legacy `verify_fetched_manifest` with
   `fe_format::verify_header`. It has no production consumer, and its scheme is the one D-76
   retires.
3. `impl PartialHexonFetch for RemoteRegistryClient` (feature `remote`) over the S3 routes.
4. `conformance::run(fetcher, fixtures) -> Report`. This is a public async function that any
   transport crate can call. Fixtures are a sealed monolithic model hexon, a sealed tileset, and a
   sealed 3-chunk container, all built with fe-format and a deterministic key.
5. In `HexonRegistry::install` (`registry.rs:200-205`), a catalogued entry whose blob is missing
   becomes `Err(RegistryError::Package(..))` instead of a `warn!`.

**Invariants:** the authz seam is unchanged (`*_as` methods, fe-hexon AGENTS §authz). The
FsBlobStore canonical layout and atomic write stay (§blob-store). The HTTP client stays behind
`remote` (D-75).

**Flow tests:**
- `http_source_passes_conformance`: an in-process registry on an ephemeral port, then
  `conformance::run`. Cases: header verified with publisher; chunk bytes equal and verified; member
  bytes equal; unknown uri → NotFound; seq out of range → OutOfRange; pinned wrong publisher → Integrity.
- `strategy_falls_back_from_a_tampering_source_to_an_honest_one`: two real registries, where one has
  a tampered `.hexon` dropped onto disk (bypassing publish). The returned bytes are the honest ones
  and the report names the demoted source.
- `legacy_install_refuses_missing_or_mismatched_blob` (table: missing, mismatched).
- `pacing_delay_table` (pure function: bytes, rate, elapsed → delay).

### S6 — Install-side enforcement + chunk containers in fe-terrain (size M)

**Owned files:** `fe-terrain/src/tiles/store.rs`, `fe-terrain/src/tiles/registry.rs`,
`fe-terrain/src/tiles/hexon_source.rs`, `fe-terrain/src/tiles/distribution.rs` (`assemble_chunks`
only, coordinated with the P3 changes already landed), `fe-terrain/src/tiles/builder.rs`,
`fe-terrain/src/AGENTS.md`, `fe-hexon/examples/build_sample_hexons.rs`, `fe-terrain/tests/hexon_integrity_flow.rs` (new).
**(Critic)** also `fe-terrain/tests/tileset_bundles_test.rs` and
`fe-test-harness/src/scenarios/two_peer_tileset_stream.rs` (P3P4 S1/S4): update any expectation
that assumed a flattened install, because step 3 keeps the sealed container.

**Steps:**
1. `HexonStore::install_tileset`:
   - **(critic)** the `hexon_id` charset guard already landed in P3P4 S1 step 6, because P3P4 S4
     made peer installs reachable a phase earlier. Keep it; add only the regression row below.
   - verify with `VerifyIfSealed` (through `import`)
   - record `verified: bool` on `InstalledTileset` (`#[serde(default)]`)
   - accept the container layout
2. `HexonTileSource::from_archive` loads containers by importing each `chunks/{seq}.hexon`. If P4
   made this source lazy, use P4's lazy path per chunk instead.
3. `assemble_chunks` keeps the publisher's **sealed container** instead of flattening it, so a
   downloaded tileset can be re-seeded with its seal intact.
4. Builder: split `package_chunked` into `download_tiles` and a pure
   `package_tiles_chunked(tiles, …, key)` that returns a sealed container. Seal is the last step,
   after the splat append. `build_sample_hexons` seals with a dev key.

**Invariants:** NFR-4 offline provenance (spec) is untouched. Monolithic installs behave as before
for legacy archives. Local reads after install are **not** re-verified, because verification happens
once at the network→disk boundary (fe-hexon AGENTS §blob-store trusts its own store the same way).

**Flow tests:**
- `tampered_sealed_tileset_install_is_refused_and_store_is_unchanged`: `registry.json` and the
  directory listing are identical before and after.
- `install_refuses_path_escaping_hexon_ids` (table: `../x`, `a/b`, `a\\b`, `C:x`, `..`, empty). No file
  is written outside `base_dir`.
- `legacy_unsealed_tileset_installs_flagged_unverified` (keeps the gis-tile-etl dist archives and
  `scripts/hexon-install-verify.ps1` working).
- `chunked_container_install_serves_tiles_from_every_chunk`.
- `assembled_p2p_download_installs_as_a_reseedable_sealed_container` (`verify_archive(RequireSealed)`
  on the stored file).

### S5 — P2P partial fetch over the PAS-P3 transport (size M)

**Owned files** (default placement, C-P3 #1 — **fixed by P3P4 §10 C3: the trait is in fe-runtime,
so the fe-sync alternative is struck**): `fe-hexon/src/partial/p2p.rs` (new),
`fe-hexon/tests/partial_fetch_p2p.rs` (new), `fe-hexon/src/AGENTS.md`.
**(Critic)** plus `fe-sync/src/tileset_transfer.rs` (child 0 → `HeaderDoc`, opaque bytes),
`fractalengine/src/terrain_bridge.rs` and `fractalengine-relay/src/main.rs` (seeding path:
the §3.5 container/monolithic/legacy rule).

**Steps:**
1. `publish_to_transport(transport, archive_bytes) -> root` puts the HashSeq
   `[HeaderDoc JSON, chunk_0..n]` for a container, or `[HeaderDoc, whole archive]` for a monolithic
   archive. It refuses unsealed input.
2. `P2pPartialFetch { transport, locator, peer }` implements `PartialHexonFetch`:
   - header = child 0
   - chunk(seq) = child seq+1, cross-checked against the signed chunk index **before** the bytes are
     returned to the strategy
   - member = read from the containing chunk/whole archive
3. `HexonLocator` maps a uri to (root, providers). It is populated from P3 advertisements' root
   hashes or set by the caller.
4. **(Critic)** The P3P4 seeding path (GUI `terrain_bridge` and the relay startup) applies the §3.5
   rule: a sealed container publishes `[HeaderDoc, chunk_0..n]` from its existing chunks with no
   re-bundle; a sealed monolithic archive publishes `[HeaderDoc, whole archive]`; a legacy
   unsealed tileset keeps P3P4's deterministic bundles.

**Invariants:** iroh/RangeSpec types stay out of public seams (D-77 E2). Sync-thread I/O uses
`spawn_blocking` (fe-sync AGENTS §sync-thread-blocking-io, NFR-3). BBR stays explicit (fe-sync
AGENTS §congestion-control).

**Flow tests:**
- `in_memory_p2p_source_passes_conformance`: the same `conformance::run` as S4, which is the A7 proof.
- `poisoned_peer_store_is_refused`: the in-memory transport maps a hash to wrong bytes. The transport
  must error, and if it does not, the strategy returns `Integrity`.
- `iroh_loopback_fetches_one_chunk_without_the_rest`: two real endpoints (the P3 loopback helper).
  Fetch the header and chunk 1 of a 3-chunk sealed container, then assert `has_blob` is false for
  chunks 0 and 2 on the fetcher. This proves the fetch is partial.
- **(critic)** `a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling`: the
  published children equal the container's signed `chunks/index.json` hashes.

### S7a — P2P download path consumes the integrity root (size M)

**Owned files:** the P3-landed regions of `fe-sync/src/sync_thread.rs` (tileset meta/chunk
handlers only), `fe-sync/src/messages.rs` (tileset events/commands only),
`fe-sync/src/tileset_transfer.rs` (confirm gate + ledger field), the tileset-ad codec P3 landed
(add `publisher_did`), `fractalengine/src/terrain_bridge.rs` (header verification),
`fe-ui/src/terrain_map/events.rs` (view state "verifying header" only), `fe-sync/src/AGENTS.md`,
`fe-sync/tests/tileset_transfer_test.rs` and
`fe-test-harness/src/scenarios/two_peer_tileset_stream.rs` (the "P3 loopback" tests extended
below; critic: these were missing from the list).

**Critic correction.** The original steps had "the fe-ui driver" verify the header. After P3P4 S4,
fe-ui is a view and the sync thread drives `DownloadTileset` end to end, so no fe-ui code ever
sees the header before chunks are requested. fe-sync is format-agnostic (P3P4 S2 invariant) and
cannot verify a signature itself. So the verification moves to the host, in a two-phase download.

**Steps:**
1. `DownloadTileset` fetches the root and child 0 only, then emits `TilesetMetaReceived` with
   the `HeaderDoc` bytes. The ledger state is `awaiting_confirmation`. **No chunk is requested.**
2. `terrain_bridge` runs `FetchStrategy`/`verify_header(RequireSealed { pin })` off the main
   thread. The pin is the locally known publisher for that `hexon_id` (TOFU, A4) if one exists,
   otherwise the ad's new `publisher_did` field. On success it sends
   `SyncCommand::ConfirmTilesetDownload { tileset_id, expected_chunk_hashes }`, taken from the
   **signed** `chunks/index.json`; on failure it sends `CancelTilesetDownload` and the view shows
   the integrity error.
3. `TilesetTransfer` fetches chunks only after confirmation. It checks each child against
   `expected_chunk_hashes` (not merely the transport hash) and emits `ChunkReceived` only on a
   match. The ledger persists the confirmed hashes, so `resume_all()` resumes only confirmed
   downloads and never re-trusts an unverified header.
4. On completion, the S6 container install runs.

**Invariants:** `SyncEvent` fan-out is unchanged for other consumers (`status.rs:135-137`).
Current-thread runtime rules apply. fe-sync parses no fe-format type: the hashes arrive as
`[u8;32]` lists.

**Flow tests:** extend P3's two-instance loopback test with:
- `seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request`
- `chunk_not_in_the_signed_index_never_reaches_install`
- **(critic)** `an_unconfirmed_download_never_fetches_a_chunk_even_across_a_restart`: restart
  while awaiting confirmation → `resume_all` requests nothing.
- **(critic)** `a_header_signed_by_a_publisher_other_than_the_pinned_one_is_refused`.

All of them assert on the event stream and the store contents.

### S7b — Hosted "install from registry" production path (size M)

**Owned files:** `fractalengine/Cargo.toml` (enable `fe-hexon/remote`), `fractalengine/src/terrain_bridge.rs`,
`fe-ui/src/terrain_map/mod.rs` (`HexonOp::InstallFromRegistry{uri}`), the Hexon Manager dialog file
that renders the Installed tab (locate at slice start), `fe-ui/src/settings.rs`
(`hexon_registry_url`), `fe-ui/src/ui_shell/right_sidebar.rs` (settings field), `fe-terrain/src/AGENTS.md`
or `fractalengine/src/AGENTS.md` (whichever documents terrain_bridge).

**Steps:**
1. Add `AppSettings.hexon_registry_url: Option<String>`, persisted, sanitized and overridden by the
   env var `FE_HEXON_REGISTRY_URL`. This follows the `RelayConfig::resolve` precedence pattern.
2. The Hexon Manager gets a URI field and an "Install from registry" button that queues the op.
3. `drain_hexon_ops` runs the install on a worker thread, never on the Bevy main thread:
   `FetchStrategy::fetch_header` (RequireSealed) → `fetch_chunk` for every chunk with progress →
   `HexonStore` container install → refresh the dialog list.

**Invariants:** follow `ui_ux.md` §6 (no faked settings: the field has a live consumer). Results
travel from the worker thread over channels (AGENTS "Key Bevy Patterns"). Send failures are
`warn!`ed (anti-pattern 6).

**Flow tests:**
- `install_from_registry_installs_a_sealed_container_end_to_end`: an in-process registry plus a temp
  `HexonStore`, driven through the drain function. Progress is ordered header → 1/N … N/N.
- `install_from_registry_refuses_a_tampered_source_and_installs_nothing`.
- Extend the existing settings table test (`saved_settings_survive_a_restart` plus `sanitize`) with a
  `hexon_registry_url` row.

### S8 — Adversarial security review-and-fix + docs close-out (size S–M)

This is a separate context from the executors, and the reviewer fixes its own findings. A security
review is mandatory: the slices touch crypto, signature verification, untrusted network input and
filesystem paths.

**Owned files:** any file from S1–S7b for fixes, plus the docs in §5.

**Brief for the reviewer:**
- Refute that every network path uses `RequireSealed`.
- Refute that the coverage invariant is enforced at every reader.
- Refute that `HeaderDoc` raw-text hashing matches the stored bytes.
- Refute the traversal fix.
- Refute that the decompression budget holds across nested container reads (a container of bomb
  chunks).
- Refute that the semaphore permit is held for the whole fetch.
- **(critic)** Refute that no chunk is requested before `ConfirmTilesetDownload`, on both the
  fresh path and `resume_all`.
- **(critic)** Refute that an advertiser can choose its own pin. The ad's `publisher_did` is used
  only when no local pin exists for that `hexon_id`.
- **(critic)** Refute the `/chunk` and `/member` content-type/nosniff headers (S3 step 6).

Record a one-line verdict in the RUNBOOK review ledger. The orchestrator writes it, because this
planner does not own that file.

---

## 5. Docs to update (owned by the slice that changes the code; S8 sweeps)

- `fe-format/src/AGENTS.md`: new §integrity (root shape, coverage invariant, seal-last rule,
  policies) and §reader (header phase and container layout). §archive notes that `import` now
  verifies sealed archives.
- `fe-hexon-registry/AGENTS.md`: §routes table (meta, chunk, member), §Publish validation (sealed
  + asset_hash + publisher continuity), §Decompression limits (duplication removed, shared with
  fe-format).
- `fe-hexon/src/AGENTS.md`: new §partial-fetch (trait, policy layer, conformance suite, source
  ordering, the by-hash primitive).
- `fe-sync/src/AGENTS.md`: §partial-fetch-p2p (HashSeq layout, verification point), if any code
  lands there.
- `fe-terrain/src/AGENTS.md`: §hexon-store integrity and container installs, the hexon_id charset
  rule.
- `docs/hexon-format-spec.md`: "Archive Structure" (`members.json`, `chunks/`), "Signature &
  Integrity" (`:444-457` → the Merkle-extended root), "API Endpoints" (meta/chunk/member routes).
- `conductor/decisions/p2p-streaming-20260718.md`: dated "Implementation" lines under D-75/D-76.
  Also correct the stale D-77 fact "iroh-blobs wired to nothing" (blobs are routed at
  `docs_engine.rs:160`).
- Track `plan.md` Phase 5 checkboxes and the ledger row PAS-P5: left to the bookkeeping owner (this
  planner's write scope excludes them).

---

## 6. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | PAS-P3's trait name, location or chunk record differ from C-P3 | S5/S7a grep the landed P3 surface first and adapt names. Semantics are the contract. If the trait carries iroh types, flag it to the orchestrator (that would breach D-77) rather than wrapping it. |
| R2 | P4 ships its own lazy zip reader in fe-terrain | S2 promotes it into fe-format, so there is one reader and one guard set. |
| R3 | `zip` `raw_copy_file` keeps local-header quirks such as data descriptors | Covered by the S1 test `sealing_preserves_every_member_byte`. The fallback is to decompress and re-store under caps. |
| R4 | Canonical-JSON float instability (canonical-data-log review gate "no canonical-JSON floats") | `IntegrityRoot` is all strings and integers, and `HexonManifest` has no `f64`. Members are hashed as raw bytes, never canonicalized. |
| R5 | Strict registry publish breaks existing unsealed fixtures and dev workflows | S3 seals the fixtures. Legacy on-disk files still index and download. `seal_hexon` re-seals with the publisher key. |
| R6 | Self-certifying DIDs prove the publisher's key, not that the publisher is trustworthy | Publisher continuity (registry), advertisement-pinned DID (P2P), optional caller pin. No global allowlist (§7 A4). |
| R7 | `members.json` exceeds the 8 MiB JSON cap (> ~80k members) | Seal refuses with a clear error. Large tilesets must be containers, which the builder produces by default (S6). |
| R8 | Nested containers multiply decompression (bomb chunks inside a container) | One shared budget across outer and inner reads. This is a named S8 review target. |
| R9 | Collision with HU-P2's later rewrite of `fe-hexon/src/{p2p/fetch.rs, registry.rs}` | P5 lands first, since HU is pending and gated (RUNBOOK §7.7). The HU plan's Phase 2 must rebase on the new `FetchStrategy` (recorded in cross-cuts). |
| R10 | Sync-thread stalls during HashSeq publish of large containers | `spawn_blocking` for archive reads (NFR-3). Publish runs off the sync thread through the transport handle. |
| R11 | Relay-assisted P2P fetch is unverified (no self-hosted relay yet, R6 · D-23) | The loopback test runs with `FE_SYNC_RELAY=disabled`. Relay-assisted fetch rides the §7 step-5 relay acceptance test, against a local iroh-relay 1.3 container. |

---

## 7. Deferral audit (user rule: no unneeded deferrals)

| Item left out of P5 | Why it is legitimately out |
|---|---|
| C2 — HTTP `Range` on `/download` | **Ratified decision.** D-75 chose C1, and C2 is listed only as an optional later bonus for third-party tools. Building it would create a second partial-fetch abstraction, which the decision record warns against. |
| D3 — per-chunk signatures | **Ratified decision.** D-76 rejected it ("overkill; a hash committed under one signed manifest is equivalent trust"). |
| A2 per-tile blobs for hot zooms | **Ratified trigger.** D-73 says "only if over-fetch proves painful". There is no measurement showing that (evidence.md §3: every measured archive fits in 1–8 MiB bundles). |
| Collapsing fe-hexon's legacy `signature.txt` scheme, re-typing `announce.rs`/`discover.rs` onto fe-format, rebuilding `HexonPackage` on `seal` | **Missing user go.** These belong to `hexon_unification_20260716` (HU-P2), a pending track that RUNBOOK §7 step 7 says needs its own go. P5 needs none of it: it runs entirely on the fe-format stack that survives unification, and covers the legacy path only through the install `asset_hash` checks (S4.5). |
| P2P discovery of **non-tileset** hexons (generic announce with a root hash) | Same gate: discovery rides `announce.rs` on the legacy manifest (HU-P2). P5's `HexonLocator` accepts caller-supplied roots, so the hook exists. |
| Re-sealing the user's existing gis-tile-etl `dist/*.hexon` | **Hard external dependency.** That is a sibling repo, never pushed, and the publisher key lives outside this repo. P5 ships `seal_hexon`, and legacy installs keep working, flagged unverified. |
| Bucket FR-5 pull-through, device-payload records, directory assets (D-5/D-17) | **(Critic: re-classified.)** RUNBOOK §4's "second campaign" line is an *unasked assumption*, not a user decision. **Device-payload records are not deferred:** CDL-SPEC10 A3-A6 builds them in this campaign and owns the `asset`/`device_payload`/`device_format` tables. Bucket FR-5 and directory assets belong to `hexon_p2p_bucket` / D-20 W5, other tracks outside these five plans, so they are out of *scope*, not deferred. Whether the §4 campaign split stands is raised to the user (CDL-SPEC10 §7.2 A1). P5 provides `fetch_by_hash` and changes no schema. |
| In-app verification of S7b's UI and of a real install over a WAN registry | **User-gated** (RUNBOOK §4: every in-app verification is user-owned). |
| Closed-loop GPU-byte residency | It belongs to PAS-P4 (the D-74 "phase 2" layer). It is not a P5 item. |

Nothing else from FR-5/FR-6, D-75 or D-76 is left out.

---

## 8. Cross-cuts

- **PAS-P3:** contract C-P3 (§3.4). S7a edits only the handler regions P3 owns, after P3 lands.
  `assemble_chunks` changes from "flatten" to "keep the sealed container" (S6), and P3's tests that
  assert a flattened install need that expectation updated.
- **PAS-P4:** one reader (S2 converges with P4's). The chunk `blake3` is the D-74 `bundle_hash` of
  the pointer-set. The lazy source reads containers per chunk. Local reads are not re-verified.
- **hexon_unification (HU-P1/P2/P4):**
  - `HexonManifest.integrity` is additive and HU-P1's new variants compose with it.
  - **HU-P1's "fe-format gains zero dependencies" check is amended:** `blake3` is added, and
    `fe-runtime` becomes optional (the footprint shrinks overall).
  - HU-P2's rebuilt `HexonPackage::build` must call `fe_format::seal` and drop `signature.txt`.
  - HU-P2's announce re-type should carry the HashSeq root.
  - HU-P4's PetalSnapshot export must seal its archive (snapshot blobs then get `asset_hash`
    enforcement for free).
- **2026-10-13 R4/R5 generic asset model (D-5/D-17):**
  - `asset.content_hash` == `AssetEntry.asset_hash` == the chunk `blake3` == the iroh blob hash
    (§2, last bullet).
  - `FetchStrategy::fetch_by_hash` is the verified, transport-neutral primitive that the device
    payload's list of asset ids and bucket pull-through (HPB FR-5, "fetch-by-hash from peers on
    miss") will call.
  - P5 deliberately changes no tables.
- **hexon_p2p_bucket:** its open question 1 (should a directory-asset manifest reuse fe-format?) gets
  a default answer from P5. A directory asset is a sealed hexon whose `members.json` is the file
  list, verified the same way. Bucket FR-5's overlap with PAS-P3 is resolved by both using the same
  transport trait.
- **CDL-W4 / CDL-SPEC10:** there is no code overlap. CDL-R2's enrolment artifact is a different
  signed envelope (its fields `{principal, device key, enrolled_at}`). Do not reuse
  `fe_format::sign_manifest` for it unless the canonical-log float rule is re-checked. SPEC-10 could
  later observe `FetchStrategy` source demotions, but no hook is added now.
- **Role propagation:** nothing in P5 reads or writes `role`. fe-api install routes keep the Editor+
  fe-policy gate (`fe-api/src/terrain.rs:629-640`). Registry authority is the bearer token plus the
  publisher signature, which is independent of verse RBAC.
- **2026-10-12 decisions:**
  - "No consumers, breaking is cheap" justifies strict registry publish.
  - The self-hosted relay (D-23) is needed only for the relay-assisted variant of S5's loopback test,
    which rides §7 step 5.
  - Push-on-green applies at campaign end.

---

## 9. Assumptions (highest reversal cost first) and user decisions

**No blocking user decision.** Every open point has a default, listed here:

1. **A1:** PAS-P3's transport trait is free of iroh types and lives in `fe-runtime`. **(Critic:
   now guaranteed — P3P4 S2 was amended to put it there.)** *Reversal:* the
   P2P impl moves into fe-sync, which gains an fe-hexon dependency (moderate: one file and Cargo).
1b. **(Critic) Seeder rule (§3.5).** Sealed containers re-seed their signed chunks, sealed
   monolithic archives seed whole, and only legacy unsealed tilesets are re-bundled. *Reversal:*
   allow re-bundling of sealed monolithic archives with per-member verification against
   `members.json` (moderate).
2. **A2:** Unsealed (legacy) archives still install from **local** sources, flagged
   `verified: false`. Every network path and registry publish requires a seal. *Reversal:* make local
   installs strict too (cheap: one policy argument). The cost is that every existing gis-tile-etl or
   sample archive must be re-sealed first.
3. **A3:** Chunking happens only at publish time; seeders never re-chunk. *Reversal:* allow seeder
   chunking with seeder-signed tables. This is expensive, because it adds a second trust root.
4. **A4:** Publisher trust is the signed DID plus per-`hexon_id` continuity (TOFU) plus an optional
   caller pin. There is no global allowlist. *Reversal:* add an allowlist setting (cheap, additive).
5. **A5:** Source order is LAN peer → low-latency peer → remote peer → registry. *Reversal:* a
   `prefer_registry` flag (cheap).
6. **A6:** The registry URL is an `AppSettings` field with an env override, and the Hexon Manager gets
   a minimal URI field (S7b). *Reversal:* drop the UI and keep it env-only (cheap). Alternatively, the
   user's UX track could own a browse/search UI later.
7. **A7:** `blake3` goes into fe-format, and `fe-runtime` becomes an optional default feature.
   *Reversal:* move the hashing into the registry crate (moderate; it duplicates verification).
8. **A8:** The whole-file `/download` route stays, for legacy/unsealed packages and third-party tools.
   *Reversal:* restrict it to sealed packages (cheap).

---

## 10. Critic amendments (2026-10-13)

Build order: PAS-P3P4 → **PAS-P5** → CDL-W4 → CDL-SPEC10 → role propagation. Where P3P4 and P5
overlap, P3P4 owns the shared piece and P5 adapts (§3.5).

| # | Change | Class |
|---|---|---|
| C1 | §3.5 added. The C-P3 hedges are resolved to P3P4's landed names: trait in fe-runtime, `TilesetBundleIndex`, `bundle_for_tile`, `HexonArchiveIndex`. | cross-plan collision |
| C2 | S1 adds no `ChunkTable` type; S2 extends `HexonArchiveIndex` instead of adding `reader.rs`, and drops `chunk_for_tile`. | cross-plan collision (would have made two readers and two index types) |
| C3 | The `hexon_id` traversal fix moved **out** of S6 into P3P4 S1. P3P4 S4 already makes peer installs reachable, a phase before P5. | ordering bug |
| C4 | S7a rewritten as a host-verified, two-phase download (`ConfirmTilesetDownload`). The original "fe-ui driver verifies" contradicted P3P4 S4 (the sync thread drives) and P3P4's format-agnostic fe-sync. The missing test files were added. | cross-plan contract + missing tests |
| C5 | Seeder rule (§3.5) reconciles P5 A3 "seeders never re-chunk" with P3P4's deterministic seeder bundling. S5 implements it. | cross-plan contradiction |
| C6 | S3 serves `/chunk` and `/member` as octet-stream attachments with nosniff, and has a test. | security |
| C7 | The D-5/D-17 deferral row is re-classified: device-payload records are built by CDL-SPEC10 A3 in this campaign; RUNBOOK §4 is an assumption, not a user decision. | mis-justified deferral |
| C8 | Reviewer brief (S8) extended with the confirm gate, pin provenance and headers. | security review scope |
