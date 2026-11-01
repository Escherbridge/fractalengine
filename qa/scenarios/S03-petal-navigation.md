---
type: Scenario
id: S03
gold: G3
title: Navigate between petals (cards and sidebar)
fixture: basic
verified_in_pass: "01"
tags: [gold, navigation, sidebar, hierarchy]
---

# S03 — "Go from Petal A to Petal B and back"

**Success signal (gold G3):** the user can reach Petal B from Petal A and
return, by the cards AND by the sidebar tree, with the scene swapping each
time (cubes in A, empty grid in B) and the app staying alive.

## Steps

1. Open QA Basic from the sidebar.
   ```
   F click 43 290 --wait 10 && F shot --label s03-verse
   ```
   **Checkpoint 1:** header "QA Basic — Select a Fractal", one "QA Fractal"
   card ("2 petals"), verse picker reads "QA Basic".
2. Open the fractal from its card.
   ```
   F click 340 140 --wait 10 --capture --label s03-fractal
   ```
   **Checkpoint 2:** "QA Basic / QA Fractal — Select a Petal to enter",
   cards "Petal A · 3 nodes" and "Petal B · 0 nodes".
3. Enter Petal A from its card.
   ```
   F click 330 160 --wait 60 && F wait-settle && F shot --label s03-petal-a
   ```
   **Checkpoint 3:** three red cubes; crumb "QA Basic / QA Fractal / Petal A".
4. Back out with Escape, then enter Petal B from its card.
   ```
   F key Escape --wait 10 --capture --label s03-escape
   F click 522 160 --wait 30 --capture --label s03-petal-b
   ```
   **Checkpoint 4:** after Escape the petal cards are back; after the click
   the crumb reads "… / Petal B" and the viewport is an empty grid.
   *(pass 01: Escape does nothing in a petal although the hint line says
   "Esc = back out one step" — QA-0008. Fallback so the rest of the
   scenario still runs: `F click 246 52 --wait 10` ("◀ Back"), then the
   Petal B card.)*
5. Expand QA Basic in the sidebar and go back to Petal A from the tree.
   ```
   F click 9 290 --wait 10 --capture --label s03-tree-expanded
   ```
   Locate the "Petal A" row under QA Basic › QA Fractal in that capture, then
   `F click <x> <y> --wait 30 --capture --label s03-tree-petal-a`.
   **Checkpoint 5:** Petal A's cubes are back; the app is alive (exit 0).
6. Evidence: `F camera` → `active.petal_id == {petal_a_id}`.
7. **Sidebar petal row from a verse-level view** (the path a new user takes
   first). Back out to the fractal picker (`F click 246 52` "◀ Back", then
   `F click 252 52` "◀ Fractals"; `F camera` → `active` has no fractal_id),
   then click a petal row of QA Basic in the sidebar tree (locate it in the
   capture; pass 01 used `F click 74 353` for "Petal B").
   **Checkpoint 7:** the app enters the clicked Petal B. *(pass 01: the app aborts —
   `build_scope: petal_id requires fractal_id`, QA-0001, S1. Run this step
   LAST: it ends the run.)*

## Expected visuals

- The verse row collapsing when clicked (L-01 "sidebar collapse trap") is
  recorded, not failed: compare `s03-verse` with the launch shot.
- The crumb/ID line and the "◀ Back"/"◀ Fractals" button must not overlap
  the title (V-01).
