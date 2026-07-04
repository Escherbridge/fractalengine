# Implementation Plan: P2P Mycelium Completion

## Track ID
`p2p_mycelium_completion_20260701`

## Phase 1: IrohDocsReplicator Integration (3-4 hours)

### Step 1.1: Examine iroh-docs 0.35 API
```
Location: Cargo.toml, docs.rs
Task: Read iroh-docs crate documentation for Engine, Doc, Author
Verify: namespace creation, set operation, subscribe pattern
```

### Step 1.2: Refactor IrohDocsReplicator
```
Location: fe-sync/src/replicator.rs
Commands:
- patch IrohDocsReplicator struct to hold Engine reference
- patch write_row to call Engine::set 
- patch subscribe to call Engine::subscribe
- patch close to call Doc::close
```

### Step 1.3: Pass Engine to Replicator
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Modify handle_open_verse_replica to create Engine
- Pass Engine to IrohDocsReplicator::new
- Add proper error handling for namespace creation
```

### Step 1.4: Test IrohDocsReplicator
```
Location: fe-sync/src/replicator.rs
Commands:
- Add test: iroh_docs_replicator_write_and_subscribe
- Verify: write_row emits to subscribe channel
```

---

## Phase 2: Petal-Level Replication (2-3 hours)

### Step 2.1: Add Petal Replica Map
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Add: let mut petal_replicas: HashMap<String, Box<dyn PetalReplicator>>
- Initialize in sync thread init
```

### Step 2.2: Implement SubscribePetal
```
Location: fe-sync/src/sync_thread.rs (handle function)
Commands:
- Create namespace_id from petal_id (hash or derived)
- Create IrohPetalReplicator with namespace
- Insert into petal_replicas map
- tracing::debug success
```

### Step 2.3: Implement UnsubscribePetal
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Remove from petal_replicas
- Call close() on replicator
- tracing::debug success
```

### Step 2.4: Update Messages Enum (if needed)
```
Location: fe-sync/src/messages.rs
Verify: SyncCommand::SubscribePetal already exists (it does)
```

---

## Phase 3: Real-Time Transform Sync (3-4 hours)

### Step 3.1: Define TransformUpdate Message
```
Location: fe-sync/src/messages.rs
Commands:
- Add: #[derive(Clone)] pub struct TransformUpdate
- Fields: node_id, position, rotation, scale, timestamp, author_id
- Add Serialize/Deserialize
```

### Step 3.2: Integrate iroh-gossip
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Add iroh_gossip::Host import
- Initialize gossip host in sync thread
- Add: subscribe_to_petal_topic(petal_id)
```

### Step 3.3: Handle UpdateNodeTransform
```
Location: fe-sync/src/sync_thread.rs (existing handler)
Commands:
- Construct TransformUpdate with all fields
- Broadcast via gossip host to petal topic
- tracing::debug broadcast sent
```

### Step 3.4: Handle Incoming Gossip
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Addtokio::select! arm for gossip messages
- Parse TransformUpdate
- Skip if author_id == local_did (loop prevention)
- Send SyncEvent::NodeTransformed to main thread
```

### Step 3.5: Apply to Bevy Entities
```
Location: fe-ui (likely verse_manager.rs or new system)
Commands:
- Listen for SyncEvent::NodeTransformed
- Find entity by node_id
- Update Transform component
```

---

## Phase 4: Gossip Topic Subscription (2 hours)

### Step 4.1: Subscribe on Verse Open
```
Location: fe-sync/src/sync_thread.rs
Commands:
- In handle_open_verse_replica
- Add: subscribe_to_verse_topic(verse_id)
- Store topic handle for cleanup
```

### Step 4.2: Subscribe on Petal Open
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Already done in SubscribePetal (Step 2.2)
- Verify topic matches petal_id
```

### Step 4.3: Cleanup on Close
```
Location: fe-sync/src/sync_thread.rs
Commands:
- In handle_close_verse_replica: unsubscribe verse topic
- In UnsubscribePetal: unsubscribe petal topic
```

---

## Phase 5: Tileset P2P (4-5 hours)

### Step 5.1: Implement AdvertiseTilesets
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Parse advertisements_json to Vec<TilesetAdvertisement>
- Broadcast via gossip to connected peers
- Include: tileset_id, chunk_count, size_bytes
```

### Step 5.2: Implement RequestTilesetMeta
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Look up tileset in local registry
- Serialize TilesetMeta to JSON
- Send to requesting peer via gossip response
```

### Step 5.3: Implement RequestChunk
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Read chunk from blob store
- Send via iroh-blobs direct transfer or gossip
- Track in-flight transfers
- Emit SyncEvent::ChunkReceived on completion
```

### Step 5.4: Implement CancelTilesetDownload
```
Location: fe-sync/src/sync_thread.rs
Commands:
- Remove from in-flight tracking
- Abort transfer if running
- tracing::debug cancellation
```

### Step 5.5: End-to-End Test
```
Commands:
- Start two instances
- Instance A advertises tileset
- Instance B requests metadata
- Instance B requests and receives chunk
- Verify chunk content matches
```

---

## Verification Steps

After each phase, verify:

### Phase 1 Verification
```bash
cd /mnt/c/Users/atooz/Programming/fractalengine-workspace/fractalengine
cargo test -p fe-sync iroh_docs_replicator
```

### Phase 2 Verification
```bash
cargo test -p fe-sync petal_replic
```

### Phase 3 Verification
```bash
cargo build -p fe-sync
# Manual: drag node in two instances, verify transform syncs
```

### Phase 4 Verification
```bash
cargo build -p fe-sync
# Check logs for "subscribed to topic" messages
```

### Phase 5 Verification
```bash
cargo test -p fe-sync tileset
# Full manual test with two instances
```

---

## Rollback Plan

If issues arise:
- Phase 1/2: Revert replicator.rs to use MockVerseReplicator (already works)
- Phase 3: Disable UpdateNodeTransform (stub it again)
- Phase 4: Disable gossip topics
- Phase 5: Stub tileset commands again

All stubs already exist in git history.

---

## Estimated Total Time

| Phase | Hours |
|-------|-------|
| Phase 1: IrohDocsReplicator | 3-4 |
| Phase 2: Petal Replication | 2-3 |
| Phase 3: Transform Sync | 3-4 |
| Phase 4: Gossip Topics | 2 |
| Phase 5: Tileset P2P | 4-5 |
| **Total** | **14-18** |

---

## Branch Strategy

```bash
git checkout -b track/p2p-mycelium-completion
# Implement phases sequentially
git add -A && git commit -m "feat(fe-sync): Phase N — <description>"
# After all phases complete
git checkout main && git merge track/p2p-mycelium-completion
```
