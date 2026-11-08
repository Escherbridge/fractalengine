---
type: Review
date: 2026-11-08
title: Independent critic of the scripted P2P simulator, pass 3 (QA-0129/0161/0162/0164 product fixes + harness 8f25326b/4de932f2)
reviewed_commits: [84fcb68a, 4257984b, 3510bc35, fce97bf8, 6c3de28c, 515eae67, 8f25326b, e4510e00, cb68c70e, 125636a6, 4c948f8e, e403d17b, 14ccc129, 95f13ad2, 8cb92d6d, 4de932f2]
fix_commits: [589b39b0, f26aff49]
previous_pass: 3bc40251 (pass 2, PASS-WITH-FIXES (4), fixes e4aebd15)
---

# Critic: scripted P2P simulator, pass 3

VERDICT: PASS-WITH-FIXES (3)

This pass ran in a fresh context and tried to refute the claims. The four original harness commits were
reviewed in passes 1 and 2, so this pass re-checked them only where the new commits touch them. It read
the twelve new commits line by line:

- product: 6c3de28c, 515eae67, e4510e00, cb68c70e, 125636a6, 4c948f8e, e403d17b, 14ccc129, 95f13ad2, 8cb92d6d
- harness: 8f25326b, 4de932f2

It found three defects and fixed all three: two in the product and one in the harness. They are
registered as QA-0166, QA-0167 and QA-0168. Product fixes and harness fixes are in separate commits, as
the 02:45 steer requires.

## Fixed: product (589b39b0)

1. **QA-0166 (S2): a restore that another author had overtaken published nothing.**
   - The bug: e4510e00 and e403d17b skip `set_bytes` when this author's own entry for the key already
     holds the same bytes (`own_entry_holds` used `Doc::get_exact(own author)`).
   - Why that breaks convergence:
     - Verse, fractal and petal rows carry no version. `apply_static_row` MERGEs them in arrival order.
     - The snapshot keeps `single_latest_per_key`.
     - So: Alice sets a petal name, Bob overwrites it, and Alice sets her earlier bytes back. The skip
       fires, every peer stays on Bob's name, and Alice shows her own name forever.
   - The fix: the skip now requires the newest entry across all authors (tombstones included) to be ours
     with the same hash.
     ```rust
     let latest = iroh_docs::store::Query::single_latest_per_key()
         .key_exact(key.as_bytes())
         .include_empty()
         .build();
     ... .is_some_and(|entry| entry.author() == author && entry.content_hash() == hash);
     ```
   - The drone case keeps its saving. The owner is the only writer of the mesh asset row and the node
     rows, so the newest entry is still the owner's.
   - Test and mutation: the real-transport journey `doc_replicator_replicates_row_over_real_transport`
     now has Bob overtake the key, and Alice's identical restore must reach him. With the old
     own-author query, the journey goes red at `fe-sync/src/replicator.rs:2601`.
   - Rationale: `fe-sync/src/AGENTS.md` §unchanged-rewrite.
2. **QA-0167 (S3): the QA-0162 Access-tab listing failed the whole tab past 1024 statement rows.**
   - The bug: `entity_access::load` read raw `role_grant` rows, and a grantee has one row per (scope,
     grantor), with revoked rows kept forever. The load capped those rows at 1024, although at most 256
     DIDs are ever listed. A city with per-petal grants reaches that row count and loses its only re-grant
     path, which was the QA-0162 bug again.
   - The fix: grantees and members are now `GROUP BY peer_did LIMIT 257` and checked against `MAX_PEERS`.
   - Test and mutation: the durable Access journey seeds 1100 revoked statements for one grantee and
     requires exactly one listing. With the old ungrouped query, it goes red at
     `role_propagation_durable_test.rs:307`.

## Fixed: harness (f26aff49)

3. **QA-0168 (S3): the whole-lap rule hid real lag.**
   - The bug: 4de932f2 dropped any lag pair a whole number of flight laps long (±1.5 s), and it did so on
     every flight-driven target. Its own note says that drone node/transform values never recur, but the
     code never checked that.
   - Mutation: a copy of `city-lite-20261108-040352` with the editor's transform observations delayed by
     30.2 s reported no lag at all (shared 0, no S2).
   - The fix: `_flight_lap_if_periodic` applies the lap only to a target that can recur. That means the
     target took at most `rate_hz * period_s` (+2) values during a flight of at least one lap.
     - The whole-petal query takes 34-44 values against 60 per lap, so it keeps the dismissal.
     - The node/transform rows take 160-290 values, so they lose it.
   - Dismissed pairs are now counted per peer in a new report column, "whole-lap sightings (max s)", so
     the rule never truncates silently.
   - Dry run:
     - The mutated copy now reports two S2 rows at p95 30.8 s.
     - Re-analyzing copies of city-lite 040352, 020155 and 033443 and p2p-core 034236 gives the same
       severities as before. The 040352 query keeps its 10 dismissed whole-lap pairs, and they are now
       listed.

