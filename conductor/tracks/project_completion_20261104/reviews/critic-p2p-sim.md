---
type: Review
date: 2026-11-08
title: Independent critic of the scripted P2P simulator, pass 2 (QA-0156/0157/0158 product fixes + QA-0159 scenario)
reviewed_commits: [84fcb68a, 4257984b, 3510bc35, fce97bf8, 3acc5655, 9406322c, e7a65f50, 3de14e42]
also_read: [929c79ca]
fix_commits: [e4aebd15]
previous_pass: fb8709f9 (PASS-WITH-FIXES (12), fixes 9cfbdd4f + c357bf03)
---

# Critic: scripted P2P simulator, pass 2

VERDICT: PASS-WITH-FIXES (4)

This pass ran in a fresh context and tried to refute the claims. It re-read the four harness commits
against the harness criteria, and it read the four new commits line by line against the product
criteria. Those new commits are QA-0158 upload publication, QA-0157 unarmed table, QA-0156
order-independent node merge, and the QA-0159 p2p-core scenario. A sibling lane committed 929c79ca
while this pass ran (QA-0159 c/d/e/f: FireGate, step-scoped windows, periodic lag, exact locator). That
commit was read, but only for barrier and exit-path safety.

One product defect was found and fixed. QA-0158 reopened the QA-0133 revoke race for unplaced uploads.
Three harness defects were found and fixed: a teardown that could skip the peer stop, a stopped peer's
reusable PID, and silently unjudgeable oversize observations. All four fixes are in e4aebd15. The fixed
product finding is registered as QA-0160 (S3, fixed_source_live_pending).

## Fixed: product (e4aebd15)

1. **QA-0160: QA-0158 made a demoted Editor's racing upload fetchable and re-servable for good.**
   - QA-0158 dropped the live-node floor from `scene_asset_transfer::inspect`. Every sync job names no
     node, so a Fetch or Serve needed only the anchored asset row plus Viewer.
   - Inbound `asset` rows are insert-if-absent and never re-judged (`apply_anchored_asset_row`). Before
     storing a row, that function strips the uploader's identity (`owner_did`).
   - So consider an upload that raced its author's revoke. Any peer that had not yet learned the revoke
     admitted the row, fetched the GLB on arrival, and kept re-serving it to third peers indefinitely.
     QA-0133 promised that never happens.
   - The fix:
     - Inbound apply records the replica author, in the same CREATE transaction, in a local
       `asset_inbound_author` table. The table is never replicated and is on the dev-reset list
       (`schema.rs`).
     - `inspect` covers a Fetch or Serve that names no node, for a row with no local upload/adoption
       proof. It admits that row only through `unplaced_transfer_fence`: either a live node references the
       row in its Petal, or the recorded author holds Editor at the anchor scope now. Both conditions are
       fenced inside the admission transaction.
   - Rows stored before this table existed have no recorded author. They need the live reference, as
     they did before QA-0158.
   - The extended journey `op_log::tests::a_failing_statement_rolls_back_the_op_log_row_too` checks the
     sequence: fetched while the author is Editor, Fetch and Serve refused after the revoke, served again
     once a live node places the row.
   - **Mutation check:** with the fence disabled, the journey goes red at "a demoted author's unplaced
     upload is never fetched".
   - Rationale: `fe-database/src/handlers/AGENTS.md` §scene-asset-transfer "Unplaced rows".

## Fixed: harness (e4aebd15)

2. **A raising teardown could skip the exact-identity peer stop** (`run.py` `finally`).
   `prober.stop()`, `sampler.stop()` (a `TimeoutExpired` on its second wait) and `recorder.close()` (its
   flush raises on a full disk, which this host has hit before) ran unguarded ahead of `host.stop()`. Any
   one of them raising leaked every peer. Each step is now isolated and logged, and the first error is
   kept as the run failure. Dry run: a fake prober (OSError), sampler (KeyboardInterrupt) and recorder
   (OSError) all raised, and `host.stop` still ran with the recorded owner directory. The run returned 1.
3. **The host sampler kept sampling a stopped peer's PID** (`Host.stop_peer`). Between `stop_peer` and
   `restart`, a reused PID would have charged another process's CPU to that peer in `host.csv`. The
   stopped member is now dropped from the sampler's PID file. `restart` already rewrites the file.
4. **Oversize observations were silently unjudgeable** (`prober.py`). Values over 4 KiB (64 KiB for
   hierarchy) were recorded by digest only. The analyzer reads such a value as MISSING, so an expectation
   on a large properties bag would report "never held", which looks like a product miss. Two changes:
   - The inline budget is now 256 KiB for every probe. Observations are change-logged, so the budget
     costs per change, not per poll.
   - A value over the budget carries `oversize: true`, and the analyzer raises one harness anomaly that
     lists every such series.

   The analyzer hunk was in the working tree when the sibling lane committed the whole file, so it
   landed inside 929c79ca. It is verified on a scratch copy of run `p2p-assets-20261107-232544` with one
   injected oversize observation, which produced exactly one harness anomaly naming that series.

## Claims checked and upheld

- **QA-0156: order-independent merge with a named tiebreak.**
  - Complete snapshots are an (HLC, publisher) register.
  - A markerless live row that arrives after a versioned one is `NotApplicable` (`inbound.rs`
    "unversioned live row deferred"), so complete beats markerless in either order.
  - Markerless versus markerless uses the legacy-writer precedence (owner first, then the larger DID).
    The writer's own replica cannot diverge from it, for two reasons. Current publishers never send
    markerless rows. And QA-0131 denies markerless rows from statement-derived Editors before dispatch.
  - The owner flag is stable, because admission needs the verse row, so `verse_owner` is known at
    apply.
  - Two deleters' tombstone markers settle on the larger canonical JSON.
