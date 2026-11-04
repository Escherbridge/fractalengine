---
type: Review
date: 2026-11-04
title: QA36 focused evidence independent review
---

# QA36 independent full evidence review

Reviewed 2026-11-04. Verdict: **approve the frozen packet as bounded focused evidence, with the navigation claim correction below.** It supports panel width restoration, divider preservation of a selected node and camera pose, and a fresh partial-coverage-to-complete genuine-zero volume transition. It does not establish an observed departure from and return to the petal. This is an evidence review, not a new execution or source approval.

## Frozen identity and integrity

- Manifest: `qa/passes/36/packet-manifest.json`, SHA-256 `2c28abd2f725696bffd655dc1727d0a1114077cb6f6ac1b1c9b2f24a96ea6473`.
- Independently read and hashed all **154 payloads**, including **14 PNGs**. Every byte count and SHA-256 matches the manifest. The actual recursive payload file set equals the manifest file set; no extra or missing payload was found.
- Stage receipt names source commit `fa9555632380977c41fe89ac984fcb491c47295a`, `source_dirty: true`, source fingerprint `7F6169158690AB568D4F75350269EC0D7FABE59A2EB394EB5C1C2BACCB28EF6E`, and executable SHA-256 `676BDE69AFC2F2247E16797EFFA1340DF427E6652D484FB8316113A88F519238`. This packet is evidence for that recorded staged build, not a clean-tree or future-build assertion.
- Reviewed existing `.omc/research/qa_packet_lib.py` decoding and finite patterns and independently recomputed the published scan: **139 text payloads**, four patterns, zero matches, exact encoding/count agreement. The only unscanned text payload is the scan report itself; PNGs are outside this text scan. UTF-8 BOM and UTF-16 BOM are decoded explicitly and strictly; the UTF-16 relative-stop refusal was included. No replacement-character payloads were found. This finite scan is not a general proof that arbitrary secrets cannot exist.

## Panel restoration and selection

Recomputed logical panel widths from each raw state's physical viewport/window geometry divided by pixels-per-point:

| State | Left | Right | Scale |
|---|---:|---:|---:|
| Initial | 220 | 320 | 1.0 |
| Intermediate scale | 220 | 320 | 1.25 |
| 1024-wide constrained window | 161.59375 | 120 | 2.0 |
| Restored default window | 220 | 320 | 1.0 |
| Custom left drag | 280 | 320 | 1.0 |
| Custom right drag | 280 | 380 | 1.0 |
| Custom constrained window | 161.59375 | 120 | 2.0 |
| Custom restored window | 280 | 380 | 1.0 |
| Selected node before divider drags | 220 | 320 | 1.0 |
| Selected node after divider drags | 250 | 350 | 1.0 |
| Defaults restored later | 220 | 320 | 1.0 |

The initial camera derivation is honestly retained as `custom-panel-resize-comparison-initial-invalid.json`, SHA-256 `cfe06df87418d4a31c25b4e87832c9bf1adfcdbceb444739c35ac58c26cfb214`. Its missing top-level field comparisons were invalid. The corrected report identifies `camera.pose`; I independently required the actual `position`, `target`, `orbit`, `up`, and `fov_deg` keys and compared their real values across before/after drags/restoration. All five match exactly. The selected-node camera comparison independently passes the same required-field check.

The selected node remains `01M6KKZHWBGYT7PY0QX6VZNZ9R`; the whole selection object, including position `[-2, 0, 2]`, quaternion `[0, 0, 0, 1]`, and scale `[1, 1, 1]`, matches exactly before and after divider drags. The selected screenshot shows Lucky Duck still selected and the Inspector still displaying that node. This evidence covers the recorded single-node selection and divider sequence, not arbitrary selection groups or all tools.

## Volume transition and history

The actual partial report UI and screenshot explicitly state incomplete coverage and **5402 of 16384** required terrain points. The raw node has only `material: qa-loam`, `node_kind: earthwork_region`, and `region_id: r1`; neither persisted volume property is present. The raw history is version **4**. Unavailable volume is withheld rather than stored as zero in this observed case.

The configured rectangle uses radius 800 and delta 0. The subsequent single stable-scope API shrink retains region `r1`, material `qa-loam`, operation `raise`, and delta 0, changing its footprint to a centered 10 by 10 metre square. The readback matches the request. The completed screenshot and UI show area **100 m²**, **16384/16384** samples, and zero selected and combined cut/fill/net, with material displayed. This is a genuine completed zero case with a zero-delta proposal, not a nonzero numerical oracle.

Independently compared partial and complete node/history payloads:

- History entries at versions **1–4 are exactly unchanged** as parsed entries, including timestamps, source DID, operation data, and after-state.
- The only two new history entries are `cut_volume_m3 = 0.0` at version **5** and `fill_volume_m3 = 0.0` at version **6**.
- Both entries carry atomic save `earthwork-volume-save-01M6KNXBE4Q0A2PPDGMTQXNQW5`, `first_version: 5`, `committed_version: 6`; version 5 is marked `inside_atomic_batch` and version 6 `available`.
- Their recorded source DID equals the original creation source DID. This is receipt consistency for this fresh owned case, not a queued caller-authority, remote actor, or revocation proof.
- The node envelope, all other node data, and all pre-existing properties remain unchanged. No transform or unrelated property was added by this measured save.
- Raw history bytes for the completed, before-navigation, after-up-command, after-back-command, and final captures all have SHA-256 `589d229d73f9acc1388d31f2da2f0e2c3f7152518a7d2f601584274293c1ef86`; each remains version 6. The corresponding captured node data also remain equal.

