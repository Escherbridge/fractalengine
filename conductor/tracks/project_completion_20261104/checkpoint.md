---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Current completion state and next actions
last_verified_utc: 2026-11-07T18:15:00.000000+00:00
---

## ORCHESTRATOR STEER (binding; maintained by the orchestrating session, do not edit or remove)

- 2026-11-07 13:15 (hand-over + security review of fix:1): fix:1 landed 573114dd b743d7e5 cf20e55e 988abcc0 (all 10 run-1 items fixed_source_live_pending incl. the QA-0119 reactive loop); coord:2 correctly bailed on the 10:40 steer; peer-swarm is resumed for its CRITIC only, with three mandatory security fixes filed as QA-0131 (S1, empowerment re-read re-admits rows written while unauthorized), QA-0132 (S1, re-judge queue drained before success = fail-open revocation), QA-0133 (S2, prefetch before admission). p2p-sim RUN starts after that critic commits. P2P-SIM TRIAGE: treat these three ids as exercised by p2p-core steps 13a/13b plus a new step 'revoke, revoked writes, re-grant: rows must stay absent on every peer' (add it to tools/qa/sim/scenarios/p2p-core.json before run 1 if missing). QA-0119 has landed in source: the Run resource gate may use N=4 if free RAM >= 6 GB and measured idle CPU per peer < 0.5 core.
- 2026-11-07 10:40 (direction change, user): swarm runs 2+ with LLM peer agents are SUPERSEDED by a pre-scripted simulator with per-action timeseries analysis (.omc/plans/p2p-sim.js -> tools/qa/sim/: scenario DSL, one-process concurrent driver with a threading.Barrier, probers, pandas analyzer, generated + Khronos glTF assets). The orchestrator stops peer-swarm after fix:1 and runs only its critic. TRIAGE:1 / FIX:1: finish run 1 normally; do NOT spend effort on swarm_barrier.py GUI-timeout or cross-agent barrier issues (superseded); spend it on product fixes (QA-0117 peer-book cap, QA-0118 event-driven blob fetch, QA-0119 reactive render loop + RequestRedraw from bridges, the run-1 product findings: -0.0 canonicalisation, atomic Clear Properties, loser-side convergence). Everything you mark fixed_source_live_pending is verified by the simulator's first run.
- 2026-11-07 10:15 (QA-0119, S2, host contention — the user saw the peer UIs glitching): CPU is the bottleneck, not GPU/RAM (total 100%, fractalengine x4 = 57% of 12 logical cores, GPU 18%, 6 GB free). Root cause fractalengine/src/main.rs:511 forces `WinitSettings::continuous()` whenever QA is enabled, and Bevy's default is continuous as well, so an idle window burns 1.5-2.3 cores. FIXER (next Fix phase; this also makes run 2 viable at N=4): (a) default to `WinitSettings::desktop_app()` (focused `UpdateMode::reactive(5 s)`, unfocused `reactive_low_power(60 s)`) for every build; (b) QA determinism without a blanket continuous loop: input_bridge, the screenshot/vision bridge, ui-tree/ui-state probes and the DB/sync result drains send `bevy::window::RequestRedraw` when a request arrives and on every frame while a job is active (an injection, a posed capture, a settle wait), so the loop runs only while there is work; keep FE_QA_A11Y_CONTINUOUS as the explicit opt-in it already is; (c) verify with the existing bridge journeys that a synthetic input batch and a posed capture complete within their budgets under desktop_app(); (d) measure idle CPU per window before/after and put the numbers in the commit body. COORDINATOR (run >= 2): resource gate must include CPU: until QA-0119 lands, N = min(RAM rule, floor((logical_cores - 4) / 2.5)) = 3 on this 12-thread host; after it lands, N=4 is fine. Also cut harness load: observer polls every 2 s (not 500 ms), peers take ONE screenshot per step (no +25 s/+4 min re-shoots unless a verdict is 'diverged'), and peer pollers run at 1 Hz. TRIAGE: step verdicts and latencies from run 1 taken while CPU was pinned at 100% are upper bounds, not product numbers — say so where a latency is cited; do not file S2 latency findings from run 1 alone except QA-0118 (14 min cannot be contention). CRITIC: confirm no bridge job can stall because the loop went to sleep (every wait path requests a redraw).
- 2026-11-07 10:25 (QA-0119 follow-up, run >= 2 harness): after killing two runaway processes and boosting peer priority the peers simply expanded to 82% of all cores — continuous loops consume whatever is free, so only the reactive-mode fix changes this. Stopgap applied to run 1 by the orchestrator: disjoint CPU affinity per peer (0x007/0x038/0x1C0/0xE00). COORDINATOR: (a) until QA-0119 lands, peers.ps1 sets the same disjoint affinity at launch (Start-Process then ProcessorAffinity by exact PID) and N=3; (b) the visible windows are on the user's desktop: use scene-mode (`--no-ui`) captures for every checkpoint that does not assert UI chrome, since window-mode capture poses, settles and restores the camera in front of the user and reads as glitching; keep window-mode only for UI assertions; (c) tile the peer windows (non-overlapping, e.g. 2x2 at 960x540 on a 1920x1080 desktop) so occlusion does not add present stalls.
- 2026-11-07 10:05 (swarm run 1, steps 3-8 read live): step 6 is the first real proof of QA-0100 — barrier 6 released three REST PATCHes within 28 ms (owner 28.0 / editor 14.5 / editor2 8.5 ms late), all answered ok:true persisted, and all 4 peers converged to N2 position [-5,0,0] (editor 1.3 s, editor2 1.9 s, owner 11.2 s). Step 7 proves QA-0099 live: cube3 (GUI Delete+Confirm), n3 (REST) and cube2 (MCP) are 'node not found' on every peer with cascade tombstones. TRIAGE: mark QA-0099 and QA-0100 fixed_verified AT THE REST/MCP LAYER with this evidence, but keep a GUI-race sub-item open because step 5 did NOT exercise the race: barrier 5 timed out (180 s) with editor2 still preparing its GUI action (owner/editor then fired solo 128-178 s late), and 5b released 3-ready but the owner's --then never produced a done.json. That is a HARNESS defect, not a product result — do not file it against the product. COORDINATOR (run >= 2): (a) GUI-actor steps: every actor PREPARES fully (navigate, select the node, focus the field) BEFORE calling the barrier, and --then is one pre-validated fe.py command (give fe.py a --dry-run that parses+resolves without sending, and have each actor run it before the barrier); (b) --timeout 600 for GUI steps, 180 for REST; (c) the helper writes NAME.failed.json {error} when --then raises, so a silent miss like owner/5b cannot recur; (d) add step 0b: a no-op barrier with ALL peers to prove the sync before step 1; (e) state per step exactly who must call the barrier (count = named participants; non-actors call without --then) — the viewer skipped 5/5b. FIXER / TRIAGE classify from the same observations: (1) editor2 reads n1 rotation as [-0.0, 0, -0.0] where other peers read [0,0,0] after a REST/MCP write — negative zero leaks through one door and will break byte-for-byte fingerprints: canonicalize -0.0 -> 0.0 at the write boundary (S3); (2) Clear Properties replicates non-atomically — the viewer saw {sw_owner} alone for ~1.2 s between the full map and {} (step 8) — batch the clear into one replicated op or clear the property map as one register (S3 unless a user can act on the partial state); (3) the race LOSER (owner, step 6) took 11.2 s to converge vs 1-2 s for the others — check whether the AppliedKeptLocalTransform / stored-row scene-change path waits for a periodic tick instead of applying on receipt (S2 if it exceeds the 15 s budget under load). UX INTAKE (polish §QA-intake, RELOCATE): 'Clear Properties' is not in the node row's Options menu; it is only reachable via Options... -> Node Options dialog -> Clear Properties -> Confirm (editor2, step 8). CRITIC: confirm the step-6 convergence rule (edit_seq, then total_cmp tiebreak) is what actually selected [-5,0,0], not arrival order.
- 2026-11-07 09:25 (QA-0118, from swarm run 1 live evidence): the owner's placed GLB blob reached viewer/editor2 only after ~14 min (stdout 'Blob missing' 15:43->15:57 UTC) and the editor still lacked it at 16:00+ (471 warnings, placeholder cards); asset_path metadata was identical on all peers within seconds. editor's hash-named file /data/blobs/fc/fc0c7d7c98743c1053b9d83967eb05e759d40ad5d45911ef7a1052f0526d2a1d size=1664 (owner 1664) mtime=09:00:03 local. Scene-asset BLOB fetch is therefore not event-driven. TRIAGE: (1) BEFORE stopping any peer, capture from each live peer: fe.py --peer X logs (all), ui-state, one Petal A screenshot, peers.ps1 -Action Info, and the run dir's hash-named file size/mtime + data/p2p size — stopping first destroys the only live evidence; (2) classify QA-0118 S1 if the editor never received the bytes, S2 otherwise; keep it in open_s1s2. FIXER: in fe-sync (scene_assets, replicator SceneProviderRoster, the 'tileset re-advertise' and 60 s per-peer cooldown added in 64b90964) make a blob referenced by an arriving node row fetch IMMEDIATELY from any confirmed provider on InsertRemote/SyncFinished, with bounded exponential retry in seconds and one log line per attempt; turn the load_hierarchy_handler 'Blob missing' loop into one warning per asset until its state changes. Extend the fe-sync asset-replication journey with 'row arrives -> blob fetched within N s'; no new test fn. COORDINATOR (run >= 2): launch peers with FE_QA_RUST_LOG='info,fe_sync=debug' so provider/fetch lines reach stdout.log (launch.ps1:185). PEER AGENTS: for any asset step, record your run dir's hash-named file size/mtime and the first/last 'Blob missing' timestamps in step-K.json. CRITIC: verify the fetch is event-driven and bounded.
- 2026-11-07 09:30 (QA-0118 precision): blob file mtimes in the peers' data/blobs/fc/ — owner 08:41:57 (created), viewer 08:57:52, editor2 08:57:51, editor 09:00:03 local. Two peers fetching within ONE second of each other ~14 min after creation, and the third on a later tick, means the trigger is a periodic provider-side announce/re-advertise cadence (or a shared timer), not per-peer retry backoff and not 'never': classify QA-0118 as S2 and look for the ~14-15 min period first (re-advertise interval, roster refresh, or a SetLatency/cooldown multiple) — the fix is to also fire on row arrival. Check too whether the editor GUI re-renders the cubes after the late arrival or keeps placeholder cards until a petal re-open (that would be a second, UI-side defect).
- 2026-11-07 08:45 (QA-0117, S2, from the security review of 64b90964): `peer_book::merge` at fe-sync/src/sync_thread.rs:1006 appends up to 16 remembered peers to the explicit bootstrap/invite peers with no cap on the TOTAL, and `scene_assets::opened_with_roster` (fe-sync/src/scene_assets/mod.rs:199) refuses the whole scene-asset layer when `peers.len() > 16`, so a saturated book + one invite peer = assets unavailable on open (the QA-0063 symptom by another route). The recorder (fe-sync/src/peer_book.rs:71) never evicts, so 16 stale addresses never heal. FIXER (next Fix phase, or the first fixer that touches fe-sync): (a) merge at most `16 - peers.len()` book entries so explicit peers always win and the total never exceeds the downstream limit; (b) make the book an LRU keyed by last-synced time: entries carry `last_seen`, evict the oldest entry not in the current synced set when full, drop entries older than 7 days on load, and load the old `Vec<EndpointAddr>` format compatibly; (c) confirm `SceneProviderRoster::peers()` holds only sync-confirmed peers (never forwarded `InsertRemote.from` entries) and say so in fe-sync/src/AGENTS.md §peer-book. Extend the fe-sync peer-book/restart journey with the saturation case; no new test fn. TRIAGE: keep this id in open_s1s2 until a fixer closes it. CRITIC: verify the cap and the eviction.
- 2026-11-06: none yet. The orchestrator adds dated steers here; every agent must apply them over any other instruction except safety rules.


