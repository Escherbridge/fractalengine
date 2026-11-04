---
type: Review
date: 2026-11-04
title: QA38 failed-run evidence review
---

# QA38: independent full frozen-evidence review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED as faithful focused evidence of the recorded successes and failures. The actual run verdict remains `FAILED_NEW_S1_QA0071`.** No packet blocker found. This approval does not turn the run into a clean pass or close the unintended-authoring finding, other layout defects, campaign gates, native automation, authority testing, CI or release work.

Reviewed frozen `qa/passes/38/packet-manifest.json`, SHA256 `133024b57f5a2eaf59e3ed27f88a70b1e5ecc8fe9e3f6980dd7e25f4b8f4378a`: 152 payloads, 153 files including the manifest, 16 actual PNGs. Independently checked the complete file inventory, every raw payload length/hash, all 128 original-payload anchors, context/cleanup/stage source copies, all five original log hashes and all 16 original screenshot byte copies. Independently inspected every actual image, read raw input outcomes and state/UI/layout/terrain/hierarchy/node/history records, and recomputed the material comparisons below. Applied the shared UI/UX guidance. Only read-only file/hash/JSON/arithmetic/image/source inspection and this main review artifact were performed; no source/Git mutation, build/test, app/process/OS/input/capture, API/DB/store or network action.

## Run, build and provenance

The fresh owned run is `.qa/run/20261104-104626-392`, root PID115032, creation ticks `639294111886613559`, executable under `.qa/stage/7f0de44a-20261104104107782`. Context, stage manifest and launch log agree on source `7f0de44a316d1e8ab340586dc7b3db7eb3a65a7d`. The stage records source_dirty=true and source fingerprint `416CB6E9FCFF567760A033B2B3D63DDA46750766EB9471C80E95FA2B8601358E`; this is not a clean-source claim. Independently hashed the actual staged executable to `6b78f5885bae34f09144f936dda0ea228ff9b60a44e827ff3d252e9ccf199ec9`, matching both records.

The actual stage log records successful completion in206.3s with zero compiler warning/error diagnostics. The fresh deterministic terrain fixture launch completes in15.5s and records PetalA `01M6KTPXJXZKP4H1XTF07JFZ6W` and PetalB `01M6KTPXPWA4DRXFACEQT9JT9V`. This is the divider/header stage, distinct from QA37's older fa955563 stage and from later QA0071 correction source. The fixture's analytic-volume descriptor is setup metadata here; QA38 supplies no new accepted numeric-volume oracle result.

## All actual images inspected

Every image matches the screenshot receipt's dimensions and the original run's PNG bytes. Thirteen are1280x720, two are1024x720, and one is1600x900.

| Actual PNG | Independently observed content |
| --- | --- |
| initial.png | Initial hierarchy and Inspector around the Genesis Verse browser, with browse header and scene grid |
| petal-a.png | Active PetalA scene with opaque backed Back/breadcrumb controls and four-object Petal count |
| selected-after.png | Cube1 visibly selected in hierarchy and scene; Inspector reports position[-12,0,0] and unit scale |
| stable-after.png | Cube1 remains selected after the separate repeated divider drags; both panels visibly change width |
| scale075.png | Compact .75-scale controls and text; the dividers remain distinct, while other controls are visibly small |
| size1600-scale125.png | Larger window at1.25 scale with backed header, selected Cube1 and Inspector |
| size1024-scale200.png | Header wraps across rows on an opaque background; narrow Inspector and hierarchy content visibly overflow or clip; ruler overlaps the hierarchy/footer |
| restored.png | Original1280x720 scale restored with selected Cube1 and previous260/360 panel widths |
| circle-armed.png | Circle is armed, options say to click the viewport to place, and PetalA still has four objects; stored-selection loading/disabled controls are also visible |
| after-armed-back.png | Actual Fractal/Petal browser after Back, PetalA now shows five nodes and an extra Earthwork row in the hierarchy |
| scroll-bottom.png | Inspector scrolled to its bottom at2x; visible disabled Add button, large clipped content, and ruler overlapping hierarchy/footer |
| results.png | Actual five-row Results dock with sortable columns, query panel and top edge below the scene |
| results-after-edge.png | Native vertical scrollbar visibly covers the nominal far-right header edge; no right-edge sort success can be inferred |
| results-hscroll.png | Later columns such as interactive/node_id/petal_id/position are visible following horizontal scrolling |
| results-resized.png | Results top edge is raised and several actual rows are visible; Close Results remains available near the top edge |
| after-results-close.png | Results dock is closed and the scene height is restored |

