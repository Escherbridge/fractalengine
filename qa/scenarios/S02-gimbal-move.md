---
type: Scenario
id: S02
gold: G2
title: Select a GLB node and move it with the gimbal
fixture: basic
verified_in_pass: "01"
tags: [gold, selection, gimbal, transform]
---

# S02 — "Pick the cube and slide it along X with the arrows"

**Success signal (gold G2):** the picked cube visibly moves to the right
along the red X arrow, and the move persists (REST read-back of the node's
position shows x changed). Time limit: 2 minutes of driving.

## Steps

1. Enter Petal A of QA Basic (cards, never the sidebar petal row — see S03 step 7).
   ```
   F click 43 290 --wait 10            # sidebar "QA Basic" (prints view_changed_verse)
   F click 340 140 --wait 10           # "QA Fractal" card
   F click 330 160 --wait 60           # "Petal A" card
   F wait-settle
   F shot --label s02-petal-a
   ```
   **Checkpoint 1:** three red cubes in the viewport (Cube 2 right, Cube 3
   left of the centre one); title crumb reads "QA Basic / QA Fractal / Petal A";
   hint line "Click an object to select it" at the bottom.
2. Select the centre cube (Cube 1, world origin).
   ```
   F click 640 370 --wait 10
   F key g --wait 5 --capture --label s02-move-tool
   ```
   **Checkpoint 2:** Move is highlighted in the toolbar, a "Deselect" button
   appears, the gimbal (red X, green Y, blue Z arrows) sits on the centre
   cube, and the right panel shows "Gimbal active — drag an axis to move".
3. Drag the red X arrow head to the right.
   ```
   F drag 750 408 850 450 --wait 10 --capture --label s02-dragged
   ```
   **Checkpoint 3:** the cube and its gimbal moved right along X (it now
   overlaps Cube 2 at x≈2).
4. Evidence read-back (not a UX step):
   ```
   F api GET /api/v1/nodes/{node_ids[0]}
   ```
   `data.data.position.coordinates[0]` must be > 0 (it was 0 at launch).

## Expected visuals to judge every checkpoint against

- The selected node is distinguishable from the unselected ones (outline or
  tint), not only by the gimbal. *(pass 01: no selection outline — QA-0006.)*
- The Inspector's Position X reflects the drag without a manual refresh.
  A previous probe's blank Inspector claim was not confirmed: Maps was
  covering the viewport in that capture. Record only visible evidence.
- If Maps is open, record how much viewport remains (QA-0015) and close
  it before dragging. The prior probe does not establish why Maps opened.
- Nothing in the status bar contradicts the scene ("0 models" with three
  cubes on screen is a finding — QA-0004).