- **Tombstones win and never resurrect.** The tombstone check precedes the precedence check, and a
  live row over a local tombstone stays `SkippedTombstoned`.
  - `reconcile_petal` keeps the blind merge but has no production caller.
- **QA-0157: no table an honest hash is refused from.**
  - `unarmed` is keyed only by hashes still in `by_hash`, which is bounded by the replica's own
    entries.
  - The sweep's work is bounded instead: a round-robin of 4096 hashes per 5 s, with one author verdict
    per tick.
  - Stale order entries are compacted once they exceed 2×live + budget.
  - The journey proves a late-named Editor behind 12,305 Sybil hashes is armed in 4 sweeps.
- **QA-0158: role checks on receipt.**
  - Publishing an upload fences actual local Editor, the upload proof and exact metadata before it
    enqueues.
  - Receivers admit the row only on the author's current Editor role.
  - Serve still requires the requester's current Viewer role.
  - With fix 1, a Fetch or Serve of an unplaced row also follows the author's current role.
- **No request-reachable panic in the new code.** Checked:
  - `proven_asset_wire` indexes `assets[0]` only after its `len()==1` guard.
  - `emit_uploaded_asset` indexes `rows[0]` after an `ensure!`.
  - `marker_outranks` indexes object keys from the object itself.
  - `LegacyAuthor` uses its derived `Ord`.
- **Restart catch-up, event-driven blob fetch, reactive loop.** The new commits do not touch these
  paths. The pass-1 findings hold: stable ports, content re-fetch and isolated-write sync for catch-up;
  fetches on row arrival, NeighborUp, empowerment and rewrite; watched queues wake the reactive loop.
  - QA-0158 adds an event-driven trigger: the asset row's arrival.
- **One process clock.** `runtime.Clock` uses `perf_counter`, and `host.csv` is converted through one
  wall/perf anchor pair.
- **Barrier alignment is measured.** Barrier events carry `fire_at` and `fire_late_ms`.
  - 929c79ca's `FireGate` opens only when every remaining participant has arrived, and an arrived
    participant blocks until it opens.
  - A post-arrival exception therefore cannot open a gate early.
  - A double withdraw needs `_event` itself to raise after a withdraw, which means the recorder already
    failed. Fix 2 now makes that failure a clean, peer-stopping exit.
- **Probers cannot cause the refusals they measure.** GET probes are not rate-limited in fe-api. The
  only limited family is `/query` plus the timeseries query (10/s per DID).
  - The prober refuses a scenario over 6 query or 8 timeseries targets.
  - The schema floors `query_ms` at 1000 ms, so the worst case is 6/s from the prober plus 2/s from
    `lookup_node`, under 10/s.
  - Rate-limited answers count as tick errors and are never recorded as state.
- **Predicates belong to the scenario.** The analyzer evaluates the scenario's predicate, path, value,
  `after`, `needs` and `meta.severity`. Built-in severities are defaults only.
  - 929c79ca now also caps "held then regressed" by `meta.severity`.
- **No silent truncation of anomalies.** Every anomaly is listed, and the fingerprint key cut says so.
  With fix 4, oversize values are named too.
- **Writes only under `.qa/` and `qa/`.** Run directories are under `qa/sim/runs`. Spools, stage and
  pointer files are under `.qa/`.
- **3de14e42 scenario.**
  - It depends on harness args (`exact`, `wait_role`, `expect: any`, needs at fire time) that existed
    only in the uncommitted tree when it landed. 929c79ca now carries them.
  - `run.py validate` passes all four scenarios on the committed tree.
  - 929c79ca also corrected the armed-confirm names (`Confirm Revoke role grant for <did>`), which the
    3de14e42 names could never match.

## Not fixed (residual, stated)

- **A `peers.ps1 Start` killed by the harness's 1200 s timeout leaves its half-launched peers
  unrecorded.** `TerminateProcess` skips the PowerShell `finally` rollback. This is unchanged from pass 1.
  Peers can only be attributed by a launch journal. Scanning `.qa/run/*` by mtime could stop another
  session's live peers, which the process-cleanup rule forbids. Every non-killed Start failure still
  rolls back by exact identity, and a normal Start takes 1-2 min.

## Gate (this phase)

- `cargo check -p fe-database -p fe-sync`: green.
- `cargo +1.99.0 clippy -p fe-database --all-targets -D warnings`: green. fe-sync is untouched by this
  pass.
- Touched batches:
  - R10 lib filter, plus `handlers::replicated_row`, `handlers::scene_asset_transfer` and
    `handlers::scene_publication`: 170 passed, 10 ignored.
  - The extended journey `op_log::tests::a_failing_statement_rolls_back_the_op_log_row_too`: passed,
    and red under the mutation.
  - `--test schema_idempotent` (30 passed) and `--test device_payload_test` (4 passed). The latter is the
    inbound anchored-asset apply.
- Python dry run:
  - `run.py validate`: 4 scenarios OK.
  - Fake-collaborator run of `run.run` with every teardown raising: the peers were still stopped and the
    run returned 1.
  - `Prober._emit` budget and oversize marking.
  - Analyzer re-run on a scratch copy with an injected oversize observation.
- No app was started, so there were no peers to stop.
