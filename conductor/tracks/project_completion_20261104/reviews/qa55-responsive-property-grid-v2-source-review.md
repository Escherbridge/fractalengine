---
type: Review
title: QA55 grid boundary correction source review
date: 2026-11-04
---

# QA55 responsive property grid v2 independent source review

Verdict: **approved for integrated compilation and subsequent verification, source scope only**. V2 resolves my GRID-B1 source blocker: the destructive slot no longer forces the full pair width into the supported 52-point body, and the canonical helper can stack the actual confirmation leaves while the parent allocates their natural height. I found no remaining concrete source blocker in this four-path slice. QA0074 remains open pending matching execution and live acceptance.

Only this report was written. No production source, packet, Git state, app, store or process was changed; no implementation parser, formatter, compiler, test, runtime, API, DB or network operation was run. Root/local panels guidance and `C:/Users/atooz/.gemini/UI-UX-guidance.md` were applied. This review does not supply a build/test pass or claim a visible 52-point live result.

## Frozen identity

Packet: `.omc/research/qa55-responsive-property-grid-v2`, recorded baseline `22039d5add074d4df0e18f5847286606d95407ac`.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| manifest.json | 3866 | `992493c02bb6d252e48b1ae9d1e2cdf61dbf7145efda005ff8a36f1c37453dcb` |
| source.patch | 27161 | `7dcaa80303e5982560f52e52e7aa63b7da28b6ac45f9a6fb456eaf1af9977e36` |
| v1-to-v2.patch | 11793 | `e9e56f9b29517bf409d7a6502fde9e4fd110a3159f19f8e5e24f59efc2b85bc7` |
| fe-ui/src/panels/inspector.rs | 283206 | `db1385664c69c858b18f88b604d21d956b2b4768523d21015e63f8fe297ba022` |
| fe-ui/src/panels/property_grid.rs | 7279 | `5300bf230ca711daf67c90dcc1455587365e7b69f3e203c70a7221c846806f01` |
| fe-ui/src/panels/widgets.rs | 25074 | `b47d226a429138b138f0e5ed6086734bccfd29e35e3547019b217144ea0b8d68` |
| fe-ui/src/panels/AGENTS.md | 71734 | `fb6ca2d661702d337d8e12ab3305b734925d62ceb7e304bd4e89882a822af53c` |

All four raw, pre-v2 and baseline snapshot files match their recorded sizes/hashes. All four actual working files match frozen raw bytes exactly. I read the full combined patch, the v1-to-v2 correction and affected production/journey context. The earlier v1 source blocker remains preserved separately; this report supersedes that blocker for these exact v2 bytes.

## GRID-B1 resolution and layout boundaries

`property_grid.rs:38-40` now gets the pair reservation from the same `widgets::confirmation_pair_width` used by the canonical helper. That function (`widgets.rs:222-235`) measures the actual small-label font and padding, includes the greater of the shared physical target and current interaction height, rounds individual widths upward, and adds the caller's gap. The ordinary row's 8-point gap and the helper's actual scope spacing therefore share one explicit measurement seam instead of two independent text assumptions.

`property_grid.rs:140-150` takes the new path only when a compact action's measured slot exceeds the actual table width. It allocates a body-width top-down child, sets that width explicitly, and lets the contents determine the resulting height. Fitting slots retain the existing exact rectangle and clip path. `widgets.rs:252-280` compares the real remaining width with the measured pair; below it, unarmed Delete and the armed leaves use width-bounded wrapped-label buttons, and the pair is vertical. At fitting widths, the previous small-text buttons and horizontal layout are retained. At the supported 52-point body, the destructive child is thus 52 points wide and the two leaves each receive that width, rather than an indivisible wider pair allocation.

