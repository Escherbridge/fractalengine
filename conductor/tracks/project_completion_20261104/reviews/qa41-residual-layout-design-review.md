---
type: Review
status: approved_design_only
---

# QA41 residual layout design review

Verdict: **the residual defects are supported by the retained measurements, three inspected images, and production source. Proceed with focused corrections at the existing shared layout boundaries. This is design approval only; no implementation, build, test, live execution, packet freeze, or acceptance is asserted.** Main remained frozen while the parent operated QA41. This review did not use Git or operate the application. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`.

## Evidence and interpretation

At 0.75 scale, `owner-physical-quarter-layout-check.jsonl` reports 13 actual measured failures: twelve hierarchy Open/Select targets have physical widths 16.1953125–19.7109375 pixels with height 20.015625; the hierarchy splitter is 19.99999237060547 pixels wide. These are different causes. The twelve name targets need actual allocation changes. The splitter is floating arithmetic at the exact threshold, but should still be corrected in production geometry rather than weakening the checker. The report explicitly has incomplete coverage, fourteen unknown text observations, and unvisited/occluded content unknown; it is not a full layout verdict.

Inspected actual saved PNGs:

- `130801-291-qa41-scale-075.png`: deep tree names truncate to very short fragments; the ruler paints across the left/footer boundary. The scale bar and footer text visibly compete.
- `130854-582-qa41-1024-scale2.png`: the section rail occupies three rows; Inspector tabs occupy three rows; the content body has almost no visible height. The ruler again paints through the Hierarchy/footer area.
- `130924-806-qa41-1024-scale2-bottom.png`: after the body reaches its bottom offset, only a truncated Property/schema section header fits. This demonstrates poor usable space, rather than proving all its controls unreachable.

The narrow UI record gives Inspector viewport `(796,612,228,52)` physical pixels, clip `(796,606,228,58)`, content height 3830.5. The bottom record retains that viewport and reaches offset 3778.5, exactly `3830.5-52`. Zero measured hit violations in that profile does not establish usable Inspector content. The native scroll now correctly stays inside the visible parent budget; inflating its minimum would reintroduce the prior mismatch.

## 1. Reserve the complete tree row before allocating its controls

`fe-ui/src/panels/tree_entry.rs::row` currently computes `(available_width - 3*target_size - 8).max(8)` for the name. It allocates an icon slot, a name Button, an eye slot, and an Options slot. Nested indentation consumes more of the fixed hierarchy content width. `sidebar.rs` bases that content minimum on the normal 220-point outer panel, gutter, and scroll reserve; it does not guarantee a full deepest row. The current name Button does not specify `min_size`.

Pinned egui 0.33.3 confirms `Ui::add_sized` uses a centered/justified child with the supplied maximum size; it is not a universal minimum-width adapter. Native Button inherits ordinary interaction height, but its width minimum defaults to zero. This explains why the corrected height alone does not protect the name width.

Use one readable row-metrics calculation beside the common row renderer: icon slot + name slot + eye slot + Options slot + the exact three inter-slot gaps, with frame margins owned separately. Every interactive slot needs at least the shared `target_size`; the painted glyph stays text-sized. Keep the flexible name column larger when room permits. Its native Button should also carry an explicit square minimum as a final local invariant.

Before drawing that horizontal row, reserve a child width of at least this complete minimum, using the real local width after indentation. Let the existing both-axis tree ScrollArea own horizontal overflow when the panel cannot contain it. Do not merely clamp name width upward while retaining undersized parent geometry: that can push or clip Options and give the impression of overlapping controls. Either allocate an explicit row-width child or propagate the full minimum into the actual scroll content; verify the resulting native response and clip. A separate hierarchy minimum may improve ordinary readability, but must account for depth and the current scale instead of relying on a wider fixed panel.

Keep stable entry IDs, actual Button response, full accessible names/tooltips, selected/primary/partial styling, separate eye/Options targets, native context menus, navigation callbacks, and existing role/scope gates. The whole row must not become a new competing click target. Keep the reserved divider gutter and scrollbar clip intact. Horizontal scrolling is the constrained fallback, not a license to shrink any target below the physical minimum.

## 2. Quantize divider geometry, including its actual endpoints

`layout_policy.rs::DividerMetrics::from_scale` currently takes `max(5,10/scale)` without the upward GUI-quantization used by ordinary `target_size`. `PanelWidthState::register` subtracts/adds that radius around an arbitrary floating edge. The saved left strip consequently loses a few millionths of a physical pixel at 0.75.

Round the required radius upward to the public GUI quantum, then construct the actual hit endpoints outward on the physical-pixel grid (or reserve a documented physical rounding margin in the shared metric). Validate the final finite physical span, not only `radius*2*scale` before coordinate subtraction. A slightly larger shared strip is preferable to a threshold epsilon in QA. Quantization must stay bounded and reject invalid/unrepresentable scale geometry as before.

Use the same resulting metric for actual custom side strips, scoped native Results resize radius, panel interior gutter, overlay clearance, and camera/scene exclusion. Preserve the physical-pixel gap from neighboring controls, ordinary visual widths, restored SDK style, source/scale/context invalidation, pending gesture cancellation, and release ownership. Outward endpoints must not create a sliver that accepts a divider drag inside a region still admitted by camera routing; reconcile effective endpoint expansion with the shared exclusion/gutter rather than changing only the observed response.

## 3. Give the Inspector a compact navigation mode based on visible body space

The actual producers are `right_sidebar.rs::section_chrome/section_rail` and `inspector.rs::right_inspector/inspector_navigation`. Both the seven-button section rail and four-tab bar wrap outside the scroll body. At 1024 physical pixels and scale 2, this legitimately measured navigation consumes nearly all the sidebar height. The current `visible_scroll_height` and bounded native scroll minima correctly report the resulting small budget; they do not solve the UX.

Use a single layout policy to select compact navigation from actual available width and height. In the compact branch, show the current section in one native labeled selector/menu, and the current Inspector tab in one native labeled selector/menu. Keep a compact selected-object/deselect row rather than a large row of empty space. The normal rail and tabs can remain for wider/taller layouts. Preserve at least several control rows of body space when the window permits it; for example, a body-budget target of `max(3*target_size,96 points)` is a reasonable initial design value, bounded by the genuinely available height. Evaluate the rendered body, not a global pixel-width breakpoint alone. If even the compact header cannot fit, use a reachable scroll/navigation fallback rather than extending past the footer.

Refactor the existing tab activation side effects into one small function called by either presentation. It must retain `set_active_tab`, draft lifecycle, token pagination reset/scope population/query-on-activation, and the current object/selection authority. Section switching must continue through `RightSidebarState` and its toggle/reveal semantics. Keep all sections/tabs reachable through keyboard and actual native accessible controls with unambiguous active labels. No reduced hit sizes, owner-role exception, or hidden direct action is needed.

Retain the existing native vertical body ScrollArea ID and actual clip budget. The change is to the fixed header budget, not to overflow measurement or property schema state. Verify ordinary single selection, multiple selection, empty selection, pending edits, API Access privacy/gates, and return to the same tab/draft/scroll state. This design improves the object-data workflow without creating an independent Inspector state manager.

## 4. Separate ruler placement from camera scale calibration

Both `fe-terrain/src/ruler_plugin.rs::draw_scale_bar` and `draw_unscaled_chip` anchor to `ectx.content_rect()`. The former also derives meters-per-pixel from that height. Neither receives `fe-ui::plugin::ViewportRect`, and the ruler system is registered in `EguiPrimaryContextPass` without an ordering relation to `gardener_ui_system`. The screenshots show the resulting whole-window placement problem.

Introduce a small terrain-owned HUD input resource for the actual logical scene rectangle and its validity stamp; bridge it from the host that already imports both fe-ui and fe-terrain. Keep fe-ui free of a fe-terrain dependency. Publish the current CentralPanel/visual ViewportRect after the UI has finalized panels and Results. Use an explicitly ordered same-pass chain: shell geometry completed -> host HUD-input bridge -> ruler draw -> egui output completion/Owner observation -> existing native provider handoff. Named producer sets are clearer than depending on plugin insertion order. An Update bridge would be one frame stale because the actual rect is assigned inside the egui pass.

Stamp/validate context or window ownership, pass, finite positive rect, and pixels-per-point. Missing/currentness failure should omit this cosmetic HUD for that pass, never fall back to the whole window or reuse a prior window/scale rectangle. Clip the painter to the actual scene rectangle, inset the complete label/shadow/stroke bounds, and keep it outside footer, Results, dividers, and other overlays. Clamp the preferred ruler length to the usable anchor width and omit it when even the bounded label/chip cannot fit. The fixed unscaled chip must use the same anchor/clip path.

**Do not automatically replace the meters-per-pixel denominator with CentralPanel.height().** The current orbit camera spawns `Camera3d` with the default render viewport; the inspected production paths do not assign a cropped Camera.viewport when panels change. The 3D image is drawn behind chrome. Therefore the visible CentralPanel is an anchoring/clip region, not evidence of the camera's projection height. Read the actual orbit Camera render-viewport dimensions for scale math. A precise unit convention is required: use physical camera viewport height for meters per physical pixel; turn a desired 120-point visual bar into physical pixels using current egui pixels-per-point, compute the nice-number bar there, and convert the resulting physical length back to egui points for painting. Alternatively derive an explicitly named meters-per-point from those same inputs. Native window DPI alone is insufficient because the QA UI scale changes pixels-per-point independently.

Keep the existing effective terrain world-scale conversion and existing down-looking perspective approximation explicit. This placement correction does not prove a general ground-distance ruler for every oblique camera orientation. If camera rendering is separately cropped in the future, the actual Camera viewport input must change with it; do not crop the camera as an incidental ruler fix because that changes projection, picking, and composition.

### Native paint contract and scheduling

The ruler is an actual **Foreground `ruler_scale_bar` direct painter**, separately source-audited in `fe-ui/src/native_accessibility/AGENTS.md`; it is not an egui Area and is not discovered by the Areas scan. Preserve that explicit numeric/fixed-text admission and update the inventory to describe the new bounded placement/input contract. The Background path-label producer remains conservatively refused in positive capture profiles when edited point rows exist. Relocating the ruler does not remove that refusal or confer native capture evidence.

The current eight-producer `surface::finish` happens inside `panels/mod.rs` before the outer gardener system assigns ViewportRect, while some separately audited painters run later. Do not add a ninth mandatory Ruler producer and mark it after that finished publication. The minimal scope can retain the existing audited fixed/numeric direct-painter treatment, with a source-visible ordered draw before egui output completion. If the implementation instead makes ruler completion/validity part of dynamic native publication, explicitly move final surface completion to after the real ruler producer and all required producers, or add a separate late failure transition that invalidates an already published surface; verify the actual host paths and guard/epoch ordering. A stand-in helper or synthetic mirror callback is not proof of the real paint producer. Preserve Owner QA's read-before-provider-remap order and the native Windows suppression/alternate-publisher gates.

## Required verification after authoring

Extend the existing affected journeys rather than adding trivial tests mirroring the formulas. Exercise actual native tree Button responses at deep Verse/Fractal/Petal/Node rows, native hierarchy horizontal scrolling, eye/Options neighbors, splitters and adjacent camera exclusion at 0.75/1/1.25/2 scales and a same-context return to 1. The final observed target-and-clip geometry must meet the intended minimum without allocating through a neighboring target or gutter.

At 1024x720 scale 2 and at normal 1280x720, inspect the real Inspector at both scroll ends with populated properties/schema and pending drafts. Record body height/content offset and demonstrate Properties/Data/API Access/Query and section switching. A clean hit checker is necessary but does not replace screenshot and usable-body review.

For enabled and disabled terrain, verify the actual ruler paint bounds inside the central scene after side-panel resize, collapse, Results open/close, window resize, scale change/restoration, and navigation. Check scale calibration independently against actual camera viewport and effective world scale, including that toggling a sidebar alone does not spuriously change meters-per-pixel when camera projection remains full-window. Preserve the existing native/Owner gate journeys in the integrated final sweep. No full/native capture, retained restart, role/provenance, or numerical volume acceptance follows from this design review.

## Read anchors

These files were read and remain live/unfrozen QA41 evidence; hashes identify the reviewed bytes rather than a final packet:

| Path | SHA-256 |
| --- | --- |
| `qa/passes/41/owner-physical-quarter-layout-check.jsonl` | `1ae50b70e6b64327fe0c89bfc751ba5a4d2a255f54c8c6544c1224e1d0145aa5` |
| `qa/passes/41/owner-narrow-double-layout-check.jsonl` | `a15352c0b679dd9d0ae018665ecc6ca24dd5f3e923251a90a8109ed89b28cdd8` |
| `qa/passes/41/owner-narrow-double-ui.jsonl` | `dc6a489281ea6d97629099ac21e75611091ba9d44aec58db9ddee65ef29e54b0` |
| `qa/passes/41/owner-narrow-bottom-ui.jsonl` | `805fbe2ac6521eb46ac4154347609285810e88932dda2fb7f67d464f730742f9` |
| `.qa/run/20261104-130242-645/shots/130801-291-qa41-scale-075.png` | `50b1ec37e20d7356bdb0cddd72d17fd059b4f33677f51b532e4da06449426b63` |
| `.qa/run/20261104-130242-645/shots/130854-582-qa41-1024-scale2.png` | `5d8291291ea64cf4b50408da01dd6baf6b98aadee14258df9513c13ad222204d` |
| `.qa/run/20261104-130242-645/shots/130924-806-qa41-1024-scale2-bottom.png` | `1bc396a0cb408fa142f20dd297eb2b14752fefa3dc98e88751aefe3d37c5cd7b` |
| `fe-ui/src/panels/tree_entry.rs` | `e95da62a2d24d2dc5b94a7b9c3756ce6aa454eaef20eb182ad4badfc93c1894a` |
| `fe-ui/src/ui_shell/layout_policy.rs` | `212e5d69fe8215d1156e8aae10f10ae47b3437460e30cf6824d1031ac909e372` |
| `fe-ui/src/panels/inspector.rs` | `f154bc7beaf5323bdd51fa9d8c629d7e2bc9c4ed3fa548767f1305b6e178c22d` |
| `fe-ui/src/ui_shell/right_sidebar.rs` | `aeef203bc9d572976a15eb3f87d42c0f86dc44a8b6b411ce4e6fc5c6534fead4` |
| `fe-terrain/src/ruler_plugin.rs` | `e24d02145624340b7a7126cbd40313fbcead8da8844b066f44ed0ce5890784bb` |
