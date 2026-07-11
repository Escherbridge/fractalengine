# Implementation Plan: Relay Peers & Data Horizon

## Phase 1: Data Horizon Resource & Layers
Status: pending

### Tasks
- [ ] 1.1 Create `fe-sync/src/data_horizon.rs` with `DataHorizon` struct (active/warm/cold/pinned layers)
- [ ] 1.2 Implement warm petal LRU (VecDeque, max 10)
- [ ] 1.3 Implement `DataHorizonEvent` enum for layer transitions
- [ ] 1.4 Add `update_data_horizon` system driven by `NavigationState`
- [ ] 1.5 Write tests: LRU eviction, pinning, active layer tracking

## Phase 2: Multi-Verse Replica Management
Status: pending

### Tasks
- [ ] 2.1 Add `SyncCommand::UpdateDataHorizon` with snapshot payload
- [ ] 2.2 Implement replica diffing in sync thread (open new, close removed)
- [ ] 2.3 Support metadata-only sync for warm verses (rows but not blobs)
- [ ] 2.4 Respect MAX_OPEN_REPLICAS from mycelium_scaling track
- [ ] 2.5 Write tests: multi-replica open/close, warm vs active behavior, limit enforcement

## Phase 3: Verse Pinning & Relay Config
Status: pending

### Tasks
- [ ] 3.1 Add `RelayConfig` and `PinnedVerse` to `AppSettings`
- [ ] 3.2 Implement "Pin this Verse" / "Unpin" UI actions
- [ ] 3.3 Pinned verses always included in DataHorizon
- [ ] 3.4 Add relay settings panel (pinned list, per-verse cache, global quota)
- [ ] 3.5 Auto-open replicas for pinned verses on startup
- [ ] 3.6 Write tests: pin persistence, unpin cleanup, quota display

## Phase 4: Blob Serving (Peer-to-Peer Fetch)
Status: pending

### Tasks
- [ ] 4.1 Implement blob fetch from VersePeers on local miss
- [ ] 4.2 Implement blob serving (respond to incoming hash requests)
- [ ] 4.3 Add relay peer priority in fetch source selection
- [ ] 4.4 Implement fallback chain: local → relay → all peers → fail
- [ ] 4.5 Add per-peer rate limiting for outbound blob requests
- [ ] 4.6 Write tests: two-peer fetch, relay priority, rate limiting

## Phase 5: Headless Relay Binary
Status: pending

### Tasks
- [ ] 5.1 Add `--relay` CLI flag and argument parsing
- [ ] 5.2 Create `relay.rs` headless entry point (DB + sync + iroh, no Bevy)
- [ ] 5.3 Support `--verse-id` + `--invite` for single-verse relay
- [ ] 5.4 Support `--config` for multi-verse relay from RON file
- [ ] 5.5 Structured logging with sync status, peer count, cache usage
- [ ] 5.6 Write tests: headless boot, no GPU init, graceful shutdown

## Phase 6: Auto-Seed on Verse Creation
Status: pending

### Tasks
- [ ] 6.1 Auto-pin verse on `DbResult::VerseCreated`
- [ ] 6.2 Mark auto-seeded vs manually pinned
- [ ] 6.3 Allow unpinning own auto-seeded verse
- [ ] 6.4 Add "Seeding this verse" UI badge
- [ ] 6.5 Write tests: auto-pin on create, persistence, unpin
