---
type: Review
date: 2026-11-08
title: Independent critic of the scripted P2P simulator, pass 4 (harness 84fcb68a/4257984b/3510bc35/fce97bf8 as of 6c4ed9c4, plus the product claims)
reviewed_commits: [84fcb68a, 4257984b, 3510bc35, fce97bf8]
reviewed_head: 6c4ed9c4
fix_commits: [d3b21945, 61b947ca]
previous_pass: b9e95206 (pass 3, PASS-WITH-FIXES (3), fixes 589b39b0 + f26aff49)
---

# Critic: scripted P2P simulator, pass 4

VERDICT: PASS-WITH-FIXES (5)

This pass ran in a fresh context and tried to refute the harness rules and the product claims. Nothing but
docs changed after pass 3 (f26aff49..6c4ed9c4 touches only `checkpoint.md` and this file), so the pass read the
harness as it stands at HEAD (`tools/qa/sim/*`, `tools/qa/fe.py`) rather than the four commits' original text, and
re-tested the product claims where pass 3 had not looked: the render-loop wake list and request-path panics.

It found five defects and fixed all five: four registered (QA-0169, QA-0170, QA-0171, QA-0172) and one
unregistered logging gap (item 4). Harness and product fixes are in separate commits.

## Fixed: harness (d3b21945)

1. **QA-0171 (S3): the prober's token reads could break the app's token writes.** This breaks the rule that
   probers must not perturb what they measure.
   - The bug: `fe.read_json_file` used plain `open()`. CPython shares read and write, but not delete. The prober
     reads each peer's `tokens.json` up to three times per round (`role_in`, `verse_token`, `debug_token`).
   - The app replaces that file atomically after every role change (`fe-identity` `replace_owner_only_file` ->
     `std::fs::rename`). Rust's rename falls back to a POSIX-semantics `FileRenameInfoEx` replace.
   - Reproduced on this host: with a plain-`open()` reader holding the target, that replace fails with
     ERROR_SHARING_VIOLATION (32). With a reader that grants `FILE_SHARE_DELETE`, it succeeds, and the reader
     keeps the old bytes.
   - The consequence: `fe-api/src/dev_token.rs` logs "could not write dev API tokens" and does not retry. The file
     then holds the old role until the next verse-set change, and so does the role probe and every token the
     driver presents. A missed demotion would read as a replication failure.
   - It is latent: across all 323 `.qa/run` roots there are 0 such errors (835 successful writes in the last 60).
2. **QA-0170 (S3, open since run 10): a read during the swap failed a GUI action.**
   - The fix for 1 and 2: `open_shared_text` opens with read, write and delete sharing through `CreateFileW`.
     `read_json_file` retries a PermissionError or a torn read for up to 1 s.
   - Dry run:
     - A torn file recovers when it is rewritten 0.3 s later. Otherwise it raises after 1.0 s.
     - A missing file still raises FileNotFoundError at once.
     - The replace succeeds while a shared reader is open.
3. **QA-0169 (S3, open since run 9): "Restart catch-up" measured the wrong thing.**
   - The bug: the figure took the last met expectation of the step. That included 12.e0/e1, which are anchored
     on the editor's own writes at +75 s and +76 s, so a 2-10 s catch-up read 61 s.
   - It also ran from the `peers.ps1 Restart` acknowledgement, which comes after the restarted peer has often
     already caught up. With only the right expectations, that gives a negative catch-up (-2.36 s).
   - The fix: count only the expectations with `after: <restart id>`. Time them from the restarted peer's first
     answered probe after the restart fired (the new "back up" column).
   - Re-analysis of copies of runs 10 and 9:
     - Catch-up now reads 2.26 s and 2.18 s, against 61.4 s and 62.0 s before.
     - Anomalies, severities and met counts are unchanged (11 {S3 8, S2 3} 310/317, and 8 {S3 8} 310/317).
   - The scenario half: step 11's prepare now presses `s` on the owner. Step 10 had left the Brush armed, and its
     hint covered the step-14 Query capture in runs 9 and 10.
4. **Logged argument lists were cut to 64 items silently.**
   - The fix: `driver._scrub` now appends `"<N more>"`. `args_digest` still covers the full arguments.

## Fixed: product (61b947ca)