These support focused header contrast/backing and wrapping observations. They do not prove all hierarchy levels, long names, large counts, every responsive state, all keyboard/accessibility operations or a complete visual audit.

## Divider geometry, preference restoration and camera evidence

Recomputed physical geometry directly from the six raw UI/state captures rather than relying only on the author derivation:

| Capture | Window / scale | Effective hierarchy/right widths in points | Both physical divider widths |
| --- | --- | --- | --- |
| Initial |1280x720 /1.0 |220 /320 |20px |
| After first selected drags |1280x720 /1.0 |260 /360 |20px |
| Smaller UI |1280x720 /.75 |260 /360.0104167 |20.0000038 /20.0000610px |
| Larger window |1600x900 /1.25 |260 /360 |20px |
| Narrow, enlarged UI |1024x720 /2.0 |161.59375 /120 |20px |
| Restored |1280x720 /1.0 |260 /360 |20px |

The actual drag receipts move the hierarchy divider220→260 and right divider960→920, each one admitted command. Effective widths cap at the narrow2x size, then return to260/360 after restoring the window/scale. This supports preference retention under these exercised caps; it is not proof of every cancellation/focus/modal path.

Both initial and stable before/after captures preserve selected node `01M6KTPXX1YHQDA6A0J5983XS1` and the entire native transform exactly: position[-12,0,0], rotation[0,-0,0,1], scale[1,1,1]. The **initial camera comparison does differ** in position, target, orbit and up; only fov matches. Its before sample is immediately after selection and target/distance progress toward the selection focus. The packet retains this mismatch and does not count it as camera-preservation proof or establish its cause conclusively.

The separate stable-baseline check actually repeats both drags,260→280 and920→900. Independently compared all required present `camera.pose` fields—position, target, orbit, up and fov_deg—and all are exactly equal before/after. The selected ID and full transform also remain exactly equal. That separate bounded check earns camera-preservation credit; it does not erase the earlier mismatch or prove arbitrary scene/camera operations.

## Populated Results and neighboring controls

The actual UI template selects all nodes in PetalA, then one Run Query click opens five rows in33ms. The fifth row includes the unintended Earthwork already created earlier; do not interpret the row count as five originally seeded PetalA objects.

- The left-neighbor header click[232,638] changes sort from null to asset_id ascending, matching the actual header visible atx231. This is a positive neighboring-control check.
- The nominal far-right click[948,638] leaves sort at asset_id. The actual screenshot shows the native scrollbar at that edge. It is not a successful right-edge sort or proof that the whole nominal header is interactable.
- The visible interior click[930,638] changes sort to elevation ascending. This is a separate positive result with its own receipt/readback.
- Horizontal scroll moves the asset_id header fromx231 tox-569, exactly800 physical pixels, and the actual screenshot shows later columns.
- The actual Results top-divider drag[600,587]→[600,500] moves the scene bottom587→500, an87px resize. All five required present camera pose fields compare exactly equal before/after.
- The near-top Close click[412,512] closes Results, restores the scene bottom to692, and preserves the same required camera pose fields. This proves the exercised Close target works near the resize edge; it does not establish every Results neighbor or column splitter.

All26 recorded input outcomes across the packet are admitted/ok; their consequences still require these state/pixel checks. In particular, an admitted click is not automatically a successful sort or a safe operation.

## Retained layout defects and Inspector bottom distinction

All eight measured layout records retain coverage_complete=false. Initial, selected-after, restored and1600x900@1.25 have zero measured violations, with12,50,50 and52 unknown entries respectively. The.75 case has101 small-hit-target violations and54 unknowns. The1024@2 case has ten Inspector `clipped_without_scroll_range` observations and38 unknowns; the bottom-scroll case retains ten and39. Results has four visible6px-wide column splitters reported as small targets and145 unknowns. The new20px side dividers do not waive these other failures or fill the unknown coverage.

At Inspector bottom, two separate raw captures—after the first scroll and the additional confirming scroll—agree on vertical offset3530.5, content height3658.5 and reported viewport height128:3658.5−128=3530.5. The actual scissor is only58px high, fromy606 to664, while the reported viewport startsy612 and extends128px. Those are distinct geometry facts, not evidence of extra available scroll range.

