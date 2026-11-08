---
type: Review
date: 2026-11-07
title: Independent critic of the scripted P2P simulator and its fix waves
reviewed_commits: [84fcb68a, 4257984b, 3510bc35, fce97bf8, fe1e6b75, 11b44d4f, cfd7fce6, 47f447e9, fb8877e9, 5067e466, 6b427aee, cb55a7e2, 6267e951, 48edbb2f, 0ce45566, 5b229880, ace5388b, f7a3b864]
also_read: [745edd5e, e89c9f7b]
fix_commits: [9cfbdd4f, c357bf03]
---

# Critic: scripted P2P simulator (harness + product fixes)

VERDICT: PASS-WITH-FIXES (12)

Fresh-context review set out to refute the claims. It covered the 18 listed commits, plus the harness
commits that landed while it ran (745edd5e QA-0152, e89c9f7b QA-0153), because the critic fixes
touch the same files. The mandatory steers QA-0154 and QA-0155 were fixed here. One gap that is
outside the reviewed diffs was registered (QA-0156) and not fixed (see "Not fixed").

## Fixed: product (9cfbdd4f)

1. **QA-0154: a Serve admission jumped queued security writes** (`fe-database/src/command_lane.rs`).
   The lookahead took the first Serve admission anywhere in 128 queued commands, so a queued `RevokeRole`
   (or role/ACL/replicated-row apply) could be judged after the serve. Now
   `take_while(is_transparent).position(is_serve_admission)`. `is_transparent` is an allowlist of reads,
   transforms, readings and device payloads. Every other variant is a barrier, including any variant added
   later (it fails closed). Trade-off, documented in fe-database `AGENTS.md` §command-lane: an admission
   behind an `ApplyReplicatedRow` burst waits again. QA-0143's other half still holds (busy means
   `RateLimited`, code 2, and the requester retries). Journey `shared_handle_test`: the serve still jumps
   a 40-read burst, and a revoke queued ahead now lands before the admission. **Mutation check:** with the
   barrier disabled the journey goes red at the new assertion ("the queued revoke ran before the admission").
2. **QA-0155: the re-fetch table could be bypassed with Sybil authors and dropped hashes for good**
   (`fe-sync/src/replicator.rs`, `docs_engine.rs`).
   - Only an entry author that `BlobAccessGate::names_member` admits can arm a re-fetch. That means the
     owner or a role-view DID. `open_to_all` is ignored, because otherwise any key would pass.
   - Other rows wait unarmed (capped at 16384). A 5 s absolute-deadline sweep arms them once their author
     is named, and never evicts anything while doing so.
   - Each admitted author waiting on a hash holds its own slot in it.
   - A full table evicts only spent entries. A member at its cap evicts only its own entries.
   - After 10 fast attempts a hash retries every 5 min. A provider it has not tried yet (up to 16 per hash)
     restarts the fast attempts.
   - Journey `doc_replicator_replicates_row_over_real_transport` covers: 100 Sybil authors cannot evict an
     honest hash; an author named later arms on the sweep; a full table refuses a new hash rather than evict
     an unspent one; only a spent entry makes room; a second provider revives a spent hash. The QA-0145
     late-content leg now also proves gated arming, because the row lands only after Bob's role view names
     Alice.
3. **Reactive loop: some drained queues never requested a redraw** (QA-0119 criterion; `main.rs`,
   `terrain_bridge.rs::watch_results`). These queues are drained every frame but were not watched, so their
   work waited for the idle tick (up to 1 s while unfocused): network events, lifecycle events, and the
   terrain assemble and streamed-bundle results. The meta-verdict queues stay unwatched on purpose:
   `discard_terminal_state` drops their drain, and a watched queue nobody drains would keep the loop awake
   forever.

## Fixed: harness (c357bf03)

4. **Probers recorded rate-limited answers as state.** `/query` answers a rate limit with `200 ok:false`.
   `_round` turned that into `{"absent": true}`, which can satisfy `absent`/`excludes` and open a false
   divergence episode. Now `is_transient` (5xx, 408, 429, or a rate-limit/busy/timeout envelope) counts as
   a tick error and is never recorded as an observation.
5. **Probers could cause the refusals they measure.** `/query` and the distributed timeseries query allow
   10/s per DID, and the prober shares the driver's DID. The driver's `lookup_node` already polls `/query`
   at 2 Hz. `Prober._check_rate_budget` now refuses a scenario whose active query targets exceed 6, or whose
   timeseries targets exceed 8, at any step. The current scenarios peak at 2 of each.
6. **Observation timestamps were taken when the round read the result, not when the response arrived.** A
   slow sibling request (a 0.4 s node read in the dry run) added its delay to every later job's latency.
   Each job is now stamped as it completes.
7. **A barrier could hang for its 120 s timeout.** A barrier action whose `requires` peer was absent
   returned before `gate.wait`, so the other participants waited out the timeout, and a `prime_gui`
   exception killed the thread the same way. The live-barrier filter now checks `requires`, and a
   `prime_gui` failure aborts the gate.
