---
type: Scenario
id: S01
gold: G1
title: Create a verse, a fractal and a petal from the UI
fixture: basic (any)
verified_in_pass: "01 (probe: verse step only)"
tags: [gold, hierarchy, dialogs, create]
---

# S01 — "Make a new world, a fractal in it and a petal to build in"

**Success signal (gold G1):** a new verse appears in the sidebar tree and
the verse picker, the user enters it, adds a fractal and a petal from the
"+ New …" cards, and lands in an empty petal scene — without reading docs.

## Steps

1. Open the Create Verse dialog from the "+" beside the verse picker.
   ```
   F click 204 55 --wait 10 --capture --label s01-dialog
   ```
   **Checkpoint 1:** a centred "Create Verse" dialog, Name field focused
   (caret visible), Create disabled while empty, Cancel + close-X present.
2. Name it and submit with the keyboard first, then the button.
   ```
   F type "QA Created Verse" --wait 3
   F key Enter --wait 20 --capture --label s01-enter
   ```
   **Checkpoint 2:** the dialog closes and the verse is created. *(pass 01
   probe: Enter does NOT submit — the dialog stays open; QA-0007, S3.)* If
   it is still open: `F click 522 404 --wait 30 --capture --label s01-create`.
   **Checkpoint 2b:** "QA Created Verse" is listed in the sidebar tree.
   Evidence: `F info` → `roles` gains a third `owner` verse (the dev-token
   writer re-mints on a verse-set change).
3. Enter the new verse (click its row in the tree), then the "+ New Fractal"
   card (locate it in a fresh shot, right of any existing cards).
   ```
   F click <verse row> --wait 10           # prints view_changed_verse
   F shot --label s01-verse
   F click <+ New Fractal> --wait 10 --capture --label s01-fractal-dialog
   F type "QA Created Fractal" --wait 3
   F click <Create> --wait 30 --capture --label s01-fractal-made
   ```
   **Checkpoint 3:** the fractal card appears ("0 petals" — note "1 petals"
   pluralization when it gains one, V-03).
4. Open the fractal card, then "+ New Petal", name it, Create, open it.
   **Checkpoint 4:** crumb "QA Created Verse / QA Created Fractal / <petal>",
   empty grid, hint line visible, status bar not contradicting the view.

## Expected visuals

- Every dialog: one primary action, Enter submits, Escape cancels.
- The new verse does not collapse or reorder the existing tree rows.
