---
type: Review
status: changes_requested
---

# QA41 residual layout v1: independent boundary review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**REQUEST CHANGES.** The tree, divider and compact Inspector changes are source-supported improvements at the existing interaction boundaries. Two ruler defects remain: visible scene UI is not excluded from its placement, and the new nested-pass ordering does not establish current camera/transform data in the outer schedule. No additional concrete correctness, privacy or authority blocker was found in the full sixteen-file slice.

This is a source-only review. No compiler, parser, formatter, test, application, OS/native/input/capture, API/DB/network operation or Git mutation was performed. Only this review artifact was written.

## Exact reviewed scope

The isolated checkout is `.omc/worktrees/qa-residual-layout`, actual HEAD `d8da3a46e9861e9191845ea283e37fb7e5ea3472`. Independently verified all sixteen working files against their byte lengths/hashes in the frozen manifest and against every corresponding `raw/` snapshot file.

- Manifest SHA-256: `eb0c27033fa7684385a889bc221714583d0a5745ecc389171bb9653d0cfa4d34`.
- Full patch SHA-256: `3430a70004ae24c9597a6c8482de2f4ba0395c73d8f367c30c98b4a88e3bd526`.
- Author report SHA-256: `a9edccc880efc9a3dfa692ae9f6b230970aa77ad55fab99210f0870f34b66288`.

