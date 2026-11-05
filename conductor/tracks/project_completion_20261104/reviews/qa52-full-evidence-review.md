---
type: Review
title: QA52 focused evidence review
date: 2026-11-04
status: approved_at_recorded_scope
---

# QA52: independent corrected-case evidence review

**APPROVE at the recorded single-object CtrlLeft/ShiftLeft numeric-replay and
camera-ownership scope; no integrity blocker or material erratum.** Reviewer
`/root/legacy_helper_io_fix`, 2026-11-04 local, did not author this packet or the
camera correction. Only this review is written; frozen payloads remain unchanged.

Manifest SHA-256 `5b64160f777d41757931fae9999edffa8c4e6cd697ee91d607a75adf1cd90ddd`
matches; all52 payload sizes/hashes and original-artifact anchors match. Seven
context/cleanup/launch/stop/stage-log/receipt/manifest copies match originals.
All five published PNGs match original run-local image bytes and response sizes;
**all five actual1280x720 images were inspected**.

Raw actions focus X once, Ctrl+A, text_entry2.500 without refocusing, scroll and
one Apply. Complete draft/scrolled AccessKit textinputs contain exactly2.500.
One CtrlLeft+Z and one CtrlLeft+ShiftLeft+Z are recorded with successful command
counts. Complete stored reads and history independently establish:

| Read | X | Version | Edit sequence | History entries |
| --- | ---: | ---: | ---: | ---: |
| Before | 0 | 4 | 0 | 4 |
| Apply | 2.5 | 5 | 1 | 5 |
| Undo | 0 | 6 | 2 | 6 |
| Redo | 2.5 | 7 | 3 | 7 |
| Final, after far-view restoration | 2.5 | 7 | 3 | 7 |

Original complete entries1..4 are identical prefixes. Each new entry is
full_transform with matching after-state position; current_state_verified=true
and no next page. Y/Z and every non-position/non-edit_seq row field, including
properties/rotation/scale, remain unchanged. Final node and history bytes equal
Redo, not merely equivalent projected values. Final selected state reports
X2.5/Edit saved and no known pending UI operation; its settled definition still
does not certify DB/sync/terrain completion.

All seven **complete camera API responses**, not just selected coordinates,
independently share SHA-256
`c7ec7ee8e7103c904f75619711cee58618ac321392138398313717151f1e577c`:
two baselines, Apply, Undo, Redo, restored and final. Complete pose comparisons
have zero mismatches: focus[0,0,0], position[3.1623528,2.2358551,3.1623528],
yaw45/pitch26.5623245/distance5/FOV45. The separate temporary far capture requests
position[8,6,8]/target[0,0,0], reports settled8 frames/restored=true, and its
subsequent restored/final API responses confirm the original pose exactly.

Visual evidence agrees:171412 draft shows X2.500 and selected Cube1;171503 Apply
and171554 Redo show X2.500 with matching framing;171515 Undo shows X0.000 with
the central cube silhouette back in that view;171800 provides the requested wider
view. No command-induced downward camera drift is visible or recorded here.
Identical fixture meshes can overlap near X2/-2 versus edited X2.5; silhouette
count does not prove a missing mesh or exact native identity/picking. Asset details
still request refresh; their audit/provenance is not newly verified.

Stage2dac9033-20261104170849379 records commit
2dac90333307c6307eee9382250c080a7f8124fe, dirty=true and source-match-at-launch=true.
Actual app279227808fc5ba58b8638eccd2efd0ca25face7ef36da791dc6e77950104356b
and helper5f5c9c4e7fbcfd9d901eae31ac86b9380a07cf4687b46071facb0297982e8515
hashes match their seals. Original receipt/log record production build172.9s/
terminal0, not tests. Actual private recovery line examined5/enqueued5 matches
the probe/raw-log anchor; local enqueue is not peer delivery.

Eight exact recorded app/descendant identities match cleanup absence at
2026-11-05T00:21:03.5349869Z, not a global orphan census. Independently strict-
decoded46 covered text payloads plus2 private logs reproduce zero four-pattern
credential matches. Images were viewed, not text-scanned; later scan/manifest
metadata are outside that scan and universal absence remains false. Stdout75802/
hash8929281201389d61a9c5ea6d564619c2f4ecc0c6b0519acf3660fa5744ab845c,
INFO495/WARN9/ERROR0, panic/current-error0 match; stderr empty.

Acceptance excludes other Ctrl/Super sides, plain navigation/Q/Shift, focused
text/capture, easing/terrain variants, inverse-link receipts, groups/concurrency/
restart, peer convergence, native UIA/Invoke/capture freshness, full rich-data/FULL,
final integrated sweep, exact-SHA CI/tag/release. No implementation/parser/compiler/
test execution, app/OS/native/API/DB/network action or Git mutation occurred.
