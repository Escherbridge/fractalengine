# Implementation Plan: P2P Mycelium — Embedded DB + Asset Sharing

## Overview

This plan is divided into 6 phases, progressing from schema foundation through local blob store, bridge wiring, and P2P invite flow. Each phase ends with a verification checkpoint. Phases A-C (foundations + migration) can ship independently and deliver value on their own (cleaner DB, no base64 bloat). Phases D-F (replication) build on top.

- **Phase A**: Schema + blob store foundation (local-only, no iroh networking)
- **Phase B**: Import-flow refactor + Bevy asset reader wiring
- **Phase C**: Migration job + backward compat
- **Phase D**: iroh endpoint lifecycle + blob peer-fetch
- **Phase E**: SurrealDB ↔ iroh-docs bridge + per-Verse namespaces
- **Phase F**: Invite/join flow + gossip peer discovery + status UI

Estimated total: 34 tasks across 6 phases.

**Prerequisite:** Current GLTF import bug-fix track should land first (glb_stability_20260405) so the import flow is stable before we refactor it.

---

## Phase A: Schema + Blob Store Foundation

Goal: Add `content_hash` column, create the iroh-blobs filesystem store, expose a `BlobStore` abstraction. No networking yet.

Tasks:

- [ ] Task A.1: Add migration SQL constant `DEFINE_ASSET_CONTENT_HASH` in `fe-database/src/schema.rs` for `DEFINE FIELD IF NOT EXISTS content_hash ON TABLE asset TYPE option<string>` (TDD: write test that the constant contains the expected SQL; implement; refactor)
- [ ] Task A.2: Relax existing `DEFINE_ASSET` `data` field from required to `option<string>` so new rows can omit it (TDD: write test that asserts both variants of the schema apply without error on an in-memory SurrealKv; implement; refactor)
- [ ] Task A.3: Apply `DEFINE_ASSET_CONTENT_HASH` in `fe-database/src/rbac.rs::apply_schema` or equivalent migration-on-open path (TDD: write test that opens a fresh DB and verifies `content_hash` column exists; implement; refactor)
- [ ] Task A.4: Define `BlobStore` trait in new file `fe-sync/src/blob_store.rs` with methods `add_blob(bytes) -> Result<Hash>`, `get_blob_path(hash) -> Option<PathBuf>`, `has_blob(hash) -> bool`, `remove_blob(hash) -> Result<()>` (TDD: write tests against a mock impl; implement trait + mock; refactor)
- [ ] Task A.5: Implement `FsBlobStore` wrapping `iroh_blobs::store::fs::Store` in `fe-sync/src/blob_store.rs` rooted at `{dirs::data_local_dir()}/fractalengine/blobs/` (TDD: write test creating a store in tempdir, adding bytes, reading back by hash, asserting BLAKE3 round-trip; implement; refactor)
- [ ] Task A.6: Create a `BlobStoreHandle` resource (`Arc<dyn BlobStore + Send + Sync>`) and wire it into the DB thread spawning in `fe-database/src/lib.rs::spawn_db_thread` — the store handle is shared with the DB thread via channel construction (TDD: write integration test that spawns DB thread with mock blob store, sends import, asserts blob written; implement; refactor)
- [ ] Task A.7: Add `fe-sync` as dependency of `fe-database` and re-export `BlobStore` trait for use in handlers (TDD: cargo check succeeds; implement; refactor)
- [ ] Verification: `cargo test -p fe-sync -p fe-database`, `cargo clippy -- -D warnings`, `cargo fmt --check` [checkpoint marker]

---

## Phase B: Import Flow Refactor + Bevy AssetReader

Goal: Import writes to blob store + DB reference. Bevy loads from blob store via a custom `AssetReader`.

Tasks:

