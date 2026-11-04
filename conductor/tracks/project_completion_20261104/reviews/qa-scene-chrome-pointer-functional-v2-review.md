---
type: Review
date: 2026-11-04
title: Shared scene chrome functional v2 review
---

# QA0071 scene/chrome pointer admission: independent functional v2 source review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for source integration. All three concrete blockers in the independent v1 review are corrected at their actual use boundaries.** No new blocker found in this frozen v2 delta. This is source approval only: no compilation, test execution, fresh app interaction, screenshot acceptance, finding closure or campaign credit is claimed.

Reviewed the same14 paths in `.omc/worktrees/qa-scene-chrome-pointer`, base `35485826b7008745133918e7ca67f3a278d39a12`. Independently verified:

- Full v2 patch50316bytes, SHA256 `b7b414ca5ca66aa0173123582f65f7ac6a0d6c7db6bfb8007384e3dcaaa4ef32`.
- V2 manifest5774bytes, SHA256 `dcc1749f76b1dff00f5aad0462594c23f2d1f95f7d595031bc6b12411aa1f1b5`.
- V2 report6747bytes, SHA256 `696741ede947a575fb21dd34f974e96284a9d2bc30b7fc36ece62446f343aafa`.
- V2 receipt SHA256 `46970774b5d21053459a93f253b30f9be8508738beb167af01ee55af85f034af`.
- Every current source path's exact byte length/hash; all14 original v1 raw files in `qa-scene-chrome-pointer-v1/`; and the original v1 patch/manifest/report copies, still equal to their canonical v1 artifacts and original anchors.

Read the complete v1→v2 source delta, relevant current snapshot/camera/router/world-cursor/UI/fixture paths and actual pinned SDK naming/ID/output APIs. Revisited the original review and the actual QA38 reproduction, whose full evidence review was completed separately in main `.omc/research/qa38-full-evidence-review.md`. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Read-only diff --check passes. Only this assigned main review artifact was authored; no source/Git mutation, compiler/build/formatter/test, app/process/OS/input/capture, API/DB/store or network operation was performed.

## PTR-1 corrected: no Context reentry under the temp-data read lock

Actual `ScenePointerChrome::blocks_current_ui` first copies `Option<ScenePointerChrome>` out of `ctx.data`, ends that call, and only then evaluates snapshot.current and other Context geometry APIs. The data closure now performs only the bounded temporary-store read. No current predicate or input/geometry accessor remains inside that closure. ScenePointerChrome and its snapshots remain Copy, so this is an owned fixed-size state copy, not an escaping lock-backed reference.

The completed and pending checks remain distinct. A missing/invalid completed snapshot blocks the UI's secondary-click admission; valid completed uncovered scene remains eligible. Pending chrome adds current-pass exclusions, and invalid pending geometry/overflow blocks admission without turning an empty in-progress build into a positive completed snapshot.

The existing renderer journey now calls this **actual production secondary gate**, including completed header/hint coverage, ordinary uncovered scene, absent publication, valid pending empty build retaining completion, newly excluded pending chrome, invalid pending overflow, invalid pending geometry, invalid completed publication and explicit invalidation. This reaches the boundary that the previous accepts_pointer-only assertions missed. These assertions are authored and unexecuted; they support review of the correction, not a passed runtime deadlock check.

## PTR-2 corrected: real completed accessibility IDs join real observed geometry

The Brush journey enables AccessKit on its actual egui Context and retains the completed FullOutput from each rendered pass. It finds the existing accessible control name in `platform_output.accesskit_update.nodes`, takes that actual NodeId's u64 value, and joins it to `ControlObservation.id`. It then requires allowed+clickable and a positive intersection of the real hit and clip rectangles before choosing their center.

This matches the actual APIs: workspace bevy_egui enables accesskit; pinned egui0.33.3 emits the completed TreeUpdate from the current pass's AccessKit state; ResponseExt::named sets the label on the response's AccessKit node; the pinned egui ID mapping uses response.id.value as the AccessKit NodeId; layout observations record the same response.id.value; pinned accesskit0.21.1 exposes NodeId's u64 field and node.label. No fictitious name field on ControlObservation, private-ID reconstruction or hardcoded Back geometry remains.

