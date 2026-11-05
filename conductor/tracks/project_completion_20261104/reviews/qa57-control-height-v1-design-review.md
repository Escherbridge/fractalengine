---
type: Review
title: QA57 initial height design review
date: 2026-11-04
---

# QA57 property control height: independent design review

Verdict: **APPROVED for the exact isolated three-file candidate. No design, ownership or readability blocker found.** The correction addresses the measured intrinsic-height versus allocation-height seam without changing confirmation state, property effects or viewport allocation. Source approval does not close QA-0074 or establish rebuilt control geometry; root must integrate, compile and inspect actual results.

Applied root/panels guides and C:/Users/atooz/.gemini/UI-UX-guidance.md. Only this report is written. Production and both isolated candidate families remain untouched.

## Exact scope and actual boundary

Manifest `.omc/research/qa57-property-control-height-v1/manifest.json`: 2714 bytes, SHA256 `10231b482ca389819fa09e6b261fe1718a5b79ce367c2dd5ebc97409108e7118`.
Patch: 6914 bytes, SHA256 `fb0aa81bb3d0ad3930314a63cf6d5d6959969bec9f3b630d013a5f3ec5a8328c`.
Baseline identity: `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`, qualified by exact captured before bytes. All three before/raw copies match their manifest; canonical production still matched each before copy when checked.

| Candidate panels suffix | Bytes | SHA256 |
| --- | ---: | --- |
| property_grid.rs | 7766 | `1123aaa5e58b5fb5e055d4ed0068d6a4c532959465e55e751c7ad2e8dded6c74` |
| inspector.rs | 285484 | `4afa4fcd1f1fe3ae77fda983c3b884daa0aa92addfd127b0b1fb439612ffccb1` |
| AGENTS.md | 72746 | `b5fc5a8cd4de84da43c3f3a7ea4c42503237de8ca40a0d161bf7ebe6ffefaa96` |

The observed-boundary file is exact205277 bytes/SHA256 `6f0bcffec85b968a8d49f391d0fe568bfb708195be3656c880ecf337b580d14e`. Its semantic Edit property grid_number node has id16652917212089951633, x1000/y313/w31.03125/h25. The same ID's layout observation has rect h25, hit h24 and child clip h24. Thus the source addresses an actual full-target mismatch, not a guessed 25-point style preference. This review inspected that JSON boundary, not a new running application.

## Appropriate geometry/value separation

Columns::new now measures Button/Small/Body row heights, adds active button-frame vertical padding and takes the maximum with the existing physical target/interact minimum, rounded upward. Pinned egui0.33.3 ui.rs:764-765 resolves text_style_height through fonts.row_height; button.rs:267-300 applies the interaction size as a minimum and includes frame padding in actual content allocation. The new code follows those intrinsic quantities rather than replacing24 with a hardcoded25.

The private interaction_height field keeps two meanings separate: reserved allocation height and the child interaction minimum. Row and action layout children retain the original minimum, while fixed cells and compact natural-height slots reserve the measured padded height. This matters for the existing shared confirmation pair: its width measurement and Small-label rendering continue to see the same interaction minimum. Enlarging the allocation does not silently enlarge a later confirmation minimum and trigger a different orientation inside a fixed slot. The canonical widgets.rs implementation is not edited.

This is an appropriate small layout-policy value, not a new state owner. It computes font metrics once per table, then reuses the result across rows/actions. No parser, draft, confirmation timeout/owner/reconciliation, callback, pending/permission, identifier, Inspector width floor or viewport path changes are present. Expanded editors retain their existing visible-width scope. The broader responsive design and safe disarm when actual leaf identities change are unchanged.

## Meaningful authored regression and limits

The existing journey adds a local enlarged Button-font/vertical-padding profile, requiring actual normal and armed rectangles to fit rect/hit/child/table intersections at zero horizontal offset. It includes an actual52-point body, measured pair-minus/plus-one and ordinary compact/wide widths, then arms/cancels real production widgets and checks no data/action effects. It restores the original style before later existing cases. This targets intrinsic height exceeding the old24-point slot instead of asserting only the new computed height. No new test registration or synthetic confirmation model is added.

Those cases have not run here. Actual font rounding, wrapped natural height, scaled narrow targets, full labels, compound values, retained drafts, pending/loading states and normal widths still need matching production build/live review and the final integrated sweep. No implementation parser, tests, formatter, compiler/build, app/native/process/API/DB/network action or Git mutation occurred; source/artifact/primary local egui reads and hash checks were the only verification. No FULL, peer, CI or release acceptance is claimed.
