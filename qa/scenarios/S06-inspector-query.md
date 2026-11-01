---
type: Scenario
id: S06
gold: G6
title: Run an Inspector Query-tab template
fixture: basic
verified_in_pass: "01 (probe)"
tags: [gold, inspector, query, surrealql]
---

# S06 — "Run a query template from the Inspector"

**Success signal (gold G6):** with Petal A open, the "All nodes in this
petal" template fills the editor with a petal-scoped SELECT, Run Query
returns a result listing the three fixture nodes ("Cube 1/2/3").

## Steps

1. Enter Petal A (S03 steps 1-3).
2. Make sure the right panel is the Inspector (first icon of its tab strip),
   then open its Query tab.
   ```
   F click 981 82 --wait 5
   F click 1102 134 --wait 10 --capture --label s06-query-tab
   ```
   **Checkpoint 1:** "SurrealQL Query", "Templates scoped to: <petal id>",
   an "All nodes in this petal" template chip, a placeholder listing the
   tables, Run Query disabled while empty.
3. Pick the template and run it.
   ```
   F click 1026 219 --wait 5
   F click 1015 335 --wait 60 --capture --label s06-result
   ```
   **Checkpoint 2:** editor holds `SELECT * FROM node WHERE petal_id = '<petal>'`;
   Result shows JSON rows whose `display_name`s are Cube 1, Cube 2, Cube 3.
   Also judge: is a raw JSON dump the right result view for a non-developer
   (DW-03 raw coordinates)? Record as S4 copy/UX if not.
4. Keyboard path: click into the editor, `F key ctrl+Enter --wait 60
   --capture --label s06-ctrl-enter` — "Ctrl+Enter to submit" must work.

## Expected visuals

- Pressing a tool shortcut (G/R/X) flips the right panel to "Options" and
  leaves it there (pass 01 probe) — the Inspector is one click away; record
  if that loses the user's place.
