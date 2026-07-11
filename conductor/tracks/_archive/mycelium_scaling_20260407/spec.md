# Specification: Mycelium Scaling — Backpressure, Streaming & Bounded Growth

## Overview

This track hardens the P2P Mycelium architecture (delivered in `p2p_mycelium_20260405`) for production traffic by fixing silent data loss, blocking I/O, unbounded memory growth, and missing backpressure. These issues are acceptable as stubs in Phase D/E but become critical once Phase F activates real peer networking.

The work is informed by:
- **Code review**: 3 CRITICAL, 5 HIGH, 5 MEDIUM issues (2026-04-07)
- **Architecture analysis**: 11 scaling/latency/bandwidth limitations identified
- **Deep research**: iroh-docs LWW semantics, BLAKE3 streaming, PlumTree gossip, Bevy asset streaming patterns

## Background

### Current State

The P2P Mycelium implementation (Phases A-F) is functionally complete:
- 212 tests pass across the workspace
- 7 functional test scenarios pass (including 3 two-peer scenarios)
- Blob store, sync thread, replicator, invite flow, and identity all work

### What Breaks at Scale

1. **Silent data loss**: `.ok()` on channel sends drops replication events with zero observability
2. **Head-of-line blocking**: Synchronous I/O in async contexts stalls the entire sync loop
3. **Memory leaks**: Unbounded HashMaps in VersePeers and MockVerseReplicator grow without limit
4. **RAM explosion**: Full blob loaded into `Vec<u8>` for every asset load (50 assets × 20MB = 1GB)
5. **Redundant I/O**: WriteRowEntry re-reads and re-hashes blobs that are already content-addressed
6. **No disk quota**: Blob store grows without limit, no GC for unreferenced blobs

### Research Findings

| Topic | Key Insight | Source |
|-------|-------------|--------|
| iroh-docs | LWW-per-key with range-based set reconciliation, NOT general CRDT | iroh docs |
| iroh-blobs | BLAKE3 tree structure enables streaming chunk verification (bao format) | docs.rs/iroh-blobs |
| Gossip | iroh-gossip uses basic epidemic broadcast, no PlumTree optimization yet | iroh source |
| Bevy Assets | `AssetReader::read()` returns `dyn AsyncRead` — streaming is supported, VecReader is unnecessary | Bevy 0.18 API |
| SurrealDB | Embedded mode has no P2P replication; use as local projection of CRDT state | SurrealDB docs |
| Blob Store | `spawn_blocking` for file I/O + streaming BLAKE3 (`Hasher::update`) for incremental hashing | tokio patterns |

## Phases

### Phase 1: Eliminate Silent Data Loss (CRITICAL)

Fix every `.ok()` on channel sends in the replication hot path. This is the minimum viable fix — without it, the system silently diverges under load.

**Files:**
- `fe-database/src/lib.rs` — `replicate_row()` function
- `fractalengine/src/main.rs` — replication bridge thread + blob miss callback
- `fe-sync/src/sync_thread.rs` — `evt_tx.send()` calls
- `fe-sync/src/replicator.rs` — subscriber retention logic

**Changes:**
1. `replicate_row()`: Replace `.ok()` with `try_send()` + `tracing::warn!` on failure. Add a dropped-events counter metric.
2. Bridge thread (`main.rs:69-78`): Replace `.ok()` with blocking `send()` — the bridge thread has no other work, so blocking is correct backpressure.
3. Blob miss callback (`main.rs:88-94`): Replace `.ok()` with `try_send()` + `tracing::warn!`. Add dedup set so the same hash isn't requested twice.
4. Sync thread lifecycle events (`Started`, `Stopped`): Use blocking `send()` — these are rare and must not be lost.
5. `BlobReady` events: Use `try_send()` + `tracing::debug!` on drop — high-frequency, acceptable to drop with logging.
6. `MockVerseReplicator::write_row` subscriber retention: Separate "channel full" (retain, skip event) from "channel closed" (remove). Log on skip.
7. `DbResult::Started` send in DB thread: Use blocking `send()` — startup event must not be lost.

**Tests:**
- Unit test: fill a bounded(1) channel, verify `replicate_row` logs a warning instead of silently dropping
- Unit test: verify bridge thread blocks when sync channel is full (does not drop)
- Integration test: burst 500 writes, verify all reach the sync thread (no silent drops)

