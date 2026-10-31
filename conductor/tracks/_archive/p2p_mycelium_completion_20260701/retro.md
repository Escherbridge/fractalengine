---
type: retro
track: p2p_mycelium_completion_20260701
title: "Retro: P2P Mycelium Completion"
created: 2026-10-31
---

## Summary

**What shipped**: Complete P2P Mycelium layer: real iroh-docs Engine, petal-level replication, inbound RBAC gate, fully-applying relay, timeseries fabric (ReplicationMode dispatch, shard ledger, distributed query fan-out), sync-plane admission control, and M1/M2 scrutiny hardening.

**Implementation (M1 A1-A10)**:
- F1: DocsStack in fe-sync/src/docs_engine.rs; persistent blobs/docs stores; Router ALPN routing; FE_P2P_DIR env.
- F2: Real Doc operations (VerseReplicator/PetalReplicator async); Capability::Write import; RowChange with payload+author+timestamp; LiveEvent pump.
- F3: Inbound admission gate (admit_inbound_row) with role resolution + RBAC (Editor+); verse-manifest bootstrap window; scope-injection guard.
- F4: Relay as fully-applying replica; DbCommand seam; per-verse replica lifecycle; graceful shutdown (SIGINT + 750ms wait + exit 0).
- F20: Reconciliation self-deadlock fix; entry-author identity alignment; namespace id derivation (Ed25519 verifying key); mock fallback offline-only.

**Implementation (M2 A11-A17)**:
- F5: IoT timeseries ingestion with replication (insert_readings_with_replication); ReplicationMode row/timeseries union semantics.
- F6: Shard fabric (ShardId = petal/anchor/bucket); placement (power-of-choices, mirror/sharded/balanced modes); retention-race rationale.
- F7: Distributed query fan-out over gossip (SubmitComputeTask); planner (window/latest/all shards); commutative merge (fold_attribution).
- F21: Verse-manifest race fix (PendingWrites retain-and-flush); FE_SYNC_BOOTSTRAP support.
- F22: GUI verse-open path fix (DbResult::VerseCreated now carries namespace_id).
- F23: Sync-plane admission control (verse_id match, forged_sender detection, undeclared_requester denial); ledger self-declaration only.

**Verification**:
- M1 evidence: F1 121 tests, F2 133 tests + two_peer_replica_sync scenario, F3 169 tests + two_peer_tombstone_sync scenario, F4 6 relay tests + graceful shutdown e2e, F20 145 tests + live relay e2e.
- M2 evidence: F5 184 DB tests + 88 API tests, F6 183 sync tests (placement 15, sharding 11) + scenario 11, F7 distributed_query tests + scenario 12 + fe-api 11 tests, F21/F22 all suites green, F23 215 sync tests.
- Single workspace sweep at mission end: 3,245 tests passed, 0 failed.

**Left open / handed off** (future phases):
- Phase 2: petal-level namespaces (multiple namespace secrets per verse).
- Phase 3: gossip receive (SUPERSEDED by row-level replication through iroh-docs).
- Phase 4: per-petal gossip topics (started, partial; verse-level topics complete).
- Phase 5: tileset P2P (not started).
- n0 hosted-relay EOL: 2026-12-31.
- Relayed-delivery attribution (requires per-message signing).
- __shards Editor+ gate (no seam-side form without DB-side state).

**Lessons**:
- Real iroh-docs Engine requires persistent stores; SurrealKV per-handle file lock forces channel fallback on Windows (M3/F24 fixed REST ingest via fallback).
- Entry-author identity alignment (endpoint SecretKey import) is critical for multi-host write attribution and role-denied row replays.
- Namespace id = Ed25519 verifying key, not keyed-BLAKE3, enables secretless doc reopens and consistent derivation.
- Sync-plane rows (__peers, __shards) never reach DB thread; fabric state lives in iroh-docs namespace, not SurrealDB.
- Timeseries union semantics (insert-only, order-independent, tombstone-immune) are enforced at the inbound seam, not on the wire.

**Commits** (selection): c004a33 (F1), 3f957c4 (F2), ea158b7 (F3), bef1be0 (F4), 0205e4a (F20), 7ca1213 (F5), 5430fc3 (F6), d5e2ce7 (F7), cfce22a (F21), 92128e1 (F22), 098f029 (F23), 9ffd1c4 (F24), d198bd2 (F19 sweep). Full range: c004a33..0205e4a (M1), 7ca1213..098f029 (M2).
