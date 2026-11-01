---
type: QA Pass Verdict
pass: "01"
date: 2026-11-01
stage: 215b12f
run_dirs: [20261101-131330-321, 20261101-132108-690]
scenarios: [S02, S03]
---

pass 01: gold 1/2, new S1/S2 2, coverage 2/2

## Gold tasks

| Gold | Scenario | Verdict | Evidence | Reason |
|---|---|---|---|---|
| G2 | S02 select and move | PASS | `shots/132206-464-resumed-s02-move-tool-after.png`, `shots/132220-279-resumed-s02-dragged-after.png` | The centre cube selected, its X gimbal moved, and REST read-back confirmed X=1.8780697584152222, previously 0. |
| G3 | S03 petal navigation | FAIL | `shots/132248-643-resumed-s03-petal-b-after.png`, `shots/132305-261-resumed-s03-tree-petal-a-after.png`, `shots/132314-792-resumed-s03-verse-level-after.png` | Cards and same-fractal tree navigation worked; a petal row from the verse-level view crashed the app. |

The resumed pass read each captured PNG. S02 used the actual arrow head
at (750,408), rather than the old scenario's guessed point. The uploaded
GLB renders three red cubes; one cube moved onto another after the drag.

## Findings by severity

| ID | Severity | Finding | Evidence / exact reproduction |
|---|---|---|---|
| QA-0001 | S1 | Sidebar petal navigation creates an invalid hierarchy and aborts the app | From Petal A with QA Basic expanded: `F click 246 52 --wait 10`, `F click 252 52 --wait 10`, `F camera` confirms no fractal id, `F click 75 353 --wait 10 --capture`. Before-crash PNG: `shots/132314-792-resumed-s03-verse-level-after.png`. Panic at `fe-policy/src/scope.rs:70`; call site `fe-ui/src/panels/sidebar.rs:461` invokes `navigate_to_petal` without the parent fractal. The client returned exit 3 (`request_dropped`). |
| QA-0015 | S2 | Maps can consume almost the entire central viewport | Prior probe PNG `shots/131138-720-s02-inspector-after.png` shows Maps covering x≈334..1279, leaving about 114px of the central scene. Open Maps with `F click 855 20 --capture`. The claim that it opened by itself is unconfirmed and is not used as a root cause. |
| QA-0004 | S3 | Status counts contradict the populated scene | `shots/132149-568-resumed-s02-petal-a.png` shows three cubes and `0 petals 0 models`. Enter Petal A via S02 steps 1–2, then `F shot`. Root cause unconfirmed. |
| QA-0006 | S3 | Selected cube has no visible outline/tint beyond its gimbal | `shots/132206-464-resumed-s02-move-tool-after.png`; `F click 640 370`, `F key g --capture`. Root cause unconfirmed. |
| QA-0007 | S3 | Enter did not submit Create Verse in the earlier probe | `shots/probe/130734-988-verse-created-after.png` retains the populated dialog. Historical probe only, not reverified in the resumed two-scenario run. Repro: S01 steps 1–2. Root cause unconfirmed. |
| QA-0008 | S3 | Escape does not navigate back after selection is cleared | `F key Escape --wait 10` clears selection, a second Escape stays in the petal; clicking Back works. `shots/132236-697-resumed-s03-escape-after.png` shows the first (deselect) action. Root cause unconfirmed. |
| QA-0016 | S3 | Inspector retains the last node's transform with nothing selected | `shots/132248-643-resumed-s03-petal-b-after.png` shows empty Petal B, Nothing selected, and X=1.878. Repro: move Cube 1, Escape, Back, Petal B. Root cause unconfirmed. |
| QA-0017 | S3 | Header crumb overlaps Back; footer control and hints are clipped/dim | `shots/132149-568-resumed-s02-petal-a.png`; default 1280×720 window with both sidebars. Root cause unconfirmed. |

`F` means `python tools/qa/fe.py`; screenshot paths are relative to this
verdict. IDs referenced by the unfinished probe are retained where evidence
supports them. The unsupported QA-0002 blank-Inspector claim was removed
from S02 instead of promoting it to a confirmed finding.

## Coverage

Bootstrap rotation: S02 and S03, both driven end to end. S01, S04–S08
were not part of this two-scenario bootstrap. S04/S05 additionally lack an
offline TerrainTileset: the shipped hexons have other kinds and the
installer skips them. The fixture records `terrain_bound=false`.

## Loop health

Python 3.12.10, standard library only. Stage 215b12f was reused: the
Rust/TOML/assets diff against HEAD was empty. This is source-diff evidence;
the inherited stage does not contain a reliable binary build provenance
manifest, so this pass does not claim a rebuilt current binary.

The first resumed launch (20261101-131954-417) stalled in native WebView
creation under the restricted sandbox. Exact PIDs 45648 and 11636 were
stopped. Launching in the owner context succeeded in 6.9s: health 3601ms,
bootstrap 1849ms; the fixture has one verse, one fractal, two petals and
three GLB nodes. Owner-only token ACLs required the client to run in the
same owner context. No Settings controls were used.

The final scenario reproduced the crash. `stop.ps1` reported PID 72004
and its tree already gone. No cargo was started; the integrated test
sweep remains scheduled for campaign end. Lifecycle safety and staging
provenance are independently reviewed after H3, before further passes.

## Convergence

Not converged: G3 failed and two new S1/S2 findings remain open. There is
no preceding completed pass. This artifact proves the harness can launch,
drive input, capture/read visuals, obtain persisted evidence, report a
crash as exit 3, and stop; it does not claim the application passed QA.
