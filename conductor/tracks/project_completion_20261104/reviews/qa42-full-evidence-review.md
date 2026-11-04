---
type: Review
title: qa42-full-evidence-review
date: 2026-11-04
status: approved
---

# QA42 independent complete focused-packet review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**APPROVED as faithful, bounded focused layout/property/Circle-Back evidence. No external erratum is required for the frozen summary.** This review covers the entire packet and all nine actual screenshots. It grants no FULL-pass, broad finding closure, ruler metric calibration, native helper/provider/capture, terrain journal, final sweep, CI or release acceptance.

Packet `qa/passes/42/packet-manifest.json` is **11,753 bytes**, SHA-256 **`f0f70cf5cb5b6a87b871baf28bd93ccc6aa16dbb7bd76d596820538ac64eda83`**. All **70 payload** lengths/hashes match; nine payloads are actual PNGs. All **52 original-artifact anchors** in the summary match their retained raw payloads. The summary is **13,527 bytes**, SHA-256 `d3b3fb714568bbd453638b5a8b515b65cab7a80a9853ae26cf89d3c6ba5c8d2f`. The packet remains unchanged.

Only this review was written. Inspection used read-only files, evidence JSON/JSONL decoding, bounded comparison/hash processing and `view_image` for every original screenshot. The packet builder and evidence helper were read but not executed or imported. No implementation parser, tests/compiler, application/helper, OS/process/native/API/DB/network action or Git mutation was performed. Root's current QA43 and other authors' source changes were untouched. Shared `C:/Users/atooz/.gemini/UI-UX-guidance.md` was applied.

## Source, ownership and raw-copy identity

Owned context binds the disposable terrain fixture to run `.qa/run/20261104-150850-588`, application PID 110832 with its recorded birth/executable identity, seven descendant identities, the exact published `925d7cbaddcdbd9996fc32d10078326cae0aaaa3` stage and `stage_matches_source: true`. The recorded stage retains `source_dirty: true`; this packet does not claim a clean worktree or later-source coverage. The separately reviewed successful build establishes staged app/native bytes, not native runtime acceptance.

Independently checked all nine PNG payloads against the actual original paths in their screenshot response records: every copy is raw-byte identical, has the declared byte count, and its PNG dimensions match its response pose. Also checked `owned-context.json`, `cleanup-identities.json`, `stage-manifest.json` and all three published build/launch/stop logs against the builder's retained originals: all six are raw-byte identical. Original UTF-16/BOM logs remain in their original encoding.

The 52 summary anchors preserve the action/UI/layout/API files already collected before assembly. They are not fabricated by the builder's derived summary assertions. The manifest/summary preserve actual failed or limited observations, including a cross-Verse continuation warning, stored-scene recovery refusal and incomplete geometry coverage.

## All nine actual screenshots inspected

I opened each PNG through `view_image`; these conclusions are based on visible pixels as well as state/layout records.

