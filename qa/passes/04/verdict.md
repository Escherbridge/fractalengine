---
type: QA Pass Verdict
pass: "04"
date: 2026-11-01
stage: 214045a-20261101160632442
run_dirs: [20261101-160706-189]
scenarios: [S09, S10, S06]
focused_smoke: true
coverage_complete: false
---

# Pass 04: focused layout and Results validation

Trend: focused S6 smoke; full-catalog gold-task and coverage denominators were not attempted. This pass cannot
establish convergence.

Verdict: the measured S6 layout, Results, and input-routing regressions are fixed in the exercised flows. This is a focused smoke verdict; the campaign's complete panel catalog, accessibility coverage, final integrated tests and final release checks remain pending.

## Provenance and execution

The unchanged before matrix is recorded in `baseline.md` and machine-local `shots/before/`. The final live stage was `.qa/stage/214045a-20261101160632442`, source commit `214045aab523259c56c701c8fe9741117e38a7d0`, executable SHA256 `23401FDB0682FCC34A0CC8F73A08C34F4F9BE2309081AF7C397D5DE03B39C2F9`, source fingerprint `94808225AD87143ECDD9D3508ADDC1FF1063B373778AED488699AD82E913BCDB`. GUI and installer were built serially. The stage manifest records source_dirty=true; the launcher verified the exact fingerprint. Later strict endpoint source fix `25310f1` was deliberately not rebuilt during this handoff; its runtime check is queued below.

Final owned run `.qa/run/20261101-160706-189` used explicit port 8876 and the isolated basic fixture. App PID 45228, creation ticks 639291712299457350. The run-local configuration/identity boundaries were preserved and no real Settings store was opened. No tests or clippy ran: the user requires one integrated campaign-end sweep. Existing journeys were extended, with no new test functions.

## Measured geometry and interactions

All valid evidence PNGs in before, after, after-reviewed, extended, interaction, query, and final were opened and inspected. Failed initial Data locators were quarantined in `shots/extended-unopened/` and excluded from successful evidence; actual Data windows were subsequently located by their full accessible button name and recaptured.

| Window at scale 1 | Baseline Maps viewport | Final Maps viewport | Final share |
|---|---:|---:|---:|
| 1280x720 | 81.53px / 6.4% | 660px | 51.5625% |
| 1920x1080 | 721.53px / 37.6% | 1300px | 67.7083% |
| 1024x640 | 45.47px / 4.4% | 460.8125px | 45.0012% |

The baseline selected Inspector also had controls below the visible body because Deselect's expanding layout consumed its height. The shared bounded section layout and compact trailing row now place the selected form in view. Reset confirmation and Cancel were reachable in the reserved footer; Reset was never executed. Breadcrumbs use real ellipsis with full hover text. Query editing has its own bounded scrolling area.

`shots/after-reviewed/` holds the main Inspector, Maps, Options, after-Maps and Settings matrix. `shots/extended/` adds Inspector API/Query, Maps Available/Downloads, Data Query/Annotations/Layers, Groups, Terrain Tools and Volume Report at all three sizes. The same basic fixture, scale 1 and initial section scroll positions were used for the before/after main matrix. Extra extension surfaces lack a complete before comparison.

`shots/interaction/summary.json` records Inspector scrolling preserving camera distance 5, genuine viewport scrolling changing it 5 to 6, and scrolling over a floating Data window preserving 6. `shots/final/summary.json` records sidebar and Results divider drags preserving selected Cube1, genuine empty viewport click clearing selection, genuine Cube1 click selecting it again, and a Data overlay click preserving selection. Scene and UI share one pointer-region policy; visual viewport geometry remains separate from resize-band interaction insets. The final Rotate response measured24x25 physical pixels at1024x640.

At640x480 with UI scale 2, the effective compact layout preserves the requested sidebar intent while choosing feasible visibility. The actual final visual viewport is 360px wide (56.25%) and 212px high with Results open. This proves the finite width budget, not universal compact usability: the Owner chip still wraps vertically and the compact Query tab row needs the later all-panel polish/accessibility pass.

## Results behavior