Before placing the proposal, the three raw terrain captures before configuration, after configuration, and after tool options are byte-identical, SHA-256 `4e5e7a15eca658d6577dba2c0710bd7e3e6507427c1c4dcbc04967b9c63273b7`. This supports the recorded configuration-only path making no terrain write; it does not cover all QA0024 variants.

### Required navigation claim correction

`owner-zero-nav-up.jsonl` records a successful one-command click on **Open fractal QA Fractal**, followed by `owner-zero-nav-back-petal.jsonl` clicking **Open petal Petal A**. However, **`owner-zero-after-up-state.jsonl` still reports active Petal A**, with the same verse/fractal/petal IDs. There is no retained intermediate snapshot proving `petal_id: null` or a different active petal.

Therefore accept **navigation control invocation plus exact history/node stability across the recorded commands** only. Do not call this an observed petal departure/return, scene retirement/respawn, or navigation lifecycle persistence proof. Parent acknowledged this gap and will preserve the frozen packet while correcting Conductor claims through a review addendum; a future run should use actual Back and verify the intermediate inactive-petal state. No new capture was invented or requested during this review.

## All actual images inspected

I opened and visually inspected each original PNG individually, not a generated contact sheet:

| PNG | Actual observation |
|---|---|
| `initial.png` | Default hierarchy and Inspector widths; Genesis Verse browser card. |
| `scale200.png` | Constrained 200% UI, compact top bar, narrower panels and wrapped Inspector content. |
| `restored.png` | Default geometry returns at the original window size/scale. |
| `custom-restored.png` | Wider user-selected hierarchy and Inspector widths return. |
| `selected-after-dividers.png` | Lucky Duck selected in tree; corresponding node ID and transform visible in Inspector. |
| `petal-a-after-timeout.png` | Petal A active after the timed-out command; scene visible, no object selected. |
| `brush-options.png` | Options button is focused but Inspector remains; not proof the options panel opened. |
| `shown-brush-options.png` | Options panel actually visible, Brush configuration and shape controls. |
| `configured-rectangle.png` | Rectangle selected, radius 800, delta 0, material qa-loam; placement control visible. |
| `after-zero-placement.png` | Earthwork node appears in tree; proposal representation is visible. |
| `volume-report-initial.png` | Report panel visible before explicit target selection; not yet a computed volume. |
| `region-r1-report.png` | Selected r1 reports incomplete coverage, sampled 5402/16384, no volume values. |
| `r1-complete.png` | Selected/combined zero values, material, 100 m² and all 16384 samples visible. |
| `zero-final.png` | Final complete zero report retained; poor active-petal breadcrumb contrast persists. |

The completed report's object section says **0 overlaps/candidates; 3 objects checked; 0 unresolved**, and excludes two non-geometric nodes. That is not a positive intersection or material-intersection acceptance case. The final active-petal breadcrumb is difficult to read against the scene. Layout captures also retain two measured small-hit-target failures for the 10-pixel panel dividers and explicitly incomplete instrumentation coverage. These findings remain open; the isolated later divider source is not tested by this packet.

## Logs, failures, and cleanup

Independently read the packet log summaries and matching retained original stdout/stderr files as ordinary files, without any process inspection. Original stdout exactly matches its recorded **310394 bytes** and SHA-256 `8542f01c9339e1be5c7b330dcd420942b759b91cbc597e1635717ae8acff4241`: **2089 INFO, 25 WARN, 1 ERROR**, zero panic markers, zero matches for the finite credential patterns. Stderr is empty with the standard empty-file hash.

The one ERROR at **2026-11-04T16:08:40.035209Z** is `tower_http::trace::on_failure`, HTTP 500, latency 4690 ms. It matches the retained Petal A command's `input_timed_out`, status 500, reporting all 3 of 3 input frames already applied and a 4500 ms batch deadline. The following recorded state and screenshot show the applied Petal A state; no retry of that timed-out navigation command is presented as successful acceptance. The withheld scoped error text remains withheld. This is not an error-free run.

`logs/stage.log` records successful staging in 327.5 seconds with **one compiler warning diagnostic**, unused `mut` at `fe-ui/src/ui_shell/layout_policy.rs:207`. The warning summary is not a second diagnostic. Do not describe this build as warning-free. The wrong-route/missing-kind and relative-stop-path refusals are retained failures, not accepted actions.

The successful absolute-path stop log is retained. `cleanup-identities.json` records absence of all **8 exact previously recorded identities**: the staged engine, conhost, and six WebView descendants. I compared the entire ordered identity tuples with `owner-context.json`; they match exactly and all recorded checks report both PID and identity absent. This accepts the recorded cleanup receipt, not a fresh independent OS check or a universal statement about unrecorded descendants.

## Acceptance boundary

No blocker was found to retaining this packet with the navigation correction. It gives no FULL suite/catalog coverage, native UIA or WGC capture credit, independent capture freshness, queued grant/ancestry/revocation acceptance, rollback/race/terminal-reset/database-join proof, retained-store restart, nonzero numerical oracle, positive object overlap/intersection, exact mesh intersection, paired peer, release/CI, or overall project completion credit. QA-0070 and all broader gates remain open. No apps, API calls, DB operations, network, builds, tests, process operations, source edits, or Git mutations were performed for this review; only this review artifact was authored.