## Claims checked and upheld

- **Order-independent merge with a named tiebreak.**
  - Superseding (e4510e00) replaces a queued snapshot only with the same author's newer complete snapshot
    of the same node in the same verse and petal. `SceneClock` orders by (HLC, publisher).
  - The scan stops at:
    - any non-apply command,
    - a role, structural or asset row,
    - another author's version, a tombstone or a markerless row.
  - The older slot is kept, so a reading queued behind it still finds the node.
  - Batching (125636a6) keeps one fenced transaction per row, in order.
- **Tombstones win and never resurrect.**
  - `live_snapshot_of` returns None for a tombstone, so a tombstone is never absorbed and never absorbs.
  - A deferred publication publishes before `DeleteNode`, `TombstoneNode` and every other command
    outside `runs_before_publication`, so a delete never races an unpublished transform.
- **Role checks on receipt.**
  - Every batched row still goes through `InboundAdmissions::apply` (role-at-write).
  - Transform and property writes authorize once (515eae67). The second check ran on the same serial
    lane, outside the transaction, so it fenced nothing the first did not. `scope_fence` still fences the
    ancestry inside the transaction.
  - A device payload that registers files requires the anchor petal and the node's Editor role (as
    `CreateAsset` did), and it is a Serve barrier (QA-0154 holds).
- **Restart catch-up without a stale window.**
  - Deferred publications wait at most 250 ms and publish before shutdown (`due_before(None)`).
  - `scene_publication::recover` republishes every node on open, so a crash inside the window is
    repaired on the next open.
  - The unchanged-rewrite skip keeps the entry in the replica, so set reconciliation still serves it.
- **Event-driven blob fetch.** e403d17b skips a scene-asset publish job only when the published row is
  ours and newest. With fix 1, that test is stricter.
- **Reactive render loop.** cb68c70e moves API commands to an `OwnedService` relay, which no frame needs.
  Replies return on `db_res_rx`, which `render_work` watches (`fractalengine/src/main.rs:492`). The
  unwatched `api_cmd_rx` is no longer drained by Bevy. The `PendingApiRequests` lock is never held
  across a send.
- **No request-reachable panic.**
  - In `commit_mutations_in_scope`, `facts.len() - 1` runs only when there are at least two changes, and
    `prepare_change` always emits at least one fact.
  - `take_one` guards emptiness.
  - Superseding and batching index nothing.
  - `ClearNodeProperties` is bounded to 1..=200 keys.
- **Harness: one process clock and measured barriers.** Unchanged since pass 2.
  - 8f25326b's `select_steps` raises before any peer starts (`run.py`: it runs ahead of
    `resource_gate`), so it adds no exit path.
- **Harness: the analyzer's predicates belong to the scenario.**
  - The lap dismissal is an analyzer heuristic for one class of moving targets, not a predicate.
  - With fix 3, it is evidence-gated and counted.
- **Harness: writes stay under `.qa/` and `qa/`.** The dry runs used `qa/sim/critic3/`, which was
  removed afterwards.
- **4c948f8e timing logs** are debug level and log only tool names, phase durations and the request
  path. No API route carries a secret in its path.

## Not fixed (residual, stated)

- **Superseding versus role-at-write.**
  - The case: a revoke lands between two versions c1 and c2 from one author. A receiver whose lane
    supersedes c1 with c2 then rejects c2 and keeps the prior value. A receiver that applied c1 alone
    keeps c1.
  - This is the same divergence a late-syncing peer already has: iroh-docs keeps only the latest entry
    per author and key, so a late peer never sees c1.
  - Its resolution belongs to the QA-0165 semantics decision, which the 02:45 steer reserves for the
    user. This pass did not touch it.
- **A `peers.ps1 Start` killed by the 1200 s timeout leaves its half-launched peers unrecorded.**
  - This is unchanged from passes 1 and 2.
  - The 00:40 steer assigns the launch journal to the orchestrator's executor, not to the critic.

## Gate (this phase)

- `cargo check -j4 -p fe-sync -p fe-database --tests`: green.
- `cargo +1.99.0 clippy -j4 -p fe-sync -p fe-database --all-targets -- -D warnings`: green.
- Touched journeys, green with the fix and red with it reverted:
  - fe-sync `replicator::tests::doc_replicator_replicates_row_over_real_transport`
  - fe-database `--test role_propagation_durable_test a_non_owner_creating_a_petal_gets_no_implicit_owner_row`
- Not run: every other batch, because no other region was touched.
- Python:
  - `run.py validate` passes all 4 scenarios.
  - `py_compile analyze.py`.
  - Analyzer dry runs on 4 real run copies plus the 30.2 s mutation.
- No app was started, so there were no peers to stop.
