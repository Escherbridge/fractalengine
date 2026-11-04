---
type: Review
date: 2026-11-04
title: Divider v4 functional source review
---

# VIS-003 divider v4: independent functional and source API review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for source integration. No blocking functional, routing or inspected public-API issue found.** This is separate from the design approval. Compilation, authored journey execution and live visual/input acceptance remain pending root-owned gates.

Reviewed frozen `.omc/worktrees/qa-panel-width-preference` at base/HEAD `2bce88aff512a9998eb120ddbcde8b78b4f7aa84`, the v4 report/receipt, complete production changes from approved v2 and the v3-to-v4 correction, prior v2 functional approval and main v4 design review. Applied shared guidance at `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Read actual pinned egui 0.33.3 implementation where necessary. Root's live main fa955/QA37 was not inspected or operated.

All ten raw source hashes independently match the receipt. Independently verified patches:

| Patch | Bytes | SHA256 |
| --- | ---: | --- |
| Combined v2 to v4 | 39,970 | `50980329980c0b66e1e48884aaeb97215b3ca7065249f871a9e9ec27462dfba6` |
| v3 to v4 correction | 18,557 | `37f5689dc3c5754694810bf7121dcfba75308f0e2495ba66aac8747a272f1f88` |
| Full v4 from checkout base | 62,781 | `6b726d6ed18c998ffad5dcccbf87e01e8e5bbf39c09bac330f4244ddf9dc3a7f` |

Read-only git diff --check passes. Affected Rust declaration counts match the receipt: panels/mod 1, sidebar 1, plugin 19, layout_policy 0, left_sidebar 3, results_dock 0, right_sidebar 13, topbar 10. No declarations were added. Only this main review artifact was written; no production/Git mutation, compiler/build/formatter/test, app, process/OS/input/capture, API/DB/store or network operation occurred.

## Width ownership and cancellation remain coherent

The production `PanelWidthState` remains the approved v2 owner: preferred width is independent of displayed caps; drag starts from actual previous displayed outer width, previews without overwriting preference and commits only on primary release with actual visible movement exceeding half a physical pixel. Stationary and outward cap-only motion preserve the prior preference. Default outer budgets remain 220/320; adding inner gutters or removing cost-bearing strokes does not become a preference update.

The captured geometry now also contains current DividerMetrics. Context, viewport, preallocation available rect, bounds, scale, metrics or pass-continuity change cancels preview. Existing focus/Escape/popup/modal/animation/unavailable-section/portal/quarantine handling and held-button suppression remain; there is no input-capture release for another owner. Left current `close_pending` is still passed through the actual caller and applied before prepare, with no renderer overwrite. Right close gating remains before its section render. Release on the first pending-close frame therefore cannot save a preview before the modal appears.

The public stable divider IDs still describe actual Background-layer drag responses registered after panel content. They retain semantic vertical Splitter names/orientation without inventing an AX Focus, numeric resize or keyboard contract. Removal of the unused mutable Ui binding matches actual shared-reference API use and introduces no behavior change.

## Shared metrics, adjacent content and actual routing

`DividerMetrics::from_scale` supplies a single checked policy: radius=max(5,10/scale) logical points, giving at least 20 physical pixels across a complete strip; gutter=ceil(radius+1/scale) reserves at least a physical-pixel gap beyond its inclusive edge. Finite positive scale and representable i8 margin are checked before conversion. Invalid metrics cancel owned gestures, disable Results resizing and make scene/overlay input unavailable rather than reusing stale geometry. Clipped/obstructed target size still requires actual live measurement; the arithmetic alone is not a whole-window compliance claim.

Per-pass metric refresh and Results-edge clearing occur before the old one-time apply_style return, so a same-Context scale cycle does not ratchet the SDK radius or keep an old edge. The cache additionally matches current pass and scale; fallback derives the same metrics. The native Results TopBottomPanel alone runs inside the RAII radius scope, restoring the original SDK field on return or unwind. Portal and other native panel/window owners do not inherit this expanded policy.

Pinned egui TopBottomPanel reads and registers its actual top resize response using `resize_grab_radius_side` inside its show call, so the scoped field applies at the actual interaction owner. `Frame::side_top_panel` has native margins 8/2 and no stroke. V4 raises Results top margin to the checked gutter and raises each side margin only when actual preallocation available geometry borders that side owner. An absent side retains its native eight-point margin. Default/max/min height behavior and native resize ownership remain unchanged.

The important v4 correction is complete at the interaction boundary: Results and both existing side-panel bodies intersect inherited clip with their actual inner content rect before emitting controls. Pinned ScrollArea expands content clip by clip_rect_margin, then intersects its inherited parent clip. A margin alone would let overflow reclaim the gutter; the explicit parent clips prevent that for cells, headers, nested scroll content and scrollbars. Their independent native popup layers remain independent. Both-axis scrolling stays available, the hierarchy content floor subtracts the actual gutter plus the existing 12-point scroll reserve, and its deep Add Node/requirements row wraps instead of intrinsically widening the default panel.

Results records its actual outer response and current cumulative pass after show, including a frame where Close Results changes its open flag during rendering. Scene/overlay bottom exclusion requires that same-pass top edge actually coincide with the visual CentralPanel bottom. It no longer relies on the already-mutated open flag or an old preferred height. On the next pass, no Results render means no current edge. Side exclusions likewise derive from actual visual boundaries against the viewport.

The central interactive overlay uses an inset, clipped child Ui. The returned parent CentralPanel response remains the visual ViewportRect and camera projection budget. `plugin.rs` now computes CameraPointerRegion from the explicit metrics and actual Results edge after the SDK style scope restores; it cannot accidentally read the old SDK radius. Existing strict `viewport_interaction_rect` endpoint ownership, physical-to-egui conversion and shared CameraPointerRegion/NodeManager arbiter remain unchanged. No auth, selection, tool, DB, scene transform or native A2 producer contract was rewritten by this delta.

## Public APIs and authored journey quality

The inspected pinned public APIs support this source shape: UiBuilder/max_rect and child Ui clipping; Margin's i8 fields and ZERO; Context style mutation/read_response/interaction_snapshot; Response sense and interaction rectangles; ScrollAreaOutput's public inner_rect/content_size/state.offset; AccessKit role/orientation setters. The two existing topbar endpoint assertions now explicitly construct Bevy Vec2 before CameraPointerRegion::accepts_pointer, whose parameter is Bevy Vec2, preserving coordinates and the existing physical 2x conversion. No production topbar geometry changes accompany that fix. No obvious import, visibility, return-type or constructor mismatch was found by inspection; this is not compilation evidence.

The retained width journey still authors default/custom restoration, cap-only and actual displayed-width drags, current close-before-modal cancellation, held suppression, focus/Escape/scale loss and reopen behavior through actual public responses. New assertions measure complete physical strips, current policy refresh, default outer budgets, strict camera endpoints and one-physical-pixel interior neighbors. They check that the Results-only style scope restores before later owners render.

V4 also reaches actual production Results content rather than an empty fixture: 64 rows and 24 columns via ResultsDockState::complete, required overflow on both axes, first-column sort/data control presence, actual hit intersected with actual clip, and X/Y clearance. It covers both sides, either side and neither side at 0.75/1/1.25/2x, with native eight-point no-owner margin checks and a separate same-Context scale-restoration sequence.

The cfg(test)-only accessor records actual public ScrollAreaOutput geometry/offset, not a fabricated resize response. Hover probes obtain actual native hovered IDs from Context::interaction_snapshot, measure read_response, require scrollbar click/drag semantics and then require a real pointer drag to change the corresponding table offset. They do not reconstruct private SDK IDs. This is meaningful authored coverage of populated/expanded scrollbars. The panel harness deliberately uses simplified side bodies; it does not replace live adjacent sidebar/rail/Options/eye interaction evidence. All assertions and hover-probe assumptions remain unexecuted here.

## Integration and acceptance limits

Integrate the reviewed contribution against the approved v2 baseline using the verified combined delta or an equivalent reviewed merge; the checkout's older base is not authorization to overwrite unrelated newer main contributions. Preserve current A2/volume/SDK fixes and separately owned artifacts. This review does not certify an uninspected integration result.

Root still needs the integrated build/final sweep and actual target/hit measurements after clipping and overlap resolution; neighbor edge clicks for side rows/Options/eyes/rail/table/header/scrollbar/Back/Browse all; constrained/deep/long-name/default/custom-width screenshots; high-scale horizontal-scroll-and-invoke; selection/camera preservation; and actual minimize/restore while dragging. Pre-existing six-point Results column splitters are separate controls and can legitimately remain checker findings. No checker exemption or general splitter-compliance claim was added.

VIS-003 live closure, complete layout coverage, clean gold passes and campaign/release acceptance are not granted by this source review. Preserve failed QA36 and frozen QA37 evidence with their actual limitations.
