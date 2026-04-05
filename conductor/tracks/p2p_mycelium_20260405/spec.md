# Specification: P2P Mycelium — Embedded DB + Asset Sharing

## Overview

This track migrates fractalengine from a single-peer embedded database with base64-encoded GLB assets stored inside SurrealDB rows to a hybrid architecture where:

1. **Binary assets** live in a content-addressed iroh-blobs store (BLAKE3-indexed)
2. **SurrealDB rows** reference blobs by hash, never by inline bytes
3. **Database state** replicates peer-to-peer per-Verse via iroh-docs (range-based set reconciliation)
4. **Peer identity** unifies ed25519 keys across iroh node ID, iroh-docs author, DID, and Verse membership

This is the mycelial network layer: the underground sync substrate that lets Verses, Fractals, Petals, and Nodes propagate between peers without a central server. The architecture follows the seven local-first ideals (Kleppmann/Ink & Switch 2019): fast local response, multi-device, offline-first, collaborative, long-lived, private by default, user-owned.

## Background

### What exists

- **Existing deps (in `Cargo.toml` workspace):** iroh-blobs 0.35, iroh-gossip 0.35, iroh-docs 0.35, libp2p 0.56, ed25519-dalek 2.2, surrealdb 3.0 (kv-surrealkv), blake3 1
- **Identity foundation (`fe-identity`):** `NodeKeypair` (ed25519) with `to_did_key()`, sign/verify already implemented
- **Asset ingestion (`fe-renderer/src/ingester.rs`):** `GltfIngester` validates GLB magic, computes BLAKE3 `AssetId`, writes to `~/.cache/fractalengine/assets/{hash}` — content-addressing foundation already exists
- **Asset loading (`fe-renderer/src/loader.rs`):** `load_to_bevy(AssetId)` resolves cache path → Bevy `Handle<Gltf>`, falls back to `placeholder.glb`
- **Current DB schema (`fe-database/src/schema.rs`):** `asset` table has `data: string` field storing base64 GLB bytes directly; `node` table has `asset_id: option<string>` referencing it
- **Current import flow (`fe-database/src/lib.rs::import_gltf_handler`):** reads GLB file → base64-encodes → writes to `asset.data` + copies file to `fractalengine/assets/imported/{asset_id}.glb` for Bevy loading
- **Network crate (`fe-network`):** exists with libp2p deps but is not yet wired to asset or DB flows
- **Verse membership:** `verse_member` table with `peer_did`, `invite_sig` already in schema

### What is missing

- No iroh endpoint running — iroh-blobs/docs/gossip crates are in `Cargo.toml` but not instantiated
- No content-hash column in `asset` table — only ULID `asset_id` and base64 `data`
- No iroh-blobs store — GLBs live in two redundant places (base64 in DB + loose files in `fractalengine/assets/imported/`)
- No CRDT bridge between SurrealDB writes and iroh-docs entries
- No Verse invite flow — `verse_member` rows are created, but there's no `DocTicket` generation/consumption path
- No per-Verse iroh-docs namespace provisioning
- No peer discovery within a Verse (gossip topic unused)
- No lazy blob fetch on peer miss — loader falls back to placeholder but never attempts a pull
- No migration path from base64-in-DB to blob-ref-in-DB

### External research (see `research/p2p-mycelium/findings.md`)

- iroh-blobs 0.35 is officially the last production-quality version per maintainers; 0.93-0.99 are dev versions with breaking changes
- iroh-docs uses range-based set reconciliation (1D), sibling to Willow protocol (3D); Willow may succeed iroh-docs post-1.0
- SurrealDB 3.0 has NO built-in multi-peer replication — the bridge is our responsibility
- Comparable system: Jazz.tools `CoMap` ≈ our `Node`, `FileStream` ≈ our `Asset`
- Content-addressed stores have no true delete semantic — tombstones only

## Functional Requirements

### FR-1: Asset Schema Migration (Hash-Reference Pattern)

**Description:** Replace the base64 `data` field on the `asset` table with a `content_hash` reference, while maintaining backward compatibility during migration.