| Screenshot | Dimensions / recorded UI scale | Actual visible result and limits |
| --- | --- | --- |
| `151047-855-qa42-initial.png` | 1280×720 / 1 | Nonblack Genesis browser with readable compact Fractal card, hierarchy and empty Inspector. The fallback world-unit label is contained above the footer. This is the initial Genesis view, not an active terrain-Petal result. |
| `151137-651-qa42-petal-a.png` | 1280×720 / 1 | Active Petal A scene and terrain render; active hierarchy ancestry is distinguishable. The bottom-left ruler is separated vertically from the actual bottom tool hint and footer. Empty Inspector remains consistent with no selection. |
| `151206-486-qa42-1280-075.png` | 1280×720 / 0.75 | Selected Cube 1 renders red and is visibly highlighted in the tree; its Inspector is populated. Compact controls remain visible in this snapshot. Rightmost tree Options have intentional scroll clipping; this is not proof all targets are fully exposed or activated. |
| `151233-733-qa42-1024-2.png` | 1024×720 / 2 | Compact Tools/Panels and Inspector Section/Tab selectors leave substantial body space. Breadcrumbs wrap to several rows and cover part of the scene; right Inspector text/asset path remain narrow and heavily wrapped/truncated. Ruler clears the footer. This is improved body access, not polished global layout acceptance. |
| `151324-266-qa42-tree-right-2x.png` | 1024×720 / 2 | Horizontal hierarchy scrolling exposes the right-side visibility/Options controls. Corresponding left labels/icons are deliberately clipped by the scroll viewport. These Options controls were exposed, not opened or functionally accepted. |
| `151325-673-qa42-inspector-bottom-2x.png` | 1024×720 / 2 | After one recorded wheel action, the blank property form is visible with Type, Value and disabled Add. Its blank/invalid state does not prove an inaccessible valid action. Table contents and earlier Inspector sections are above the current scroll position. |
| `151403-678-qa42-valid-draft-2x.png` | 1024×720 / 2 | Valid key draft and exact wrapped value `saved at 2x` are visible; Add is enabled and fully reachable. The narrow key text is horizontally clipped inside its field, while raw UI records preserve the complete value. |
| `151424-966-qa42-1600-125.png` | 1600×900 / 1.25 | The table shows the saved `qa42_probe` row beside the three original fixture properties. Row actions remain off to the horizontal side and are not exercised. Cube 1 selection and scene remain visible; ruler stays inside the scene above the footer. |
| `151647-992-qa42-armed-circle.png` | 1600×900 / 1.25 | Circle placement instructions and Cancel/clear are present, with the lower generic Brush hint. Ruler is visibly above that actual hint and footer. The Options panel also shows “Loading stored selection state”; disabled controls/transient messaging remain unresolved rather than accepted. |

None of the nine images is a black/empty capture. Smaller tree icon strokes, selection highlighting and readable opaque navigation are visible in these particular states. Remaining narrow wraps/truncation, blank-square glyphs in some icon controls, offscreen actions and transient messaging prevent a claim that the complete UI is visually finished. Compact Section/Tab controls are visible but their menus were not activated. The numerical ruler labels are rendered observations only; no independent world-to-pixel or first-vision/FOV calibration was measured.

## Layout and action evidence

The four layout results faithfully record zero measured violations with `coverage_complete: false` and `observed_texts: 0`:

| View | Observed controls | Unknown entries | Intentional scroll-content entries |
| --- | ---: | ---: | ---: |
| Petal A / 1280×720, 1 | 103 | 13 | 8 |
| 1280×720, 0.75 | 142 | 57 | 18 |
| 1024×720, 2 | 123 | 42 | 91 |
| 1600×900, 1.25 | 134 | 50 | 25 |

The raw UI coverage string explicitly excludes unvisited hits, text and floating-area occlusion. Thus zero measured violations is not a complete target denominator or proof of no visual clipping. The screenshot inspection is a separate required check, not inferred from these zeros. The layout/state observations use distinct nearby frames, not an invented single atomic camera/UI capture.

At 1024×720/2, the actual Inspector scroll viewport is `{x:798,y:282,w:226,h:382}` physical pixels. Its stored content is much longer than the viewport; one recorded pointer move plus wheel delta `dy:-120` changes its vertical offset from 0 to 3478.5 and exposes the property form. Hierarchy horizontal offset changes from 0 to 92.8125 with the recorded horizontal wheel action, matching the screenshot's exposed Options edge. These records support the specific 382-pixel-body and reachability claims. They do not prove every scroll adapter or every long form works.

Independently joined eight named action records to corresponding actual UI nodes and layout controls by ID: Select Cube 1, New property key/value, Add property, Brush, Circle mode, Arm placement and Back. Each has one matching node/control, matching name, an enabled target in the paired UI snapshot and a click inside its observed bounds; each click records one applied command. The value-field join uses the retained post-entry valid-form UI at the same geometry. This supports actual ordinary UI interaction rather than a direct data write; it does not assert UI-tree and action frames were atomic or confer native-provider credit.

