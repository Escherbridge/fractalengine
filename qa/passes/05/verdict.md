---
type: QA Pass Verdict
pass: "05"
date: 2026-11-01
stage: 74a817e-20261101165518420
run_dirs: [20261101-165610-675, 20261101-170234-101]
scenarios: [S04, S05, S06, S10]
focused_smoke: true
coverage_complete: false
---

# Pass 05: deterministic terrain, query and endpoint proof

Verdict: fixture generation, strict API setup, seeded UI queries, persisted constant-depth earthwork values, and isolated divider endpoint checks pass. The first displayed earthwork reports fail: stale configuration and missing terrain coverage can produce cached zero or partial values. Exact UI values are demonstrated only after an explicit footprint refresh. This is not a healthy S05 end-to-end result or campaign convergence.

## Provenance

Combined GUI/helper stage `.qa/stage/74a817e-20261101165518420` records commit `74a817ecaabdfe3906695fb5410640d0bffcd15f`, source fingerprint `EE428A97FAF4CD24DF3550B2C8718E163BD34D4044E740EC5E8AD636DB12CDB3`, executable SHA256 `A228D6F35E8BE265A41C4AB8E9D5BB6E1A3CFD5873D36F1D12D86A92954F1146`, and source_dirty=true. Launcher verification used the current fingerprint. GUI build took 4m30s, helper 1m23s; serial stage total 370.1s, jobs 2. No tests or clippy ran; the integrated campaign-end sweep remains pending.

S7 fixture commits and the property correlation prerequisite were integrated with S2 semantic names before this build. The first earlier stage failed strict setup after a property persisted but its API reply timed out; `setup-failure.md` preserves that failure. The combined stage completed strict basic and terrain fixture setup, including unselected property mutations, exact echoes/readbacks, numeric values, marker positions and terrain JSON. It therefore verifies QA-0022's original live trigger; the existing expanded correlation journeys remain unexecuted until the final sweep.

## Determinism and bindings

Two independent executions of the staged real TerrainRGB export/bundle/assemble/install generator produced identical source, descriptor and bundle index hashes in `generator-hashes.json`. Each installed PNG was byte-compared by the generator. The basic run has one bound petal B and an unbound A; the terrain run has both A and B bound. Both strict launch oracles return exactly three numeric value nodes 10/20/30 and sum 60, excluding asset-free marker rows. Source/native/world scale is 1, zoom17, offline nine-tile ring. The flat square is absolute elevation zero; runtime subtracts the observed center-tile mean 0.13673553465355326m. Authored Y=0 is not a claim of exact surface seating.

The window capture `shots/170330-247-s7-terrain-hill-window.png` visibly shows the separate hill crest and plane, with settled=true and tiles_pending=0. An earlier scene-only capture at the same hill pose was black and unsettled after 600 frames (`shots/170151-685-s7-hill-silhouette.png`); that capture mode is not validated by the successful window image.

## Query and numeric oracle

Actual SQL was entered into Inspector Query and submitted with Run Query. Expanding the real Results divider made all three rows visible together in `shots/172028-462-s7-ui-query-three-values-expanded.png`: Cube1/2/3, qa_value10/20/30, positions(-12,0,0), (0,0,-12), (12,0,0). The separate aggregate submitted with Ctrl+Enter visibly returns fixture_count3/fixture_sum60 in `shots/170428-368-s7-ui-query-count-sum.png`. API fixture setup is corroboration, not a substitute for these UI actions.

Initial A was contaminated by an unintended freeform r1 before the intended Rectangle Apply. Its persisted footprint is a 24-vertex circle centered(-3.1857097,-2.2412362), radius5, delta0.5, created00:04:52.577Z. The explicitly applied origin rectangle r2 Raise2 was created00:05:21.773Z and persisted fill200. The earlier circle persisted fill38.83056640625. `raise-first-config.json` retains both records. The first UI report showed only r1 and fill23.10 despite the two-record stored config; after A→B→A it showed both but cached zero. This evidence is retained as failure, not used as the clean oracle.

Separate clean petals were then used. B was still fresh; C Raise Oracle was created through API setup and bound with exact descriptor PUT/GET validation. Real Brush UI set Radius5, Rectangle, Lower positive Delta1 on B and Raise positive Delta2 on C; each Apply Rectangle was clicked once. Explicit pointer movement to the UI controls preceded edits. Each petal has exactly one persisted region; `persisted-oracles.json` asserts finite raw cut/fill with absolute tolerance0.001 and expected nonzero counts:

