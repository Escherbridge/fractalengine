---
type: Scenario Catalog
title: FractalEngine agent-QA scenario catalog
created: 2026-11-01
tags: [qa, scenarios, gold-tasks, agent-vision]
---

# Scenario catalog

Each `S<NN>-*.md` is one scenario the `fe-qa-pass` skill
(`.claude/skills/fe-qa-pass/SKILL.md`) drives through `tools/qa/fe.py`.
Every step is an `fe.py` command line; every **checkpoint** is a screenshot
plus what it must show. A scenario that carries a `gold:` id is a **gold
task**: a job a user must be able to do, stated in their words, with a
success signal judged from the screenshot (and, where noted, one REST
read-back that is evidence only, never a UX step).

## Conventions

- `F` means `python tools/qa/fe.py` run from the repo root. Under Git Bash
  `export MSYS_NO_PATHCONV=1` first, or `/api/...` arguments get rewritten.
- Coordinates are physical pixels for the default **1280x720** window the QA
  launcher opens. They are a starting guess, not truth: before every click,
  find the named control in the latest screenshot and use its centre. A
  control that is not where the scenario says is itself a finding (S4)
  only if the layout really moved.
- `{verse_id}`, `{petal_a_id}`, `{node_ids[0]}` … come from `F info`
  (`fixture` block, written by `tools/qa/launch.ps1 -Fixture basic`).
- Every input step uses `--wait 10` unless stated; add `--capture --label <name>`
  to get the post-action PNG in the same call (it spends the 1/s capture
  budget; fe.py retries 429 for you).
- A click that opens another verse prints `view_changed_verse` (exit 0):
  the host denies the rest of that batch under the old verse's token by
  design. Re-shoot to see the result.
- Exit 3 mid-scenario = the app died. That is an S1 finding: copy the panic
  from `<run>/stderr.log`, stop, relaunch, continue with the next scenario.

## Index

| id | gold | user words | fixture |
|---|---|---|---|
| S01 | G1 | "Make a new world, a fractal in it and a petal to build in" | none needed |
| S02 | G2 | "Pick the cube and slide it along X with the arrows" | basic |
| S03 | G3 | "Go from Petal A to Petal B and back" | basic |
| S04 | G4 | "Give this petal a map and see the terrain" | basic + TerrainTileset hexon |
| S05 | G5 | "Sculpt a cut, see the cut/fill heatmap and read the volume" | S04's petal |
| S06 | G6 | "Run a query template from the Inspector" | basic |
| S07 | G7 | "Move, maximize and close the window" | basic |
| S08 | - | polish visual checklist (18 items, `.omc/research/polish-ux-20261031.md`) | basic |

## Rotation (fe-qa-pass §rotation)

Every pass runs the **gold set** (S01-S07, success path only) plus a rotating
slice of exploratory checks: S08 items `((NN-1)*6 % 18)+1 …` six at a time,
so items 1-6, 7-12, 13-18 come round every three passes. Bootstrap pass 01
ran S02 + S03 only, to prove the loop.

- S10: [Layout resilience and Results dock](S10-layout-resilience.md), basic fixture; three-size measured geometry and explicit remaining coverage.
