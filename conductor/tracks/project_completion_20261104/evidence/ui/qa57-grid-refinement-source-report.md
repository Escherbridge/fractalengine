---
type: ImplementationReport
title: QA57 measured and readable property grid refinement
date: 2026-11-04
---

# QA57 grid refinement — combined isolated candidate v1

Frozen for independent functional/design review. Only this candidate directory
was written; all three canonical production files remain exact root6ca before
bytes. Both height freezes are preserved. Parent snapshots match height-v2 raw
bytes, and its converted-margin/font-measurement block is inherited byte-exact.
No native candidate or frozen QA packet was edited.

Independent triage `qa57-narrow-property-key-review.md` (SHA256
`171c6af402c9df40a559640d1b236254af517cc3089e98ea04936b80821e4cfd`)
inspected original67/79-point-body PNGs showing every key as an ellipsis. Correct
semantic action names and horizontal reachability do not make those keys readable.
This authoring report relies on that visual triage; it claims no new live image.

## Production refinement

Columns owns one stacked-content branch below the existing96-point key reserve
plus measured value-control minimum and8-point gap. In that mode key/type occupy
the complete body in a natural-height cell, followed by the full-width value and
the existing wrapped actions. The Property heading also wraps naturally. Where
two compact columns fit, the key now retains at least96 points; the260-point
body's existing38% proportion and wide four-column layout remain unchanged.
The mode is based on existing readable reserves, not the failed sample widths.

Inspector uses the same row callback, stored preview/expand, Edit/Copy/Delete,
confirmation and draft branches. Only key/type label wrapping follows Columns'
mode. Parent-scoped node/key and column IDs remain; actual leaf changes can
disarm safely across resize. No new editor/state owner, mutation, guard bypass,
confirmation state machine, horizontal-scroll requirement or viewport floor.
Extra key height belongs to existing vertical scrolling.

Pinned egui0.33.3 Label::layout_in_ui sets actual available wrap width and allocates
the galley size (`widgets/label.rs:183–191,230–267`). Epaint0.33.3's word/punctuation
break policy falls back to a character boundary (`text/text_layout.rs:372–402,
1009–1016`), allowing overlong tokens to use full lines. The new scope does not
reapply the former fixed two-row clip. No custom text renderer or break-every-
character policy is introduced. Height-v2's exact Margin conversion, WidgetText
font precedence and separate interaction minimum remain intact.

## Authored checks and limits

The same existing Inspector journey retains both height profiles and prior
action/draft/clipboard cases. Additions render original grid_number/payload/
remove/text keys, a long unbroken key and Unicode key at actual52/67/79 and both
sides of the measured transition. They inspect native painted Shape::Text data:
no elision, complete glyph characters and every placed row within paint/table
clips. Long narrow keys must wrap into multiple rows and use more than one glyph
per line. Short fixture keys/type stay complete above transition; ordinary long-
key truncation/hover outside stacked mode remains existing behavior.

The numeric draft keeps exact raw-150.250 across stacked/compact/wide transitions;
cancel leaves stored properties and action queue unchanged. No new test
declaration/registration. These checks remain unexecuted and cannot prove visual
glyph readability or successful compilation. Root must inspect rebuilt originals'
1024/1280x900/2 counterparts and full target geometry. GUI57 did not reach live52;
that remains an authored boundary, not claimed live coverage.

## Exact packet

Manifest **4284 bytes**, SHA256
`8ccb7063b74001dda2f35761cf01b19fed85d47f6e3146a93b3732d0f204abd8`.
Full source.patch **21996 bytes**, SHA256
`7d781060dfa9ab43556b44ed0e4fa3c48bed7ec9655796f2a0c8a9f14a20606f`.
parent-to-refinement.patch **15692 bytes**, SHA256
`e20b861a6761774c3c0580dca9319caf68e0a9ec984d1952f7d8f41dafccf35e`.
Manifest maps each canonical path to before/parent/raw bytes and hashes, with
height/triage anchors. Baseline identity is6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a;
exact bytes qualify the stage's dirty-source provenance.

| Raw panels suffix | Bytes | SHA256 | Full +/− | Parent delta +/− |
| --- | ---: | --- | ---: | ---: |
| property_grid.rs | 8996 | 3d7a6656cd484a4b1fa83491625a8165b9f09e76b8abadfc27c36937f725e6d4 | 48/11 | 33/7 |
| inspector.rs | 290792 | c00b41b5ae688d11675920f70ce1ea7da0694b7af5865c6703ef753e5f8a2bbd | 117/6 | 79/6 |
| AGENTS.md | 75411 | 7f6871c493295542d9c006b7992fe31589f40350d3292fd9e6826a07b70dbd09 | 52/4 | 32/4 |

Filesystem/text/hash checks: production mismatch0, parent mismatch0, inherited
height block exact. No implementation parser/tests/formatter/compiler/build,
apps/OS/native/API/DB/network or Git mutation. Independent source reviews, root
integration/build and matching live checks remain required; no FULL/live-fix credit.
