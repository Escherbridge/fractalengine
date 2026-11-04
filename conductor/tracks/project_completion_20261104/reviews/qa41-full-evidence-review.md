---
type: Review
status: approved_focused_with_external_erratum
---

# Independent QA41 frozen evidence review

**Verdict: APPROVE publication as `FOCUSED_CORRECTIONS_WITH_OPEN_LAYOUT_FAILURES`, accompanied by the external summary erratum.** The recovered Owner tree, exact Circle/Back protection, recorded hint/ancestor protection, one legitimate Circle placement, and saved property value have supporting raw evidence. This is not a clean layout, native-provider/capture, journal, authority, retained-store, full-sweep, CI, or release acceptance. QA0071/QA0072 may remain open for their broader required gates.

Reviewed the frozen packet at `qa/passes/41` with manifest SHA-256 **`3bf5ba5a56cc2dc0a8825a4930d0d6a20c2198ab24ba35b02b39edd91175e359`**. No packet, application, production source, Git, API, database, build, test, or process state was changed. This review only read retained evidence and wrote this report. Applied the shared UI/UX guidance.

## Packet integrity and provenance

- All **132 payloads**, totaling **11,402,383 bytes**, match their recorded lengths and SHA-256 values. The directory contains exactly those payloads plus its manifest: **133 files**, no extra/missing paths or duplicate manifest names.
- All **106 original-payload anchors** in `packet-summary.json` match the current raw payload bytes. No newline/JSON normalization was used for those comparisons.
- All **17 actual PNGs** were independently opened and visually inspected. Their original image paths from the saved screenshot receipts hash identically to the copied packet PNGs. The author's nine-inspected/eight-pending flags were not mistaken for completed independent inspection.
- The finite credential scan's **114 text files** were independently rescanned using its four exact patterns. All counts agree: **112 UTF-8 files and two UTF-8-with-BOM files, zero matches**. The two BOM files are cleanup and stage metadata. The scan includes the summary; its own `credential-scan.json` is the sole non-PNG payload outside its file list. This limited pattern scan is not a general proof of absence of secrets.
- Source/stage receipt names commit **`d8da3a46e9861e9191845ea283e37fb7e5ea3472`**, fingerprint `1219AA51001998FD07CFFACD970CC7FE53D2C4F5232E13189AF161340DFE8EA1`, and `source_dirty: true`. The retained executable independently hashes to `965b6e41fde2cb336174d02acaa4e2e15b741d1a93ffb7c11336158d9979b585`, matching stage and owned-context receipts. This is matching staged-source evidence, not a claim of a pristine release checkout.
- Retained stage log reports success in **86.5 seconds**, without compiler warning/error diagnostics. Launch completed in **15.1 seconds**, run `.qa/run/20261104-130242-645`, root PID 119056. Raw stage/launch/stop log copies and the summary's retained stdout/stderr files independently match their stated hashes.
- Saved stdout is **117,675 bytes**, with **736 INFO, 27 WARN, two ERROR, zero panic markers**. Both ERROR entries correspond to the retained navigation timeouts described below. Stderr is empty. No warning-free runtime claim is supported. Runtime stdout/stderr remain local source receipts described by hash, not copied raw packet payloads.

## Mandatory external erratum

The frozen `packet-summary.json` limitation index 6 incorrectly states that an unsupported `layout` command left an original zero-byte artifact. **There is no zero-byte file in the frozen packet or its manifest, and none in the 106 original anchors.** The raw packet must remain unchanged.

Read and approve `conductor/tracks/project_completion_20261104/evidence/qa41-summary-erratum.json`, SHA-256 **`9a9cff4ae43a5c5b63bfbd4bfa28941e01289489def0073578ed8246425fa8ea`**, which binds that precise field to the frozen manifest hash. The inspected `.omc/research/qa_pair_drive.py` calls `fe.main(...)` inside redirected stdout before `path.write_text(...)`; it does not catch argparse `SystemExit`. The correction is therefore consistent with the actual driver: the unsupported command failed before the artifact write. The parent reports stderr was visible in the tool result, but raw stderr for that command was not archived. This review does not invent a retained stderr receipt. The later supported `layout-check` has separate saved evidence.

Approval is for the frozen packet **with this external correction**, not for the erroneous frozen sentence in isolation.

## Owner observation recovery

`owner-initial-ui.jsonl` is a successful physical-pixel UI tree at frame 2578/scale 1. Its **102 layout controls all join actual flattened node IDs**, with no duplicated control IDs in that set. This independently confirms the physical-ID join needed for actual Owner locators after the ordering correction. It does not demonstrate native Windows provider identity or native invocation; those use separate remapped identities and gates.

