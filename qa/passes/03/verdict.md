---
type: QA Pass Verdict
pass: "03"
date: 2026-11-01
stage: d1b6e57-20261101150348464
run_dirs: [20261101-150528-519]
scenarios: [S09-native-dialogs-window]
scope: focused-surface-smoke
---

pass 03: gold 0/0, new S1/S2 1, coverage 0/9

This focused S4 smoke verifies dialog and window interaction boundaries. It
does not complete a full gold-task rotation or establish campaign convergence.
All checkpoint PNGs referenced below were opened and visually inspected.

## Native dialog and actual file operations

The final staged application ran with the basic fixture, isolated storage,
synthetic-input opt-in and the actual loopback listener at
`http://127.0.0.1:8874`. Its owned root PID was 56096. From the Genesis
verse-level view, clicking the named Petal A row restored the complete QA
Basic / QA Fractal / Petal A ancestry without crashing.

The viewport context menu's real Add GLTF Model action opened Import GLTF.
Queuing `fractalengine/assets/samples/info_panel.glb` as an open answer and
clicking Browse filled the real form. A queued cancellation preserved its
path and display name. Clicking Browse with an empty queue or a queued save
answer displayed typed `dialog_answer_missing` or
`dialog_answer_kind_mismatch` errors while the scene continued rendering.
No native blocking picker appeared.

Clicking the real Import button closed the form and persisted node
`01M6CJH46TW21NHBF6NAK1H4J8`, named info_panel, under Petal A. The hierarchy
API confirmed four nodes in that petal and the imported node's stored
`blob://fc0c7d7c98743c1053b9d83967eb05e759d40ad5d45911ef7a1052f0526d2a1d.glb`
asset path. The resulting frame visibly contains four red cubes; it does
not claim faithful imported asset materials or full rendering correctness.

After selecting info_panel through its sidebar label and scrolling the
Inspector, the real Download asset button consumed a queued save answer and
wrote `<run>/exported-model.glb`. Its SHA-256 and the imported run-local
sample's SHA-256 both equal
`ED52F7192B8311D700AC0CE80644E3852CD01537E4D62241B9ACBA023DA3D54E`.
A second UI export over the previously selected run-local input path
succeeded, demonstrating that completed import reads no longer pin that
file for the remainder of the run. The saved-path status was visible in
the Inspector and a toast.

| Checkpoint | Evidence |
|---|---|
| Open answer fills real form | `shots/150718-410-s4-browse-open.png` |
| Cancellation preserves form | `shots/150746-387-s4-browse-cancel.png` |
| Empty queue is typed and nonblocking | `shots/150750-858-s4-browse-empty.png` |
| Wrong kind is typed and nonblocking | `shots/150818-187-s4-browse-kind-mismatch.png` |
| Actual import completed | `shots/150858-229-s4-import-completed.png` |
| Actual export saved | `shots/151155-941-s4-export-saved.png` |
| Export overwrites released input | `shots/151206-126-s4-export-overwrite-released-input.png` |

Live path admission rejected `../escape.glb`, `model.glb:stream`,
`C:relative.glb`, `NUL.txt`, `COM¹.glb`, and `\\server\share\file.glb` with
HTTP 409 `dialog_path_not_confined`, without transient retries. A real
`<run>/s4-junction` junction targeting `.qa/s4-outside` was also refused for
`s4-junction/escape.glb`; that exact junction was subsequently removed
nonrecursively. A valid queued open cancellation survived a later rejected
traversal answer, then the Maps Install from file button consumed it without
an empty-queue toast (`shots/151355-287-s4-queue-preserved-after-refusal.png`).

GLTF open/save and Maps cancellation were exercised through real UI
consumers. GPX import/export, folder consumption, hostile concurrent file
replacement, and normal native-provider OS pickers were not exercised live
in this focused run. Existing extended journeys cover the relevant provider,
lease, path, queue and API invariants; their execution remains deferred to
the integrated campaign-end sweep.

## Physical window controls and app UI scale