| Petal | Persisted regions | Cut m³ | Fill m³ | Net m³ |
|---|---:|---:|---:|---:|
| B Lower1 | 1 | 100 | 0 | -100 |
| C Raise2 | 1 | 0 | 200 | 200 |

The initial reports both displayed zero despite resident visible geometry and eventual nonzero persistence: `shots/171521-496-s7-lower-first-report.png` and `shots/171844-701-s7-raise-clean-first-report.png`. Changing Radius5→5.01→5 triggers a fresh exact footprint report. Only after this workaround do `shots/171559-985-s7-lower-refreshed-report.png` and `shots/171922-837-s7-raise-refreshed-report.png` show Area100.00, Cut100/Fill0/Net-100 and Cut0/Fill200/Net200 respectively. The 128² lattice is the expected integration geometry, not an observed coverage count; no sample count was exposed. W4-S3 must fix incomplete-state reporting, tile-residency refresh and latest-configuration ordering before S05 can pass without this workaround.

One navigation to C returned HTTP504 after5s but actually completed; authoritative state was checked and navigation was not repeated. A few stale panel locators were refused and corrected using actual state; they do not count as successful actions. The unintended original r1 requires pointer ingress investigation; it did not recur during the clean pre-move sequence.

## Strict S6 endpoint check

`endpoint-proof.json` records three isolated drags at actual ppp2, window1920×1080, fresh geometry each time: left divider+radius, right divider-radius, Results top-radius. Each preserves exact Cube1 selection and exact camera state. The radius5points is source-derived from pinned egui0.33.3 style default and inspection of app style mutation sites; it was not exported as runtime telemetry. A genuine empty interior click clears selection, and a genuine rendered Cube1 interior click selects its known node ID (`shots/170041-819-s6-interior-pick-after-deselect.png`). An earlier combined drag sequence changed geometry/camera and is excluded from the unchanged-camera claim.

All checkpoint images named above were opened and visually inspected. Other exploratory images and failed locators are not positive evidence. This pass does not cover the full panel catalog, a human screenreader session, physical monitor DPI, same-run restart, or final integrated tests.

## Normal OS accessibility and paired wall-frame measurement

Before changing Rust, the same stage launched run20261101-172421-326, root43000, port8880, Fixture none, DisableSyntheticInput, A11yTiming and A11yContinuous. The bounded identity-verified OS UIA helper initially failed generically. A direct bounded owned MTA worker then found eleven named caption/tool/Data buttons with finite bounds, and the normal wrapper retry also passed. This proves static controls through the normal HWND provider with synthetic input disabled; it does not establish a complete screenreader workflow. Debug ui-state remained403qa_gate_refused and pointer input remained403disabled after provider activation.

The real FE collector completed 1128 baseline and1102 active frame samples in the same focused1280×720, ppp1, no-node scene with continuous cadence and Fifo presentation. Baseline mean17.6802ms/median16.3611/p9528.7276; active mean18.1348ms/median16.5886/p9529.5111. Mean difference+0.4546ms(+2.5714%), median+0.2275ms, p95+0.7835ms. It reports zero missing/invalid/duplicate measurements and five excluded report-write frames. These are observed wall FRAME_TIME values including scheduling, vsync, GPU and application work; they are neither CPU cost nor a causal isolated accessibility overhead estimate. `os-accessibility-timing.json` retains the summary, conditions, static provider results, refusals and limitations; the private raw collector file remains under the run root. This is a current-stage baseline, not a profile of future source changes.

## Cleanup

Basic run20261101-165610-675 root58340 and terrain run20261101-170234-101 root77996 were stopped through exact recorded identity cleanup. Final terrain stop returned0 at17:20:58.728 local; subsequent recheck found zero matching identities among root plus seven recorded descendants. The scripts did not kill unrelated processes. Run-local persistence and evidence remain for inspection. No Cargo or app process is retained by this slice.

The optional OS run's stop attempted all pinned identities but returned1 because four WebView processes exceeded the shared15s deadline. A subsequent exact recheck found zero matching identities among root plus seven recorded descendants. This is eventual absence, not a successful first cleanup exit.