Key and value text-entry records preserve `qa42_probe` and `saved at 2x`. The valid-form UI exposes those exact buffers and enabled Add. One recorded Add click is followed by a successful API read of that same selected Cube 1 with exactly:

`{"qa42_probe":"saved at 2x","qa_fixture":"terrain","qa_role":"value","qa_value":10}`

The wide screenshot independently shows those four rows. This accepts the focused string addition while preserving the fixture properties; it does not cover edit/copy/delete, malformed schemas, journal sequencing, history replay, peer delivery or persistence through restart.

## Circle Back comparison and authority limits

The packet records a normal Brush click, Circle mode selection and one Arm click, then the armed screenshot/UI with placement instructions and Cancel. Back's actual enabled control has the same recorded ID/geometry as the single click at `(340,78.75)`. The subsequent state clears the active Petal and selection while retaining the current Verse/Fractal. No input timeout is replayed in these action records.

Independently compared the complete raw before/after `terrain` response files: byte-identical. Independently compared the complete raw before/after hierarchy responses: byte-identical. Their hashes and body contents are retained. This establishes the bounded reported comparison across the one armed Back action. The terrain endpoint returns Petal terrain configuration; it is not a dump of every geometry/region/journal/history record. Consequently this packet does not establish zero mutation to every persistent table or historical time series. It contains no positive Circle placement or independent expected-volume/geometry comparison.

The initial cross-Verse open correctly records a view change and denial of the rest of the old-token batch, followed by records scoped to the new Verse. It also preserves a stored-scene recovery transaction-refusal warning. Selected/narrow/wide state records retain `terrain_diagnostic: stale / scene_petal_mismatch`, and the armed image shows loading-state messaging. UI `settled: true` explicitly excludes DB/sync and terrain completion guarantees. These unresolved observations prevent any claim of full stored-scene recovery, transform enablement, terrain report freshness or diagnostic correctness. They do not invalidate the directly observed property save or exact Circle Back response comparisons.

## Cleanup, logs and finite credential checks

The stop log records the owned run kept after stop. The cleanup receipt at `2026-11-04T22:17:48.3449054Z` names exactly the owned context's application plus seven recorded descendants; all eight have `pid_present:false` and `recorded_identity_present:false`. I compared the complete identities, not just PIDs. This supports absence of those eight recorded identities at that timestamp; it is not a fresh OS query or proof about descendants born after the recorded snapshot.

The retained local application stdout matches its summary: 82,865 bytes, SHA-256 `aa8b7e47a41f78bd3a13c2891128f2b9ebf89d1042f6511578062fcc3537326e`, strict UTF-8, 607 lines, 517 INFO, **23 WARN**, zero ERROR and zero panic markers. Stderr is empty with the standard empty SHA-256. Compiler diagnostic and finite credential counters also match. A stored-scene recovery refusal is present; zero ERROR does not imply warning-free or successful recovery. Raw application logs remain local, with their strict-decoded summaries published.

Independently repeated the four declared finite credential-pattern counts over all 60 text inputs listed by `credential-scan.json`, including preserved encoding checks: no mismatch and zero matches. The local application log summary's same four counts match. The scan expressly denies universal secret-absence proof and does not scan PNG text or every possible credential spelling. This approval does not expand that claim.

## Acceptance boundary

The frozen verdict `FOCUSED_LAYOUT_AND_CIRCLE_BACK_VERIFIED_WITH_LIMITS` is supported at its stated scope. No correction to the packet-summary facts or limits is needed, and no payload should be rewritten. Preserve the raw warnings, incomplete geometry and original captures when publishing an exact-body OKF copy of this review.

QA42 contributes focused evidence for the inspected responsive states, a reachable valid narrow string-property addition, shown ruler/chrome separation and one armed Circle Back response comparison. Results, compact selector activation, divider dragging, held gestures, the other brush/shape negative matrix, offscreen property row actions, metric/first-vision calibration, journal comparisons, native/role/peer execution, retained-store restart, full catalog coverage, two clean FULL passes, final integrated sweep, exact-SHA CI and tagged release verification remain open. QA43 is a separate packet and receives no advance credit here.
