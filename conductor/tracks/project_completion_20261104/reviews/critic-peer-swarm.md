---
type: review
title: Critic, peer-swarm P2P replication commits
date: 2026-11-07
reviewed: [6e7c7df9, 64b90964, 77886047, b5b2269a, 0b4f43de, c897e199, 573114dd, b743d7e5, cf20e55e, 988abcc0]
fixes: [b852c4b1, 34540ca0]
---

# Critic: peer-swarm replication (fresh context, refute-first)

VERDICT: PASS-WITH-FIXES (10)

The ten commits make peers converge on the paths the swarm exercised. The three
mandatory security findings were real, and so were three more routes to the same
QA-0131 bypass. On top of those, the critic found a dropped-revoke window, an
unconvergent tombstone re-judge, a re-judge that could never restore a node, and a
16-collaborator ceiling that turned off asset replication. All ten are fixed in
`b852c4b1` and `34540ca0`. Two narrow residuals are filed as S3: QA-0134 and QA-0135.

## Mandatory fixes

1. **QA-0131 (S1): grant cycling re-admitted rows written while revoked.** This is fixed
   at the source, not only on the empowerment path, because the same bypass also ran
   through the Deferred retry and the restart replay.
   - Role statements now carry continuity: `editor_since` and `manager_since`
     (`fe-policy/src/role_grants.rs:105` `continuing`, `:121` `as_of`, `:618`
     `materialize_at`).
   - A scene row is admitted only if its author holds Editor now and also held it at
     the row's `_fe_scene` HLC (`fe-database/src/role_grants.rs:350`
     `written_while_authorized`). The check runs in `apply_complete`
     (`scene_publication/inbound.rs:194`) and in `rejudge` (`:50`).
   - The empowerment re-read no longer replays `node/` rows. It sends one `restore_only`
     `RejudgeReplicatedNodes` batch instead (`fe-sync/src/sync_thread.rs:1901`).
   - Inbound role-statement HLCs are now observed (`op_log.rs:122`), so a new editor's
     next write is stamped after its grant.
2. **QA-0132 (S1): the re-judge queue was drained before the work succeeded.**
   - Both queues are now peeked, and a DID is acknowledged only after one batched
     command per verse was accepted (`sync_thread.rs:1769`, `:1850`).
   - A closed replica keeps the DID until it reopens. A failed read or a full channel
     keeps it until the 1 s kept-work tick (`:78`, `:977`).
3. **QA-0133 (S2): a blob prefetch ran before the row was admitted.**
   - `handle_inbound_row_change` now returns an `InboundVerdict` (`:1601`).
   - A prefetch starts only for a forwarded row whose author holds Editor+ in the
     fabric's role view (`:1981`). Seed reconciliation judges this after the whole
     snapshot has replayed.
   - At most 8 prefetches run at once, with a bounded backlog
     (`scene_assets/mod.rs:32`, `:297`).

## Additional defects found and fixed

4. **Deferred retry and restart replay admitted grant-cycled rows.** The write-time
   check sits in `apply_complete`, so it covers these routes as well as the re-read.
5. **Markerless (legacy) node rows had no write clock.** A revoked editor could get
   unmarked rows and tombstones, including empty `del` entries, admitted on re-grant.
   These rows are now denied when the author's authority comes from role statements
   (`replicated_row.rs:208`). Current publishers always send versioned snapshots.
6. **A revoke row dropped on a full DB channel left the DB plane judging with stale
   roles.** Such role rows are now kept on the fabric and resent before any re-judge
   or restore (`sync_thread.rs:1811`).
7. **The tombstone re-judge did not converge.** An inadmissible tombstone stayed on
   peers that had admitted it, while peers that refused it kept the live row. The
   re-judge now replaces it too (`inbound.rs:216`). That is the only exception to
   non-resurrection, and it applies only when the deleter fails the role checks.
8. **`rejudge` could never restore a node that was absent locally.** It returned
   `Unchanged` when no clock existed. `restore_only` now restores the newest
   admissible version (`inbound.rs:42`).
