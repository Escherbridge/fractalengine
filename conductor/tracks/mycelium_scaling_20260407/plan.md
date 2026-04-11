# Implementation Plan: Mycelium Scaling

## Phase 1: Eliminate Silent Data Loss
Status: pending

### Tasks
- [ ] 1.1 Fix `replicate_row()` in `fe-database/src/lib.rs` — replace `.ok()` with `try_send()` + `tracing::warn!`
- [ ] 1.2 Fix bridge thread in `fractalengine/src/main.rs:69-78` — replace `.ok()` with blocking `send()`
- [ ] 1.3 Fix blob miss callback in `fractalengine/src/main.rs:88-94` — `try_send()` + warn + dedup set
- [ ] 1.4 Fix sync thread lifecycle events (`Started`/`Stopped`) — blocking `send()`
- [ ] 1.5 Fix `BlobReady` events — `try_send()` + `tracing::debug!` on drop
- [ ] 1.6 Fix `MockVerseReplicator::write_row` subscriber retention — separate full vs closed
- [ ] 1.7 Fix `DbResult::Started` — blocking `send()`
- [ ] 1.8 Write tests: channel-full warning, bridge backpressure, burst-500 integration test
- [ ] 1.9 Verify: `grep -r '\.ok()'` returns zero hits on replication-path channel sends

## Phase 2: Streaming Asset Loading
Status: pending

### Tasks
- [ ] 2.1 Replace `VecReader` with `tokio::fs::File` in `BlobAssetReader::read()`
- [ ] 2.2 Wrap `std::fs::read` in `handle_write_row_entry` with `spawn_blocking`
- [ ] 2.3 Add max blob size check (512MB) before disk reads
- [ ] 2.4 Write tests: streaming reader memory check, async non-blocking verification
- [ ] 2.5 Verify: no `std::fs::read` calls remain in async functions

## Phase 3: Bounded Data Structures & Lifecycle
Status: pending

### Tasks
- [ ] 3.1 Add capacity limits to `VersePeers` (256 peers/verse, 1024 verses)
- [ ] 3.2 Change `MockVerseReplicator::entries` to store `BlobHash` only, add 100K cap
- [ ] 3.3 Add `MAX_OPEN_REPLICAS = 64` to sync thread `replicas` map
- [ ] 3.4 Add RAII temp file cleanup in `FsBlobStore::add_blob`
- [ ] 3.5 Replace `SystemTime::now()` with Lamport counter in `MockVerseReplicator`
- [ ] 3.6 Write tests: eviction, overflow, monotonic counter, temp file cleanup

## Phase 4: Efficient Replication
Status: pending

### Tasks
- [ ] 4.1 Change `VerseReplicator::write_row` to accept `BlobHash` instead of `&[u8]`
- [ ] 4.2 Remove disk read from `handle_write_row_entry` — pass hash directly
- [ ] 4.3 Add `BlobStore::add_blob_streaming(reader: impl Read)` with incremental BLAKE3
- [ ] 4.4 Update `import_gltf_handler` to use streaming ingestion
- [ ] 4.5 Write tests: hash-only replication, streaming hash correctness

## Phase 5: Disk Quota & Blob GC
Status: pending

### Tasks
- [ ] 5.1 Add `BlobStoreConfig` with `max_size_bytes` and `warn_threshold_pct`
- [ ] 5.2 Track total store size in `FsBlobStore`, check on `add_blob`
- [ ] 5.3 Add `gc(referenced_hashes)` method for unreferenced blob collection
- [ ] 5.4 Wire GC into sync thread on `CloseVerseReplica`
- [ ] 5.5 Write tests: quota rejection, GC removes unreferenced, size tracking

## Phase 6: Cleanup & Documentation
Status: pending

### Tasks
- [ ] 6.1 Remove `Arc<Mutex<>>` from `SyncEventReceiverRes`
- [ ] 6.2 Replace `unwrap_or_default()` with `.expect()` on JSON serialization
- [ ] 6.3 Update test harness `TestPeer` for API changes
- [ ] 6.4 Full workspace `cargo test` + `cargo clippy` verification