- [ ] Task B.1: Refactor `import_gltf_handler` in `fe-database/src/lib.rs` to: (a) compute BLAKE3 of the GLB bytes, (b) write to blob store via `BlobStore::add_blob`, (c) insert asset row with `content_hash = Some(hex)` and `data = None` (TDD: write test asserting the asset row has `content_hash` populated and `data` is null; blob exists in store; implement; refactor)
- [ ] Task B.2: Remove the redundant copy to `fractalengine/assets/imported/` directory from `import_gltf_handler` (TDD: write test asserting no file is created in `imported/` dir; implement; refactor)
- [ ] Task B.3: Update `DbResult::GltfImported::asset_path` field to use `blob://{hex_hash}.glb` format (TDD: write test matching the new format; implement in `fe-runtime/src/messages.rs`; refactor — this IS a breaking schema change to the message, all senders and receivers update together)
- [ ] Task B.4: Implement `BlobAssetReader` struct in `fe-sync/src/bevy_asset_reader.rs` implementing Bevy's `AssetReader` trait; parses paths of form `blob://{hex}.glb` and delegates to the `BlobStore` (TDD: write test that a valid `blob://` path returns bytes; an unknown hash returns `NotFound`; implement; refactor)
- [ ] Task B.5: Register `BlobAssetReader` as a custom asset source named `"blob"` in `ViewportPlugin` or a new `SyncPlugin` before `DefaultPlugins` are added (TDD: write integration test spawning a Bevy app with the reader registered and asserting a `blob://` load succeeds; implement; refactor)
- [ ] Task B.6: Update the GLTF spawn path in `fe-ui/src/plugin.rs::apply_db_results_to_hierarchy` to load via `asset_server.load("blob://{hex}.glb#Scene0")` instead of `"imported/{id}.glb#Scene0"` (TDD: write test that the spawn event carries the new path format; implement; refactor)
- [ ] Task B.7: Update hierarchy load in `load_hierarchy_handler` to emit `asset_path: Some("blob://{hex}.glb")` when a node's asset has a `content_hash` (TDD: write test loading a DB with a hash-ref asset, assert the asset_path format; implement; refactor)
- [ ] Task B.8: Update seed data (`seed_default_data` + `seed_assets`) to write to blob store using BLAKE3 hash instead of ULID-as-path (TDD: write test that seed populates blob store with expected hashes; implement; refactor)
- [ ] Verification: `cargo test -p fe-database -p fe-ui -p fe-sync`, manual test: fresh DB, import a GLB, model appears; restart app, model still appears. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase C: Migration Job + Backward Compat

Goal: On DB open, migrate any existing rows with `data` to the blob store, setting `content_hash`. Deployments from before this track upgrade cleanly.

Tasks:

- [ ] Task C.1: Implement `migrate_base64_assets_to_blob_store` async fn in `fe-database/src/lib.rs` that queries `SELECT * FROM asset WHERE content_hash IS NONE AND data IS NOT NONE`, decodes each base64 payload, writes to blob store, updates `content_hash` (TDD: write test seeding a row with base64 data, running migration, asserting blob exists in store and row has `content_hash`; implement; refactor)
- [ ] Task C.2: Call the migration on DB thread startup after schema apply, before serving commands (TDD: write integration test: create DB with legacy rows, spawn DB thread, assert rows have `content_hash` post-startup; implement; refactor)
- [ ] Task C.3: Make migration idempotent: second run is a no-op (TDD: write test calling migration twice, asserting no duplicate blobs; implement; refactor)
- [ ] Task C.4: Add a `migration_status` log line at DB startup: `"Migrated N base64 assets to blob store (K bytes)"` (TDD: write test capturing the log; implement; refactor)
- [ ] Task C.5: Update `BlobAssetReader` to fall through to the `data` field if `content_hash` lookup fails, for safety during rolling migrations (TDD: write test where blob is NOT in store but `data` field IS populated — reader should return the base64-decoded bytes; implement; refactor; NOTE: this requires the reader to query DB, so actually better to keep reader store-only and rely on the eager migration)
- [ ] Task C.6: Document the migration in `docs/migration_notes.md` — stable path for users to understand what changed (TDD: none — write doc; verify with human review; refactor)
- [ ] Verification: `cargo test -p fe-database`, manual test: copy an old DB file with base64 assets into `data/`, launch app, confirm migration log appears, confirm all assets still render. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase D: iroh Endpoint + Blob Peer-Fetch

Goal: Spin up an iroh `Endpoint` using the user's `NodeKeypair`; add lazy peer-fetch for missing blobs.

Tasks:

- [ ] Task D.1: Add iroh workspace deps to `fe-sync/Cargo.toml`: `iroh`, `iroh-blobs`, `iroh-gossip`, `iroh-docs` (all workspace = true) (TDD: cargo check; implement; refactor)
- [ ] Task D.2: Define `SyncEndpoint` resource in `fe-sync/src/endpoint.rs` wrapping `iroh::Endpoint` with constructor `new(secret_key: SecretKey) -> Result<Self>` that uses default n0 relays (TDD: write test creating an endpoint with a fixed keypair, asserting non-blocking construction and non-empty `node_addr()`; implement; refactor)
- [ ] Task D.3: Wire `fe-identity::NodeKeypair` → iroh `SecretKey` conversion (both are ed25519); expose via `NodeKeypair::to_iroh_secret()` (TDD: write test that round-trips a keypair via iroh SecretKey and verifies the public key matches; implement; refactor)
- [ ] Task D.4: Create `spawn_sync_thread` function in `fe-sync/src/lib.rs` analogous to `spawn_db_thread` — owns its own tokio runtime, takes channels for `SyncCommand`/`SyncEvent`, constructs the endpoint, enters a command loop (TDD: write integration test spawning the sync thread and sending a no-op command; implement; refactor)
- [ ] Task D.5: Define `SyncCommand::FetchBlob { hash, verse_id }` and `SyncEvent::BlobReady { hash }` variants (TDD: write test for message round-trip; implement in `fe-sync/src/messages.rs`; refactor)
- [ ] Task D.6: Implement `FetchBlob` handler: query known Verse peers (static list for now; full discovery in Phase F), attempt iroh-blobs fetch from each, write bytes to blob store on success, emit `BlobReady` (TDD: write test with a mock peer-provider fetching a fake blob; implement; refactor; acceptance: 10s timeout, 3 attempts max)
- [ ] Task D.7: Add `BlobAssetReader` miss-path: on `AssetReader::read` returning `NotFound`, send `SyncCommand::FetchBlob` then return `NotFound` (v1 — no retry integration yet; Bevy will show placeholder until user retries) (TDD: write test asserting `FetchBlob` is sent on miss; implement; refactor)
- [ ] Task D.8: Wire `SyncCommandSender` as a Bevy resource so the asset reader can send commands (TDD: write test; implement; refactor)
- [ ] Task D.9: Add `SyncStatus` resource with `online: bool, node_addr: Option<NodeAddr>` updated from sync thread heartbeats (TDD: write test; implement; refactor)
- [ ] Verification: `cargo test -p fe-sync`, manual test: launch app, verify sync thread starts without blocking UI, `SyncStatus::online = true`. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase E: SurrealDB ↔ iroh-docs Bridge + Per-Verse Namespaces

Goal: Replicate `verse`/`fractal`/`petal`/`node`/`asset` rows via iroh-docs. Each Verse has its own namespace.

Tasks:

- [ ] Task E.1: Add `namespace_id: string` column to `verse` table schema (TDD: write test asserting column exists; implement in `fe-database/src/schema.rs`; refactor)
- [ ] Task E.2: On `DbCommand::CreateVerse`, generate a new iroh-docs `NamespaceSecret`, store `NamespaceId` in DB and `NamespaceSecret` via `keyring` crate under key `verse:{verse_id}:namespace_secret` (TDD: write test that after verse creation, the namespace secret can be retrieved and matches the stored namespace_id; implement; refactor)
- [ ] Task E.3: Define `VerseReplicator` trait in `fe-sync/src/replicator.rs` with `open(namespace_id, secret) -> Result<Self>`, `write_row(table, id, data)`, `subscribe() -> BoxStream<RowChange>`, `close()` (TDD: write tests against mock impl; implement trait + mock; refactor)
- [ ] Task E.4: Implement `IrohDocsReplicator` wrapping `iroh_docs::Replica` implementing the trait (TDD: write integration test with two in-process replicas, write on one, observe via subscribe on the other, assert eventual consistency; implement; refactor)
- [ ] Task E.5: Add `OpenVerseReplica { verse_id }` / `CloseVerseReplica { verse_id }` to `SyncCommand`; sync thread maintains a `HashMap<verse_id, IrohDocsReplicator>` of open replicas (TDD: write test for open/close lifecycle; implement; refactor)
- [ ] Task E.6: On any row write in `verse`/`fractal`/`petal`/`node`/`asset` tables, after the DB write succeeds, serialize the row to JSON, hash it, write bytes to blob store, send `SyncCommand::WriteRowEntry { verse_id, table, record_id, content_hash }` to the sync thread (TDD: write integration test that a CreateNode command results in a SyncCommand::WriteRowEntry being sent; implement via a small helper `replicate_row!` macro; refactor)
- [ ] Task E.7: On iroh-docs subscribe stream delivering a remote entry, fetch the referenced blob (content-hash), decode JSON, apply as SurrealDB upsert bypassing the normal CREATE path (use `UPDATE {table}:{id} MERGE {...}`) (TDD: write test with mock DB that asserts upsert is called with correct content; implement; refactor)
- [ ] Task E.8: Loop prevention: tag each iroh-docs entry with `author_id`; on subscribe, drop entries where `author_id == self.author_id` (TDD: write test with a locally-written entry, assert it is NOT re-applied; implement; refactor)
- [ ] Task E.9: Tiebreaker for equal-timestamp LWW: compare author public-key byte order as secondary sort key, deterministic across peers (TDD: write test with two concurrent writes at same timestamp, verify deterministic resolution; implement; refactor)
- [ ] Task E.10: On Verse navigation (active_verse_id change), automatically send `OpenVerseReplica` for the new verse and `CloseVerseReplica` for the previous (TDD: write Bevy system test; implement; refactor)
- [ ] Verification: `cargo test -p fe-sync -p fe-database`, manual test: run two instances locally, manually copy namespace_secret from instance A to instance B, observe that node creation on A appears on B within 2s. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase F: Invite/Join Flow + Gossip Discovery + Status UI