8. **Fingerprints were truncated silently.** A fingerprint compared at most 400 nodes per petal and gave
   no sign of it. Now an `X <petal>` key carries the full node count and the run log names the cut.
9. **Peer leak on an unexpected Start record.** `peers.ps1 Start` could exit 0 with a record that
   `_read_record` rejected, which left `owner_dir` unset, so cleanup skipped running peers. The pointer
   that Start wrote now reaches cleanup.
10. **Peer leak on Restart.** `peers.ps1 Restart` threw on a changed identity before recording the new
    process, so Stop could never reach it. Restart now records the process first and checks afterwards.
11. **Writes outside `.qa/` and `qa/`.** The PowerShell output spools used `%TEMP%`. They now go to
    `.qa/tmp`.
12. **Truncated key list in the report.** The fingerprint line listed only the first 20 diverged keys
    without saying so. It now says so, and the full list stays in `anomalies.json`.

## Claims checked and upheld

- **One process clock.** The harness uses `Clock.perf_counter`. `host.csv` is converted through one
  wall/perf anchor pair. Only deadlines and payload stamps use other clocks.
- **Barrier alignment is measured.** Each barrier event carries `fire_at` and `fire_late_ms`, and the
  report's "Barrier alignment" table prints spread, worst lateness and RTT per step.
- **Predicates belong to the scenario.** The analyzer evaluates the scenario's predicate, path and value.
  Severity defaults to S2/S3 only when the scenario sets none.
- **The anomaly list is never truncated.** The report lists every anomaly, and macro bursts aggregate
  with full `action_ids`.
- **Tombstones win and never resurrect.** `merge.rs` keeps tombstone dominance. In the QA-0145 re-fetch, a
  newer entry in the same slot (including a `del` marker) drops the older waiting content, so late content
  never lands over a newer row.
- **Role checks on receipt.** The DB applies `role_gate` and `admit_inbound_row` to every
  `ApplyReplicatedRow`, including rows kept and resent after a full channel (QA-0138). QA-0137's Serve
  still requires the requester's current Viewer role.
- **Restart catch-up has no stale window.** Stable ports keep the address peers cached, content re-fetch
  covers the failed docs download, and isolated-write sync covers a missing gossip neighbor.
- **Blob fetch is event-driven.** A fetch starts on row arrival, NeighborUp, empowerment, an owned fetch
  or a rewrite (QA-0143 wakes), with a capped back-off in between. The sync loop wakes at 1 Hz while the
  unavailable set is non-empty. That costs a little but is bounded.
- **No panic reachable from a request in the new code.** I checked the indexing in `ts_partial`, the
  lane's `remove`, `variant_name`, the decision tasks in `docs_engine`, and the unsafe Win32 calls in
  `topbar` (argument-only).
- **QA-0142 coalescing is sound.** The serial DB thread and its FIFO result channel make "any
  HierarchyLoaded satisfies earlier requests" valid.

## Not fixed (registered)

- **QA-0156 (S2, open).** Replicated node rows merge every non-transform field (`properties`,
  `display_name`) in arrival order. Only the transform is an `edit_seq` register with a named tiebreak
  (`merge.rs`). The reviewed commits did not introduce this. However, QA-0145 re-fetch and QA-0138 kept
  rows widen cross-author reordering, so two authors' concurrent property writes can converge differently
  on different peers. It has not been observed yet: run 4's final fingerprints agree. A per-key or per-row
  clock is a schema and write-path change across `SetNodeProperty`, `DeleteNodeProperty` and the editor
  batch, so it belongs in a fix lane with its own journey, not in a critic patch.
- **QA-0079 (pre-existing, open).** `shared_handle_test` still ends red at its restart re-open: os error 33,
  a SurrealKV lock, on Windows. This happens on HEAD without any critic change, and also with the burst
  removed and with plain `recv()` instead of the lane. The new QA-0143 and QA-0154 assertions all pass
  before it.
- **Residual, documented.** If a `peers.ps1 Start` is killed by the harness's 20-minute timeout, peers it
  launched have no record to stop them by. This is mitigated only by peers.ps1's own rollback.

## Gate (this phase)

- `cargo check -p fe-database -p fe-sync -p fractalengine`
- `cargo +1.99.0 clippy -p fe-database -p fe-sync -p fractalengine --all-targets -D warnings`: green,
  re-run on the committed tree.
- Named journeys, per the QA-0154/0155 steers:
  - `cargo test -p fe-sync --lib doc_replicator_replicates_row_over_real_transport`: 1 passed.
  - `cargo test -p fe-database --test shared_handle_test api_reader_reads_db_thread_writes_through_the_shared_handle`:
    the new assertions pass, then it hits the pre-existing QA-0079 failure.
  - Touched batch R4 subset `cargo test -p fractalengine --bin fractalengine -- terrain_bridge::`: 40 passed.
- Python dry run:
  - `run.py validate`: 4 scenarios OK.
  - Prober budget refusal.
  - Offline prober round: per-response stamps, and a rate-limited query was not observed.
  - Re-analysis of run `p2p-core-20261107-202131` (on a copy).
  - `run.powershell` spool under `.qa/tmp`.
- No app was started, so there were no peers to stop.
