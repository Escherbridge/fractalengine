---
type: Track Spec
title: Splat Marching-Squares Interpolation — Density Beyond Native Tile Zoom
tags: [feature, terrain, splat, rendering, lod, splat_marching_squares_20260712]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Specification: Splat Marching-Squares Interpolation

**Track ID:** `splat_marching_squares_20260712`
**Crates:** `fe-terrain` (splat only)

## Context

`splat_lod_zoom_20260712` (sibling track, in progress) fixes splat resolution
by requesting higher real tile zoom from the tileset as the camera gets
closer. That approach has a hard ceiling: once `splat_desired_zoom` would
exceed the tileset's actual `max_zoom`, there is no higher-resolution source
data to fetch — FR-5 of that track clamps to `max_zoom` and holds current
density rather than erroring.

This track is the density fallback for that ceiling: when the camera wants
more resolution than the tileset can natively provide, synthesize
additional splat density by interpolating/upscaling the existing highest-
zoom tile data, rather than just holding flat density indefinitely.

User explicitly deferred this in the original splat-resolution discussion
(prioritizing the zoom-pipeline fix first) but then asked for it to be
built in parallel as a separate worker rather than sequenced strictly
after — treat this as a genuinely parallel, independently-mergeable track,
not a blocked one. It composes with the sibling track at the boundary
described below; it does not require the sibling track's code to exist to
be developed (it operates on already-baked highest-zoom tile data,
independent of how that data was fetched).

## Refinement (user, 2026-07-12, with screenshot) — coverage/hole-filling

Observing the close-up splats, the real defect is **holes**: dark background
shows through the gaps between blurry splat blobs (plus visible tile-seam
grid lines). The goal is not "smaller blobs via higher zoom" (smaller blobs
still leave gaps) but **fill every hole until no untouched background
remains**, driven by scale + proximity:

- **Place a new splat in each hole**, sized by proximity to nearest existing
  splats — large enough to touch/overlap its neighbors and close the gap.
- **Logarithmic shrink:** as the local field densifies (neighbors closer),
  newly-added splats get progressively smaller — big holes filled with big
  splats, small remaining gaps with small splats. Scale is driven by
  **nearest-neighbor distance**, not a fixed average of endpoint scales.
- **Coverage-driven termination:** keep filling until every spot is touched
  (a new splat's coverage radius overlaps its neighbors / no gap remains) or
  the splat needed would fall below the degenerate floor. Not a fixed pass
  count.
- **Decouple from the max_zoom ceiling gate:** holes are visible at *normal*
  zoom too, so hole-filling runs whenever gaps exist — not only past the
  tileset's max_zoom. The zoom pipeline (sibling track) is complementary
  (brings in real higher-res data) but does not by itself close holes.
- Keep the seam guard and the anti-degenerate-cluster floor from below.

FR-2/FR-3/FR-5 below are re-aimed toward this coverage goal.

## Functional Requirements

- **FR-1 Ceiling detection:** a pure function
  `splat_needs_interpolation(requested_zoom: u8, actual_zoom: u8) -> bool`
  (or equivalent signal) that is true when `requested_zoom > actual_zoom`
  — i.e. the camera-driven desired zoom exceeds what was actually fetched
  (the tileset's `max_zoom` ceiling). This is the hook point where this
  track's logic engages; it must not fire when zoom is satisfied by real
  tile data (that path stays exactly as `splat_lod_zoom_20260712` leaves
  it — no behavior change when the ceiling isn't hit).
- **FR-2 Marching-squares density synthesis (logarithmic fill):** given a
  baked splat tile's existing point/density field at `actual_zoom`,
  synthesize additional splat points at sub-tile positions using a
  marching-squares-style interpolation over the underlying heightmap/
  imagery samples already present in the tile (no new network/tile
  fetches — this operates on data already resident from the highest zoom
  fetched). Fill is **logarithmic**: each successive pass roughly halves
  point spacing (recursive subdivision of the marching-squares cells) —
  coarse gaps filled first, progressively finer detail each pass. Pure
  function, bevy-free, unit-testable: input existing splat point set +
  density target, output an augmented point set.
- **FR-3 Deterministic, non-jittery output:** the interpolated points must
  not reintroduce the "dot-grid" look that round-3 jitter/overlap tuning
  (`fe-terrain/src/splat/synth.rs`) already fixed for the base density —
  reuse or extend the same jitter/overlap approach for synthesized points
  so upscaled and native points are visually indistinguishable in density
  pattern.
- **FR-4 Integration point with sibling track:** `reconcile_splat_chunks`
  (`fe-terrain/src/splat/render.rs:119-181`, being modified by the sibling
  track to own `Vec<sub-mesh>` per chunk) should call this track's
  synthesis function when `splat_needs_interpolation` is true for a given
  sub-tile, augmenting that sub-tile's baked mesh with synthesized points
  before building the final mesh. Expect a small integration seam here at
  merge time since both tracks touch `reconcile_splat_chunks` — keep this
  track's changes as an additive call inserted at one clear point, not a
  restructure of the function, to minimize conflict surface.
- **FR-5 Termination = overlap-or-seam (geometric), not a fixed pass
  count:** the logarithmic fill (FR-2) stops when EITHER (a) **overlap is
  reached** — the next synthesized point would fall within the jitter/
  overlap radius of an existing point, reusing `synth.rs`'s round-3
  overlap/spacing threshold as the "dense enough" stop signal (do not
  invent a new one); OR (b) the **tile-seam boundary** with the
  neighboring tile is hit — never synthesize past the tile's own footprint
  edge into the neighbor's territory, so adjacent tiles don't double-fill
  the shared boundary (which would cause a visible density seam / z-fight).
  A hard pass ceiling (~4-5) remains only as a degenerate-input backstop;
  the primary terminator is the overlap-or-seam condition. No retry storm.

## Out of scope

Photorealistic upscaling/ML-based super-resolution; per-splat individual
LOD; changes to the tile fetch/cache pipeline (this track only touches
already-resident tile data); mesh LOD (unrelated, owned by
`terrain_lod_hardening_20260711`).

## Verification

Unit tests for FR-1 (ceiling detection boundary conditions) and FR-2/FR-3
(density synthesis point count, no-jitter-regression sanity — e.g. no
degenerate/overlapping point clusters). In-app verification (once
composed with the sibling track): user zooms in past a tileset's native
max zoom on a splat-view petal and confirms density continues to increase
smoothly rather than flattening out abruptly at the ceiling.
