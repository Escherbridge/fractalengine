---
type: QA Pass Verdict
pass: "06"
date: 2026-11-01
stage: 9650d69-20261101175541248
source_fingerprint: D888A437F9594CF8E29EEF53205391EAE2113100F9141847897CDF6D5395F22B
run_dirs: [20261101-175614-744]
journeys: [J03, J04]
focused_smoke: true
full_coverage: false
---

pass06: gold0/11, new S1/S2 3, GUI coverage0/664, API coverage0/216, OS coverage0/9.

The focused terrain and typed-property behaviors below are verified by actual UI actions, inspected images and independent readback. Formal checkpoint records remain UNKNOWN: the client read starting log counters internally but did not emit them, and no separate counter snapshot was retained before cleanup. This pass does not establish full journey acceptance or convergence. No missing counters are reconstructed. The non-strict coverage command's exit0 reports incomplete coverage; it is not an acceptance result.

## Build and provenance

The combined GUI/helper stage records source `9650d69937fdbb210c09ec0e087420888f00ae37`, executable SHA256 `AA0146F39561257658697CCE5FD4BE12BD7CDE409BD078441966019F5D1EE2DC`, fingerprint `D888A437F9594CF8E29EEF53205391EAE2113100F9141847897CDF6D5395F22B`, source_dirty=true. GUI build passed in4m30s, helpers59.62s, serial stage346.9s, jobs2. An earlier invocation promoted normal Cargo stderr into a terminating PowerShell error before a Rust diagnostic; the documented separate PowerShell stage invocation completed. No test or clippy sweep ran. Existing journeys were extended; their execution remains pending the final integrated sweep.

Fresh terrain run20261101-175614-744, rootPID13208, port8881, completed strict two-petal fixture setup with three values10/20/30 and sum60. No real user storage or external dataset was used. `provenance.json` preserves stage metadata and cleanup. Client-only visibility commit `b76625c` was applied during this run; subsequent log-output fix `853ffd5` was applied after cleanup. These do not change the compiled Rust fingerprint. The canonical catalog still requires reconciliation before a final strict acceptance pass.

## W4 report behavior

Fresh Petal A received one actual UI Rectangle Apply with Radius5 and Raise positiveDelta2. Its first inspected report automatically displayed Area100.00/Cut0/Fill200/Net200 and measured16384/16384 sample pairs (`shots/175814-962-w4-raise-automatic-report.png`). Fresh B received one independent Rectangle Lower positiveDelta1; its report automatically displayed Area100/Cut100/Fill0/Net-100, also16384/16384 (`shots/180027-493-w4-lower-automatic-report.png`). No radius wiggle or duplicate Apply was used. `persisted-oracles.json` checks exactly one region per petal and finite raw values within0.001.

The Lower Apply capture_after request returnedHTTP504 after5s although the action persisted. The stored one-region result was checked before continuing; Apply was not repeated. A separate later capture succeeded. QA-0026 records this timeout-after-side-effect hazard.

Controlled missing coverage used the existing terrain API: preserve the region but move its projection origin to1°,1°, outside the installed offline ring. The real report showed **Volume unavailable**, actual0/16384 sample pairs and no Cut/Fill/Net or TSV block (`shots/180222-038-w4-incomplete-coverage.png`). Restoring the original origin automatically returned200 and full coverage without changing the footprint (`shots/180301-758-w4-automatic-resident-restored.png`). Replacing the stored region delta2→3 updated the unchanged selection to300 (`shots/180340-240-w4-latest-config-delta3.png`). Unbinding withdrew the numeric report (`shots/180412-998-w4-unbound-report-withdrawn.png`); rebinding the original config returned200 (`shots/180500-924-w4-rebound-automatic-200.png`). Switching to B displayed its own-100 result (`shots/180511-195-w4-petal-switch-lower-scope.png`), then returning to A restored its own200 result.

QA-0023 is behaviorally verified in this focused run. The finite-total/per-item readiness guard was independently source-reviewed and production-built; its expanded existing journey is pending the final test sweep. The old unintended r1 from pass05 did not recur: both fresh oracle petals retain exactly one intended rectangle. Its original cause remains unconfirmed.

## Typed data, append and conflict behavior

Actual Inspector Add flows persisted these types on known Cube1, with strict recursive readback that distinguishes booleans, strings, numbers, arrays, objects and null:

- `00_complex`: nested sensor/calibration object, timestamped sample objects, tags array and null metadata.
- `01_series`: timestamped object rows, bool quality, null and a nested numeric array.
- `02_number`: finite-150.25; `03_bool`: false; `04_string`: exact leading/trailing whitespace and newline.

`properties-object-valid.json` and the individual `properties-01_series/02_number/03_bool/04_string.json` receipts preserve authoritative values. Inspected `shots/182130-168-rd1-nested-object-valid.png` shows the Object card; `shots/183304-510-rd1-invalid-number-no-write.png` shows the retained object/array/scalar cards. This last image is property-state evidence; it does not show the invalid-number error itself.

