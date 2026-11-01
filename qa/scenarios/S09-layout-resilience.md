---
type: Scenario
id: S09
gold: layout-resilience
title: Layout and Results dock at three physical window sizes
fixture: basic
tags: [layout, query, exploratory]
---

Use an explicitly owned `--run` and port. Capture selected Cube1 with ui-scale1 and reset each section's
scroll offset to zero before both baseline and after probes. For each 1280x720, 1920x1080, 1024x640:

```powershell
python tools/qa/fe.py --run $run window size 1280 720
python tools/qa/fe.py --run $run layout-check
python tools/qa/fe.py --run $run shot --label layout-inspector
python tools/qa/fe.py --run $run click --name Maps --role button
python tools/qa/fe.py --run $run layout-check
python tools/qa/fe.py --run $run click --name Close --role button
python tools/qa/fe.py --run $run layout-check
```

Repeat Inspector Properties/API Access/Query; Maps Installed/Available/Downloads; Settings; Options for
all tools; Groups; Terrain/Report; Data Query/Annotations/Layers and its import/export dialogs. Record
actual active section/window and scan every PNG. Geometry unknowns are pending coverage, never clean.
The Reset footer must remain visible in its initial and confirm states (cancel the confirm), and full
paths/names remain reachable by hover without consuming the viewport.

Run the petal node SQL template through the UI; verify row count/timing, ascending/descending numeric
and text sort, column resizing, row/cell copy with the QA clipboard sink still suppressing the OS write,
current-petal row selection and camera framing, match markers, empty/error output and navigation
invalidation. A foreign/stale request may not claim the dock; final integrated existing journeys cover
this deterministic routing case. Captured scope is provenance, not W5 SQL authorization.

Read camera distance, wheel over Inspector, wait-settle and read distance again. It must remain equal.
Wheel over the viewport must still zoom. Repeat with a floating Data window over the viewport and a
non-default UI scale. Record protected viewport share ?45% for docked panels at all three sizes.
