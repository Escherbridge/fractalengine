---
type: ScenarioSupplement
id: S02
legacy_gold: G2
covered_by: [J02]
title: Select a GLB node and move it with the gimbal
fixture: basic
verified_in_pass: "01 historical; revised fixture journey pending"
tags: [gold, selection, gimbal, transform]
---

# Pick a cube and slide it along X with the arrows

Success requires the selected cube to move visibly along the red X axis
and the committed transform to persist. Allow two minutes of driving.

Use a fresh basic fixture for the original arrangement: Cube 1 at (0,0,0),
Cube 2 at (2,0,0), Cube 3 at (-2,0,0). Petal A has no terrain assignment.
Its extra QA oracle origin row is asset-free; it does not replace Cube 1.
The terrain fixture intentionally has a different arrangement and is not
the baseline for this journey.

1. Expand QA Basic / QA Fractal in the sidebar using current controls as
   needed. Click the named Petal A row, then inspect the resulting frame
   and `F state --path navigation`. Require the complete verse/fractal/
   petal ancestry and three rendered fixture cubes. The repaired sidebar
   path is supported; the historical QA-0001 crash is a regression check,
   not a reason to avoid it.
2. Select Cube 1 by its named sidebar label, expanding Petal A if needed:

   ```powershell
   F click --name 'Cube 1' --role label
   F state --path selection
   F api GET /api/v1/nodes/{node_ids[0]}
   F key g --capture --label s02-move-tool
   ```

   Record the actual selected node ID and before transform. The selected
   ID must equal fixture node_ids[0]. In a pristine basic run X is 0; if a
   prior action moved it, retain the actual baseline rather than assuming
   zero. Inspect the Move tool and gimbal on that cube. A separate viewport
   click selection check can use the cube's current visible geometry.
3. Locate the red X arrow head in the latest screenshot. Drag it toward
   positive X using those grounded coordinates and capture the result.
   Require the same selected node, visible cube/gimbal movement and an
   increased X relative to the recorded before value. Do not reuse a fixed
   pixel drag or require overlap with Cube 2: window size and camera pose
   affect projected displacement.
4. Read the selected node and authoritative selection again. Raw query
   storage uses GeoJSON `position.coordinates=[x,z]` and separate
   `elevation=y`; it is not a three-element position array. Require the
   committed X to increase from its actual before value and agree with
   displayed/authoritative selection after commit. Y and Z should stay
   within normal drag tolerance for an X-axis move. Re-enter the petal and
   verify the committed position persists.

At every checkpoint judge selection distinction, Inspector refresh,
visible axis feedback, clipping and footer accuracy. If Maps obscures the
viewport, record its retained width and close it before dragging. Preserve
the actual before/after captures and raw transform values; a changed query
alone does not prove a successful visible gimbal interaction.