All three real target routes remain: Back to petal list, Browse current verse fractals and Browse all verses. For each, the fixture feeds the actual strict editor gate into the tool consumer before ordinary egui press/release navigation, verifies no authoring action, performs actual UI navigation, and verifies navigation cancellation, byte-identical terrain and no DB command. It restores the active hierarchy context and re-arms between routes. The final actual uncovered-scene positive gate remains, followed by the existing real Brush handler's legitimate Circle placement assertions. This preserves the meaningful Update-before-later-UI reproduction order without executing DB writes or pretending a synthetic name lookup is a real control.

## PTR-3 corrected: public standalone semantics and explicit strict editor policy

`CameraPointerRegion` retains its two original public fields. Its public accepts_pointer implementation again matches the baseline geometry-only semantics: physical-to-egui conversion, strict interior scene bounds and the original Background/floating-layer ownership test. It requires no ScenePointerChrome publication. The existing renderer journey includes a fresh Context with configured region and no publisher, asserting this positive geometry-only case alongside strict-editor rejection for the same point and region.

The new separately named accepts_editor_pointer adds required current completed chrome to that original geometry/layer gate. accepts_editor_window_pointer first requires the exact observed Window entity, physical size and native scale, then uses the strict current physical cursor path. All production editor scene claims use the strict window helper: actual ClickArbiter resolution and world-cursor preview. Editor fixtures and divider/topbar boundary journeys explicitly use the strict point helper. The remaining production geometry-only camera use is deliberately limited to clients without the editor policy marker.

GardenerConsolePlugin installs the zero-state `EditorScenePointerRequired` Resource. Actual orbit_camera_system checks it and selects strict editor routing when present. For an opted-in editor, absent region, missing egui Context, absent/nonunique primary Window, absent publication, invalid publication or unavailable pointer all refuse mouse admission. The existing blocked branch consumes motion and scroll before skipping mouse controls, so input is not retained for later replay. In particular the missing-context branch now returns editor-pointer-blocked instead of the v1 permissive fallback. Keyboard input behavior is unchanged.

Without the marker, standalone camera clients retain their original configured-region geometry behavior, optional-region behavior and missing-egui-context fallback. The marker is independent of the region's existence, so accidentally missing editor region data does not silently opt the editor back into standalone mode. This is a narrow explicit policy split rather than a broad framework or hidden change in the public geometry API.

## Preserved design, type seams and remaining gates

V1's actual-response ownership remains: active-Petal header, browser header/body allocation and passive hint are published from their rendered response rectangles. Four fixed exclusion slots, bounded invalid/overflow handling, current physical Window/navigation stamp, Context viewport/screen/egui scale checks and separate building/completed state remain. No input-origin special case, QA pre-move, label-based production exclusion, authoring shortcut, new dependency, theme change, Inspector change or visual viewport shrink was added.

The current stamp is still recorded in ScenePointerContextSet inside the existing ordered Selection chain before claims. Camera input follows that set; world-cursor projection follows it and remains in PostSelection. Original Brush/Measure/path/gimbal/object-pick priority and captured gesture state machines remain unchanged. CameraCaptureLock's existing run_if remains. Different scene geometry and changed scope/scale still invalidate completion, and returning to old geometry cannot revive it without a new valid completion. The context-menu producer retains its existing active-gesture and floating-layer checks in addition to the corrected shared gate.

Actual camera system parameters remain within the pinned Bevy system-parameter tuple shape; the marker uses the existing Resource/Option<Res> pattern. No stale accepts_window_pointer callers remain, and no editor production path was found using geometry-only fallback. No additional definite type/API mismatch was found by source inspection. This is not a compiler result.

Declaration counts remain camera40, Brush9, topbar10, layout0:59 total, unchanged from v1. The new renderer and real navigation assertions extend those existing journeys; none was executed. The renderer journey specifically preserves strict edge/scale/stale/overflow/nonresurrection coverage while adding the secondary-gate and standalone compatibility cases.

Root owns integration, the required integrated validation sweep and fresh matching live evidence. Before QA0071 acceptance, exercise one-click armed Circle Back and both ancestor routes, ordinary uncovered placement, context menus, affected tools, held drags, camera orbit/pan/zoom, capture lock and scale/divider transitions, with terrain/hierarchy/history/navigation readbacks and actual images. Preserve immutable accepted unexpected writes and unknown outcomes without replay or rewriting the baseline. Last completed CPU UI geometry remains the stated contract; no independent GPU-presentation or native hardware-input proof is introduced. Historical QA0024's original trigger remains unproven. No QA0071/FULL/native/authority/CI/tagged-release gate closes on this source approval.
