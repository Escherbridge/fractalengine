---
type: Scenario
id: S10
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

## Nested property-table observation regression

Run J09's retained `evidence-boundaries.py` packet once in the integrated sweep.
Its ordinary horizontal table in a vertical Inspector must classify measured
overflow as scroll content. A disabled-Y viewport height0, enabled-X width0 or
nonfinite observation must instead report UNKNOWN and never claim inaccessible
or clean coverage. Fully measured genuine no-scroll clipping must still fail;
a10px visible hit must still fail the20px minimum; a disallowed modal-background
control must remain withheld. Read saved actual classifier output for each case.

For the live path, retain the current tree's ancestry, both scroll axes,
viewport/content/clip/offset geometry and PNG before/after the horizontal wheel.
In pass17 the property table recorded X enabled/Y disabled, viewport270x0,
content496x321, while the outer Inspector recorded316x492 with vertical offset739.25.
Edit/Copy/Delete moved from x1288 to1062 after horizontal offset0 to226 and actual
Edit/Cancel worked. This establishes reachable observed actions; the impossible
inner height remains a source-observation gap. Do not relabel that gap PASS,
discard the26 historical false clipping reports, or reduce target requirements.
