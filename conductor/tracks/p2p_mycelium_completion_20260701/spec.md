# Specification: P2P Mycelium Completion Track

## Overview

Complete the P2P Mycelium layer for FractalEngine, converting stubbed iroh-docs replication into functional peer-to-peer sync. This track addresses the gaps between the current scaffolding and true P2P functionality.

## Background

### Current State (from code review)

- **fe-network**: Real libp2p 0.56 with QUIC + Kademlia DHT. `SwarmHandle` builds and runs.
- **fe-sync endpoint**: Real iroh::Endpoint v0.35 binds to relay, returns node_id.
- **fe-sync sync_thread**: Real command/event loop, replica map management.
- **Blob store**: Real iroh-blobs filesystem backend.
- **IrohDocsReplicator**: STUB — wraps MockVerseReplicator (in-memory HashMap)
- **IrohPetalReplicator**: STUB — wraps MockVerseReplicator
- **Multiple commands**: Stubbed (SubscribePetal, UpdateNodeTransform, SubmitComputeTask, tileset operations)

### What Already Works

1. iroh endpoint initialization and node_id retrieval
2. libp2p swarm build with Kademlia bootstrap
3. Blob store read/write operations
4. Command/event channel between main thread and sync thread
5. In-memory replica storage with subscriber broadcast

### What's Missing (The Gaps)

1. **Phase F.1**: IrohDocsReplicator not connected to iroh_docs::Engine
2. **Phase F.2**: Petal-level replicator not connected to iroh-docs namespaces
3. **Phase F.3**: Real-time transform broadcast via gossip
4. **Phase F.4**: Verse/Petal gossip topics not subscribed
5. **Phase F.5**: Tileset P2P completely unimplemented

## Functional Requirements

### FR-1: Connect IrohDocsReplicator to iroh-docs Engine (Phase F.1)

**Description**: Replace the in-memory HashMap backing with real iroh_docs::Engine integration.

**Acceptance Criteria**:
- AC-1.1: `IrohDocsReplicator::new()` creates or opens an iroh-docs document namespace
- AC-1.2: `write_row()` calls `docs::Engine.set(&key, value)` with content hash
- AC-1.3: `subscribe()` subscribes to `docs::Engine.subscribe()` event stream
- AC-1.4: `close()` properly closes the document handle
- AC-1.5: Namespace secret handling: write requires secret, read-only doesn't

**Priority**: P0

### FR-2: Petal-Level Replication (Phase F.2)

**Description**: Full SubscribePetal/UnsubscribePetal implementation with per-petal iroh-docs namespaces.

**Acceptance Criteria**:
- AC-2.1: `SyncCommand::SubscribePetal` handler creates new `IrohPetalReplicator`
- AC-2.2: Each petal gets own iroh-docs namespace (derived from petal_id)
- AC-2.3: Petal replicator inserted into sync thread's `HashMap<String, Box<dyn PetalReplicator>>`
- AC-2.4: `UnsubscribePetal` closes and removes the petal replicator
- AC-2.5: Key encoding uses `/table/record_id` format (e.g., `/node/node123`)

**Priority**: P0

### FR-3: Real-Time Transform Sync (Phase F.3)

**Description**: Broadcast node transform changes to peers in real-time via iroh-gossip.

**Acceptance Criteria**:
- AC-3.1: `SyncCommand::UpdateNodeTransform` handler constructs a `TransformUpdate` gossip message
- AC-3.2: Message includes: node_id, position [f32;3], rotation [f32;3], scale [f32;3], timestamp
- AC-3.3: Gossip broadcast to petal_id topic (all peers viewing same petal)
- AC-3.4: Incoming gossip transforms applied to local Bevy entities
- AC-3.5: Loop prevention: skip transforms authored by self

**Priority**: P1

### FR-4: Gossip Topic Subscription (Phase F.4)

**Description**: Subscribe to iroh-gossip topics per Verse and per Petal.

**Acceptance Criteria**:
- AC-4.1: On `OpenVerseReplica`, subscribe to Verse gossip topic
- AC-4.2: On `SubscribePetal`, subscribe to Petal gossip topic
- AC-4.3: Incoming gossip messages routed to appropriate handler
- AC-4.4: Topic subscription cleaned up on replica close

**Priority**: P1

### FR-5: Tileset P2P (Phase F.5)

**Description**: Implement tileset advertisement, metadata request, and chunk fetch over P2P.

**Acceptance Criteria**:
- AC-5.1: `AdvertiseTilesets` broadcasts `TilesetAdvertisement` to connected peers
- AC-5.2: Peers receive advertisement, can request metadata
- AC-5.3: `RequestTilesetMeta` returns metadata JSON over P2P channel
- AC-5.4: `RequestChunk` transfers chunk bytes via iroh-blobs
- AC-5.5: `CancelTilesetDownload` aborts in-flight transfers

**Priority**: P2

## Implementation Strategy

### Phase 1: IrohDocsReplicator Integration (3-4 hours)

1. Add iroh-docs import to replicator.rs
2. Pass iroh_docs::Engine reference to IrohDocsReplicator::new
3. Implement write_row using Engine::set
4. Implement subscribe using Engine::subscribe
5. Add integration test with two nodes replicating

### Phase 2: Petal Replication (2-3 hours)

1. Extend sync thread to track petal replicators
2. Implement SubscribePetal handler
3. Implement UnsubscribePetal handler
4. Add namespace derivation from petal_id

### Phase 3: Transform Sync (3-4 hours)

1. Define TransformUpdate message type
2. Integrate iroh-gossip into sync thread
3. Handle SyncCommand::UpdateNodeTransform
4. Handle incoming gossip messages
5. Apply transforms to Bevy entities via events

### Phase 4: Gossip Topics (2 hours)

1. Add topic subscription on verse/petal open
2. Route incoming messages to handlers
3. Clean up on close

### Phase 5: Tileset P2P (4-5 hours)

1. Implement AdvertiseTilesets handler
2. Implement RequestTilesetMeta handler
3. Implement RequestChunk with iroh-blobs
4. Implement CancelTilesetDownload
5. End-to-end test with two peers

## Testing Strategy

### Unit Tests

- IrohDocsReplicator with mock Engine
- TransformUpdate serialization
- Key encoding (table/record_id)

### Integration Tests

- Two-node replication via iroh-docs
- Transform sync across peers
- Tileset chunk transfer

### Manual Tests

- Open same verse on two instances
- Drag node, observe transform on peer
- Import asset, verify blob propagates

## Dependencies

- fe-sync/src/replicator.rs
- fe-sync/src/sync_thread.rs
- fe-sync/src/messages.rs
- fe-network/src/gossip.rs
- fe-runtime (for BlobHash type)
- iroh-docs 0.35
- iroh-gossip 0.35

## Out of Scope

- Mobile P2P (different transport)
- Offline-first full sync (future track)
- Conflict resolution UI (future track)
- Relay server setup (infrastructure)