One composed `query_results` view/run model owns request, rows, error, sort and UI row state; SQL input stays separate and NodeManager owns domain selection. The dock renders visible rows through native virtualization, supports column sorting/resizing and row/cell copy, and follows existing correlation routing. Submitted scope is provenance only; RawQuery authorization remains W5 work.

Live query evidence in `shots/query/` covers a 3-node result, current-petal row select/frame, projection rows, ascending/descending ordering, mixed JSON types, empty results, inline parse errors and verse-change invalidation. The Copy row/Copy cell UI paths were exercised with synthetic clipboard writes suppressed; OS clipboard contents were not inspected. A column grip drag was exercised, but the capture scrolled the header out of view, so exact width-change verification remains pending. Header/body allocation alignment and a transitive explicit JSON type ordering were corrected in source. Unmatched uncorrelated replies cannot populate the dock; stale row selection and copy access are reset/bounds-safe.

`shots/final/match-beacons.png` shows visible cyan query-match beacons in the active scene. `virtualized-200.png` and state show 200 returned rows in 15ms with only 3 visible numeric row buttons emitted. Row selection/frame, sort, empty/error behavior and navigation invalidation use the same run model; there is no duplicate domain selection.

## Checker interpretation and remaining coverage

The final reviewed checker reports are regenerated from the captured tree/state facts in `shots/final/*-layout-reviewed.json`; original reports are retained. All ten final captures have zero measured violations, with 96-163 explicitly unknown controls and 14-48 unknown text widgets. `coverage_complete` remains false. The checker preserves full response rect versus actual hit rect; a partially clipped table cell is an explicit scroll-path coverage gap, not a healthy small target. Nested scrolling only explains overflow when the actual captured ScrollArea supports that axis and has overflowing content. Unknown clip/layer/text facts cannot certify a healthy layout.

Outstanding coverage includes Data Paths/Export, the remaining dialogs, Options for every tool, complete same-pass response/name/text telemetry, role/status chip wrapping, compact Query tabs and the final all-feature matrix after S2. Header width-change proof and a real idle/active frame-cost comparison are pending; no CPU sample is represented as frame timing. These limits remain visible for subsequent phases.

## Exact cleanup

Earlier owned runs 47776 (baseline), 25796 and 41724 exited through recorded-identity cleanup. The final run exposed a concrete harness defect: waiting on one WebView child before attempting root termination aborted cleanup at the first timeout. `e3a67e1` retains verified handles, attempts every termination despite individual failures, waits under one 15-second total deadline, disposes handles in finally and reports aggregate failures.

The revised stop attempt still reported deadline PIDs 47964 and 25368. A subsequent exact-PID inspection confirmed both absent and root 45228 absent. All recorded launch descendant identities are also absent. Launch descendant capture cutoff was 2026-11-01T23:07:30.4432095Z; recorded PID/start ticks were 13484/639291712301041078,29988/639291712418389002,42920/639291712419641062,52976/639291712422792477,41436/639291712422814332,40160/639291712423898048,70908/639291712424772834. PID 41436 now belongs to Git Bash with different start ticks 639291721202688527 and was left untouched. Deadline PIDs were discovered during cleanup rather than the launch snapshot, so their creation identities were not persisted in the run manifest; eventual absence is verified, not a retroactive claim of clean immediate stop. The cleanup command returned failure honestly; independent review of the committed helper remains pending. Diagnostic log: `.omc/research/s6-final-cleanup.log`.

## Next staged endpoint check

Strict containment in `25310f1` closes inclusive overlap exactly at divider grab-radius endpoints. The existing resize-band journey now includes exact min/max endpoints at pixels-per-point 2. Per authorized campaign sequencing, this source change is not part of the stage214 live proof.

In the next S7 stage, select a fixture node, read the actual panel/dock divider and configured egui grab radius, and press/drag at left-divider+radius, right-divider-radius and Results-top-divider-radius. Convert egui points to physical screenshot pixels exactly once using current pixels-per-point 2. Check selected node unchanged at each exact interaction-rect edge, then click an interior visible node and empty interior viewport to confirm genuine pick/deselect still work. Avoid timing sleeps as a substitute for authoritative state. No additional S6 app/build remains owned at handoff.
