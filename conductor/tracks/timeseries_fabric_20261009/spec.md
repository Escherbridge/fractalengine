---
type: Spec
track: timeseries_fabric_20261009
title: "Timeseries fabric — consolidated record (M2 sync-plane)"
timestamp: 2026-10-09T00:00:00Z
---

# Timeseries fabric — consolidated record

Record-only track created at mission close (F18, A31) to give the M2
sync-plane timeseries fabric a dedicated home. It does not contain new
work — it POINTS at where the implementation and VALIDATED notes actually
live, per the F18 orchestrator note (features.json F18 description): "when
you create the dedicated timeseries-fabric track, REFERENCE/CONSOLIDATE
those accumulated notes into it by appending dated pointers (never rewrite
the p2p track's history)."

## Pointers (authoritative sources — read these, not this file)

- Implementation notes + full VALIDATED history for F5/F6/F7/F21/F22/F23 +
  the F24 live-restoration feature: `conductor/tracks/p2p_mycelium_completion_20260701/metadata.json`
  (`notes` field — every dated paragraph from 2026-10-07 onward).
- Per-feature plan checkboxes + commit hashes + acceptance evidence for the
  mission continuation (F9–F19): `conductor/tracks/mission_prodready_continuation_20261008/plan.md`
  §M2/§M3, `spec.md` decision log DEC-C6/DEC-C7/DEC-C13.
- Mission acceptance assertions A11–A17: `C:\Users\atooz\.factory\missions\77c4c579-4f99-403f-8bcf-3b1ae3f0c718\validation-contract.md`.

## What the fabric is (summary, not a substitute for the sources above)

- **ReplicationMode::{Row,Timeseries}** (`fe-database/src/replication_mode.rs`):
  canonical table-to-mode mapping (`iot_reading` = Timeseries, everything
  else = Row); the inbound applier dispatches on it after the A3 admission
  gate (F5, A11/A12).
- **Shard model + ledger** (`fe-sync/src/sharding.rs`): `ShardId = (petal,
  anchor, time bucket)`; `__shards/*` + `__peers/{did}` rows ride the
  verse's own iroh-docs namespace as sync-plane state (never reaches the DB
  thread); power-of-choices placement with a seeder overflow tier, R
  clamped to reachable peers (F6, A13/A14).
- **Distributed query fan-out + merge honesty** (`fe-sync/src/distributed_query.rs`,
  `fe-api/src/timeseries_query.rs`): `SubmitComputeTask` over the verse
  compute gossip topic, commutative per-shard merges, `covered_shards`/
  `missing_shards`/`answered_hosts` metadata that never fabricates coverage
  (F7, A15/A16/A17).
- **Admission control** (F23): transport-level `verse_id`/`from_did`
  verification + declared-peer requirement on the gossip seam; `__peers`
  self-declaration-only rejection at the ledger seam; the `__shards`
  Editor+ gate was evaluated and NOT forced (no constructible seam-side
  role check — documented residual).
- **Verse-manifest open race** (F21 seam fix: bounded `PendingWrites`
  retain-and-flush; F22 GUI-leg fix: `DbResult::VerseCreated` carries
  `namespace_id`, GUI navigation opens the replica instead of silently
  early-returning). Both legs (relay + GUI) proven over the real loopback
  transport with durable own-doc read-back.

## Status

`done` — this is a closed record, not an active work item. Future
sync-plane work opens a NEW dated note on this track (or a fresh track if
the scope is large enough), never edits the p2p track's history again.
