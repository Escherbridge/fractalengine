---
type: Review
title: QA57 combined grid source correctness review
date: 2026-11-04
---

# QA57 combined grid refinement — independent correctness/boundary review

**APPROVED for the exact frozen three-file combined source candidate.** HGT-B1 is corrected and the narrow-key response follows the actual screenshot triage without increasing a panel floor. No new correctness/boundary blocker was found in this complete source delta. This is source approval only: authored journeys remain unexecuted, and neither rebuilt visual success nor FULL coverage is inferred from this review or earlier height reviews.

Packet: `.omc/research/qa57-grid-refinement-v1`, manifest 4,284 bytes SHA256 `8ccb7063b74001dda2f35761cf01b19fed85d47f6e3146a93b3732d0f204abd8`; full baseline patch 21,996 bytes SHA256 `7d781060dfa9ab43556b44ed0e4fa3c48bed7ec9655796f2a0c8a9f14a20606f`. The 15,692-byte parent delta hashes to `e20b861a6761774c3c0580dca9319caf68e0a9ec984d1952f7d8f41dafccf35e`. Independently verified all before/parent/raw lengths and hashes, both patches and every manifest anchor. Parent copies equal frozen height-v2 raw bytes; all three canonical production files still equal the root6ca before copies. The root GUI57 source and prior freezes remain untouched.

Applied the shared UI guidance at `C:/Users/atooz/.gemini/UI-UX-guidance.md` and the actual two-PNG narrow-key triage in `.omc/research/qa57-narrow-property-key-review.md`. The observed67/79-point bodies painted ellipses rather than readable keys; horizontal action reachability and complete semantic names do not discharge that defect. This source is assessed against that actual boundary, not a lowered metric.

## Exact height/font correction

Candidate `property_grid.rs:24–34` independently addresses the pinned egui behavior:

- Plain button font uses `FontSelection::Default.resolve_with_fallback(...Button...)`. Pinned egui0.33.3 `style.rs:144–173` implements override_font_id, then override_text_style, then fallback. This matches plain Button/WidgetText fallback rather than assuming TextStyle::Button ignores overrides.
- Explicit Small text measures override_font_id when present, otherwise Small; Body uses Default resolution. Pinned `widget_text.rs:406–422` applies font override before explicit text style and fallback. Thus the measured maximum covers the actual Button/Small/default rows used here without inventing a new font policy.
- Active button padding passes through `egui::Margin::from(padding).sum().y` before the interaction minimum and final ceil. Button calls Frame::inner_margin with that same Vec2 (`widgets/button.rs:285–300`), Frame stores Margin (`containers/frame.rs:104,252–254`), and epaint rounds each side (`margin.rs:115–119`). Fractional1.5 therefore contributes4, fixing HGT-B1's raw3-versus4 discrepancy. Frame-disabled padding contributes0. This is intrinsic measurement, not hardcoded25 or a target reduction.
- The original target/interact minimum stays in `interaction_height`, separately from padded allocation height. Both ordinary row and wrapped action row assign this original minimum. The existing shared confirmation-pair measurement and actual helper therefore see the same minimum; raising the child minimum to the padded allocation height would have changed pair sizing after measurement. No confirmation helper/state machine is modified.

No claim is made that arbitrary fonts or every external style profile have been executed. The source follows the actual pinned APIs; root still owns compilation and real-scale observation.

## Narrow geometry and natural text height

Columns owns one `stacked` decision: compact body below `96 + measured height + 8`. These are the existing key reserve, current value-control minimum and gap; the failure widths are not special-cased. In this branch key and value widths each equal the available body, and `row` uses vertical layout. The key/type are followed by the full-width existing value branch and then the same wrapped actions.

At or above the transition, the two-column compact calculation clamps key to at least96 while retaining at least the measured value minimum: the branch precondition ensures `width-gap-height >=96`. Widths plus gap equal the body. A260-point body retains its original38% key proportion; where that proportion previously fell below96, the existing readable reserve now takes priority. The ordinary four-column branch keeps its previous formula with the named96 constant. No Inspector/viewport floor, horizontal-scroll dependency or new panel is introduced.

The stacked `content_cell` uses a parent-scoped natural-height `scope_builder` rather than fixed_cell's explicit rectangle clip. Pinned `egui/src/ui.rs:263–340` clones the parent painter for a child; max_rect does not automatically replace the inherited clip. `scope_dyn` at `:2505–2521` advances the parent by the child's actual min_rect after rendering. Consequently the initial minimum-height rect does not reimpose the old two-row cap as wrapped key/type text grows. The stacked Property heading takes the same path. The existing table/Inspector clip remains authoritative; extra content belongs to vertical scrolling.

