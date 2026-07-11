# Specification: Relay Peers & Data Horizon

## Overview

Adds the **data interest** layer — what gets synced, cached, and relayed across the P2P network. Any peer can opt into relaying specific verses, creating a self-sustaining network where verse creators and community members ensure content availability without central servers.

This is the network/sync half of the dual-interest model. The render/visual half lives in `render_distance_lod_20260407`.

## Background

### Current State

- Sync thread opens/closes one verse replica at a time (driven by `NavigationState::active_verse_id`)
- `IrohDocsReplicator` is a stub — no actual peer data exchange
- `VersePeers` tracks peers per verse but has no discovery mechanism
- Blob fetching checks local store only; network fetch is a Phase F placeholder
- No concept of relay/pinning — peers only hold data they've directly interacted with

### The Problem

Without relays, content availability depends on the creator being online. If Alice creates a verse and goes offline, Bob can't fetch Alice's blobs even if he has the invite. The verse is effectively dead until Alice returns.

### The Solution

**Verse pinning**: Any peer can declare "I will relay Verse X". This means:
- They sync the full replica for that verse (all rows, all blobs)
- They serve blobs to other peers who request them
- They act as a seed for new joiners

This is opt-in per verse, not global. A relay for "Art Gallery Verse" doesn't store "Gaming Verse". The relay operator chooses which verses to support based on their own interest, community role, or resource capacity.

## Design

### Peer Roles

```
┌────────────────────────────────────────────────┐
│                 Peer Roles                      │
│                                                │
│  EXPLORER (default)                            │
│  - Syncs active verse only                     │
│  - Fetches blobs on demand                     │
│  - Data horizon = active petal + linked petals │
│                                                │
│  RELAY (opt-in per verse)                      │
│  - Syncs full verse(s) it pins                 │
│  - Caches all blobs for pinned verses          │
│  - Serves blobs to other peers                 │
│  - Can run headless (no GPU)                   │
│  - Configurable cache quota per verse          │
│                                                │
│  SEED (verse creator default)                  │
│  - Automatically relays the verse they created │
│  - Same as relay but auto-configured           │
└────────────────────────────────────────────────┘
```

### Data Horizon Model

The data horizon defines what a peer keeps synced beyond the currently-viewed content:

```
┌──────────────────────────────────────────────┐
│              Data Horizon Layers              │
│                                              │
│  Layer 1: ACTIVE (always synced)             │
│  - Current verse replica                     │
│  - Current petal's nodes + blobs             │
│                                              │
│  Layer 2: WARM (synced, not rendered)        │
│  - Adjacent petals in same fractal           │
│  - Recently visited petals (LRU, max 10)     │
│  - Bookmarked/favorited verses               │
│                                              │
│  Layer 3: COLD (metadata only)               │
│  - Other verses the user is a member of      │
│  - Row metadata synced, blobs NOT fetched    │
│  - Blob fetch on demand when user navigates  │
│                                              │
│  Layer 4: PINNED (relay mode)                │
│  - Full verse sync (rows + blobs)            │
│  - For verses the peer has opted to relay    │
│  - Configurable per-verse cache quota        │
└──────────────────────────────────────────────┘
```

### Relay Configuration