The actual Add property button is visible at[848,613.5,62.1875,50] and disabled. The raw textinput value for New property key is empty; the source's existing can_add predicate requires a nonempty trimmed key as well as no loading/pending state. The packet therefore correctly avoids calling Add vertically unreachable or counting this disabled action as a failed enabled-button interaction. It does not claim that a valid property was added at this size. Real horizontal overflow remains: Inspector content width292.125 exceeds228, key/editor controls extend past the window/scissor, and the custom-property table has its own much wider horizontal content. The128px-reported versus58px-clipped viewport and overflow remain open.

The ruler visibly overlaps hierarchy/footer at2x and is also present over the lower-left chrome in other images. The existing `fe-terrain/src/ruler_plugin.rs` draws against the egui whole-window content_rect; there is no QA38 evidence that a later viewport bridge correction was built or accepted.

## QA0071: actual Back also persists an unintended region/node

The exact three pre-placement terrain receipts—before Circle configuration, after configuration and after arming—are byte-identical,517bytes, SHA256 `768864fa20c45400b0035f58269b539ed8aafa92c2aa437600ae20e26f68f55d`. Configuration and arming do not themselves add a stored proposal in these receipts.

The next recorded authoring-triggering input is exactly one actual Back-to-petal-list click at[271,63], resolved from observed hit/clip geometry[245,50,52,26]. Its receipt has commands_applied=1. The after state has petal_id=null, so navigation actually occurs. At the same time, the stored terrain now contains one Raise r1 proposal, delta1, material earth and24 footprint vertices; its full footprint exactly matches the reproduction derivation.

The hierarchy grows from five total QA-tree nodes to six, with **one new node only** and all preexisting hierarchy node entries unchanged. The new node is `01M6KVH6T566T8SZ9Q9PZJ55W3`, Earthwork raise r1, in PetalA at approximately[-25.280767,0,-3.7942772]. The separate node endpoint and full history verify this persisted object: current_version1, current_state_verified=true, baseline/object_created journal entry, matching after_state and scope. This is persisted authoring, not just a transient preview or misleading object counter.

The actual armed and after-Back screenshots corroborate the four→five PetalA object change. The saved sequence records no separate scene placement click or replay between arming and that Back action. This is a concrete new QA0071 failure; it does **not** establish the historical QA0024 trigger, and later source fixes receive no runtime credit from this older packet.

## Encoding, logs, finite credential scan and cleanup

Independently decoded all136 non-PNG payloads strictly as UTF8, preserving the two actual UTF8 BOMs in cleanup-identities.json and stage-manifest.json. No NUL or replacement characters occur. All133 JSON/JSONL payloads, plus the manifest, parse with NaN/Infinity rejected. The published scan covers135 files at its creation point; independently extended the same four finite PATTERNS to every non-PNG payload and found zero matches. This is finite bearer/JWT/private-key/credential-assignment coverage, not a general proof that arbitrary secrets cannot exist.

All five original logs match published lengths/hashes and decode strictly as UTF8 without BOM/NUL/replacement. stdout is200158bytes/1413lines with1322INFO,24WARN,0ERROR; stderr is empty. There are zero panic markers, zero private current-read error markers, and zero stage compiler warning/error diagnostics. The warnings include normal explicit debug-surface notices, network/bootstrap or validation-layer notices, deferred scene publications/stored-scene recovery refusals, and delta-lattice coarsening. Their presence is retained. Zero ERROR log lines do not negate the persisted QA0071 failure.

Stage, launch and stop raw logs are copied into the packet byte-exactly. stdout/stderr remain at the original retained run paths and are bound by the summary hashes; do not describe those two raw logs as published packet payloads. The stop log reports successful stop of the exact owned run. The saved cleanup's eight identity triples match the context's root plus seven descendants exactly, and every recorded_identity_present is false. This is verified saved exact-identity evidence, not a new process census performed by this reviewer.

## Approval boundary

The frozen packet faithfully separates its focused positive divider/header/Results checks from initial camera mismatch, small targets, incomplete layout coverage, Inspector clipping/overflow, ruler overlap and the newly persisted authoring failure. Its overall failed verdict is correct. No clean FULL pass, two-consecutive-FULL campaign credit, queued grant/authority or peer delivery, retained-store restart, native UIA/WGC/helper completion, rollback/late Resolve acceptance, final integrated test sweep, CI completion, tagged release or downloadable-release artifact verification is earned here.
