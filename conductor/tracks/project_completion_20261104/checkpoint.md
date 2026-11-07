---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Current completion state and next actions
last_verified_utc: 2026-11-07T08:30:00.000000+00:00
---

## ORCHESTRATOR STEER (binding; maintained by the orchestrating session, do not edit or remove)

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

FULL QA pass 77 (1/3) on stage `a8e206c6-20261106225729173` (source `a8e206c6`, HEAD `c62aff4c` docs-only). It
covered every journey J01-J11 (J07/J09/J11 partly) and S-P2P-replication on 3 peers. Lean evidence is in
`qa/passes/77` (local, verdict.md). The register `qa/findings.jsonl` has 105 records. The pass is not converged and gold
is 1/11: only G4 terrain passes (raise 200 / lower -100 exact, 16384/16384 samples).

- New S1: QA-0099. Node delete always fails. GUI Confirm Delete, the Delete key, REST DELETE and MCP delete_node all
  return "TombstoneNode soft-delete failed". This is the live counterpart of QA-0077. The suspected cause is the
  `crud.rs` tombstone member-set guard comparing NONE against a rebound JSON null.
- New S1: QA-0100. When two peers commit different X for the same node at the same instant, they stay diverged
  permanently (7 vs -7), even after the editor restarts. This is a regression from pass 74 CP5.
- New S2: QA-0101. The Viewer's Join dialog reports "timed out" although the join succeeded. The Viewer never learns a
  native provider, so GLB placeholders and missing terrain follow.
- New S2: QA-0102. Clear Properties only clears the annotation keys.
- New S2: QA-0103. After a node leaves a group, its in-memory membership stays stale, so hiding the group still hides
  the node.
- New S2: QA-0104. The QA input bridge cannot orbit or pan, because no MouseMotion is emitted.
- Reopened S2: QA-0075. Proposal Delete and stamp-asset Clear icons paint missing-glyph squares again.
- New S3: QA-0105 to QA-0113. New S4: QA-0114.
- Closed by live re-run: 25 fixed findings, listed in verdict.md.
- Still reproducing: QA-0076, QA-0087, QA-0089, QA-0091, QA-0092 and QA-0093.
- Still open from before: QA-0077, QA-0078 remainder, QA-0079, QA-0081, QA-0082, QA-0086, QA-0088, QA-0090, QA-0094,
  QA-0098, divider eligibility, retained-store HOLD, and the release gates in `plan.md`.
- Layout-check found 0 measured violations in 36 runs (3 sizes x 10 panel states). OS/UIA checks stay out of scope
  because the sandbox returns desktop_refused.

## Next actions

1. Fix QA-0099 (tombstone guard) and QA-0100 (one deterministic tie-break shared by the docs entry and the DB merge).
   Then re-run J01 delete and S-P2P CP5 live on a fresh stage.
2. Fix QA-0101 to QA-0104 and the QA-0075 regression. QA-0104 needs synthetic MouseMotion in the input bridge.
3. Run FULL pass 78 (2/3) after the fixes, then pass 79. Convergence needs zero new S1/S2 in two consecutive passes,
   all gold PASS, and a strict checkpoint ledger.
4. Carried over: QA-0086 bootstrap persistence, QA-0087 rollback, the relay-lab and owner-restart P2P runs, QA-0081,
   test triage (QA-0077/0082/0094), CI Lint (QA-0078), the integrated sweep, CI, tag and release.

Only root owns Cargo/apps/process lifecycle/Git mutations. Preserve
`conductor/.conductor_session_log`. No source/focused approval substitutes for
remaining acceptance, and no release gate has been achieved.
