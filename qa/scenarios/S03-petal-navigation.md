---
type: ScenarioSupplement
id: S03
legacy_gold: G3
covered_by: [J01]
title: Navigate between petals using cards and the sidebar
fixture: basic
verified_in_pass: "01 historical; revised fixture journey pending"
tags: [gold, navigation, sidebar, hierarchy]
---

# Go from Petal A to Petal B and back

Success requires navigation through both cards and the sidebar, complete
ancestry at each destination, scene replacement and the app staying alive.

The revised fresh basic fixture has Petal A with four rows (three asset
cubes plus one asset-free QA oracle origin marker) and Petal B with one
asset-free origin marker and a bound offline map. B is not an empty grid.
Terrain mode has the same row counts but binds both petals. Later imports
or earthwork regions can add rows; record actual current IDs/counts before
this journey and compare against that baseline instead of assuming pristine
counts. Asset-free marker rows do not imply a visible cube.

1. Open QA Basic from its named verse row. Inspect the verse-level view:
   one QA Fractal card and two petals; navigation must have verse ancestry
   with no active fractal/petal. Locate the current card controls through
   the UI tree and screenshot instead of replaying historical coordinates.
2. Open the QA Fractal card. Require the matching verse/fractal crumb and
   Petal A / Petal B cards. On a pristine fixture their node counts are
   four and one. If the current baseline differs, the cards must show
   those actual counts, with each known marker/value node in its own petal.
3. Enter Petal A from its card. Inspect the three asset cubes, asset-free
   marker row and complete QA Basic / QA Fractal / Petal A navigation.
   Basic A has no bound terrain; terrain-mode A does. Record the current
   cube transforms, especially a Cube 1 moved by S02.
4. Press Escape and inspect whether the petal cards return. If the
   advertised back gesture fails, retain the evidence for QA-0008 and use
   the current visible Back control to continue. Enter Petal B by its card.
   Require complete Petal B ancestry, no Petal A cube entities, its marker
   row, and the bound map's rendered surface once resident. A loading map
   is not an empty-grid success. Inspect `F state --path selection` for
   stale Cube 1 selection or Inspector state after navigation.
5. Expand the verse/fractal tree if needed and click the named Petal A
   label. Require its cubes and stored transforms to return, correct full
   ancestry and a live app. Repeat named navigation to B and back. Confirm
   each scene/selection state against the baseline rather than footer
   model counts alone.
6. Back out to the verse-level fractal picker while leaving the tree
   expanded. Verify no active fractal/petal, then click Petal B directly
   from the sidebar. This is the former QA-0001 crash path. Require complete
   verse/fractal/petal ancestry, the expected B marker/map scene and
   continued API/input responsiveness. Return directly to A the same way.

Capture card counts, both scenes, the verse-level direct-sidebar path and
state readbacks. Record row-collapse behavior, overlapping crumbs, missing
terrain residency and stale selection separately. The historical abort
has a repaired regression path; do not label this step as intentionally
ending the run.
