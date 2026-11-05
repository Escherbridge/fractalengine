---
type: Review
title: QA57 combined grid design review
date: 2026-11-04
---

# QA57 combined grid refinement: independent design review

Verdict: **APPROVED for the exact final isolated three-file candidate below. No remaining design/readability/ownership blocker found.** This review evaluates the combined bytes, including inherited height-v2 and the new narrow-content mode. Earlier height/grid approvals are not substituted for this review. Approval does not prove corrected screenshots, executed glyph/geometry assertions or close QA-0074.

Applied root/panels guidance and C:/Users/atooz/.gemini/UI-UX-guidance.md. Only this report is written; all candidates, canonical source and frozen QA evidence remain untouched.

## Exact reviewed scope

Manifest `.omc/research/qa57-grid-refinement-v1/manifest.json`: 4284 bytes, SHA256 `8ccb7063b74001dda2f35761cf01b19fed85d47f6e3146a93b3732d0f204abd8`.
Full patch: 21996 bytes, SHA256 `7d781060dfa9ab43556b44ed0e4fa3c48bed7ec9655796f2a0c8a9f14a20606f`.
Parent-to-refinement patch: 15692 bytes, SHA256 `e20b861a6761774c3c0580dca9319caf68e0a9ec984d1952f7d8f41dafccf35e`.
Baseline identity: `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`, qualified by actual before bytes rather than a clean-checkout claim.

All three before/parent/raw sizes and hashes, both patches and five height/triage anchors match the manifest. Parent copies match frozen height-v2 raw files. The font/margin/interaction measurement block is byte-identical between parent and combined candidate. Canonical production still matched all three before copies when checked.

| Candidate panels suffix | Bytes | SHA256 |
| --- | ---: | --- |
| property_grid.rs | 8996 | `3d7a6656cd484a4b1fa83491625a8165b9f09e76b8abadfc27c36937f725e6d4` |
| inspector.rs | 290792 | `c00b41b5ae688d11675920f70ce1ea7da0694b7af5865c6703ef753e5f8a2bbd` |
| AGENTS.md | 75411 | `7f6871c493295542d9c006b7992fe31589f40350d3292fd9e6826a07b70dbd09` |

## Actual readability boundary and coherent layout policy

I viewed the original190146-3831024x900/2 and190150-6011280x900/2 PNGs linked by the triage. They visibly show ellipses in place of property keys, despite readable row-action labels. Accessible full names and horizontal action reachability do not solve this painted identity problem. These are before images only; no corrected image was generated or accepted here.

Columns remains the single layout-policy boundary. KEY_MINIMUM names the existing96-point readable reserve. The narrow stacked mode is selected when compact content cannot fit that reserve, the measured value-control minimum and the existing gap; it is not a threshold chosen merely to exclude67/79-point samples. Compact rows above the transition reserve at least96points for identity. Actual260-point bodies retain their38percent proportion, and the wide four-column formulas remain.

In stacked mode, the same content_cell helper gives key/type the full body width and a natural-height scope. The value follows at full width, then the existing wrapped actions. Heading and key/type labels use native wrapping. This removes the former fixed two-row clip around potentially multiline text. Extra height belongs to existing vertical scrolling; no Inspector/viewport floor or horizontal travel is added. Native wrapping can break overlong tokens across available lines without a custom text renderer or forced one-character layout. This is an appropriate small presentation strategy, not a second property editor or state model.

## Inherited height correction and ownership

The final height block converts active padding through egui::Margin before summing it. I checked pinned epaint0.33.3 margin.rs:81-82,115-118: each1.5-point side rounds to2, yielding4 rather than raw3. Thus HGT-B1's raw-padding assumption is absent from this candidate. Default Button/Body and explicit Small measurement follow the actual WidgetText font precedence; pinned widget_text.rs:406-418 and style.rs:157-168 support the override/fallback distinction. No fixed25-point workaround is introduced.

Reserved allocation height and interaction minimum remain separate, so the canonical confirmation pair sees the same minimum during width measurement and rendering. Existing natural-height oversized action slots and the single canonical confirmation state machine remain. No widgets.rs or global theme policy change is part of this combined candidate.

Inspector still renders one shared action callback with unchanged Edit, exact stored-value Copy and guarded two-step Delete effects. Loading/editing/pending guards, typed parser/drafts, expanded value scope, node/key scope and explicit detail key remain. New branches change only geometry, headings and label wrapping. Actual leaf IDs may change between presentation modes; existing confirmation reconciliation safely disarms those identities rather than promising armed continuity. No selection, saved data, pending write or manager ownership is transferred to Columns.

## Authored verification and remaining work

The existing journey retains actual complete-target intersections, arm/cancel, clipboard, dirty-draft and both enlarged-font/padding profiles. The added52/67/79-point and transition-neighbor cases inspect native painted Shape::Text data for original fixture keys, a long unbroken key and Unicode key. Non-elision, complete galley characters, placed-row clips and multiline/non-single-character checks address the observed failure more directly than semantic names alone. The numeric draft retains exact-150.250 across stacked/compact/wide transitions; cancellation must leave stored data and actions unchanged. No new registration or alternate test model is added.

These are meaningful authored assertions, not executed evidence. Galley character equality alone also does not establish font coverage or visual glyph legibility. Root must compile the matching committed source, execute the planned checks and inspect actual corrected images at the original settings plus the authored52-point boundary. Live52 was not established by GUI57. Full-target, normal/wide, compound-value, confirmation, pending/loading and actual Copy/Edit/Delete acceptance remain execution gates. No FULL, peer, native provider, CI or release credit follows from this report.

No implementation parser, tests, formatter, compiler/build, app/native/process/API/DB/network action or Git mutation occurred. Verification comprised source/artifact/primary local egui reads, byte/hash comparisons and viewing the two existing before PNGs.
