---
type: QA Investigation
title: Terrain report coverage and refresh gaps
status: runtime-reproduced-open
date: 2026-11-01
scope: S7 fixture prerequisites
---

# Report gaps retained for later hardening

Pass05 reproduced these failures on stage74a817e-20261101165518420.
Fresh one-region Raise2 and Lower1 operations persisted200fill and100cut,
but their initial UI reports cached zero. A Radius5→5.01→5 refresh displayed
the exact expected values. See `passes/05/verdict.md` and QA-0023.
Production terrain algorithms remain unchanged in that evidence checkpoint.

1. **Missing coverage can look like a measured zero.**
   `fe-terrain/src/sculpt/mod.rs::cut_fill_volume` adds volume only when
   both base and proposed samples exist. Missing pairs contribute nothing.
   `fe-terrain/src/volume_report.rs::selection_volume_report` computes area
   geometrically while its sample closures propagate missing base heights.
   `terrain_plugin.rs::answer_volume_report_requests` maps this report to
   a successful result without coverage counts or an unavailable state.
   Thus a positive geometric area with zero volumes cannot establish that
   the base map was sampled. Follow-up should expose coverage and refuse
   or clearly mark incomplete measurements. Runtime reproduction: request
   a known nonzero raise report before origin tiles are resident; capture
   the loading state, report and later resident comparison.
2. **Late tile arrival alone does not refresh a report.**
   `fe-ui/src/panels/volume_report_panel.rs::sync_volume_report_request`
   caches its selection key, footprint and petal, and reissues on a terrain
   configuration DB echo. Loaded heightfield coverage is not part of that
   key. A successful incomplete result can therefore
   remain after tiles arrive. Follow-up should include a coverage revision
   or explicit refresh action and an honest unavailable/loading state.
   Runtime reproduction: preserve the same footprint/config during tile
   arrival, compare the retained report, then explicitly change Radius
   away and back to 5 to trigger a new request.

Route both to unavailable-state/report-refresh hardening (W4-S3). The S7
scenario requires resident terrain plus nonzero expected fill 200 or cut
100 and raw tolerance 0.001. It records the 128² expected integration
lattice but does not fabricate actual sample counts when the result lacks
them. A report that needs the radius workaround must be recorded as such.
