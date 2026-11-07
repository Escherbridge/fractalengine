---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Current completion state and next actions
last_verified_utc: 2026-11-07T06:22:00.000000+00:00
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

Independent critic 2026-11-06 over terrain `b0598bbc`, api-timing `ed2f8bb1`, ui-s2s3 `2a6dc781`, p2p `35ee29fc` (source `86095074`..`d6b3d1bc`; lean evidence `qa/passes/76`, register `qa/findings.jsonl` 89 records). Fix commit `a8e206c6`, live on fresh stage `a8e206c6-20261106225729173`:

- Reviewed and upheld: scale-bar pointer withholding (QA-0024/0071), truthful late input acks (QA-0034/0026), capture-camera survival (QA-0025), minimized restore (QA-0028), screenshot release (QA-0080), selection outline / Enter-submit / Escape back-nav (QA-0006/0007/0008), committed-capture refresh and stored-read version raise, peer terrain re-fetch dedupe, role re-resolve, `QA-OPERATOR` credential (scoped REST/MCP still refuse it; a Viewer stays read-only), peers harness (fresh run roots only, exact-identity Stop).
- Fixed: QA-0095 S3 (504 `input_not_started` could race a host that already admitted the batch; SeqCst fence pair in `InputProgress`), QA-0096 S4 (outline AABB walk allocated per node per frame), QA-0097 S4 (one `ResolveLocalRole` per replicated role row; now once per batch, journey asserts it and the active-petal terrain re-fetch). `fractalengine/src/AGENTS.md` §qa-operator now states the real widening: role-floor-only node routes (`POST /verses`, crate publish) admit the local operator.
- Live pass 76 (3 peers, real invite joins): outline on selected Cube 1, Escape walks Petal -> Fractal -> Verse, Revoke+Confirm drops the editor to `Current role: Viewer` in 0.4 s with edit tools disabled. AccessKit ui-tree served the Owner and the joined Editor (QA-0072 stays closed as superseded).
- New open: QA-0098 S4 (operator token cannot use `shot --focus-* --navigate` on joined peers; click the tree instead; widening is the orchestrator's call).
- Gate: `cargo check`, `cargo +1.99.0 clippy --all-targets -D warnings` clean on fe-runtime and fe-ui; fe-runtime `input::` 1/1; fe-ui R1+R2 379/382, the 3 failures are QA-0077's (reproduced with the critic changes stashed).
- Still open: QA-0086, QA-0087 (S2), QA-0088..0094, QA-0077, QA-0076, QA-0078 remainder, QA-0079, QA-0081, QA-0082, divider eligibility, retained-store HOLD, groups/selection, rich-data/concurrency/restart, permissions/lifecycle, catalog, UX audits and release gates in `plan.md`.

## Next actions

1. QA-0086: persist last-known peer EndpointAddrs per verse and merge them as bootstrap on replica open, then re-run CP6 without the harness bootstrap. Then QA-0087 (roll the GUI back on `SceneChange::TransformFailed`; re-mint the revoked peer's dev token) and QA-0088/QA-0091.
2. Extend the 3-peer run to the Podman relay lab (`FE_SYNC_RELAY` custom relay) and to an owner restart.
3. QA-0081 chunk-meshing budget, QA-0077/QA-0082/QA-0094 test triage, QA-0078 remainder for CI Lint.
4. Remaining feature/selection/groups/data/role/catalog work, five live UX audits, two FULL passes, a single integrated sweep, CI, tag and release.

Only root owns Cargo/apps/process lifecycle/Git mutations. Preserve
`conductor/.conductor_session_log`. No source/focused approval substitutes for
remaining acceptance, and no release gate has been achieved.