5. **QA-0172 (S3): two queues that are drained every frame did not wake the reactive loop.**
   - `render_wake` must watch every queue a Bevy system drains (`fractalengine/src/AGENTS.md` §render-loop).
   - The scene-change queue (`drain_scene_changes_to_store`, which feeds the EntityStore) was never watched.
   - The fe-plugin result queue (`drain_plugin_results`) was never watched either.
   - Why it matters: the scene-bridge thread forwards a change after the DB thread's DbResult. The DbResult wake
     can therefore drain first and leave the change queued until the next idle tick (250 ms focused, 1 s
     unfocused). That delays the analytics store and the QA-0087 transform rollback to WS clients.
   - The fix:
     ```rust
     render_work.watch(scene_change_rx_bevy.clone());
     ```
     plus the plugin receiver, which is cloned from `PluginResultReceiver` right after `plugin_bridge::install`.
   - Both queues are also drained by `terminal_reset`, so the loop never stays awake on an undrained queue.
   - The queues that are still unwatched are named in the AGENTS note. Each waits at most one idle tick:
     - the tileset meta verdicts (left unwatched on purpose),
     - the one-shot API listener address,
     - the F12 save toast.

## Claims checked and upheld

- **One process clock.**
  - Driver, prober, events and observations all use `Clock.now()` (`perf_counter`).
  - `host.csv` wall times map through the single `(perf0, wall0)` anchor in `run.json`.
  - Prober observations carry the response's arrival time (`stamped`), not the time the round read it.
- **Barrier alignment is measured.**
  - `FireGate` fixes `fire_at` when the last participant arrives. A withdrawal never stalls the others.
  - The report's barrier table takes spread and late from the recorded `t_fire`, not the plan.
- **Probers stay under the API rate limits.**
  - `/query` (`claims.sub`) and the distributed timeseries query (`distributed:<sub>`) are separate 10/s buckets
    (`query_guard.rs:26`, `timeseries_query.rs:102`).
  - `_check_rate_budget` refuses more than 6 query or 8 timeseries targets per round.
  - `query_ms` is at least 1000 ms by schema.
  - Transient refusals are counted as errors, never recorded as an absent state.
  - With fix 1, the token reads no longer touch the app either.
- **The predicates are the scenario's.**
  - Every judged predicate comes from `expectations.jsonl`.
  - The analyzer's heuristics (moving targets, lap dismissal) are bounded by evidence and counted in the report.
- **Anomalies are never truncated.**
  - `anomalies.json` holds every anomaly.
  - The report's fingerprint list says "(first 20; all in anomalies.json ...)".
  - Oversize observations raise a harness anomaly.
- **Peers are stopped by exact identity on every exit path.**
  - `run.py`'s `finally` isolates each teardown and calls `peers.ps1 -Action Stop -OwnerRun`.
  - The live dry run ended with "All 3 exact recorded peer process trees stopped".
  - `stop_peer` and `restart` re-point the sampler's PID file.
- **Writes stay under `.qa/` and `qa/`.**
  - Runs go to `qa/sim/runs`, and spools to `.qa/tmp`.
  - Assets go to `.qa/assets`, and the active-verse cache to `.qa/run/<id>`.
  - The only repo write is `run.py schema`, a deliberate developer command.
- **Product claims.**
  - Unchanged since pass 3, which verified merge order, tombstones, role-at-write, restart catch-up and blob
    fetch.
  - A scan of fe-api's non-test code found no request-reachable panic:
    - `rows[0]` in `auth.rs` follows `generation_from_rows`, which requires exactly one row.
    - `serde_json::Value` indexing returns Null, not a panic.
    - The remaining `unwrap`s are mutex locks or startup code.

## Not fixed (residual, stated)

- **Superseding versus role-at-write (QA-0165 semantics).** Reserved for the user by the 02:45 steer, as in pass 3.
- **A `peers.ps1 Start` killed by the 1200 s timeout leaves its peers unrecorded.** The steer assigns the launch
  journal to the orchestrator's executor.
- **The dev-token writer does not retry a failed replace.** With fix 1, the harness can no longer cause one.
  - A retry in `fe-api` would harden the product against other readers, such as antivirus or indexers.
  - That is a product hardening item outside the simulator's reach, and it is noted on QA-0171.

## Gate (this phase)

- `cargo check -j4 -p fractalengine`: green.
- `cargo +1.99.0 clippy -j4 -p fractalengine --all-targets -- -D warnings`: green.
- Batches run: none. `fractalengine/src/main.rs` belongs to no region batch in `conductor/test-batches.md`, so no
  journey covers the wake list, and the policy forbids a new test function.
- Python:
  - `py_compile` passes for fe.py, analyze.py and driver.py.
  - `run.py validate` passes all scenarios.
  - Analyzer re-runs on copies of p2p-core 050437 and 043602 and city-lite 051726 give the same severities.
  - The shared-read and retry dry run is described under fix 2.
- Live dry run `qa/sim/runs/p2p-core-20261108-054813`:
  - Stage 1f01c930 (no non-doc source change since then), steps 1, 10, 11 and 14, N=3, affinity and tiling.
  - 42/42 expectations met, 0 anomalies.
  - `11.owner.key s` ok, and `shots/14-owner.png` shows the Query tab with 4 rows and no Brush hint.
  - Peers were stopped by exact identity. No peer is running.
