---
type: Track Retro
title: "Stamped Asset Nodes (T2) Retro — curve-following, addressable stamps"
tags: [stamped_asset_nodes_20260725, retro, spatial-builder, stamps, instancing, wave-1]
timestamp: 2026-10-11T00:00:00Z
resource: ./spec.md
---

# Retro: Stamped Asset Nodes (T2)

## What shipped

- **c6fdf0d / aeba832** (T1 spine, prerequisite): `PromoteInstance` lazy promotion
  (FR-5), tombstone/cascade/reflow, and `CallerAuth::Local`.
- **56ccc3b** (Wave 1 leaf):
  - an arc-length curve sampler for both spacing modes, with true-tangent alignment
    (`fe-terrain/src/mesh/curve.rs`);
  - sparse scale/rotation overrides and the 1-D slide;
  - the `StampRenderIndex` pick index and instancing (`fe-renderer/src/instancing.rs`).
- **623145f** (integration):
  - curve-follow stamps in the materializer (`verse_manager/path_asset_materialize.rs`);
  - the select → `PromoteInstance` → `NodePromoted` round-trip;
  - the override round-trip;
  - the lifecycle crossbeam→Bevy pump.
- **631f389**: restored the tool panels lost in the integration regression.
- **40590e9** (STAMP-GIMBAL-TRANSLATE-LOCK / T5-LOCK): a stamp never translates (D-A5).
  - `resolve_gimbal` gives a stamp no Move drag.
  - The DB rejects a stamp translate before the op-log commit.
- **This batch (2026-10-11 spatial-builder-residue, commit pending)**: the N-9 row-count
  test, `materialized_stamps_add_no_rows_and_one_promotion_adds_exactly_one`
  (`fe-database/tests/node_lifecycle_test.rs`).
  - A path stamped with ~50 instances adds zero `node` rows.
  - One promotion adds exactly one live row (`node_kind = 'stamp'`), and a re-promotion
    adds none.

## Lessons

- **Make "costs nothing until touched" a counted assertion.** N-9 was the core
  scalability promise (10k+ stamps), yet for 2.5 months only the promotion GATE was
  tested, not the row count. The promise lives at the store, so the test belongs there
  too, driven through `DbCommand` against a real on-disk DB.
- **Two enforcement layers for one invariant.** The D-A5 position lock needed both the UI
  gimbal table and the DB handler (40590e9). The UI-only lock had looked done until the
  consolidation audit found `SelectionKind::Node` standing in for stamps at the gimbal
  hit.
- **Leaf vs integration.** As with T3/T4, the leaf could prove the math. Curve-follow only
  became visible once the materializer was wired in the serial pass (623145f).

## Open residuals

- **USER-GATED:** in-app verify that stamps follow curves, single-select promotes one,
  and Move does not drag a stamp.
- Recorded follow-ups, not in scope: a custom instanced render pipeline and a criterion
  bench at 10k+ instances.