**Acceptance Criteria:**
- AC-1.1: New field `content_hash: option<string>` is added to the `asset` table schema; stores the 64-char BLAKE3 hex hash
- AC-1.2: Existing `data` field is retained as `option<string>` during the transition (previously required)
- AC-1.3: A migration job runs once on DB open: for each `asset` row where `data` is present and `content_hash` is NULL, compute BLAKE3 of the base64-decoded bytes, write bytes to the blob store, set `content_hash`
- AC-1.4: Migration is idempotent — re-running is a no-op when all rows already have `content_hash`
- AC-1.5: After migration completes, new imports write ONLY to the blob store and set `content_hash`; they do NOT populate `data`
- AC-1.6: A future Phase F compaction task can drop the `data` column once all deployments have migrated (out of scope for this track)

**Priority:** P0

### FR-2: Local Blob Store (iroh-blobs `fs` backend)

**Description:** Create a persistent iroh-blobs filesystem store at a stable path under the user's data directory. This replaces the redundant `fractalengine/assets/imported/` directory as the canonical blob location.

**Acceptance Criteria:**
- AC-2.1: A `BlobStore` resource wraps `iroh_blobs::store::fs::Store` rooted at `{dirs::data_local_dir()}/fractalengine/blobs/`
- AC-2.2: On startup, the store is opened or created; errors abort with a clear message
- AC-2.3: A public function `add_blob(bytes: &[u8]) -> Result<Hash>` writes bytes to the store and returns the BLAKE3 hash
- AC-2.4: A public function `get_blob_path(hash: &Hash) -> Option<PathBuf>` returns the local filesystem path if the blob exists, else `None`
- AC-2.5: A public function `has_blob(hash: &Hash) -> bool` returns quickly without reading bytes
- AC-2.6: The store is owned by the DB thread (not Bevy) to avoid cross-thread Handle issues
- AC-2.7: Store directory is created with 0700 permissions on Unix, restricted on Windows

**Priority:** P0

### FR-3: Bevy AssetServer Integration

**Description:** Bevy's `AssetServer` must load GLBs from the iroh-blobs store given a content hash, with lazy peer-fetch fallback.

**Acceptance Criteria:**
- AC-3.1: A new `BlobAssetReader` implements Bevy's `AssetReader` trait; asset paths of the form `blob://{hex_hash}.glb` resolve via the blob store
- AC-3.2: On cache hit: returns bytes directly from the store
- AC-3.3: On cache miss: enqueues a fetch request (returned error `NotFound` for v1; async peer fetch in FR-6)
- AC-3.4: Loader reader is registered in `ViewportPlugin` or equivalent startup before any `asset_server.load()` call
- AC-3.5: Fall-through: if `blob://{hash}` fails and a `placeholder.glb` is registered, load that instead (existing behavior preserved)

**Priority:** P0

### FR-4: Import Flow Refactor

**Description:** The GLTF import flow writes to the blob store and sets `content_hash`; it no longer writes base64 into the DB.

**Acceptance Criteria:**
- AC-4.1: `import_gltf_handler` reads the picked file, computes BLAKE3, calls `add_blob()` on the blob store
- AC-4.2: The `asset` row is created with `content_hash` set; `data` is NULL
- AC-4.3: The `node` row is created with `asset_id` referencing the asset as before
- AC-4.4: The returned `DbResult::GltfImported` carries `asset_path: "blob://{hex}.glb"` (updated from current `asset_path: "imported/{id}.glb"`)
- AC-4.5: The legacy `fractalengine/assets/imported/` copy step is REMOVED
- AC-4.6: Existing seed data (from `seed_default_data`) is updated to use the same pattern

**Priority:** P0

### FR-5: iroh Endpoint Lifecycle

**Description:** Create a single iroh `Endpoint` shared across iroh-blobs, iroh-docs, iroh-gossip. The endpoint runs for the lifetime of the app.

