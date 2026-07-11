# Research Findings: P2P 3D Engine Sync Patterns

## 1. CRDT-based Document Replication

**iroh-docs**: LWW-per-key with range-based set reconciliation (not general CRDT). Each document is a namespace (Ed25519 keypair). Entries signed by author keys. Concurrent writes by different authors are both preserved (multi-author model). Same author: last-one-wins by timestamp.

**For spatial data**: Position vectors work well with LWW since concurrent edits are rare and physics corrections supersede them. Scene graph hierarchy needs move CRDTs (Kleppmann's tree CRDT) for parent-child relationships. Mesh data: too large for CRDTs — use content-addressed blobs, CRDTs only for metadata/pointers.

**Row-level CRDTs**: Treat each entity as a separate CRDT document with stable UUID. Store CRDT state as blob column. Apply merge on read or incoming sync. Don't use DB transactions as CRDT merge mechanism.

## 2. Content-Addressed Blob Streaming

**iroh-blobs**: Uses BLAKE3 tree structure (1KB leaves, 16KB groups). The bao format enables streaming verified chunks. Partial verification without full file.

**Backpressure pattern**: Bounded `tokio::sync::mpsc::channel(32)` between producer and consumer. Sender blocks when full. Do NOT use `buffer_unordered` for chunks (drops ordering).

**For 3D assets**: Fixed-size chunking preferred over Rabin (3D binary data has low cross-file redundancy).

## 3. Gossip Fan-Out

**PlumTree**: Eager/lazy peer separation. Eager peers forward immediately (tree edges), lazy peers send "ihave" hints. Converges to O(log N) hops, O(1) duplicate rate.

**HyParView**: Active view (log N + 1), passive view (k·log N). Shuffled for healing.

**iroh-gossip**: Basic epidemic broadcast (no PlumTree optimization as of mid-2024). Higher bandwidth in dense networks.

**sqrt(N) fan-out**: `fan_out = sqrt(peers).ceil().clamp(3, 10)`.

## 4. Async Blob Store Patterns

**tokio::fs vs spawn_blocking**: `tokio::fs` is a thin wrapper over `spawn_blocking`. For many small random reads, `spawn_blocking` with synchronous I/O is faster. For large sequential writes, `tokio::fs` is fine.

**Streaming BLAKE3**: `blake3::Hasher::update()` in a read loop. For parallel hashing on large files: `blake3::Hasher::update_rayon()`.

**Store layout**: Two-level directory sharding (`blobs/ab/cdef1234...`) to avoid inode limits.

## 5. Bevy Asset Streaming

**AssetReader**: Returns `dyn AsyncRead + AsyncSeek + Send + Sync + Unpin`. Streaming IS supported — VecReader is unnecessary.

**GLB loading bottleneck**: The GLTF loader (`gltf::Gltf::from_reader()`) reads the entire file. To avoid: split GLTF into external .bin buffers loaded lazily, or store GLB chunks as separate iroh-blobs with LOD manifest.

**Custom scheme**: Bevy supports custom URI schemes for AssetReader (e.g., `blob://`). Already implemented in fractalengine.

## 6. SurrealDB Scaling

**No P2P replication**: Embedded mode is single-node. Use SurrealDB as local projection of CRDT state.

**SCHEMAFULL overhead**: ~15-30% slower than SCHEMALESS on writes. No horizontal sharding in embedded mode.

**Best practices**: UUIDv7 for time-ordered IDs. HLC timestamps for LWW. LIVE SELECT for reactive UI. Keep blobs out of DB.

**Tokio compatibility**: SurrealDB uses Tokio internally. Safest pattern: dedicated Tokio runtime for DB, expose via channels.

## Crate Recommendations

| Component | Crate | Notes |
|-----------|-------|-------|
| P2P transport | iroh 0.35 | QUIC, NAT traversal |
| Document sync | iroh-docs 0.35 | LWW-per-key, range sync |
| Blob transfer | iroh-blobs 0.35 | BLAKE3 chunked, bao streaming |
| Gossip | iroh-gossip 0.35 | Topic-based epidemic |
| Hashing | blake3 1.8 | Streaming + parallel |
| DB | surrealdb 3.0 | Embedded with surrealkv |
| Asset loading | bevy 0.18 | Custom AssetReader |
