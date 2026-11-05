---
type: Review
title: QA57 actual narrow key readability triage
date: 2026-11-04
---

# QA57 narrow property identity — visual triage and bounded plan

**Correction required under the existing QA-0074 responsive property-grid work, at S2 scope.** I inspected both original PNGs, not only semantic bounds. At the measured67/79 logical-point bodies, displayed property names collapse to ellipses, so users cannot reliably identify which property the nearby Edit/Copy/Delete controls affect. Horizontal action reachability alone does not satisfy readable text or the user's no-visual-glitches requirement. No new finding ID or blanket acceptance is proposed.

Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`: preserve understandable identity, responsive structure, complete targets and the existing workflow. This is a plan/triage report only; no source/candidate edits or execution.

## Actual evidence

- Original `.qa/run/20261104-184017-448/shots/190146-383-qa57-matrix-1024-scale2.png`:1024×900, scale2. The visible first and second row names are merely `…`; the first type is also truncated. Edit/Copy are stacked and Delete fits the body horizontally. The adjacent value1.25 and the next object's preview do not reveal their property keys.
- Original `190150-601-qa57-matrix-1280-scale2.png`:1280×900, scale2. The first three visible row identities are again `…`; Edit/Copy share a line and Delete is below. The user can see action labels but cannot distinguish `grid_number`, `grid_payload` and `grid_remove` from the painted keys.
- `qa/passes/57/matrix-1024-scale2-comparison.json` records body67 and content134physical pixels; the1280 counterpart records body79/content158. Both have horizontal offset0. Their UI receipts retain complete accessible action names such as `Edit property grid_number` and `Expand stored property grid_payload`. Those semantic names prove target identity for automation; they do **not** make the painted key readable to the user.
- Both comparisons still have `whole_targets_fit=false`: Edit/Copy rect50physical pixels against clip/hit48. That is the separately authored height correction. This report's name-readability finding does not replace, waive or claim that height boundary passed.

The source explains the observed pattern. Current `fe-ui/src/panels/property_grid.rs:44–48` assigns compact key width `min(width*0.38, width-gap-height)`. With width67/79, gap8 and height24, the key receives approximately25.46/30.02 logical points, while the value retains the remaining width. `content_cell` fixes compact content height to2 rows. `fe-ui/src/panels/inspector.rs:1936–1948` then draws the key with `.truncate()` and places its type below within that small key cell. The same short fixture keys are therefore visually erased despite correct semantic names and zero horizontal scroll.

## Minimal responsive correction

Keep the wide four-column grid and the useful existing two-column compact layout at ordinary bodies. Add **one narrow stacked-content mode** inside the existing Columns/row rendering policy; keep its mode decision in Columns rather than scattering width checks or duplicating state.

1. Enter the narrow mode when a compact body cannot reserve a legible key region plus the existing value minimum and gap. Reuse the current ordinary key reserve/measured text and current minimum-height inputs for the breakpoint; no Inspector width-floor change or viewport growth. Do not choose a breakpoint merely to exclude the actual67/79 or minimum52-point cases.
2. In that mode give the property key the **full available width**, render its actual complete text with wrapping, and let its height follow the resulting galley. Place type on the next full-width line. For underscore/dotted or other unbroken keys, wrapping must support breaking an overlong token; a lone ellipsis or tooltip-only recovery is insufficient. Do not retain the fixed2-row clipping slot around a potentially multiline key/type stack.
3. Place the existing value preview/expand control on its own full-width row, then reuse the existing wrapped action row and canonical Confirm/Cancel helper. Extra vertical scroll is acceptable; horizontal scroll must not be needed to identify a key or reach its controls. Keep summary/detail behavior bounded; this does not request rendering an entire large JSON value inline.
4. Continue the existing `(node_id,key)` persistent scope, draft map, expansion state, callbacks, pending/loading and destructive-arm ownership. Layout transitions must not alter raw text, selection, value types, saved data, clipboard text, pending writes or action dispatch. Coordinate against the separately authored exact intrinsic-height correction; neither fix should reintroduce the other's clipping boundary.

This is a presentation branch within the existing grid, not a new editor, panel floor or replacement state model. Proposed source scope is only the existing property_grid geometry/Inspector row rendering, their directory documentation and meaningful existing-journey extensions. Root must authorize source authoring separately.

## Bounded acceptance for the eventual patch

Use the original keys `grid_number`, `grid_payload`, `grid_remove`, `grid_text`, plus one long unbroken key and one Unicode key. At actual bodies52/67/79 and both sides of the selected transition, verify painted full key/type wrapping without overlap or clipping, then value and all complete row/destructive confirmation targets. Inspect actual after screenshots at the original1024×900/2 and1280×900/2 settings alongside these originals. A semantic full-name check, control count or `whole_targets_fit=true` alone cannot pass name readability.

Reuse the existing responsive journey to open/cancel a typed dirty draft, retain it through narrow↔ordinary↔wide transitions, and confirm unchanged stored data/no queued write on cancellation. Exercise exact Copy and one deliberate confirmed deletion through existing authority paths at the execution boundary. Preserve ordinary/default and wide layout behavior and all independent height checks, including the fractional frame-margin regression requested separately. Source review and authored cases supply no live or FULL coverage credit.

## Evidence pins

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Original1024×900/2 PNG | 172754 | 17d17110c81cc3d32e069dc0fc41206dbb9e9175e7f98e36c14892de3dcd56eb |
| Original1280×900/2 PNG | 202833 | a1be41d2ccc50439639454031e7840b6720fcf4b725ad873c211441ea1f053b0 |
| matrix-1024-scale2-comparison.json | 2442 | cddc2fac22d7bfb935ed99c21cc08a1e674ec2e289a2b67f628c041d50029566 |
| matrix-1280-scale2-comparison.json | 2454 | 89adf19cc4e8a46a5af6bbcd2c57626abdf071bcd0c66a554c7919c0c4fb2553 |
| owner-matrix-1024-scale2-grid-after-ui.jsonl | 192755 | b875c7e0f868c23a23dc262c2a936af2ad36d3825e05f2f7a67b595ee71b1cb3 |
| owner-matrix-1280-scale2-grid-after-ui.jsonl | 192900 | 7b170138075a50e6d5d9a4ae3985ba0e2adca9bb5ef3d3b183fab09ab7e3f355 |

Shot receipts link those original files and record include_ui=true, source orbit and the corresponding dimensions. Scope is limited to this actual property-grid state; no broader audit or unrelated tree/header fix is inferred from these images. Only this report was written. No source/candidate mutation, implementation parser/tests/formatter/compiler/build, runtime/app/OS/native/API/DB/network or Git mutation occurred.