9. **QA-0136 (S2): more than 16 synced collaborators disabled replication.**
   `SceneProviderRoster::peers()` errored past 16, which failed every scene fetch and
   stopped the QA-0117 peer-book recorder. `provider_window` now asks the explicit
   providers first, then a rotating 16-provider window of roster peers
   (`scene_assets/mod.rs:77`).
10. **A stale R7 journey.** It expected an Editor to create a petal (petal creation
    needs Manager since `17e176d3`) and an owner revoke to read `none` (a revoke
    blocks the cell, and the peer inherits the verse default). The journey was
    repaired, not deleted.

## General focus

- **Transform merge.** Current writers publish complete snapshots. One LWW register
  per node is keyed by **max (HLC, publisher DID)**, using the derived `Ord` on
  `SceneClock` (`scene_publication/wire.rs:8`). It is total, because HLC is strictly
  monotonic per publisher, and every peer applies the same comparison. That is what
  selected step 6's `[-5,0,0]`. The `edit_seq` + `f64::total_cmp` register
  (`merge.rs:41`) applies only to legacy markerless rows. The steer's description of
  step 6 named the legacy rule, not the one that ran.
- **Tombstones.** An incoming tombstone applies regardless of clock. A local
  tombstone refuses later live rows (`SkippedTombstoned`). The only exception is #7.
- **Role checks on receipt.** On the DB plane, `admit_inbound_row` gates the current
  role and the new check gates the role at write time. On the sync plane, the fabric
  gates its own tables. The sender is never trusted.
- **Restart and catch-up.** Replay goes through the same gates. Kept role work
  survives a closed replica. Rows from an author demoted while this peer was offline
  are served until the revoke arrives. That is inherent eventual consistency, not a
  stale-serving bug.
- **Panics.** No new request-reachable `unwrap`, `expect` or indexing was found in
  non-test code across the ten commits. Indexing in scene current reads is guarded by
  `ensure!(rows.len() == 1)`.
- **Process and data hygiene.** `peers.ps1` stops only recorded identities
  (pid + start ticks + exe path, `Stop-QaOwnedProcessTree`). Peers run with
  `FE_QA_STORAGE_ROOT` and the working directory set to the run dir.
  `swarm_barrier.py` writes only under `--run/swarm/barrier/<step>` and controls no
  processes. Neither touches the user's data dir.
- **QA-0119 (steer).** No bridge job can stall. Watched channels wake the loop, active
  input, vision and screenshot jobs request redraws, and `reactive(250 ms)` bounds the
  worst case. The loop can be late, never stuck.

## Residuals (filed, S3)

- **QA-0134.** An owner carve-out pin later replaced by an Editor pin at the same key
  loses the carve-out window. The replica keeps one version per key, so this needs a
  wire marker.
- **QA-0135.** A revoked author can forward-stamp a row up to the 5-minute HLC bound
  and have it admitted by a re-grant inside that window.
- **Deliberate trade-off.** A break in Editor (revoke or demotion) ends continuity. A
  re-grant does not resurrect rows written before the break: every peer removed them
  on demotion, and a fresh peer refuses them the same way. Convergent and fail-closed;
  see `fe-database/src/AGENTS.md` §write-time-role.

## Gates

- `cargo check` and `cargo +1.99.0 clippy --all-targets -D warnings` on fe-policy,
  fe-runtime, fe-database and fe-sync: clean.
- Batches, all green:
  - R7: fe-database lib role filters; durable, local, api_cap and inbound tests (12/12,
    301 s); fe-policy 85; fe-api asset_ownership.
  - R8: fe-sync 295 lib + integration (relay-assisted 4, tileset 17); fe-api
    distributed_query; fe-network; relay; fe-runtime R8 filters.
  - R10: fe-database lib R10 filters plus `replicated_row` and `scene_publication`
    (216); db, migration, canon_log, dcl20 and replication_backpressure tests.
  - IoT: device_enrolment.
  - Misc: fe-runtime messages and app.
- **Known, pre-existing failure:** `shared_handle_test` (QA-0079, SurrealKV os error 33
  on Windows). It is untouched by this change.
- No app was launched and no push was made.