# Current completion state

The full goal remains active in [goal-service.txt](./goal-service.txt),
[spec.md](./spec.md) and [plan.md](./plan.md). Prior checkpoints are preserved
verbatim in [checkpoint-history.md](./checkpoint-history.md). No gate is waived.

- Main source: `9498869e2e1577e939a10f0a1e0f9e07de3b7053` (HEAD `7902ee6203b483d8ae0ecf244e00e351de7a381f` with source-batch documentation). Stable native root/transport correction `fed8dab6` and painted icons `9498869e` each passed separate source and exact integration reviews.
- Matching production stage: `.qa/stage/9498869e-20261104204501360`, source `9498869e`, session19551 terminal exit0,256.8s, zero actual Rust/MSVC warning/error diagnostics. App/native-helper/legacy-fixture/sealer hashes verified.
- Owned apps/builds: none running. QA60 (`PID 126140`, run `20261104-204745-332`), QA61 (`PID 103968`, run `20261104-232148-169`) and QA62 (`PID 127884`, run `20261104-232515-148`) all stopped cleanly; all eight recorded identities per run verified absent (`06:19:38.1331795Z`, `06:24:41.6229843Z` and `06:25:33.2439957Z` on `2026-11-05`).
- QA60, QA61 and QA62 frozen packets have independent full-packet approvals (plus independent visual review for QA60's 10 PNGs). The 5-file read-only `DividerEligibility` diagnostic candidate (`qa59-divider-eligibility-v1`, manifest SHA-256 `5d596db66f3288b38aca4e6f3ca2c932c50d6a0a8326fd4b5e75ddc339756029`) has independent source correctness and design/boundary approvals. Proposed peer/relay/query lab plan (`p2p-relay-query-lab.md` and `evidence/deployment-fixtures/`) has independent Conductor review approval.
- No final integrated sweep, formatter, push, CI, tag or release has occurred.

## Focused evidence since the previous checkpoint

| Pass | Bounded outcome | Manifest SHA-256 |
| --- | --- | --- |
| 60 | Matching9498869e GUI: 10 PNGs (1024/1280/1600 at 1x/2x + 1159/1160/1161 at 1x + baseline), zero missing-glyph squares (`□`), clear selected tree row at 1x/2x, `whole_targets_fit=true` all 9 cases; `1160`/`1161` `Deselect` overlaps `Maps`/`Settings` (`-72.8125px`/`-71.8125px`) | 42b536d3b67c6d9f49760e08befe190b298a6df0e4f09d80bcf7cc90eb2dad17 |
| 61 | Matching9498869e native preflight refusal: separate launch command exited and closed Windows Job Object before `native observe`; controller returned `native_owned_boundary_unavailable`, `attempt_spent=null`, 0 workers | 8e94af03358ccb239a0b73c42a5a775e85e970ab25155f25ee10873dda9b0758 |
| 62 | Matching9498869e native worker execution: single-session launch + 2 `native observe` calls on live `PID 127884` spawned workers `128468`/`128856`; both passed controller admission, bootstrap ledger and `Process` image/session/token checks, then exited (`exit 2`, `worker_exited`, `attempt_spent=false`) with `desktop_refused` under `exebox` thread desktop | ea440f80f6d9a5f347b185130d2264dbc1adf9b7af78e520ac6a1cece5a76c32 |

QA60 has153 payloads/10 actual PNGs, QA61 has17 payloads/no PNGs, QA62 has20
payloads/no PNGs. Both root and the independent visual reviewer inspected all10
QA60 screenshots. Raw app logs stay private, with strict-decoded summaries and
finite credential pattern checks retained. These are focused passes, never FULL
coverage.

In QA60, every icon-only and icon+label surface rendered by `paint_icon` in
`9498869e` (topbar brand/workspace/tool/section/window buttons, Inspector rail
tabs, `Deselect object`, hierarchy visibility eyes, disclosure chevrons and
custom-property `Edit`/`Copy`/`Delete` row actions) is crisp, legible and free of
missing-glyph squares (`□`) at both `1x` and `2x`. The selected hierarchy row
(`QA60 icon probe`) is wholly inside the `Hierarchy` clip and clearly highlighted
at both `1x` and `2x`. At `1160x720@1x` and `1161x720@1x`, `topbar.rs:742`
(`ctx.viewport_rect().width() < 1160.0`) switches from compact to expanded mode
while the selected-node `Deselect object` button extends the left cluster to
`743.5` logical points (`743.5 + 489.3125 = 1232.8125 > 1160.0`), overlapping
`Toggle Maps section` (`55.5625px`) and `Toggle Settings section` (`9.25px` /
`8.25px`). Raising the compact threshold to `1248.0` resolves this overlap across
all viewport widths. `qa/passes/60/topbar-layer-oracle.json` also separates
unclipped topbar controls (`clip.y <= 1.0`) from scrolled-above-clip `Hierarchy`
(`clip.y = 113`) and `Inspector` (`clip.y = 191`) controls that `qa60_matrix.py`
bundled under `y < 40 * ppp`.

In QA62, running `launch.ps1`, both `native observe` invocations and `stop.ps1`
inside one command lifetime kept `PID 127884` live and proved that `fed8dab6`'s
worker transport returns bounded structured JSON (`fe-native-worker-result-v1`,
`helper_exit_code: 2`, `terminal_code: "worker_exited"`, `attempt_spent: false`)
on both initial and follow-up calls without `json_size_refused` or `exit 1`. Both
workers refused at `worker.cpp:170-171` (`desktop_refused`) because the host
sandbox attaches command threads to desktop `exebox-*` instead of `Default`. Per
boundary rules, `desktop_refused` is preserved intact and never bypassed.

## Current blockers and source work

Convergence-fixer recovery (2026-11-07): the dead fixer's 52-file tree was gated and committed. Source-fixed and
live-checked once on pass-78 stages, status `fixed_source_live_pending` until the swarm re-verifies: QA-0099
(tombstone guard), QA-0100 (transform LWW register in merge.rs + all-author snapshot), QA-0101 (join readiness,
dial-race resync, tileset re-advertise), QA-0102, QA-0103, QA-0104 (synthetic MouseMotion), QA-0075 (glyph
coverage), QA-0077 (17 journeys green), QA-0086 (persistent peer book) and QA-0087 (transform verdict + rollback).

- Gate: cargo check + `cargo +1.99.0 clippy --all-targets -D warnings` on fe-database/fe-sync/fe-api/fe-ui/
  fractalengine green; fe-ui lib 1057/1057, fe-database lib + touched integration batches, fe-sync 295 + 4, fe-api
  lib 95 and R9 binaries green except pre-existing failures (QA-0115/0116 re-run on a clean 1f8ef7d2 worktree):
  QA-0079 shared_handle_test, QA-0082 (terrain_and_tileset, token_lifecycle, vision_test 409, vision_bridge fov),
  new S3 QA-0115 ws_integration socket reset and QA-0116 fuzz never reaches get/set_field_schema.
- Still open: QA-0076, QA-0078, QA-0081, QA-0088 to QA-0094, QA-0098, QA-0105 to QA-0116, divider eligibility,
  retained-store HOLD, and the release gates in `plan.md`.

Swarm run 1 triage (2026-11-07, stage 9da98227, 4 peers; all peers stopped by exact identity; evidence
`qa/swarm/run1-20261107/swarm/`, register `qa/findings.jsonl`). Latencies were measured at 100% CPU (QA-0119) and
are upper bounds.

- Converged: steps 2-8, 10-11 (node rows 0-4.5 s; delete via GUI/REST/MCP; 28 ms 3-way REST race; Clear Properties;
  relay-less restart via the peer book). Verified: QA-0086, QA-0087 (transform path), QA-0099, QA-0100 (REST layer;
  the GUI race never ran, harness), QA-0102.
- Not converged: the final fingerprints differ on one key (Cube 1 `fp_deny_editor_mcp`, editor only). The
  visibility-group registry reached other peers after 47 min, and N1 was split between the owner and the other
  peers after the revoke.
- New S1: QA-0120 (a revoke from the verse list waits until the owner re-enters the verse; owner replica closed
  17:26-17:39Z), QA-0121 (a row admitted before the revoke arrived is never re-checked, so peers split),
  QA-0122 (REST/MCP property writes use the token's role, so a revoked editor writes locally),
  QA-0123 (the petal `visibility_groups` registry is never published when it changes).
- New S2: QA-0124 (a GLB that arrives late keeps its placeholder until the petal is re-entered), QA-0125 (each
  REST hierarchy read rebuilds the GUI tree and drops typed edits). QA-0103 reopened (stale membership on
  replicated peers). QA-0118 refined: blobs are fetched when a peer opens the petal (0-3 s), never when the row
  arrives. New S3: QA-0126 to QA-0130.
- QA-0101 stays pending: the join half is verified, but the terrain half never ran (step 10 skipped "Seed map").

## Next actions

1. FIX:1 (product): QA-0122, QA-0120, QA-0121, QA-0123 (S1), then QA-0125, QA-0124, QA-0103, QA-0117, QA-0118,
   QA-0119 (S2) and the steer's S3 items (QA-0128 -0.0, QA-0129 atomic clear). The tools/qa/sim simulator then
   re-verifies the GUI race (QA-0100), the seeded-terrain half of QA-0101 and every new fix.
2. Run FULL pass 78 (2/3) then pass 79; convergence needs zero new S1/S2 in two consecutive passes and all gold.
3. Fix the S3 test debt (QA-0079, QA-0082, QA-0115, QA-0116), then the integrated sweep, CI Lint (QA-0078), tag
   and release.

Only root owns Cargo/apps/process lifecycle/Git mutations. Preserve
`conductor/.conductor_session_log`. No source/focused approval substitutes for
remaining acceptance, and no release gate has been achieved.