An invalid Number NaN Add displays an accessible finite-number/InvalidJSON error and leaves the entire stored bag unchanged (`shots/183513-946-rd1-invalid-number-error-visible.png`, `invalid-number-after.json`). A malformed Object Add likewise changes nothing and shows line/column detail (`shots/183528-412-rd1-invalid-06_bad_object.png`, its readback). Malformed existing-object Save also preserves the complete bag (`shots/183536-860-rd1-edit-invalid.png`, `edit-invalid-readback.json`). A valid Save appends a third timestamp/value15.75 and a tag; authoritative confirmation is visible as **Matches stored value** (`shots/183543-263-rd1-edit-valid.png`, `edit-valid-readback.json`). This proves stored sequential JSON edits; history/replay storage is a separate upcoming slice.

A fresh dirty draft changed calibration offset to9.25. Save was initially enabled. An independent API update then stored offset4.5. The UI retained the raw9.25 draft, displayed **The stored value changed**, and disabled Save (`shots/183720-755-rd1-conflict.png`, `conflict-readback.json` records enabled→disabled). An earlier attempt without a genuinely dirty initial value is excluded. Switching to Cube2 removes the unsaved editor (`shots/183952-241-rd1-selection-reset-cube2.png`); returning to Cube1 reloads offset4.5 and the appended row (`shots/183954-525-rd1-selection-reload-cube1.png`, `properties-selection-reloaded.json`). The immediate property Delete route was exercised only to remove the disposable initial string attempt; the later approved grid's two-step Delete is not part of this build.

## Grounded client locator failure and repair

The first attempted Object selection remained String, so its JSON-looking value correctly persisted as a string. Strict readback caught this before any typed-object claim; that disposable value was deleted through the UI. Same-pass `type-picker-clipped-target.json` proves the menu row was invisible: id10382284549818889077, fullboundsy1058..1083, hidden=false, actual clip ending981 and negative hit height-77. The old client chose fullbounds center1070.5 outside the visible menu. `shots/181325-909-rd1-type-picker-grounded.png` confirms the visible menu ends before that row.

Client `b76625c` now intersects observed hit/clip/window and refuses empty/negative/disallowed hits. Exact invisible names cannot fall back to fuzzy visible controls. It refused the original target without input. Scrolling the actual popup revealed Object at953..978; the measured center965.5 selected Object, visually confirmed before Add (`shots/182056-864-rd1-object-type-confirmed.png`). The same nested input then persisted as an object. A bounded locator-harness check covered partial clip, empty/negative hit, disallowed input and explicit unobserved fallback; no campaign test sweep ran. QA-0027 is focused-verified.

## Query smoke and unresolved aggregate

The actual scoped query_rows_sql was entered and submitted through Run Query, producing three rows (`shots/184102-286-j03-current-three-value-rows.png`). Numeric ascending sort and real list scrolling exposed10/20/30 (`shots/184300-439-j03-numeric-ascending-last-rows.png`). Selecting visible row3 selected known Cube3 and its viewport beacon (`shots/184302-930-j03-select-known-third-row.png`). An observed state response confirmed node01M6CW40PWWWZ2NBP2EZ8BPS9V, selected_row2 and ascending qa_value sort at frame137330.

The aggregate SQL was entered, but the first Ctrl+Enter attempt retained the prior three-row table (`shots/184307-992-j03-current-three-sum60.png`); its filename is not proof of a sum60 result. Later window height/maximized/minimized, selection and sort changed without matching driver commands. These changes remain unattributed. The bounded retry encountered an invisible editor on a minimized160×28 window. Explicit restore was accepted but its15s client predicate timed out; the final state was restored/maximized1920×1032. QA-0028 records the symptom without actor attribution. Current-stage UI aggregate3/60 is **unverified**. The strict launch's3/60 API setup does not replace it.

The property grid requested during this slice is independently prepared. Its fresh-stage three-size/action audit follows this pass. Current card layout, owner-chip wrapping, missing icon glyphs and partially clipped Results rows remain visible limitations; this pass does not claim complete layout or J03 coverage.

## Formal evidence and cleanup

`checkpoints.jsonl` records23 observed checks as UNKNOWN with actual hashes and reasons. `coverage.json` enumerates every uncovered checkpoint and preserves zero formal acceptance. Client `853ffd5` now emits observed start/end counters and flags possible2000-entry retention gaps for future runs. No unavailable warning-only delta is relabeled quiet, and missing pass06 counters are not invented.

Stop returned0 at18:55:55.077 local. Exact recheck found zero matching identities among root13208 plus seven recorded descendants. No owned Cargo/app process remains. Every PNG named as positive or failed evidence above was opened and visually inspected. No full catalog, physical DPI, OS accessibility repeat, same-run restart, rich history, new grid, or final integrated test acceptance is claimed here.
