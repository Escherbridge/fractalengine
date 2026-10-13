---
type: Track Retro
title: "Contextual Controls (T4) Retro — object-aware right-click menu"
tags: [contextual_controls_20260725, retro, spatial-builder, context-menu, wave-1]
timestamp: 2026-10-11T00:00:00Z
resource: ./spec.md
---

# Retro: Contextual Controls (T4)

## What shipped

- **25d908c** (Wave 1 leaf): `dialogs/context_menu.rs` — the pure `menu_for(HitTarget)`
  verb table (spec Q-1), verb labels/tooltips, the two-step in-menu delete confirm with a
  live descendant count, and node-scoped `verb_action` routing (Delete is the sync-safe
  tombstone cascade, never a raw drop).
- **e5688ca** (T5): the egress seam. Copy API / Report render disabled-with-hint until
  `gis::egress_strings` yields a string (FR-4, ui_ux §6).
- **195f5cc** (serial integration pass): `node_manager/context_pick.rs::classify_context_menu`
  fills `ContextTarget` with the same pick machinery as left-click (ray/AABB +
  `TrackPickShape`, stamp ground fallback via `StampRenderIndex`).
- **e76bbb9** (D16 classifier work): right-click resolves `PathVertex` / `PathHandle` /
  `PathSegment` / `TerrainProposal`, with the edited-ribbon tier matching left-click (F10).
- **This batch (2026-10-11 spatial-builder-residue, commit pending)**: committed earthwork
  regions are now right-clickable. `HitTarget::EarthworkRegion` comes from
  `pick_ground_region` over the active petal's terrain doc. Its Delete routes to
  `SculptDeleteRegion`, which drops the record and tombstones the region's node. Before
  this, region Delete could only reach `TerrainProposalDelete`, which by design never
  touches a region.

## Lessons

- **Leaf-wave vs integration-wave.** With strict file boundaries the leaf (25d908c) could
  build the menu table and verbs but not wire them. The wiring lived in shared central
  files (`panels/mod.rs`, `node_manager/mod.rs`), so classification only went live in the
  serial integration pass (195f5cc). Plan that serial wave explicitly. The tell is a leaf
  whose every open item names a central file.
- **A table test is not reachability.** `menu_for` was exhaustively tested from day one,
  but for weeks the classifier produced only `Node`/`Stamp`/`Empty`, and region Delete
  was never reachable. Whenever a new `HitTarget` variant is added, write a pick test for
  its producer as well as a menu test for its verbs.
- **In-app verify catches what tests cannot.** The 2026-07-26 in-app pass found an emath
  `smart_aim` NaN panic in `gardener_ui_system`, plus dead tool panels, after a fully green
  sweep (2050 tests). 118e666 restored the tools, and the NaN came from unbounded
  `DragValue` ranges (closed 118e666+WIP, see ui_semantics audit). Numeric UI needs
  finite-range guards, not only unit tests.

## Open residuals

- **USER-GATED:** in-app re-verify of the right-click menu on every target kind. This now
  includes right-clicking an earthwork region and using Delete, which should remove both
  the ghost and the region.
- On an earthwork region, Copy API / Report Volume stay disabled-with-hint because the
  classified target carries no `node_id`. The region→node binding lives in
  `EarthworkNodeMap`, which the classifier does not read. Wiring it in is a small
  follow-up if region egress from the menu is wanted.