**Exit criteria:** `cargo test --workspace` passes. `grep -r '\.ok()' fe-sync/src/ fractalengine/src/main.rs fe-database/src/lib.rs` returns zero hits on channel sends in the replication path. Every send has either blocking semantics or explicit logging on failure.

---

### Phase 2: Streaming Asset Loading (CRITICAL→HIGH)

Replace `VecReader` (full blob in RAM) with a streaming `FileReader` in `BlobAssetReader`. Fix blocking `std::fs::read` in async contexts.

**Files:**
- `fe-runtime/src/bevy_blob_reader.rs` — `BlobAssetReader::read()`
- `fe-sync/src/sync_thread.rs` — `handle_write_row_entry()`

**Changes:**
1. `BlobAssetReader::read()`: Replace `std::fs::read()` + `VecReader` with `tokio::fs::File::open()` → wrap in Bevy-compatible `AsyncRead` adapter. This eliminates the full-blob-in-RAM bottleneck.
2. `handle_write_row_entry()`: Wrap `std::fs::read()` in `tokio::task::spawn_blocking()` to avoid blocking the async runtime. Add a max blob size check (e.g., 512MB) before reading.
3. `FsBlobStore::add_blob()`: Wrap synchronous file I/O in `spawn_blocking` when called from async contexts (or document that it must only be called from blocking threads).

**Tests:**
- Unit test: verify `BlobAssetReader::read()` returns a reader without loading full file into heap (mock a 100MB file, check peak RSS stays under 10MB)
- Unit test: verify `handle_write_row_entry` doesn't block the Tokio runtime (use `tokio::time::timeout` around a concurrent task)

**Exit criteria:** `BlobAssetReader::read()` never allocates more than a buffer-sized chunk. No `std::fs::read` calls remain in async functions.

---

### Phase 3: Bounded Data Structures & Lifecycle (HIGH)

Cap all unbounded data structures. Add eviction and cleanup.

**Files:**
- `fe-sync/src/verse_peers.rs` — `VersePeers`
- `fe-sync/src/replicator.rs` — `MockVerseReplicator::entries`
- `fe-sync/src/sync_thread.rs` — `replicas` HashMap
- `fe-sync/src/blob_store.rs` — temp file cleanup

**Changes:**
1. `VersePeers`: Add `MAX_PEERS_PER_VERSE = 256` and `MAX_TRACKED_VERSES = 1024` constants. On overflow, evict LRU verse or oldest peer. Make `total_unique_peers()` O(1) by maintaining a running count.
2. `MockVerseReplicator::entries`: Store only `BlobHash` (32 bytes) instead of full `Vec<u8>` data. The blob store already holds the bytes. Add capacity limit (e.g., 100K entries).
3. `replicas` HashMap: Add `MAX_OPEN_REPLICAS = 64`. On overflow, close LRU replica before opening a new one, or reject with error.
4. `FsBlobStore::add_blob`: Add RAII temp file cleanup — if `write_all` or `sync_all` fails, remove the temp file before returning the error.
5. `MockVerseReplicator::write_row`: Replace `SystemTime::now()` timestamp with a proper Lamport counter (monotonically increasing, updated to `max(local, remote) + 1` on incoming events). This fixes the non-monotonic clock issue.

**Tests:**
- Unit test: insert 257 peers into a verse, verify oldest is evicted
- Unit test: open 65 replicas, verify oldest is closed
- Unit test: verify Lamport counter is monotonically increasing across writes
- Unit test: simulate write failure, verify temp file is cleaned up

**Exit criteria:** No unbounded `HashMap` or `HashSet` in the sync crate. All data structures have documented capacity limits.

---

### Phase 4: Efficient Replication (MEDIUM)

Eliminate redundant I/O in the replication path. Pass hashes instead of bytes.

**Files:**
- `fe-sync/src/replicator.rs` — `VerseReplicator::write_row` trait
- `fe-sync/src/sync_thread.rs` — `handle_write_row_entry`
- `fe-runtime/src/blob_store.rs` — `BlobStore` trait (add streaming variant)