**Acceptance Criteria:**
- AC-5.1: A new `fe-sync` crate (or module under existing `fe-sync`) owns the `Endpoint` and exposes it as a `SyncEndpoint` resource
- AC-5.2: The endpoint is constructed with the user's `NodeKeypair` as the iroh secret key (enabling cross-protocol identity unification)
- AC-5.3: The endpoint uses default iroh relay servers (n0's public relays) for NAT traversal
- AC-5.4: Endpoint startup is non-blocking: on failure, log warning and proceed in offline mode (local writes still work)
- AC-5.5: A `SyncStatus` resource exposes `{ online: bool, node_addr: Option<NodeAddr>, peer_count: usize }` for the UI status bar
- AC-5.6: Endpoint shutdown is graceful on app exit (flush pending sync, close connections)

**Priority:** P0

### FR-6: Lazy Peer-Fetch on Blob Miss

**Description:** When a local blob lookup fails, attempt to fetch the blob from any known peer in the current Verse.

**Acceptance Criteria:**
- AC-6.1: On `get_blob_path()` miss, the BlobAssetReader enqueues a `FetchBlobRequest { hash, verse_id }` on a crossbeam channel
- AC-6.2: A dedicated async worker (tokio task under the iroh endpoint) drains the queue and attempts fetches
- AC-6.3: The worker queries the current Verse's gossip topic for peers known to have the blob
- AC-6.4: On fetch success: bytes are written to the local blob store, a `BlobReady { hash }` event fires
- AC-6.5: Bevy's asset system retries the load on `BlobReady` (via a `PendingAsset` component or similar)
- AC-6.6: Fetch timeout: 10 seconds per attempt; max 3 attempts; after failure, the node renders as placeholder and a one-line log records the missing hash
- AC-6.7: Concurrent requests for the same hash are deduplicated (request coalescing)

**Priority:** P1

### FR-7: Per-Verse iroh-docs Namespace

**Description:** Each Verse owns one iroh-docs `Replica` whose `NamespaceId` is derived from or stored alongside the `verse_id`.

**Acceptance Criteria:**
- AC-7.1: On Verse creation, a new iroh-docs `NamespaceSecret` is generated; the corresponding `NamespaceId` is stored in the `verse` table (new column `namespace_id: string`)
- AC-7.2: The creator (first verse member) also writes the `NamespaceSecret` to a local secure-keys store (reuse keyring crate from workspace deps) under a key like `verse:{verse_id}:namespace_secret`
- AC-7.3: A `VerseReplicator` trait abstracts iroh-docs specifics: `write_row`, `subscribe`, `sync_with_peer`, `create_namespace`, `open_namespace`
- AC-7.4: Per-Verse replicas are lazily opened when the user navigates into a Verse; closed when leaving
- AC-7.5: Write-access is controlled by possession of `NamespaceSecret`; read-access is controlled by possession of `NamespaceId` (public key) + peer in gossip topic

**Priority:** P1

### FR-8: SurrealDB ↔ iroh-docs Bridge

**Description:** Every write to a replicated table in SurrealDB also writes an iroh-docs entry. Every incoming iroh-docs entry applies to SurrealDB.

**Acceptance Criteria:**
- AC-8.1: Replicated tables in v1: `verse`, `fractal`, `petal`, `node`, `asset`. NOT replicated: `role`, `verse_member`, `op_log` (policy/audit, handled differently)
- AC-8.2: On every CREATE/UPDATE/DELETE in replicated tables, the DB handler computes a row JSON blob, writes it to the blob store, and writes an iroh-docs entry with `key = "{table}/{record_id}"`, `value = content_hash`
- AC-8.3: A subscriber task listens for `Replica::subscribe()` events; on incoming entry, fetches the referenced blob, decodes JSON, applies as upsert to SurrealDB
- AC-8.4: Conflict resolution: last-write-wins by iroh-docs entry timestamp; if timestamps are equal, author public-key byte order breaks ties (deterministic)
- AC-8.5: Loop prevention: entries written by the local author are not re-applied to SurrealDB (check `entry.author() == self.author_id`)
- AC-8.6: The bridge runs on the DB thread, not the Bevy main thread, to avoid blocking UI

**Priority:** P1

### FR-9: Verse Invite & Join Flow

**Description:** A Verse owner can generate a shareable ticket; a recipient can join the Verse by consuming the ticket.

**Acceptance Criteria:**
- AC-9.1: `DbCommand::GenerateVerseInvite { verse_id }` produces a `VerseInvite` containing: namespace public key, namespace secret (write cap, optional — read-only invites possible), creator node address, verse name, expiry
- AC-9.2: The invite is signed by the creator's `NodeKeypair` — recipients verify the signature matches a known member key
- AC-9.3: The invite serializes to a URL-safe base64 string for sharing
- AC-9.4: `DbCommand::JoinVerseByInvite { invite_string }` parses + verifies the invite, opens the iroh-docs namespace, syncs to populate the local `verse`/`fractal`/`petal`/`node` tables, adds a `verse_member` row
- AC-9.5: UI exposes a "Generate Invite" action on a Verse and a "Join Verse" action on the dashboard
- AC-9.6: Joining a read-only invite: recipient can observe but not write; new entries they attempt to create locally are blocked at the DbCommand layer

**Priority:** P2

### FR-10: Gossip-Based Peer Discovery Within Verse

**Description:** Members of a Verse discover each other via iroh-gossip, enabling the peer-fetch mechanism in FR-6.

**Acceptance Criteria:**
- AC-10.1: When a Verse is active, the endpoint joins a gossip topic derived from `verse_id` (e.g., `Topic::from_bytes(blake3("fractalengine:verse:" + verse_id))`)
- AC-10.2: Peers broadcast presence messages on topic join: `{ node_addr, available_hashes_bloom_filter }`
- AC-10.3: A `VersePeers` resource tracks `{verse_id → Set<NodeAddr>}` for the active Verse
- AC-10.4: FR-6's blob-fetch worker queries `VersePeers` for candidates before attempting a fetch
- AC-10.5: Peers who leave the topic are removed from `VersePeers` after a 60-second heartbeat timeout

**Priority:** P2

### FR-11: Tombstone-Based Deletion Semantics

**Description:** Deletions in a content-addressed world are tombstones. Record them explicitly and honor them locally.

**Acceptance Criteria:**
- AC-11.1: Deleting an `asset` row writes an iroh-docs entry with the same key but value pointing to a "tombstone sentinel" blob (fixed hash, content `{"tombstoned": true}`)
- AC-11.2: Tombstoned rows are hidden from all UI queries (hierarchy load filters them)
- AC-11.3: Blob store GC: blobs unreferenced by any non-tombstoned row are eligible for eviction (actual GC is P3)
- AC-11.4: Documented limitation: tombstones do NOT guarantee bytes are gone from other peers' local stores

**Priority:** P2

### FR-12: Status UI for Sync State

**Description:** The status bar shows current sync state so users understand when they're online vs offline and how many peers are reachable.

**Acceptance Criteria:**
- AC-12.1: Status bar displays: `● Online | 3 peers | Verse: Genesis` when connected
- AC-12.2: Status bar displays: `○ Offline | Local only` when endpoint is down
- AC-12.3: Clicking the status bar opens a small debug panel listing peer node IDs + last-seen times
- AC-12.4: A sync progress indicator shows during initial join (e.g., "Syncing 47/120 nodes...")

**Priority:** P3

## Non-Functional Requirements

### NFR-1: iroh Version Stability Isolation
All direct iroh API usage MUST be behind a trait (`VerseReplicator`, `BlobStore`, `VersePeers`) so that the iroh 0.35 → 1.0 upgrade (or potential migration to Willow) can be done in one crate without touching the rest of the codebase.

### NFR-2: Thread Safety
- Blob store and iroh endpoint are owned by the DB/runtime thread
- Bevy-side code only holds `Handle<Gltf>` and never calls iroh APIs directly
- Cross-thread communication exclusively via existing crossbeam channels

### NFR-3: Migration Safety
- All schema changes are additive (new columns, new tables); no column drops in v1
- Migration jobs are idempotent
- DB open MUST succeed on existing `data/fractalengine.db` files without destructive steps

### NFR-4: Performance Budget
- Local asset load: < 50ms from DB query to Bevy `Handle<Gltf>` creation
- DB-row-to-iroh-docs-entry bridge: < 10ms overhead per write (async, non-blocking)
- First-time Verse join (100 nodes, 50MB assets): < 30s over 100Mbps connection

### NFR-5: Privacy Defaults
- Verses are private by default (no discovery outside invite flow)
- No blob content is served to peers outside the Verse gossip topic
- Ed25519 keys are stored in the OS keyring, not in the DB

## Out of Scope

- **Private Petals within public Verses** (requires per-Petal capability, tracked separately)
- **Mobile/WASM targets** (iroh browser support is on roadmap but not v1)
- **Public Verse discovery / global DHT** (this is deliberately NOT Mastodon)
- **Blob store GC** (P3, tracked separately)
- **Willow protocol migration** (blocked on upstream iroh-willow stabilization)
- **Semantic merge of structural edits** (we use LWW; richer merges are future work)
- **GDPR "right to erasure" compliance** (documented limitation, not a feature)

## Success Criteria

- A user can import a GLB, close the app, reopen it, and see the model persisted and rendered
- Two peers running on the same LAN can share a Verse invite, and nodes created on peer A appear on peer B within 5 seconds of an active connection
- Existing `data/fractalengine.db` files from before this track open without data loss
- DB file size reduction of > 90% for an app with 100MB of imported assets after migration
- All iroh usage is contained to `fe-sync` crate; removing iroh from other crates compiles clean
