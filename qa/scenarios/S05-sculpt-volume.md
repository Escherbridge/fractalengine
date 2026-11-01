---
type: Scenario
id: S05
gold: G5
title: Sculpt a proposal, read the cut/fill heatmap and the Volume report
fixture: S04's terrain-bound petal
verified_in_pass: "none (blocked by S04's precondition, QA-0005)"
tags: [gold, terrain-tools, sculpt, proposal, earthwork, volume]
---

# S05 — "Sculpt a cut, see the cut/fill heatmap and read the volume"

**Precondition:** S04 passed in this run (terrain on screen).

**Success signal (gold G5):** a proposal exists, the sculpted area shows the
cut/fill heatmap (cut and fill in distinct colours over the terrain), and
the Volume report shows numeric cut / fill / net within 10 s — never a
"Computing…" that never resolves (R-01).

## Steps

1. Brush tool: `F key b --wait 5 --capture --label s05-brush`.
   **Checkpoint 1:** Brush highlighted; the hint line names the brush
   gestures and fits at 1280 (V-02); the options arm shows brush settings.
2. Open Terrain Tools (Options arm / tool panel — locate "Add proposal") and
   click "Add proposal". **Checkpoint 2:** a new proposal row, selected.
3. Sculpt a cut: drag across the terrain centre twice.
   ```
   F drag 520 420 680 420 --wait 20
   F drag 520 440 680 440 --wait 20 --capture --label s05-sculpted
   ```
   **Checkpoint 3:** a visible depression with the cut/fill heatmap over it.
4. Open the Volume report (locate "Volume" in the tool panel / Data window),
   choose the region if asked, then:
   ```
   F wait-settle --timeout 15 && F shot --label s05-volume
   ```
   **Checkpoint 4:** cut, fill and net volumes with units; no stale
   "Computing…" after 10 s. Evidence (probe only):
   `F api GET /api/v1/petals/{petal}/earthwork/summary`.