Reviewed all changed/new production and existing-journey code, all six affected directory-documentation deltas, the approved residual design, relevant unchanged viewport/camera/ownership consumers, and pinned local egui/Bevy scheduling and camera APIs. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`.

Paths: `fe-terrain/src/{AGENTS.md,ruler.rs,ruler_plugin.rs}`; `fe-ui/src/{AGENTS.md,plugin.rs,native_accessibility/AGENTS.md,panels/AGENTS.md,panels/inspector.rs,panels/sidebar.rs,panels/tree_entry.rs,ui_shell/AGENTS.md,ui_shell/layout_policy.rs,ui_shell/right_sidebar.rs}`; and `fractalengine/src/{AGENTS.md,main.rs,terrain_bridge.rs}`.

## B1 — medium, blocking visual acceptance: scene clipping does not prevent hint overlap

The new `ViewportGeometry::overlay_current` and host `sync_ruler_hud` pass only a divider-inset scene rectangle to `RulerHudInput`. `paint_scale_bar` and `draw_unscaled_chip` then choose that rectangle's bottom-left interior. Neither receives or checks the actual rectangles of existing painted scene chrome.

The existing real `fe-ui/src/viewport.rs:445–466` producer paints `viewport_tool_hint` at `rect.bottom() - 8`, with eight-point frame margins, and records its actual clipped response with `ScenePointerChrome::exclude`. That response is not consumed by the new ruler bridge. Therefore fitting inside the scene and avoiding side dividers/footer does not establish non-overlap with the hint. The ruler paints on Foreground after the hint's Middle layer, so an overlap places the ruler over the hint rather than resolving it by existing layer order.

This is a concrete ordinary-state conflict. Retained QA41 `owner-hint-armed-ui.jsonl` has the actual Brush hint text at `(313, 661)` with size `(554.1875, 15)` at 1280×720/1x; its frame extends eight points around that text. The new lower-left anchor places an ordinary ruler into that same bottom band and across the hint's left portion. The long ordinary Select hint is another affected shape. The changed ruler has not been executed, so this review does not claim a new-build screenshot; the conflict follows from the production producers and retained ordinary geometry.

Required correction: carry a bounded, same-pass record of actual occupied scene-chrome rectangles to ruler placement, or use another explicit shared placement contract that incorporates the actual hint response. Choose a complete ruler/chip footprint that avoids those rectangles; omit this cosmetic HUD when no supported placement fits. Do not guess the hint height from tool text or merely alter layer order. Preserve current context/pass/scale checks, scene clipping, actual divider clearance, and the host bridge's crate dependency direction.

The existing ruler assertions test fit within a bare scene rectangle; they do not render the real hint and cannot detect this conflict. Extend a meaningful existing producer/integration journey and retain real matching screenshots at normal and high scales, with the long hint, Results open/closed and constrained scenes. A synthetic empty rectangle does not establish adjacent-producer acceptance.

## B2 — medium, blocking measurement correctness: outer camera updates are unordered

The production `draw_scale_bar` now reads `GlobalTransform`, `Projection` and `Camera::physical_viewport_size`, while the bridge stamps current egui geometry. The new `GardenerUiSet::GeometryReady -> RulerHudSet::Bridge -> Draw` chain correctly orders those three systems **inside** `EguiPrimaryContextPass`. It does not order that nested pass against camera computation or transform propagation in the outer `PostUpdate` schedule.

Pinned `bevy_egui 0.39.1/src/lib.rs:1198–1204` registers the pass loop and `end_pass_system` in `EguiPostUpdateSet::EndPass`. Its PostUpdate configuration chains EndPass, ProcessOutput and PostProcessOutput, but supplies no CameraUpdateSystems/TransformSystems ordering. Pinned `bevy_render 0.18.1/src/camera.rs:64–71` registers the real `camera_system` in `CameraUpdateSystems` during PostUpdate. Transform propagation is likewise an outer PostUpdate producer. Neither the new main registration at `fractalengine/src/main.rs:915–921` nor an existing inspected host edge constrains EndPass after both sources.

As a result, a resize or camera pose update can reach the current egui pass while the ruler still reads the previous computed camera viewport or previous GlobalTransform; those producers can run later before render extraction. A current geometry stamp cannot validate unrelated camera computation. The source report's current-camera calibration claim therefore needs an actual schedule guarantee, not only the correct physical-pixel formula.

Required correction: explicitly order the actual outer pass execution/calibration read after the real `CameraUpdateSystems` and `TransformSystems::Propagate`, or publish a separately owned current camera-calibration snapshot after those actual producers and consume it through a correctly ordered same-pass path. A system-set relation placed only inside the nested egui schedule cannot order systems in outer PostUpdate. Check the real schedule graph for cycles and retain ProcessOutput, Owner observation, native provider handoff, vision/capture and input/window ordering. Do not crop the camera as a side effect.

Extend a meaningful existing journey using actual production registration for window/camera change boundaries; a handcrafted `Camera` with a static custom viewport, as used by the current ruler test extension, verifies the conversion formula but not this production ordering. Matching live resize/orbit/focus and UI-scale checks remain required.

## Reviewed interactions and design boundaries

### Tree rows

The explicit child width reserves all four slots and three gaps after indentation, while the native name Button independently retains the shared minimum. Existing both-axis scrolling can carry the resulting width instead of shrinking deep names below the minimum or allocating through eye/Options controls. The change stays inside `tree_entry::row`; it adds no competing whole-row click target, role exception or selection owner. Existing explicit entry namespaces, accessible full names, menu/eye callbacks, primary/selected/partial styling and text-sized glyph painting remain.

The existing sidebar journey is extended through actual production responses, hierarchy depths, horizontal overflow and wheel reachability. Those authored assertions remain unexecuted and do not prove actual platform scrolling or pointer dispatch. I found no concrete new tree authority/ID/selection blocker in the source delta.

### Dividers

The production metric now reserves upward-rounded physical margin, computes outward x endpoints with f64 intermediates, and checks the final clipped span rather than weakening the checker. Camera exclusion uses conservative clearance, overlay content and panel interior use the larger gutter, and native Results keeps the existing scoped/restored radius. Invalid or unrepresentable geometry suspends the existing owner. Existing pending/held/release, focus/modal/source/scale/context continuity logic remains.

The new actual-response endpoint assertions are relevant extensions of existing journeys; they are not executed evidence. I found no concrete ordinary-window sliver that is newly shared by the divider and admitted camera region. Actual side/native Results neighbor clicks, scale restoration and camera preservation remain live gates.

### Compact Inspector and section navigation

The narrow controls reuse native menu Buttons and the existing section `toggle` method. Both tab presentations call one `activate_inspector_tab`, preserving `set_active_tab`, token secret/draft lifecycle, page reset, scope population and fetch-on-activation. The added channel warning does not log token values. Popups retain their native surface marker. The main Inspector scroll identity is unchanged, and pending batch controls move into its body without gaining a new mutation or permission path.

The compact navigation has a separate bounded scroll fallback and retains selection/deselect ownership in NodeManager. Existing property/object-data/token synchronization still precedes rendering. The authored menu journey checks actual callbacks/state, selection, property/query drafts, scroll identity and secret clearing; it does not execute all section renderers or establish OS keyboard/UIA acceptance. I found no additional privacy, stale-token or selection-authority blocker in these changes.

### Ruler geometry, units and native publication

The bridge keeps fe-ui independent of fe-terrain, uses actual egui Context identity/pass/scale, clears failed/stale inputs and omits unsupported geometry. The unit conversion correctly separates physical Camera viewport height from scene placement dimensions, applies current pixels-per-point to the desired visual length, and converts the resulting physical bar back to points. Actual text mesh/layout bounds and stroke/chip fit checks are materially better than whole-window anchoring. These formula and clipping improvements should survive the two corrections above.

The fixed/numeric ruler remains the existing separately source-audited direct painter, rather than a ninth mandatory producer marked after `surface::finish`. The existing conservative edited-point label refusal, eight-producer completion, Owner-before-provider ordering and Windows suppression floors are unchanged. This source inventory is not native capture freshness or a rendering acknowledgement. General oblique ground-distance accuracy remains outside the placement correction, as documented.

## Re-review and remaining evidence

Preserve v1's full patch, manifest, report and raw files. Repair B1/B2 as one coherent slice, then publish replacement anchors and independently review the complete delta, especially the actual producer and outer schedule paths. No source approval, compilation or execution is granted here. Main's later reviewed changes and evidence must survive eventual integration.

All new existing-journey assertions, integrated build, actual screenshots/actions at the required sizes/scales, ruler calibration during real camera/window changes, native/Owner gates, FULL passes, final sweep, CI and release artifacts remain unexecuted or unfinished acceptance requirements.