This also fixes the former one-row-height clipping. Pinned egui 0.33.3 `ui.rs:1554-1579` routes `allocate_ui_with_layout` through a child scope, and `scope_dyn` at lines 2510-2516 advances the parent's cursor using the actual child `min_rect`. The wrapped action row consequently accounts for the taller stacked group; it is no longer painted into a fixed one-control-high cell. The ordinary fixed slots remain unchanged. Compact key sizing now preserves a value-control minimum where the actual body permits it; the wide measured columns are retained. No Inspector width floor, scene viewport allocation, or horizontal offset is raised/reset to conceal the defect.

The native horizontal scroll container and its identity remain; this is compatible with fitted content and the existing observation seam. Actual zero offset and whole clips must still be demonstrated during execution, especially when migrating an already-scrolled UI state. I found no source reason requiring renewed horizontal travel for the reviewed 52-point action/confirmation arrangement.

## Confirmation, data and callback preservation

This is one canonical state machine with two presentations. The explicit arm-owner ID, four-second deadline, `confirmation_state`, `Confirmation::new`, generation/lifetime checks, native registration, actual Confirm/Cancel ID reconciliation, establishment requirement, own/outside-click handling and removal/update logic remain. I compared the post-layout reconciliation/activation/disarm block and the unarmed creation block to pre-v2 source; both are exact. The helper does not introduce an alternate immediate-delete callback or accept an unestablished confirmation. `native_accessibility/confirmation.rs:38-76` still retires an arm whose leaf IDs or lifetime/semantic ownership no longer match; layout resizing does not bypass that gate. A resize may safely disarm and require arming again.

The fitting-width button builders preserve the old labels, small text, fill and horizontal construction. Below the pair width, the same helper also improves its other callers' presentation; no caller's authorization or action callback is changed. The source call inventory includes dialogs, schema rows, path controls and the reset surface, so their existing canonical confirmation journey should remain in the eventual integrated sweep.

Inspector still uses one shared row-action closure for both layouts. Edit's loading/editing gate and typed draft insertion, exact Copy serialization and toast, Delete's loading/pending gate and `(delete-property, node_id, key)` salt, and deferred delete action remain unchanged. The extra Edit/Copy `.wrap()` affects their bounded presentation only. Node/key scoping, detail expansion state, stored JSON, exact string whitespace, number/integer parsing, schema validation, save/cancel, pending/unknown-write handling and mutation enqueue paths are unchanged. No permissions, authority, DB/API schema or provenance fence is weakened.

Source inspection found no concrete Rust closure borrow or pinned egui API issue: the layout helpers accept the passed closures, the mutable row-action borrow ends before the subsequent draft lookup, and the confirmation controls are constructed once in the selected layout branch. This is not a substitute for compiler results.

## Authored verification and remaining gates

The existing Inspector journey now measures the actual body/frame padding and shared pair width, then exercises an actual 52-point table body, one point below/above the measured pair boundary, and 260/320/640-point frames (`inspector.rs:2979-2995`). It requires zero horizontal offset; whole action and confirmation rectangles intersected with their actual hit, child and table clips; minimum target size; unchanged content width/offset; stable Edit/Copy origins on arm/cancel; actual editor height; and retained exact dirty draft when crossing the responsive breakpoint. It drives real egui arm/cancel/edit pointer events, checks exact clipboard output, and retains the existing confirmed-delete path. These are meaningful checks of this failure mode rather than a control-count-only mirror. No test declaration was added: Inspector remains four `#[test]` declarations and widgets remains seven. All authored assertions are unexecuted here; model readback remains a simulation rather than DB persistence evidence.

Root may proceed with the planned integrated build and final sweep. Matching live acceptance must inspect the original 1280 default layout and the actual narrow/high-scale Inspector, activate Edit/Copy/Delete including the stacked Confirm/Cancel pair, verify resulting typed values/history and cancellation, retain dirty drafts through resizing, and cover loading/pending/disabled states. Screenshots and actual clipped leaf bounds are needed before closing QA0074. This source approval earns no peer, retained restart, native capture or FULL acceptance credit.