Pinned Label (`widgets/label.rs:183–191,230–267`) wraps to actual available width and allocates the produced galley. Epaint's break policy (`text/text_layout.rs:372–402,1009–1016`) prefers ordinary boundaries and can fall back to a character boundary for an overlong token. The candidate uses Wrap only for stacked key/type/heading labels; it does not force every character onto a separate line or substitute ellipsis/tooltip-only recovery. Ordinary long-key truncation/hover above the transition remains its existing policy, not a new promise of full-key display at every width.

## State and callback boundaries

The full inspector production delta changes heading placement and label wrap modes only. The existing row-action closure, stored preview/expansion control, Edit/Copy/Delete callbacks, loading/editing/pending conditions, parser, saved values, typed drafts, selection and manager ownership remain intact. Existing `(node_id,key)` row scope and `("property-column",column)` scopes continue to bind the same domain targets. No separate editor state or duplicate commit route is introduced.

Canonical Confirm/Cancel remains the sole destructive state machine. Moving physical leaf layout can safely disarm through existing leaf-binding behavior; no armed-continuity promise is needed or added across resize. Within a layout, the existing normal/armed/canceled geometry and action checks remain. The correction neither grants permissions nor weakens pending/unknown-write ownership. Root source is still unchanged while GUI57 runs.

## Authored regression is meaningful and uses actual pinned APIs

The existing journey retains its earlier responsive editor/draft, clipboard, arm/cancel and no-write assertions. New height profiles `(Button20,padding4)` and `(Button24,padding1.5)` require intrinsic control height above24 and compare complete rect/hit/child/table intersections at actual52, pair±1 and260/640 widths. Actual arm/cancel is retained with unchanged stored properties and empty action queue. The fractional case now exercises the exact prior HGT-B1 counterexample, rather than another integer-padding example.

The new narrow block covers original grid_number/payload/remove/text, a long unbroken key and a Unicode key at52/67/79 and transition±1. It asserts measured actual body/mode and complete targets. Its text checks inspect `FullOutput.shapes` containing native `Shape::Text`, match Galley text, reject elision, reconstruct glyph characters, and intersect every placed row against paint/table clips. The APIs are real public fields/methods in pinned epaint0.33.3: `Galley.elided`, `Galley::text`, `PlacedRow.row`, `Row.glyphs`, `Glyph.chr`, and `PlacedRow::rect_without_leading_space` (`text_layout_types.rs:534,567–618,666–669,792`). This is stronger evidence than checking a semantic label alone and does not add an implementation parser or custom renderer.

Long stacked labels must occupy multiple rows with multiple glyphs on at least one row. The numeric draft keeps exact `-150.250` through stacked/compact/wide frames; Cancel leaves data/action queue unchanged. Style and the prior stored fixture are restored before the original later clipboard/deletion checks. No extra test declaration/registration is added.

These assertions are **authored, not run**. Original character identity can survive font fallback to a missing-glyph box, so glyph-character equality cannot certify visual glyph quality. The report appropriately leaves actual screenshots required. Root must rebuild the approved bytes and inspect the original1024/1280×900/2 counterparts, ordinary/default/wide states, full normal/armed controls, key/type readability and transitions. Actual live52 was not reached in GUI57 and remains a future execution boundary. No earlier screenshot is evidence of this candidate's rendered result.

## Approved exact raw source

| Canonical path | Raw bytes | SHA256 |
| --- | ---: | --- |
| fe-ui/src/panels/property_grid.rs | 8996 | 3d7a6656cd484a4b1fa83491625a8165b9f09e76b8abadfc27c36937f725e6d4 |
| fe-ui/src/panels/inspector.rs | 290792 | c00b41b5ae688d11675920f70ce1ea7da0694b7af5865c6703ef753e5f8a2bbd |
| fe-ui/src/panels/AGENTS.md | 75411 | 7f6871c493295542d9c006b7992fe31589f40350d3292fd9e6826a07b70dbd09 |

Only this review report was written. Read-only source/evidence/hash inspection was used; no implementation parser/tests/formatter/compiler/build, app/OS/process/native/API/DB/network or Git mutation occurred. Approval is bounded to these bytes and does not approve later integration changes without reconciliation.