**Changes:**
1. Change `VerseReplicator::write_row` signature from `fn write_row(&self, table: &str, record_id: &str, data: &[u8])` to `fn write_row(&self, table: &str, record_id: &str, content_hash: BlobHash)`. The replicator only needs the hash, not the full blob data.
2. `handle_write_row_entry`: Remove `std::fs::read(&blob_path)`. Pass the `content_hash` directly from the `SyncCommand::WriteRowEntry` to the replicator. No disk read needed.
3. Add `BlobStore::add_blob_streaming(&self, reader: impl Read) -> Result<BlobHash>` for streaming ingestion with incremental BLAKE3 hashing. This avoids the 2× memory overhead for large imports.
4. Update `import_gltf_handler` to use streaming ingestion when available.

**Tests:**
- Unit test: verify `write_row` with hash-only signature works for replication
- Unit test: verify streaming `add_blob` produces same hash as non-streaming for identical content
- Benchmark: compare memory usage of streaming vs non-streaming for a 50MB blob

**Exit criteria:** `handle_write_row_entry` performs zero disk reads. `write_row` accepts `BlobHash` not `&[u8]`.

---

### Phase 5: Disk Quota & Blob GC (MEDIUM)

Add disk space accounting and garbage collection for unreferenced blobs.

**Files:**
- `fe-sync/src/blob_store.rs` — `FsBlobStore`
- `fe-runtime/src/blob_store.rs` — `BlobStore` trait

**Changes:**
1. Add `BlobStoreConfig { max_size_bytes: u64, warn_threshold_pct: u8 }` with defaults (e.g., 10GB max, warn at 80%).
2. Track total store size in `FsBlobStore` (update on add/remove). Check before each `add_blob` — reject with error if quota exceeded.
3. Add `fn gc(&self, referenced_hashes: &HashSet<BlobHash>) -> Result<GcStats>` method. Removes blobs not in the referenced set. Returns count and bytes freed.
4. Add `fn store_size(&self) -> u64` and `fn blob_count(&self) -> usize` for monitoring.
5. Wire GC into the sync thread: periodically (e.g., on `CloseVerseReplica`) collect referenced hashes from open replicas and run GC.

**Tests:**
- Unit test: add blobs exceeding quota, verify rejection
- Unit test: add 3 blobs, GC with 2 referenced, verify 1 removed
- Unit test: verify `store_size` is accurate after add/remove/gc

**Exit criteria:** Blob store has a configurable size limit. Unreferenced blobs are collected.

---

### Phase 6: Cleanup & Documentation (LOW)

Remove unnecessary `Mutex` wrapper, fix minor issues, document the architecture.

**Files:**
- `fe-sync/src/status.rs` — `SyncEventReceiverRes`
- `fe-database/src/lib.rs` — `unwrap_or_default` on JSON serialization
- `fe-test-harness/` — update harness for new APIs

**Changes:**
1. Remove `Arc<Mutex<>>` from `SyncEventReceiverRes` — crossbeam `Receiver` is already `Send + Sync`.
2. Replace `serde_json::to_string(&row_json).unwrap_or_default()` with `.expect("row_json is a valid Value")`.
3. Update test harness `TestPeer` for any trait changes from Phases 1-5.
4. Add `SCALING.md` to the track documenting capacity limits, backpressure semantics, and performance characteristics.

**Tests:**
- All existing tests pass with API changes
- Test harness 7 scenarios still pass

**Exit criteria:** `cargo test --workspace` passes. `cargo clippy --workspace --exclude fe-auth -- -D warnings` clean. No `Arc<Mutex<>>` around thread-safe types.

## Non-Goals

- Implementing actual iroh-docs networking (Phase F of p2p_mycelium track)
- PlumTree gossip optimization (future track when iroh-gossip doesn't natively support it)
- io_uring integration (Linux-only, not needed for Windows development)
- Horizontal SurrealDB scaling (out of scope — SurrealDB is local-first projection)

## Dependencies

- `p2p_mycelium_20260405` must be complete (it is)
- No new crate dependencies required for Phases 1-3
- Phase 4 may benefit from `tokio::fs` if not already in use
- Phase 5 may use `fs2` crate for `available_space()` checks (already a transitive dep)

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Bevy AssetReader API changes in 0.18 | Low | Medium | Check `docs.rs/bevy` before Phase 2 |
| `VerseReplicator` trait change breaks test harness | Medium | Low | Update TestPeer in Phase 6 |
| Lamport counter adds complexity | Low | Low | Simple monotonic u64, well-understood pattern |
| Quota rejection UX | Medium | Medium | Surface quota errors to UI in future track |
