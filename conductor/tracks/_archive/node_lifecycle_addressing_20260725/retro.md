---
type: retro
track: node_lifecycle_addressing_20260725
title: "Retro: Node Lifecycle Addressing (Spatial Builder Program Spine)"
created: 2026-10-31
---

## Summary

**What shipped**: Data-layer spine of the Spatial Builder Program: sync-safe delete (tombstone + cascade + path-reflow event), stable per-node addressing substrate for read/write API, lazy node-promotion primitive for tens-of-thousands of stamp nodes.

**Implementation**:
- fe-entity-store address.rs — stable addressing substrate.
- fe-database handlers — tombstone delete, cascade delete, cascade re-flow.
- fe-policy node_lifecycle gate — lifecycle authorization.
- fe-sync lifecycle forwarder — P2P lifecycle propagation.
- CallerAuth::Local (@2377089) — internal operation markers.

**Verification**:
- 15 of 15 plan items [x] with evidence.
- Spine landed @c99dcf2; consumers integrated @195f5cc / @118e666.
- Live P2P merge wiring (fe-database/src/handlers/replicated_row.rs merge::apply_replicated_node) landed @3f957c4 / @ea158b7 (mission M1).
- Harness scenario two_peer_tombstone_sync covers end-to-end delete/cascade/promote over real loopback.
- In-app verification: user-gated.

**Left open / handed off**:
- Q1-Q4 design questions await ratification (address form, promotion trigger, cascade confirm, tombstone GC).
- Cascade op-append + row UPDATE not one transaction (fe-database/src/AGENTS.md §lifecycle).
- Tombstone GC/compaction and merge-safety proof deferred (Q-4 ratified 'unbounded for now').
- Metadata 'notes' array preserved for history (marked 'NOT started' at track creation — kept to show evolution).

**Lessons**:
- Stable addressing requires entity_store exclusive ownership — sharing with other systems causes churn.
- Tombstone + cascade in the DB handler is the single place for correctness; sync-safe by design when reply echoes are properly tracked.
- Lazy promotion (not immediate full-node on create) is essential to scale: 10k+ addressable nodes must not spawn 10k+ DB rows on first select.
- Reflow event on cascade delete is the contract: downstream petal-scoped systems (paths, etc.) must key on it.

**Commits**: c99dcf2 (spine); 195f5cc, 118e666 (consumer integration); 3f957c4, ea158b7 (M1 replication); all 15 plan items evidenced. Residual: cascade op-append + UPDATE not atomic (documented, awaits transaction shape).
