---
type: Scenario
id: S04
gold: G4
title: Set a petal map and see offline terrain
fixture: basic or terrain
verified_in_pass: "pending S7 build and live validation"
tags: [gold, terrain, maps, petal-map, offline]
---

# Give this petal a map and see the terrain

Launch `tools/qa/launch.ps1 -Fixture basic` for a plain Petal A and an
offline terrain-bound Petal B. Use `-Fixture terrain` for two bound petals
when continuing into the independent raise/lower oracle in S05. The staged
generator creates a real TerrainTileset and installs it through verified
bundle assembly. Generation or binding failure fails launch; no absent-map
fallback is a valid fixture.

Define `F` as `python tools/qa/fe.py --run <this-run>`. Read `F info` for
the exact IDs and `<run>/terrain-fixture/descriptor.json` for the serialized
config, tile ring and oracle. This setup is evidence, not a gold UI step.

1. Enter Petal B through its named sidebar row. Capture and inspect the
   viewport, then open Maps using the named button. Installed must contain
   **QA flat sector and analytic tent hill**, id `qa-analytic-terrain`.
   Its map control must report Active map. Record column alignment and
   viewport width as layout evidence.
2. To exercise actual map selection, clear that active assignment using
   its row control, inspect the result, then click the same row's Set petal
   map control. Locate each control from the current UI tree and screenshot.
   The UI may select its normal hybrid policy; the installed source has no
   external tile URL. Do not substitute a REST PUT for these gold actions.
3. Close Maps with its named Close button. Capture now and after residency
   settles. Inspect both the window and a scene-only capture. Verify a real
   rendered terrain surface with a flat origin sector and the separate
   hill around local (80,80), not just the editor grid. A camera capture
   aimed at the descriptor hill can help inspect this landmark.
4. Probe `F camera`: projection must be present and world scale 1. Read
   `/api/v1/petals/{petal_b_id}/terrain` as evidence: enabled, TerrainRGB,
   installed id, min/max zoom 17 and scale 1 must match. Local bound tile
   coverage and a visible surface are prerequisites for S05; camera
   `wait-settle` alone does not certify asset or terrain completion.

Success requires the real UI map assignment, visible loaded terrain,
matching projection/units and no silent failure. If tiles never load,
record a blocker and do not count a blank grid or zero report as success.
The descriptor's nine offline 256px tiles cover a 3x3 ring; there is no
external dataset or GIS installation prerequisite. S09 covers queued
Install from file; synthetic Browse without an answer returns a typed
in-app error and must never open a blocking native picker.
