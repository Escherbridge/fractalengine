---
type: ImplementationReport
title: QA57 first control height candidate
date: 2026-11-04
---

# QA57 property control height — isolated candidate v1

Frozen for independent source review. Only this candidate directory was written;
the three canonical production files remain exact before bytes. The separate
native provider candidate manifest/patch also retain their original hashes.
Baseline identity: `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`; exact captured
before bytes, rather than a clean-checkout claim, establish the source comparison.

Root's actual QA57 boundary is `Edit property grid_number` rect y313/h25 versus
its child clip y313/h24 and hit h24. The complete target comparison correctly
fails even though width now fits. The raw attempt remains unchanged at
`qa/passes/57/owner-probe-default-grid-after-ui.jsonl` (205277 bytes, SHA256
`6f0bcffec85b968a8d49f391d0fe568bfb708195be3656c880ecf337b580d14e`).

Pinned egui0.33.3 primary source explains the source seam:

- `src/widgets/button.rs:44–48`: ordinary Button falls back to Button text style.
- `:272–274,285–300`: interaction height is a minimum; normal framed buttons add
  both vertical padding margins. RichText.small is not Button.small().
- `src/atomics/atom_layout.rs:274–280`: allocation takes text content plus frame
  margins, then the requested minimum. A24-point minimum is not a24-point cap.
- `src/ui.rs:764–765`: text_style_height measures the resolved font row through
  fonts.row_height, including egui's rounding.

`property_grid.rs` now takes measured Button/Small/Body font row heights plus
active button-frame vertical padding, then the existing target/interact minimum,
rounding the reserved row height upward. Both fixed cells and compact actions
use that allocation height. No hardcoded25-point replacement is introduced.

A separate interaction_height retains the original child minimum. Raising it to
the padded row height would make the existing confirmation pair render against
a different minimum from its earlier shared width measurement. Keeping those
concepts separate preserves canonical confirmation sizing/state and fits its
Small labels in the measured allocation. No widgets/global layout change,
Inspector floor, viewport allocation, callback, ID, permission or draft change.

The existing Inspector journey retains its original matrix/draft checks and adds
one enlarged Button-font/padding profile. At actual52 points, pair±1 and260/640
frames, normal and armed complete rect/hit/child/table intersections must fit at
zero horizontal offset; actual arm/cancel must leave data/actions unchanged.
Original style is restored. No new test declaration or registration; all cases
remain unexecuted here.

Manifest **2714 bytes**, SHA256
`10231b482ca389819fa09e6b261fe1718a5b79ce367c2dd5ebc97409108e7118`.
Standard `source.patch` **6914 bytes**, SHA256
`fb0aa81bb3d0ad3930314a63cf6d5d6959969bec9f3b630d013a5f3ec5a8328c`.
Manifest maps canonical paths to exact before/raw hashes:

| Candidate suffix | Bytes | Raw SHA256 | Added/removed |
| --- | ---: | --- | ---: |
| property_grid.rs | 7766 | 1123aaa5e58b5fb5e055d4ed0068d6a4c532959465e55e751c7ad2e8dded6c74 | 10/3 |
| inspector.rs | 285484 | 4afa4fcd1f1fe3ae77fda983c3b884daa0aa92addfd127b0b1fb439612ffccb1 | 36/0 |
| AGENTS.md | 72746 | b5fc5a8cd4de84da43c3f3a7ea4c42503237de8ca40a0d161bf7ebe6ffefaa96 | 13/0 |

Filesystem/text/hash inspection only: production mismatches0, native frozen
anchor mismatches0. No implementation parser, tests, formatter, compiler, apps,
OS/native/API/DB/network or Git action. Remaining: independent source reviews,
root integration after live freeze, build and full-target live recheck at the
required widths/scales. This source candidate provides no live-fix/FULL credit.