Each size command waited on actual exported window dimensions. Required
sizes 1280×720, 1920×1080 and 1024×640 all matched; 1920×1440 also matched
while investigating the existing Inspector layout issue. These are window
control proofs, not claims that every layout passes at those dimensions.

Maximize reported 1920×1032 and `maximized=true`; Restore returned to the
previous 1024×640 size. Minimize reported `minimized=true` with 160×28
physical client dimensions, and the same API remained serviceable to
Restore, which returned 1024×640 and `minimized=false`.

At 1920×1080, `window ui-scale 1.25` reported both app scale and actual egui
pixels per point as 1.25. A named Maps button click resolved physical
coordinates (1388.33984375, 25) and opened Maps. Scale was restored to 1.0.
This is app UI-scale evidence; physical monitor DPI was not changed or tested.

Evidence: `shots/151224-200-s4-size-1280.png`,
`shots/150632-446-s4-petal-a-1920.png`,
`shots/151236-609-s4-size-1024.png`,
`shots/151256-628-s4-maximized.png`,
`shots/151301-668-s4-app-ui-scale-125.png`, and
`shots/151317-243-s4-named-maps-at-125.png`.

Both a debug Close request and the real caption X displayed the same
Close FractalEngine? confirmation, exported in `open_windows`; the process
remained alive. Cancel close dismissed it and preserved the app. The final
Close FractalEngine button removed the window gracefully; the input reply
reported `no_window` during teardown, then the listener stopped answering.
The OS WindowCloseRequested route shares the same implementation, but an
independent OS close event was not injected in this smoke.

Evidence: `shots/151330-089-s4-close-confirmation.png` and
`shots/151358-647-s4-caption-close-confirmation.png`.

## Findings and next slice

QA-0018 remains open. Selecting info_panel initially left the Inspector
mostly blank: Download asset was at y=1221 in a 1080-high window and y=1581
after resizing to height 1440. Scrolling exposed the controls. Evidence:
`shots/151100-933-s4-selected-import.png`,
`shots/151111-669-s4-tall-export-control.png`, and
`shots/151135-785-s4-inspector-scroll.png`. This reproduces without first
opening Maps, extending the earlier finding's trigger conditions.

QA-0019 is a new S2 input-routing finding routed to S6. With the pointer
inside the Inspector at physical (1760,1000) in a 1920×1440 window,
`F scroll -20 1760 1000` moved Inspector content into view and also zoomed
the camera. Orbit distance changed from 4.999999523162842 to 15.0 while
focus stayed [-0.38541126251220703,0,3.08420467376709]. Camera position
changed from [2.7769415378570557,2.2358551025390625,6.246557235717773] to
[9.10164737701416,6.7075653076171875,12.571263313293457]. This should be
resolved alongside the layout issues, without broadening S4.

## Build, provenance and cleanup

Source commits for S4 are 572da0b, c002e9f and d1b6e57. Final staging with
`-Jobs 2` passed: GUI build 4m17s, installer helper 46.38s, total staging
327.9s. Stage source fingerprint is
`9AD4695FFB03F3DF25CCCABC3347A836C244F86E06331E596B42263E33FBA2C8`;
executable SHA-256 is
`CF9D8FC753CDA05BF78CDDC4091BD3AD0A703D7CBCABD1EDA4D2885BC2F2CDA2`.
The manifest records base commit d1b6e5707b8400f838d10885dd598b28bb4aeafc
and `source_dirty=true`; launch validated the exact current fingerprint.
Local build log: `.qa/s4-final-stage.log`. There were no tests or clippy
runs; the single integrated sweep remains with the parent campaign.

Immediately before confirming close, the owned process tree was refreshed.
`stop.ps1 -Run 20261101-150528-519` completed at 15:14:34 local time.
Subsequent exact identity checks found zero live recorded identities among
root PID 56096 and its seven descendants. Stop correctly warned that
children born after the last captured snapshot cannot be proven absent
after root exit. No source/cargo process is retained by this slice.

Full convergence, same-run restart, actual app frame cost, terrain fixture,
normal OS accessibility-provider behavior, full naming audit and S6 layout
repairs remain outside this focused verdict.