Goal: Turn the manual-secret-copy flow from Phase E into a proper invite ticket flow, add gossip peer discovery, and surface sync state in the UI.

Tasks:

- [ ] Task F.1: Define `VerseInvite` struct with `namespace_id, namespace_secret_opt, creator_node_addr, verse_name, expiry, signature` in `fe-sync/src/invite.rs` (TDD: write serialization round-trip test; implement with ed25519 signature over the canonical bytes; refactor)
- [ ] Task F.2: Implement `DbCommand::GenerateVerseInvite { verse_id, include_write_cap: bool, expiry_hours: u32 }` → returns a URL-safe base64 invite string (TDD: write test generating an invite, parsing it back, verifying the signature; implement; refactor)
- [ ] Task F.3: Implement `DbCommand::JoinVerseByInvite { invite_string }` — verifies signature, opens iroh-docs namespace in read or read-write mode based on `include_write_cap`, kicks off initial sync, inserts local `verse_member` row (TDD: write integration test simulating two peers with real replicas, generate invite on A, consume on B, assert B sees A's nodes; implement; refactor)
- [ ] Task F.4: Add "Generate Invite" button to Verse context menu in sidebar, displays a modal with the copyable string + QR code area (TDD: write UI test that clicking the button opens the modal; implement in `fe-ui/src/panels.rs`; refactor; QR rendering can be P2)
- [ ] Task F.5: Add "Join Verse" button to dashboard/toolbar, opens a text input for pasting invite strings (TDD: write UI test; implement; refactor)
- [ ] Task F.6: Gossip topic per Verse: on `OpenVerseReplica`, also join gossip topic derived from `verse_id` via `iroh_gossip::Topic::from_bytes(blake3("fractalengine:verse:{id}"))` (TDD: write test with two peers joining same topic; implement; refactor)
- [ ] Task F.7: Broadcast presence on topic join: `PresenceMessage { node_addr, verse_id, timestamp }`; maintain a `VersePeers` resource (TDD: write test; implement; refactor)
- [ ] Task F.8: Update `FetchBlob` handler to query `VersePeers` for candidates instead of the static list placeholder from Phase D (TDD: write test with mock VersePeers; implement; refactor)
- [ ] Task F.9: Add sync status to the status bar in `fe-ui/src/panels.rs::status_bar` showing `● Online | N peers | Verse: {name}` or `○ Offline | Local only` (TDD: write UI snapshot test; implement; refactor)
- [ ] Task F.10: Add peer debug panel (click status bar to open) listing peer node IDs + last-seen times from `VersePeers` (TDD: write UI test; implement; refactor)
- [ ] Verification: `cargo test`, full manual test: two machines on same LAN, user A generates invite, user B joins, user A imports a GLB, user B sees the node + model within 10s. `cargo clippy -- -D warnings` [final checkpoint]

---

## Verification Strategy

Every phase ends with both automated tests AND a concrete manual test scenario. The manual tests are intentionally cumulative:

| Phase | Manual Test |
|---|---|
| A | `cargo test` passes; schema applies to fresh DB |
| B | Import GLB → close → reopen → model persists |
| C | Legacy DB file opens and migrates without error |
| D | Sync thread starts; `SyncStatus::online` visible in logs |
| E | Two local instances sync nodes via manually-copied namespace secret |
| F | Two networked peers sync via invite ticket |

## Risk Register

- **R1: iroh 0.35 → 1.0 breaking changes** (High prob, Medium impact): mitigated by `VerseReplicator`/`BlobStore` trait abstractions
- **R2: iroh-docs deprecated for Willow** (Medium prob, Medium impact): same mitigation
- **R3: SurrealDB schema drift between peers** (Medium prob, High impact): future phase G — for v1, all peers must run same version
- **R4: First-time sync too slow over slow networks** (Medium prob, Medium impact): add progress UI in F, measure in verification
- **R5: Key storage via keyring crate platform inconsistency** (Low prob, High impact): test on all 3 platforms in F verification
- **R6: Blob GC leaks or over-deletes** (Low prob, Medium impact): GC is out of scope; track separately

## Dependencies on Other Tracks

- **glb_stability_20260405** (sibling track, should land first) — cleans up the current GLTF rendering bugs before we refactor the import path
- **scene_graph_bridge_20260402** (existing track, can land in parallel) — provides Bevy entity ↔ DB node sync which is orthogonal to replication

## Estimated Effort

- Phase A: ~1 day (isolated, low-risk)
- Phase B: ~1.5 days (touches multiple crates)
- Phase C: ~0.5 days (migration, small)
- Phase D: ~2 days (iroh endpoint + fetch worker)
- Phase E: ~3 days (bridge is the tricky part)
- Phase F: ~2 days (UX + polish)

**Total: ~10 days of focused implementation.** Phases A-C deliver immediate value (DB file shrinks, no more base64 bloat) and can ship independently.
