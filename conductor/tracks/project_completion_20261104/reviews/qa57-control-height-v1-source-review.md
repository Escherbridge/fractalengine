---
type: Review
title: QA57 fractional margin correction requested
date: 2026-11-04
---

# QA57 property control height — independent correctness/boundary review

**REQUEST CHANGES: HGT-B1, frame-margin conversion must match pinned egui.** The isolated candidate is narrowly scoped and preserves interaction/state ownership, but its new height formula can still reserve less than the actual framed button height for supported fractional padding. Do not integrate as a fully corrected intrinsic-height boundary until this small math/test correction is reviewed. No runtime or live-fix acceptance is supplied.

Reviewed the exact three-file packet `.omc/research/qa57-property-control-height-v1`: manifest 2,714 bytes SHA256 `10231b482ca389819fa09e6b261fe1718a5b79ce367c2dd5ebc97409108e7118`; full patch 6,914 bytes SHA256 `fb0aa81bb3d0ad3930314a63cf6d5d6959969bec9f3b630d013a5f3ec5a8328c`. Independently checked every before/raw byte length/hash, patch hash, observed GUI57 anchor and both frozen native candidate anchors. Zero mismatches. All three current canonical production files still match before bytes; the isolated candidate did not change root GUI57's source. Apply the shared UI guidance at `C:/Users/atooz/.gemini/UI-UX-guidance.md`: complete visible/hittable targets and simple shared geometry remain the standard.

## HGT-B1 — raw padding differs from actual integer frame margins

Candidate `raw/fe-ui/src/panels/property_grid.rs:27–28` computes active vertical padding as `ui.spacing().button_padding.y * 2.0`, then ceilings the total text height. That does not model the actual pinned egui0.33.3 frame sizing:

- `egui-0.33.3/src/widgets/button.rs:272–274,285–300` keeps interact height as a minimum, selects button padding from button_frame, and passes its Vec2 into `Frame::inner_margin`.
- `egui-0.33.3/src/containers/frame.rs:104,252–254` stores an `epaint::Margin`, converting that Vec2 through `Into<Margin>`.
- `epaint-0.33.3/src/margin.rs:115–119` rounds **each side** before storing it: `Margin::symmetric(v.x.round() as _, v.y.round() as _)`.
- `egui-0.33.3/src/atomics/atom_layout.rs:274–280` adds the converted frame margin sum to actual content height, then applies minimum size.

Concrete source-level counterexample: Button is the largest measured row height T, content dominates the interaction minimum, and vertical button padding is1.5. Egui converts each vertical side to2, so the actual button needs `T+4`. Candidate reserves `ceil(T+3)`, which is strictly smaller than `T+4` for every fractional part of T. Thus a normal fixed action/cell can again clip the leaf's full rect/hit area. This is not an invented25-point threshold or a claim that GUI57 used fractional padding; it is a precise supported-style boundary of the proposed measured formula. The added regression profile uses integer padding4.0 and cannot distinguish these formulas.

**Concrete minimal fix:** when button_frame is active, measure the same conversion used by Button, for example `egui::Margin::from(ui.spacing().button_padding).sum().y`; otherwise use0. Keep the separate interaction minimum. Add a fractional vertical-padding profile (e.g.1.5 with enlarged dominant Button font) to the same existing complete-target/arm/cancel journey. Require complete normal and armed rect/hit/child/table intersections at the actual minimum body and pair thresholds. Update the directory rationale to say converted frame margins, rather than raw floating padding. No new abstraction, global floor or state model is needed.

This is the only requested source correction from this review. No additional speculative blocker is inferred from arbitrary style overrides, font fallback or unexecuted test outcomes.

## Accepted source assessment, subject to HGT-B1

The underlying correction addresses a real measured failure: the pinned `qa/passes/57/owner-probe-default-grid-after-ui.jsonl` is 205,277 bytes, SHA256 `6f0bcffec85b968a8d49f391d0fe568bfb708195be3656c880ecf337b580d14e`. Its semantic Edit property grid_number leaf is y313/h25; the recorded child/hit allocation is24. A minimum interact height is not a maximum padded button height. The candidate correctly uses text-style heights rather than replacing24 with hardcoded25.

`Columns::new` measures Button, Small and Body row heights and takes their maximum before adding frame padding and applying the existing target/interact minimum. `Ui::text_style_height` (`egui/src/ui.rs:764–765`) obtains the resolved font row height with egui rounding. The single resulting allocation height flows to fixed ordinary cells, compact content/action geometry and dynamic full-width action children. The layout can grow vertically instead of narrowing targets or raising an Inspector/viewport floor. The intrinsic content plus frame formula is the right seam; HGT-B1 concerns its exact conversion.

Keeping `interaction_height` separate is necessary here: `confirmation_pair_width` in canonical `fe-ui/src/panels/widgets.rs:222–236` measures Small Confirm/Cancel labels against target/interact height before child rendering. The canonical confirmation helper at `:239–280` uses that same interact minimum and chooses stacked layout when the pair exceeds width. Assigning the padded allocation height as a new child minimum would change that calculation between measurement and render. The candidate instead assigns the preserved interaction minimum in both `Columns::row` and `actions_row`; the shared confirmation helper/state is untouched. Both normal and compact action branches continue to use the existing slots/dynamic-height behavior.

The only production source change is property_grid geometry; inspector changes are inside the existing test journey and directory documentation records the reason. Exact patch comparison shows no change to action callbacks, IDs, selection, authorization, loading/pending gates, destructive arm/expiry, property parsing, saved data, clipboard formatting, drafts, journal or manager ownership. One shared state model remains in use. No width floor, viewport routing or native observer source changes.

## Meaningful authored regression and its limits

The existing Inspector journey's prior responsive matrix, real editor/draft/cancel, exact clipboard and destructive-action checks remain intact. The new block at candidate `inspector.rs:3164–3199` enlarges Button font20 and padding4, measures the shared pair, and checks actual52-point body, pair±1 and260/640 widths. It verifies complete control rect/hit/child/table intersections with zero horizontal offset, actual Delete arm and Cancel, unchanged saved properties and an empty action queue; style is restored afterward. Explicit `rect.height()>24` prevents the enlarged-font case silently missing the original height seam. This is meaningful integrated existing-journey coverage, not an implementation-mirroring assertion or extra test registration.

These assertions have **not run**. The new block does not replace the original matrix and supplies no live success or guarantee of compilation. Add the fractional-margin profile for HGT-B1; root then owns integrated test/build and fresh full-target live checks at required widths/scales. Actual screenshots must be inspected after rebuilding; a source formula or semantic control count alone is insufficient.

Reviewed raw source pins:

| Canonical path | Raw bytes | SHA256 |
| --- | ---: | --- |
| fe-ui/src/panels/property_grid.rs | 7766 | 1123aaa5e58b5fb5e055d4ed0068d6a4c532959465e55e751c7ad2e8dded6c74 |
| fe-ui/src/panels/inspector.rs | 285484 | 4afa4fcd1f1fe3ae77fda983c3b884daa0aa92addfd127b0b1fb439612ffccb1 |
| fe-ui/src/panels/AGENTS.md | 72746 | b5fc5a8cd4de84da43c3f3a7ea4c42503237de8ca40a0d161bf7ebe6ffefaa96 |

Only this review report was written. Read-only source/evidence/hash inspection was used. No implementation parser, formatter, tests, compiler/build, application/native/OS/process, API/DB/network or Git mutation occurred; frozen v1 and canonical source remain untouched.
