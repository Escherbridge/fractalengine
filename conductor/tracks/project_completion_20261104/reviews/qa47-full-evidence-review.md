---
type: Review
title: QA47 focused evidence review
date: 2026-11-04
status: approved_at_recorded_scope
---

# QA47 independent focused evidence review

Verdict: **APPROVED for the packet's bounded local recovery enqueue, actual scene/selection, unchanged numeric-attempt outcome and recorded cleanup facts, with the external invalid-input clarification below.** No valid numeric transform regression, successful save/history/replay/Undo, peer delivery, native UIA, FULL acceptance, metric report, CI or release credit. The immutable packet was not rewritten.

Reviewed `qa/passes/47/packet-manifest.json`: 6,676 bytes, SHA256 **`82a0d0125e0a3a64b5ae27b08bd8710c46b13baebc20b216aa8a5df64d35c9c4`**. Every one of its 39 payload files matches recorded bytes and SHA256, with no unlisted payload or missing file. All 25 original-artifact anchors and three screenshot entries in `packet-summary.json` match. The summary is 7,285 bytes, SHA256 `b60aad0068a76079f7a55e46d86f5e403d5d86e29b0c9d41521e145ac677f763`. All three actual frozen PNGs were separately opened with view_image and visually judged; they also match their original run-local bytes exactly. No implementation execution or live system query was used.

## Required external clarification

The original packet conservatively says no saved transform and cause/submission state unproved. Its phrase that the draft screenshot shows focused `002.500` describes only the visible substring. The full native UI snapshot records **`0.002.5000`**, which is malformed numeric input. This distinction must accompany any subsequent project status or interpretation of QA47. It must not be labeled a failed valid `2.500` transform, nor evidence that Enter is mandatory.

Exact anchors:

- Initial UI frame 3127: enabled Position (m) X, ID 3478998624401797978, value `0.000`, not focused.
- `owner-edit-position-x.jsonl`: focus click then one `text_entry: "2.500"`, with no select-all/replace or Enter. The CLI inserts at the cursor; it does not replace the field automatically.
- Draft UI frame 9884: same field ID, value **`0.002.5000`**, focused. The screenshot horizontally scrolls that full string to keep the caret visible.
- One recorded scroll makes Apply visible. `owner-apply-transform-once.jsonl` contains one click at (1020.078125,539.5) inside the enabled actual Apply node transform button ID 6082097978130472780. Commands-applied is an input receipt, not a DB submission acknowledgement.
- After UI frame 17184: same field at `0.000`, unfocused. The three before/after/late node responses are identical, as are all three complete histories; current_version remains 4.

The independent source trace `.omc/research/qa47-numeric-transform-source-trace.md` (9,940 bytes, SHA256 `e57935cc81171b83c225fffaf57f507e91d3b2d38506f5c4a5cc14c6e8797ae3`) identifies a separate real UX validation gap: displayed meter strings update the converted Apply buffer only after successful parse; invalid text leaves the last valid buffer, while Apply parses that older buffer. An unchanged payload has an explicit silent Undo no-op branch. This explains the observation conditionally; the packet has no internal dispatch/buffer receipt proving the exact branch taken. No code change or S2 acceptance arises from this review. Preserve this source finding separately from the lack of a valid-edit positive result.

For a fresh separately authorized input case, source supports focus once, key ctrl+a, type 2.500 **without --into**, observe the complete valid value, then one Enter OR one fresh Apply click. Reclicking through --into after select-all may collapse selection. No such input was issued by this reviewer.

## Actual images and navigation

All three images are 1280x720, readable/nonblack, and show the real application chrome and local 3D scene. `162714-296-qa47-typed-witness-recovery.png` shows three red cube objects plus a translucent marker, Petal A and Cube 1 selection, zero position and the local scene-asset sharing card. `162913-170-qa47-transform-draft.png` shows the focused clipped draft after Inspector scrolling and visible Apply. `163017-946-qa47-transform-saved.png` shows unfocused zero after the one click; its filename does not certify saving. Cube geometry has not visibly moved between these frames.

