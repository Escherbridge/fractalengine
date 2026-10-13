---
type: Track Retro
title: "Sculpt & Earthwork Regions (T3) Retro — brush/shapes, region nodes, real-unit volume"
tags: [sculpt_earthwork_regions_20260725, retro, spatial-builder, sculpt, earthwork, wave-1]
timestamp: 2026-10-11T00:00:00Z
resource: ./spec.md
---

# Retro: Sculpt & Earthwork Regions (T3)

## What shipped

- **e96fa75** (Wave 1 leaf): the Bevy-free `fe-terrain/src/sculpt/mod.rs`.
  - Footprints: brush, circle, rect and polygon.
  - Raise/Lower/Level/Smooth ops over a read-only base surface (NFR-1).
  - The `EarthworkRegion` record.
  - Heightfield cut/fill volume in real units via the scale authority.
- **195f5cc** (integration):
  - Committed regions become addressable node rows (`node_kind = earthwork_region`, plus
    material and cut/fill).
  - The bake reports volumes back onto the node, with a changed-value DB gate.
- **118e666**: Brush promoted to a first-class tool (`node_manager/brush_interaction.rs`),
  with the tool panels restored.
- **e5688ca / e76bbb9**: T5 earthwork summary, plus D-6 units and preview.
- **This batch (2026-10-11 spatial-builder-residue, commit pending)**:
  - `sculpt_ops_are_bit_identical_on_repeat` covers every op × footprint (rect, circle,
    polygon) × `world_scale` (1.0, 0.01). Heights and cut/fill (`apply_sculpt`,
    `sculpt_cut_fill`, `EarthworkRegion::cut_fill`) are compared with `to_bits()`. This
    re-ticks Phase 2.
  - The UI producer for region delete: right-click a region → Delete →
    `SculptDeleteRegion` (`HitTarget::EarthworkRegion`, `pick_ground_region`). A handler
    test proves the region record is dropped while the palette proposals stay, and that
    the region's node is tombstoned with `CallerAuth::Local`.

## Lessons

- **A handler with no producer is not a feature.** `handle_delete_region` was correct
  and tested from 195f5cc, but no UI path ever produced `SculptDeleteRegion`. The menu's
  only ground tier read `ProposalEditState`, which deliberately filters out
  material-tagged regions (the finding #1 data-loss guard). Region delete was therefore
  unreachable. For every new `UiAction`, look for a producer as well as a test of its
  match arm.
- **"Pure" deserves a pinned test anyway.** The ops were RNG-free by construction, but
  the plan's determinism test is what keeps them that way. Any later move to a
  HashMap-ordered lattice or a parallel reduction now goes red.
- **Integration regression.** The 2026-07-26 in-app pass found terrain tools dead and the
  app aborting on an emath NaN, despite a green 2050-test sweep. Both were fixed in
  118e666 and the NaN guard work. Run an in-app smoke after every integration wave.

## Open residuals

- **USER-GATED:** in-app verify of brush/shape commit → overlay → volume report, and of
  right-click Delete on a region removing both the ghost and the region.
- Deferred by plan: layered strata (SCULPT-STRATA) and survey-grade volume.
- Region Copy API / Report Volume from the right-click menu stay disabled-with-hint (no
  `node_id` on the classified target). See the contextual_controls retro.