The initial screenshot is an ordinary rendered 1280x720 Genesis Verse browser with the hierarchy and Inspector. Actual Petal A navigation subsequently has matching scene UI/state evidence. An initial broad Petal locator refused as ambiguous before input. The exact cross-Verse navigation receipt reports the expected old-token boundary and requires observing/reacquiring the changed Verse rather than reusing the previous token. That receipt includes a withheld/recovery warning and a generation-change possible log gap; it is not authority/provenance acceptance.

## Armed Circle and chrome protection

The retained Circle shape selection and one Arm click use actual observed physical Button locators. The armed screenshot and UI show the placement instruction, disabled Arm control, and enabled Cancel. The exact Back click targets `Back to petal list` at `(271,63)` once.

Independently recomputed raw comparisons:

| Comparison | Result / SHA-256 |
| --- | --- |
| Circle configuration, armed terrain, before Back, after Back | All raw-identical: `6aea9f69d9f78fa1328d43882172cd32129a142bfe00ed1c3974258f997414a3` |
| Hierarchy before versus after Back | Raw-identical: `0cd32343d3cb52318748579b7bb5855deb6e9e37c8e0939bca49beaa7f0cbcde` |
| World-positive terrain versus before/after hint and after ancestor | All raw-identical: `cc8f0e4518aa8da3314763e7189fac90806412c8c1123bdaed740b1fadb3d220` |
| World-positive hierarchy versus before/after hint and after ancestor | All raw-identical: `ecabeedd9c8924b9c545ded7746c0f3401ebd2c9444bffb32938e8406d1d7fa6` |

Unlike QA36's limited invocation/history check, QA41 **observes departure**: after Back, state frame 8112 has Petal `null` and the same active Fractal, and the screenshot shows the two-Petal Fractal browser. After the later ancestor click, state frame 29040 has both Fractal and Petal `null` and the screenshot shows the Verse's Fractal browser. This is concrete state and image evidence.

For the later hint check, the recorded rearming succeeds; click `(600,668)` is inside the visible tool-hint area. The after-hint UI still shows enabled Cancel/disabled Arm, supporting continued armed state before the recorded ancestor click. The ancestor locator is `Browse current verse fractals`, clicked once at `(405.859375,64)`. The raw comparisons above support no terrain/hierarchy mutation for these specific controls. A later attempted extra Arm refuses `locator_not_found` before input; it must not be counted as another accepted arm.

There is **no historical journal byte comparison** in this packet. Raw terrain and hierarchy stability is not journal stability, nor proof of the historical QA0024 trigger. Rectangle, Polygon, continuous Brush, camera/held gestures, secondary-click, other chrome, and every missing combined boundary remain outside this focused proof.

## Legitimate world placement remains functional

The positive sequence has a successful fresh Arm, a saved armed pre-click UI/image, and exactly one successful world click at **`(680,450)`**. Its after-image visibly contains the blue region and new Earthwork tree entry.

Independent before/after hierarchy processing finds **five existing nodes before, six after**, with exactly one added node: `01M6M2S1HGV4BRE9B50D0EV2JP`, `Earthwork raise r1`, in Petal A. All five existing node objects remain equal. Terrain adds exactly **one proposal**, `r1`, operation `raise`, material `earth`, delta `1.0`, with **24 footprint points**; all other terrain fields remain equal. The focused comparison records its position `[1.4082023,0,0.9412624]` and the retained terrain contains the matching footprint.

This accepts the narrow positive control that ordinary scene placement still works. It does not establish volume accuracy, intersections, reports, numerical overlap, persisted journal semantics, or retained-store restart behavior.

## Property workflow: successful narrow save, poor usable space retained

The valid draft was entered through ordinary physical UI input at normal 1280x720/scale 1. Saved frame 39697 proves `New property key = qa_form_probe`, type `string`, `New property value = qa41 readable value`, and an enabled Add button. Inputs were not newly typed into the tiny scale-2 viewport.

After resize to 1024x720 and scale 2, frame 39783 retains the exact draft. At the initial bottom offset **3838**, Add is offscreen. One recorded wheel action `(dx=0,dy=1)` at `(910,635)` changes the Inspector offset to **3758**, frame 41067. Its actual Add Button rect is `(848,618,62.1875,50)`, with visible hit height **46**, losing four pixels at the bottom. The saved screenshot visibly shows that enabled Add target. One physical click at **`(879.09375,641)`** succeeds.

The authoritative subsequent node response contains exactly `qa_form_probe: "qa41 readable value"` alongside the existing `qa_fixture`, `qa_role`, and `qa_value` entries. The selected object remains Cube 1 with position `[-12,0,0]`, identity rotation and unit scale. The final normal-scale 1600x900 image visibly shows the new row and exact value.

