---
type: Scenario
id: S04
gold: G4
title: Set a petal map and see terrain
fixture: basic + an installed TerrainTileset hexon
verified_in_pass: "none (blocked: no offline TerrainTileset sample, QA-0005)"
tags: [gold, terrain, maps, petal-map]
---

# S04 — "Give this petal a map and see the terrain"

**Precondition:** `F info` → `fixture.terrain_bound` or at least one map in
the Maps "Installed" tab. `launch.ps1` installs any `TerrainTileset` hexon
from `sample-hexons/dist/` into the run's `FE_HEXON_DIR`; none ships today
(the three samples are Terrain/Scene/GpxCollection), so this scenario is
**BLOCKED** and recorded as such until one does. Do NOT click "Install from
file…": it opens a native `rfd` dialog synthetic input cannot drive and
that blocks the frame loop (fractalengine `src/AGENTS.md`
§agent-input-surface) — the next input call then times out.

**Success signal (gold G4):** after "Set petal map", the row reads "Active
map ✓" AND terrain tiles render in the viewport within ~10 s, with no
silent failure (M-03/X-01).

## Steps

1. Enter Petal B (S03 steps 1-4, cards path).
2. Open Maps: `F click 856 20 --wait 10 --capture --label s04-maps`.
   **Checkpoint 1:** Maps panel with Installed/Available/Downloads tabs,
   the run's hexon dir path, at least one installed map row with its "Map"
   column ("Set petal map"), and header/body columns aligned (M-01).
3. Click that row's "Set petal map" (locate it), then shoot now and 2 s later:
   ```
   F click <Set petal map> --wait 10 --capture --label s04-set
   F wait-settle --timeout 15
   F shot --label s04-terrain
   F shot --no-ui --label s04-scene-only
   ```
   **Checkpoint 2:** "Active map ✓"; terrain visible in the window shot and
   in the scene-only render; `F camera` → `projection` is non-null.
4. Close Maps (`F click 1250 89`). **Checkpoint 3:** the viewport returns
   to its full width. *(pass 01 probe: the right panel stays at the Maps
   width and squeezes the viewport to ~110 px — QA-0003, S2.)*