Stored in `AppSettings` (created by `render_distance_lod_20260407`):

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct RelayConfig {
    /// Verses this peer is pinning/relaying.
    pub pinned_verses: Vec<PinnedVerse>,
    /// Maximum total cache size for all relayed content.
    pub max_relay_cache_bytes: u64,
    /// Whether to accept relay requests from peers (serve blobs on demand).
    pub serve_blobs: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PinnedVerse {
    pub verse_id: String,
    pub verse_name: String,
    /// Max cache for this verse's blobs. None = unlimited (within global quota).
    pub max_cache_bytes: Option<u64>,
    /// When this pin was created.
    pub pinned_at: String,
}
```

### Headless Relay Mode

A relay can run without GPU:

```sh
# Run as a headless relay for a specific verse
fractalengine --relay --verse-id <id> --invite <invite_string>

# Or relay multiple verses from a config file
fractalengine --relay --config relay.ron
```

This spawns:
- iroh endpoint (for peer connectivity)
- Sync thread (for verse replication)
- Blob store (for content caching)
- DB thread (for row storage)

But NOT:
- Bevy render pipeline
- UI / egui
- Camera / input systems

## Phases

### Phase 1: Data Horizon Resource & Layers

Implement the data horizon model as a Bevy resource that tracks sync scope.

**Files to create/modify:**
- `fe-sync/src/data_horizon.rs` (new) — `DataHorizon` struct with layer tracking
- `fe-sync/src/lib.rs` — add module
- `fe-ui/src/plugin.rs` — system that updates data horizon from navigation state

**Changes:**
1. `DataHorizon` struct with `active_verse`, `active_petal`, `warm_petals: VecDeque<String>` (LRU, max 10), `cold_verses: HashSet<String>`, `pinned_verses: HashSet<String>`
2. `update_data_horizon` system: when `NavigationState` changes, update active/warm layers. Push previous petal to warm LRU. Evict oldest warm entry at capacity.
3. `DataHorizonEvent` enum: `PetalBecameActive`, `PetalBecameWarm`, `PetalBecameCold`, `VerseUnpinned` — emitted when layers change, consumed by sync thread to open/close replicas.

**Tests:**
- Navigate through 12 petals, verify warm LRU evicts oldest (keeps 10)
- Pin a verse, verify it stays in pinned layer regardless of navigation
- Verify active layer always contains current petal

**Exit criteria:** Data horizon tracks navigation and emits layer change events.

---

### Phase 2: Multi-Verse Replica Management

Allow the sync thread to hold multiple open replicas (active + warm + pinned), not just one.

**Files to modify:**
- `fe-sync/src/sync_thread.rs` — manage multiple open replicas driven by DataHorizon
- `fe-sync/src/messages.rs` — add DataHorizon-driven commands

**Changes:**
1. New `SyncCommand::UpdateDataHorizon { horizon: DataHorizonSnapshot }` — tells sync thread which verses/petals to keep open
2. Sync thread maintains replicas for: all active verses + all pinned verses. Warm verses get metadata-only sync (rows but not blobs). Cold verses are closed.
3. On `UpdateDataHorizon`: diff current open replicas vs desired set. Open new, close removed.
4. Respect `MAX_OPEN_REPLICAS` from `mycelium_scaling_20260407` Phase 3 — pinned verses count toward the limit.

**Tests:**
- Open 3 verse replicas, verify all active
- Navigate away from a verse, verify it moves to warm (replica stays open for metadata)
- Navigate to a 65th verse with 64 limit, verify LRU warm replica is closed

**Exit criteria:** Multiple verse replicas can be open simultaneously, driven by data horizon.

---

### Phase 3: Verse Pinning & Relay Config

Add UI and persistence for verse pinning. Any peer can pin verses to relay.

**Files to modify:**
- `fe-ui/src/settings.rs` — add `RelayConfig` to `AppSettings`
- `fe-ui/src/plugin.rs` — pin/unpin verse actions in UI
- `fe-sync/src/data_horizon.rs` — pinned verses always in data horizon

**Changes:**
1. `RelayConfig` struct added to `AppSettings` (see Design section above)
2. "Pin this Verse" button in verse context menu — adds to `pinned_verses`
3. "Unpin" removes from pinned list
4. Pinned verses are included in `DataHorizon` regardless of navigation
5. Relay settings panel: shows pinned verses, per-verse cache usage, global quota
6. On startup: automatically open replicas for all pinned verses

**Tests:**
- Pin a verse, restart app, verify it's still pinned and syncing
- Unpin, verify replica closes
- Exceed global quota, verify warning (but don't prevent pinning — user chooses what to drop)

**Exit criteria:** Users can pin/unpin verses. Pinned verses sync in background.

---

### Phase 4: Blob Serving (Peer-to-Peer Fetch)

Enable peers to serve blobs to each other via iroh. This is the actual P2P data exchange.

**Files to modify:**
- `fe-sync/src/sync_thread.rs` — implement blob fetch from peers (Phase F activation)
- `fe-sync/src/verse_peers.rs` — peer discovery for blob requests
- `fe-sync/src/endpoint.rs` — accept incoming blob requests

**Changes:**
1. When `FetchBlob` misses locally: query `VersePeers` for peers in the same verse, request the blob by hash from them via iroh-blobs protocol.
2. When receiving a blob request: check local blob store, if present and `serve_blobs == true`, stream the blob back.
3. Prioritize relay peers (peers with the verse pinned) as blob sources — they're more likely to have the content.
4. Fallback chain: local store → relay peers → all verse peers → fail with "blob not available"
5. Rate limiting: max concurrent outbound blob requests per peer (prevent abuse)

**Tests:**
- Two-peer test: Alice has blob, Bob requests it, verify Bob receives correct bytes
- Relay priority: prefer relay peer over regular peer for fetch
- Rate limiting: verify requests beyond limit are queued, not dropped

**Exit criteria:** Peers can fetch blobs from each other. Relay peers serve as reliable blob sources.

---

### Phase 5: Headless Relay Binary

Create a headless mode that runs relay without GPU/UI.

**Files to create/modify:**
- `fractalengine/src/main.rs` — add `--relay` CLI flag
- `fractalengine/src/relay.rs` (new) — headless relay entry point

**Changes:**
1. CLI argument parsing: `--relay` flag triggers headless mode
2. `--verse-id <id>` + `--invite <invite_string>`: pin a specific verse
3. `--config <path>`: load relay config from RON file
4. Headless mode: spawn only DB thread + sync thread + iroh endpoint. No Bevy app, no GPU.
5. Logging: structured tracing output with sync status, peer count, cache usage
6. Graceful shutdown on SIGINT/SIGTERM

**Tests:**
- Start headless relay, verify it syncs a verse
- Verify no GPU initialization in headless mode
- Verify graceful shutdown cleans up iroh endpoint

**Exit criteria:** `fractalengine --relay --invite <str>` runs a headless relay. Can be deployed as a service.

---

### Phase 6: Auto-Seed on Verse Creation

When a user creates a verse, they automatically become a seed (relay) for it.

**Files to modify:**
- `fe-ui/src/plugin.rs` — auto-pin on verse creation
- `fe-database/src/lib.rs` — emit event on verse creation for auto-pin

**Changes:**
1. On `DbResult::VerseCreated`: automatically add the verse to `pinned_verses` in `RelayConfig`
2. Mark as `auto_seeded: true` so it can be distinguished from manually pinned
3. User can unpin their own verse if they want (e.g., they transferred ownership)
4. UI indicator: show "You are seeding this verse" badge

**Tests:**
- Create verse, verify it appears in pinned list
- Verify auto-seeded verse syncs in background
- Unpin auto-seeded verse, verify it stops syncing

**Exit criteria:** Verse creators automatically seed their content. Network sustainability bootstrapped by default.

## Dependencies

- `render_distance_lod_20260407` Phase 1 (creates `AppSettings` — this track extends it)
- `mycelium_scaling_20260407` Phase 1 (backpressure — needed before real P2P traffic)
- `mycelium_scaling_20260407` Phase 3 (bounded replicas — this track opens multiple)

## Non-Goals

- DHT-based content discovery (future track — requires broader network topology)
- Payment/incentive for relay operators (future track — token economics)
- Encryption at rest for relay caches (future track — relay sees content it pins)
- Cross-verse blob deduplication (iroh-blobs handles this naturally via content-addressing)

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Relay abuse (peers pin everything, exhaust disk) | Medium | Medium | Per-verse quota + global quota |
| iroh-blobs streaming API changes | Low | High | Pin iroh version, test against specific API |
| Headless mode misses Bevy resource initialization | Medium | Low | Integration test that boots headless + sends commands |
| Cold start problem (no relays exist yet) | High | High | Auto-seed on creation ensures creator is always first relay |
