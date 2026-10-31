---
type: retro
track: timeseries_fabric_20261009
title: "Retro: Timeseries Fabric (Sync-Plane M2)"
created: 2026-10-31
---

## Summary

**What shipped**: Consolidated record track for M2 sync-plane timeseries fabric. This track intentionally carries no duplicate implementation history — it is a named home for future workers. Full implementation notes on p2p_mycelium_completion_20260701.

**What the fabric delivers**:
- ReplicationMode enum (row/timeseries) with table dispatch at inbound seam; timeseries rows are insert-only, order-independent, tombstone-immune.
- Shard model (ShardId = petal/anchor/bucket); fabric (settings + peer roster + ledger); placement (mirror/sharded/balanced, power-of-choices, seeder overflow).
- Distributed query fan-out (SubmitComputeTask over gossip); planner (window/latest/all shards); commutative merge (fold_attribution for honest coverage).
- Sync-plane admission control (verse_id match, forged_sender detection, undeclared_requester denial, self-declaration-only ledger).
- Verse-manifest race fixes and GUI verse-open path fix.

**Verification**:
- A11-A17 (all M2 assertions) PASSED: F5 (A11/A12), F6 (A13/A14), F7 (A15/A16/A17), F21/F22/F23 (fixes).
- M2 scrutiny validator re-run after F22/F23 → VALIDATED.
- M2 user-testing validator → VALIDATED.
- Single workspace sweep: 3,245 tests passed.

**Left open / handed off**:
- iot_reading retention/GC deliberately not wired (fe-database/src/AGENTS.md §replication-mode).
- `__shards` Editor+ gate evaluated and not forced (no seam-side role check possible; spec.md Admission Control section details why).
- Replication-bridge Full-drop best-effort loss (outstanding_decisions D-63).
- Gossip plane NOT virtualized (roster empty in sim; distributed query exercises only local peer until roster seeded).

**Lessons**:
- Sync-plane rows (__peers, __shards) live in iroh-docs, NOT SurrealDB; this partition is essential for deterministic placement under membership churn.
- Timeseries union semantics (insert-only, never overwrite, tombstone-immune) are enforced at the inbound seam before the DB thread sees them; correct by construction.
- Distributed query merge is commutative: any receiver/merge order yields the same result; covered-shard attribution is the correctness guarantee (never fabricated coverage).
- Entry-author identity (endpoint SecretKey import) enables multi-host write attribution and role-denied-row replays; critical for distributed enforcement.

**Navigation for future workers**:
- Full implementation history: conductor/tracks/p2p_mycelium_completion_20260701/metadata.json (F5-F7, F21-F24 validated notes).
- Per-feature evidence + commit hashes: conductor/tracks/mission_prodready_continuation_20261008/plan.md (M2 section), spec.md (DEC-C1..C20).
- Residual decisions awaiting ratification: outstanding_decisions_20260715 (D-3, D-5, D-28) and later ledger updates.

**Commits** (reference): 7ca1213 (F5), 5430fc3 (F6), d5e2ce7 (F7), cfce22a (F21), 92128e1 (F22), 098f029 (F23), 9ffd1c4 (F24 residual).