This proves this valid draft survived the scale change, the partially clipped Add could be reached after one wheel action, and its value saved. It does **not** prove comfortable editing at scale 2 or every schema control reachable. Narrow Inspector viewport remains **52 physical pixels** high; its content is thousands of pixels tall. The bottom screenshots show only a truncated Property/schema header. Zero measured hit violations in that narrow profile must not be promoted to clean visual acceptance.

At 0.75, the packet retains **13 measured failures**: twelve Open/Select name targets are 16.1953125–19.7109375 physical pixels wide despite 20.015625 height, and one splitter width is 19.99999237060547. Both 0.75 and 2x images show the ruler competing with Hierarchy/footer. The independent residual design review describes concrete shared-row, divider, Inspector-navigation, and same-pass ruler corrections; none are implemented or accepted by this packet.

## Timeouts, refusals and cleanup

Two accepted navigation timeouts remain recorded without replay in the saved sequence:

- `owner-return-petal-a-sidebar.jsonl`: `input_timed_out`, status 500, explicitly **three of three input frames applied** before its 4500-ms completion deadline. The later state/image observes Petal A. Stdout records the matching 20:06:01 ERROR/4703-ms response.
- `owner-form-return-petal-a.jsonl`: HTTP 504, five-second window-answer timeout. The subsequent timeout-state receipt observes Petal A after the prior Verse-browser state. Stdout records the matching 20:12:38 ERROR/5012-ms response. A completed navigation after a timeout does not make that transport result successful; observe state before any subsequent work.

Ambiguous locators and missing Arm refusals remain distinct from accepted input. No failed placement was retried into a duplicate successful placement in the recorded sequence. The unsupported command has the external erratum described above.

The retained cleanup receipt checks exactly **eight `(PID,start_ticks,exe_path)` identities**, matching the owned context's root plus seven descendants, with no missing or extra identity. Every recorded identity and PID is absent at **`2026-11-04T20:16:43.8987664Z`**. This is inspection of saved exact-identity cleanup evidence; this review did not inspect running processes or operate cleanup.

## Complete visual inventory

Every listed image below was opened during this independent full review, including the copies previously inspected during the design task.

| PNG | Dimensions | Actual visual observation |
| --- | --- | --- |
| `owner-initial-shot.png` | 1280x720 | Genesis Verse browser, tree and empty Inspector; ruler at footer boundary. |
| `owner-petal-a-shot.png` | 1280x720 | Active Petal A scene, selected tree path, empty Inspector. |
| `owner-circle-armed-shot.png` | 1280x720 | Circle selected, placement instruction and Cancel active in Options. |
| `owner-after-armed-back-shot.png` | 1280x720 | Two-Petal browser; actual departure from scene. |
| `owner-positive-before-shot.png` | 1280x720 | Armed Circle scene; byte-identical PNG to earlier armed shot, not independent visual variation. |
| `owner-after-world-circle-shot.png` | 1280x720 | Blue region and added Earthwork entry visible. |
| `owner-after-ancestor-shot.png` | 1280x720 | Verse Fractal browser; ancestor departure visible. |
| `owner-after-return-timeout-shot.png` | 1280x720 | Returned Petal A scene after first navigation timeout. |
| `owner-physical-quarter-shot.png` | 1280x720 | 0.75-scale short tree name targets; ruler/footer overlap. |
| `owner-narrow-double-shot.png` | 1024x720 | Three-row section rail and tabs; extremely small body; ruler overlaps footer/tree. |
| `owner-narrow-bottom-shot.png` | 1024x720 | Scrolled body shows only truncated Property/schema section. |
| `owner-restore-shot.png` | 1280x720 | Cube 1 selected, normal-scale custom property form. |
| `owner-form-bottom-shot.png` | 1280x720 | Initial empty property form at normal-scale bottom. |
| `owner-form-valid-narrow-shot.png` | 1024x720 | Valid draft retained offscreen at the narrow bottom; visible truncated schema header. |
| `owner-valid-form-scroll-up-shot.png` | 1024x720 | One wheel step exposes enabled Add, with bottom clip retained. |
| `owner-form-after-shot.png` | 1024x720 | Post-save narrow body remains tiny; visual alone does not prove saved value. |
| `owner-finish-shot1600.png` | 1600x900 | Normal scale restored, Cube 1 and new exact property/value visibly listed. |

No FULL, native helper/provider/capture, WGC, peer, authority/grant continuity, retained restart, journal, complete brush/chrome/camera sweep, CI, or tagged-release acceptance is granted. The frozen packet plus external erratum faithfully supports its bounded success-and-failure verdict.