The selected screenshot highlights Cube 1 and its active hierarchy ancestry. Labels in the narrow tree are elided and several toolbar/Inspector glyphs appear as empty squares; a large blank Inspector header area remains. These images are not evidence of universally polished or glitch-free UI. No calibrated metric/ruler/report interaction appears in the three images. The finite layout observation fields themselves declare unknown widget/text/floating occlusion coverage; there is no full layout claim to inherit.

The initial state is Genesis Verse without selection. The Petal-A click receipt records the expected cross-verse token transition and explicitly marks its log generation gap. The subsequent scoped Cube-1 selection receipt, UI/state and images agree on the public node/Petal/Verse IDs. That generation gap is not treated as an empty complete log history; the final original local application log is independently summarized below. Final state remains the same selection and zero pose with no exposed notice. `settled` explicitly excludes DB/sync/terrain completion; `scene_snapshot_unavailable` remains an unavailable diagnostic, not a report failure or a positive report result.

## Recovery, build and provenance

The complete original local stdout hash is verified against the packet summary and final probe: 68,498 bytes, SHA256 `6b4932f4d3c8db34223c034d7498054f2ea6a43591be279a08239119933bf571`. Strict UTF-8 decoding yields 523 lines, 447 INFO, 9 WARN, zero ERROR/panic, zero private current-read diagnostics and zero compiler warning/error diagnostic markers, matching the summary. stderr is empty with its standard empty SHA256. The final recovery line matches original stdout exactly:

> stored scene recovery enqueued; peer delivery is not certified

Its public scope is the QA Basic Verse with examined=5/enqueued=5. The final source log contains zero publication-deferred and primary asset-metadata failure markers. This supports successful local enqueue in this pass and absence of the earlier observed error markers here; it proves no peer receipt, convergence, durable restart, concurrent-mutation rollback or exact runtime asset field type. The captured scene-asset card is local byte availability, not peer delivery.

The frozen stage manifest is byte-identical to the retained actual stage manifest. Actual staged fractalengine.exe, seal_hexon.exe, legacy_adoption_fixture.exe and native_observer.exe each match its declared hash. The local source commit is a16eab87032a41d012e6936776d8dcd9228b7a54, fingerprint `9C2210961157E3595A67981D06DBDA5EA1AAB712BD0CD742588FBA76793715EC`, **source_dirty=true**. Context reports stage_matches_source at launch; do not recast this as a clean-commit-only build or present-time fingerprint comparison.

The stage log exactly matches `.omc/research/completion-typed-asset-native-component-stage.log`; it records completed Cargo builds, worker.cpp and final staging in 110.5s. The PowerShell NativeCommandError wrapper around normal Cargo stderr is not a compiler diagnostic. The frozen build receipt records root-owned session7513 terminal exit0; a standalone terminal tool-response archive is not in the packet, so that exit value is root receipt provenance rather than independently recovered process status. No reviewer build/test was run. Launch log records PID118148 and complete in15.8s; its exact bytes match the retained original. Build success does not grant runtime/native/test acceptance.

## Cleanup, finite scan and preservation

The owned context, launch/stop logs and cleanup receipt match their exact retained `.omc/research/qa47-*` originals. Cleanup has the same eight complete recorded identities as context: app PID118148 plus seven descendants, with their original executable paths and creation ticks. All eight recorded checks have pid_present=false and recorded_identity_present=false at `2026-11-04T23:33:21.0742491Z`. The stop log reports stopped/run retained. This is a timestamped recorded-identity absence check, not an independent current process inspection or exhaustive orphan census; no store deletion or database inspection was performed.

Independently recomputed all four fixed credential patterns across all 35 explicitly listed text inputs using their strict UTF-8/BOM/UTF-16 encodings: zero count mismatches or positive matches. The scan explicitly sets universal_absence_claim=false; it is not proof against every credential format. Original raw app logs remain outside the published packet; only their verified summaries are included.

All 39 payload hashes and the manifest remained unchanged. This review creates only this report. No code/docs/source packet mutation, new input/replay, implementation parser/test/compiler/formatter, app/process/OS/native/API/DB/network operation or Git mutation. Acceptance remains focused and bounded; the larger goal still needs a valid transform and historical save/replay case, invalid-draft UX correction if authorized, complete report/peer/native work, integrated final tests, CI and tagged downloadable artifacts.
