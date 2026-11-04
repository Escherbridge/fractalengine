---
type: Review
date: 2026-11-04
title: Shared scene chrome security v2 review
---

# QA-0071 pointer v2: independent security/correctness re-review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for source integration of v2. PTR-1, PTR-2 and PTR-3 are corrected in the inspected source.** No remaining blocker was found. This supersedes the v1 source approval where it incorrectly characterized the secondary-click Context lock path and standalone compatibility; it does not replace the functional review's factual v1 findings. Compilation, authored assertions and live acceptance remain pending.

## Exact frozen revision and preservation

Same isolated `.omc/worktrees/qa-scene-chrome-pointer`, documented base `35485826b7008745133918e7ca67f3a278d39a12`. This re-review used file/source inspection and hashes, no Git operations.

| Artifact inside checkout `.omc/research/` | Bytes | Independently verified SHA-256 |
|---|---:|---|
| `qa-scene-chrome-pointer-source-v2.patch` | 50316 | `b7b414ca5ca66aa0173123582f65f7ac6a0d6c7db6bfb8007384e3dcaaa4ef32` |
| `qa-scene-chrome-pointer-frozen-manifest-v2.json` | 5774 | `dcc1749f76b1dff00f5aad0462594c23f2d1f95f7d595031bc6b12411aa1f1b5` |
| `qa-scene-chrome-pointer-source-report-v2.md` | 6747 | `696741ede947a575fb21dd34f974e96284a9d2bc30b7fc36ece62446f343aafa` |
| `qa-scene-chrome-pointer-v2-receipt.json` | 7803 | `46970774b5d21053459a93f253b30f9be8508738beb167af01ee55af85f034af` |

All **14 current raw file counts/hashes** match the v2 manifest; path set is unchanged. All 14 preserved v1 source files independently match their original manifest. Original canonical patch/manifest/report are byte-identical to their copies under `qa-scene-chrome-pointer-v1/`, retaining hashes `23ed70c8…`, `54f995d6…`, `ea5dd7c9…`. Nine files changed for the correction, five remain raw-byte-identical. Existing test declarations remain camera **40**, Brush **9**, topbar **10**, layout **0**: **59→59**.

## PTR-1: actual Context lock correction

The functional review is correct: pinned egui 0.33.3 Context::data calls its read transaction, but Context::input at context.rs:913-915 uses **self.write**. viewport_rect and pixels_per_point call input. Therefore the v1 secondary gate's snapshot.current calls inside ctx.data were a same-Context read-to-write recursive acquisition. My v1 statement that this was only shared read/read nesting was incorrect.

V2 `ScenePointerChrome::blocks_current_ui` now copies the fixed-size state out in a completed ctx.data call, then evaluates all snapshot/current geometry predicates on the local value. No Context method is called from that data closure. Observe/finish/accepts likewise retain their copy-outside-lock discipline; begin_pass samples geometry before its data write. The production secondary-click gate therefore no longer contains the identified recursive lock path.

The existing renderer journey now invokes **blocks_current_ui itself**, covering completed header/hint, uncovered scene, absent/invalidated state, pending empty build retaining old coverage, newly added pending chrome, pending overflow and nonfinite geometry, and invalid completed publication. This reaches the actual production boundary that the v1 accepts-only assertions missed. These assertions remain unexecuted.

## PTR-2: real label/ID/geometry join

The Brush journey enables AccessKit and retains actual completed FullOutput. It obtains each rendered Back/ancestor label's actual NodeId from the AccessKit update, then joins that ID to the actual `ControlObservation.id`. It requires allowed/clickable and a positive observed hit/clip intersection before deriving the point. The nonexistent ControlObservation.name access is removed; no new field, hardcoded coordinate or reconstructed private SDK ID substitutes for it.

Actual Back and both ancestor paths still route the production Brush consumer through the strict gate, then send normal UI press/release and require unchanged terrain bytes, no authoring action or DB command, real navigation and armed-intent cancellation. The uncovered-scene positive Circle path is retained. Existing label/NodeId equality matches the repository's actual response/accessibility ID seam; test compilation remains a separate gate.

## PTR-3: standalone contract and explicit strict editor policy

Public `CameraPointerRegion` retains its original two geometry fields. Its public **accepts_pointer** again performs only the original physical conversion, strict interior edges and Background/floating-layer gate; it no longer demands an editor publication. A standalone client supplying a region and egui Context therefore preserves its prior configured-region contract, in addition to the prior no-region mode.

New explicit **accepts_editor_pointer** conjoins required current completed chrome with that unchanged geometry gate. **accepts_editor_window_pointer** adds exact actual Window Entity, physical dimensions/native scale and current physical cursor. The production editor router and world-cursor preview call the strict window helper; their test fixtures use the strict point helper. The viewport's secondary-click producer still uses blocks_current_ui and floating-layer checks.

GardenerConsolePlugin installs the zero-state **EditorScenePointerRequired** marker alongside CameraPointerRegion. The marker carries policy only, no duplicate geometry or navigation state. In orbit_camera_system:

- Marker present, Context present, region present: strict editor window/chrome helper is required.
- Marker present, region missing: pointer is blocked.
- Marker present, Context missing: pointer is blocked; motion/scroll are consumed by the existing blocked-pointer branch.
- Missing/ambiguous Window with a configured region blocks pointer; editor mode also invalidates chrome.
- Marker absent with region present: original geometry-only helper is used.
- Marker absent with no region or missing Context: prior standalone/no-UI fallback remains.

Inspected all production call sites in renderer/UI/host source. Geometry-only accepts_pointer appears in the strict helper's conjunction and the explicitly marker-absent standalone camera branch; no production editor consumer directly takes a permissive geometry-only path. The marker is inserted by the editor plugin, and no removal path was introduced. Existing camera capture-lock run_if, shared ScenePointerContextSet ordering and keyboard capture semantics remain unchanged.

The renderer journey now includes a positive configured standalone region with no editor publication and a negative strict-editor control for the same point/geometry. It does not obtain compatibility merely by publishing editor state into every fixture. Actual system-level missing-Context/region/window behavior and mouse-event draining still require execution.

## Preserved product contract and limits

The actual clipped header/browser allocation/passive-hint registration, fixed four-slot publication, pending/completed separation, stamp/window/context/viewport/scale/navigation checks and geometry nonresurrection rules remain. Strict editor readers refuse absent/invalid completion rather than treating it as empty chrome. Browser blank allocation remains UI-owned; uncovered active-Petal scene remains eligible. Camera framing, divider metrics, existing claim order and captured gesture state machines are unchanged.

No role/grant, API/DB authority, original accepted data, object/group selection owner or tool persistence contract is altered. Last completed CPU UI geometry remains the contract; no independent GPU presentation or native physical-input proof is implied. The retained QA38 Circle/Back reproduction is separate from the historical pass05 QA-0024 trigger, which remains unreconstructed and unclaimed.

Root still owns exact integration, compilation and the once-at-end integrated existing-journey sweep. Required live checks include one armed Circle Back/ancestor click with byte-identical terrain and no new region/node/history, normal uncovered placement, real secondary menu, hints/wrapped headers, all relevant tools, held drags crossing chrome, camera orbit/pan/zoom, configured standalone mode, strict editor missing witnesses, capture/dividers/scales and separate native physical input. Preserve erroneous accepted regions/history and unknown outcomes without replay or rewriting. No finding, full pass, CI/release or project completion closes on this source review.

Only this review artifact was authored. No production or Git operation, compiler/build/formatter/test, app, OS/process/input/capture, API/DB/store or network operation occurred.
