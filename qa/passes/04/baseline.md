# S6 measured baseline

Stage `eb0f263-20261101152419374`, source commit `eb0f263`, dirty instrumentation only.
Owned run `20261101-152439-646`, port8876, basic fixture, ui-scale1, Cube1 selected, initial scroll offset0.
Exact recorded process tree stopped at 2026-11-01 15:27:33 local. All fifteen PNGs read.
Machine-local tree/state/check JSON and PNGs: `shots/before/`. Settings was re-captured with role=button after
the initial locator also matched a hint label.

| Size | Surface | Viewport px | Share | Violations | Explicit gaps |
|---|---|---:|---:|---:|---:|
| 1280x720 | inspector | 740.00 | 57.8% | 0 | 86 |
| 1280x720 | maps | 81.53 | 6.4% | 1 | 89 |
| 1280x720 | options | 81.53 | 6.4% | 1 | 89 |
| 1280x720 | after-maps | 81.53 | 6.4% | 1 | 86 |
| 1920x1080 | inspector | 721.53 | 37.6% | 1 | 86 |
| 1920x1080 | maps | 721.53 | 37.6% | 1 | 89 |
| 1920x1080 | options | 721.53 | 37.6% | 1 | 89 |
| 1920x1080 | after-maps | 721.53 | 37.6% | 1 | 86 |
| 1024x640 | inspector | 45.47 | 4.4% | 1 | 86 |
| 1024x640 | maps | 45.47 | 4.4% | 2 | 89 |
| 1024x640 | options | 45.47 | 4.4% | 1 | 89 |
| 1024x640 | after-maps | 45.47 | 4.4% | 1 | 86 |

Coverage: selected Inspector properties, Maps installed, Options Select and Inspector after Maps at all sizes;
Settings rendering/network/timeseries captured separately. Query/API controls are below the broken baseline fold;
Data and other dialogs/tabs remain unmeasured baseline coverage. Scroll-reachable content is intentionally not
called clipped. Visual bounds alone cannot establish minimum hit targets. QA-0018 is visually established by
Deselect consuming the body even though its offscreen controls can scroll. No tests/clippy executed.
