# QA34 and QA35: independent full focused-packet evidence review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED as focused evidence with open visual findings.** No blocking evidence or accounting defect was found in these frozen packets. This verdict does not supply full-coverage/gold/convergence credit, close VIS-002/VIS-003, or establish adoption, peer rendering, normal UIA/WGC, final tests, CI or release acceptance.

## Exact reviewed anchors

| Packet | Reviewed packet-manifest SHA-256 | Payloads / total files | Original root inputs | PNGs |
| --- | --- | --- | --- | --- |
| QA34 | `bcb12de4995097144f0d73f0e905df42708368587b368ae702e132fce4d2eda5` | 39 / 40 | 23 | 5 |
| QA35 | `e2140e1c91f988e7d201f8cbabd5083e5b98118c09fcecfeb359aed5e291d2e1` | 45 / 46 | 29 | 5 |

Independently enumerated every packet file, matched the manifest file sets, sizes and hashes, and rechecked every original-root hash. All ten PNGs match their recorded run originals byte-for-byte and their PNG dimensions agree with the capture receipts. Public contexts and the shared cleanup copy also match their retained source files exactly. Safe stage/launch/stop logs are exact source copies. Read the packet builder and packet library to assess provenance/accounting rather than treating generation as independent verification.

The dirty root `.gitattributes` already contains directory-specific `-text` rules for both new packets. Parent publication must still verify staged Git blobs against the reviewed raw bytes. Derived review/bookkeeping updates require a regenerated manifest and an explicit anchor back to these reviewed manifests; immutable captures must remain unchanged.

## Build and run provenance

Both contexts name stage `9cdd3132-20261104072325049`, source `9cdd31325b83f5870bad78d7c8d949ff40f7bdd2`, and the same source fingerprint and executable hash. Context manifests equal the copied and actual stage manifest. Independently hashed the staged executable; it matches `27261FA333F1CE8944D8AF470F59F93814B87BDF4809138FD961C1D0ACF20299`. `stage_matches_source=true` and `source_dirty=true` are explicit, without a clean-checkout claim.

The actual build log records successful production staging in 223.8 seconds with zero compiler warning/error diagnostics. QA34 launches root `20261104-072401-246`, PID 108820, in 14.4 seconds with preparation-only legacy input. QA35 launches separate root `20261104-073857-380`, PID 116612, in 9.3 seconds with fixture none. These are separate fresh runs with distinct Genesis IDs, not a retained-store restart or adoption proof. No campaign-end test suite was executed by this packaging/review lane.

The shared cleanup capture contains exactly 16 identity records, eight for each run. Each run's root plus seven captured descendants maps exactly by PID, start ticks and executable to its public context; every `original_identity_present` is false. A reused PID is correctly distinguished by different start ticks. Both copied stop logs record Stopped. This supports absence of those exact captured identities, not an unlimited descendant census or name-based process claim. No OS query was performed by this reviewer.

## Actual images and focused positive scope

Opened and inspected every packet PNG:

- QA34: `initial.png`, `verse-1024.png`, `verse-1600.png`, `petals-1024.png`, `petals-1600.png`.
- QA35: `scale125.png`, `scale125-1024.png`, `scale200-1024.png`, `options-card.png`, `create-card.png`.

QA34 visibly supports readable horizontal `1 petal` counts in the Verse-to-Fractals cards at 1280x720, 1024x720 and 1600x900, and readable `3 nodes` counts in Fractal-to-Petals cards at 1024x720 and 1600x900. Accessible samples independently report the corresponding label/textrun geometry, with count labels on their own row. QA35 supports the `1 petal` Fractal card at application scale 1.25 in 1280 and 1024 widths and at scale 2 in 1024 width. It does not establish `3 nodes` Petal-card scaling.

VIS-002 remains visible: world-grid lines cross unbacked title/breadcrumb text. VIS-003 remains visible: high-scale panel constraints clip tree controls and leave much narrower panel allocation when ordinary scale/window size returns. The Verse header is cramped after restoration. Readability of the small card count does not imply the surrounding high-scale layout is acceptable.

Independent state arithmetic confirms QA35 left/right physical widths of 275/400 at 1280 scale 1.25, 275/288.21875 at 1024 scale 1.25, 323.1875/240 at 1024 scale 2, and 161.59375/264 after returning to 1280 scale 1. The left logical width therefore changes from 220 to 161.59375 and remains there on restoration. These are actual session geometry observations. The separate `qa-ui-scale-panel-width-investigation.md` records the source-supported PanelState explanation and an unimplemented proposed ownership seam; it is not cross-restart persistence or a verified fix. Horizontally overflowing tree controls have not been shown revealed and invoked by this packet.

Every layout report has zero measured violations and `coverage_complete=false`. QA34 reports 12 or 13 unknown entries per saved layout, QA35 reports 12, 12 and 21. Every report has zero observed text measurements. The 2x report includes uninstrumented clipping/scroll/overlap cases. The packet correctly avoids turning these incomplete geometry checks into visual acceptance or all-controls reachability. Application scale testing is not physical monitor-DPI evidence.

## Actual actions, refusals and state retention

QA34's broad `Open fractal Genesis Fractal` locator is ambiguous between tree and card. Its refusal record has status 0 and no commands; the later exact card locator is a separately recorded applied click. Subsequent state and screenshots show the Genesis Fractal Petal list. The current-Verse breadcrumb click is also recorded applied. Browse-All returns 404 and the next state query refuses `no_active_verse_covered`; this is an observation gap after the attempt, not a successful All-Verses state or zero-effect proof. No retry or successful All-Verses claim appears in the packet.

QA35 records actual card Options open and dialog Close, then New Fractal open and Cancel. PNGs show the settings and blank Create dialog; final state/tree has only Inspector open. The Close locator explicitly has `bounds_window_fallback_clip_unknown`, so this is not a universal observed-clip assertion. The actual Create button remains disabled with an empty name; no Create submission is claimed.

Independently compared complete hierarchy response bytes before and after Create/Cancel. They are identical, hash `095cc1f2ebe57590053b317e19ce4cad492bfcc4453872a4ddaba3ca84ad04b8`, and parse to one Verse, one Fractal, one Petal and three nodes. This supports preserving that full response through the sampled Cancel flow; it is not proof of unrelated history/storage scopes or a submitted create/delete transaction.

## Strict log and finite credential accounting

Independently decoded and recounted all five referenced logs for each packet, including local unpublished stdout/stderr. All ten are strict UTF-8, with no BOM fallback, replacement decoding or NUL. Source bytes, hashes, line counts, ANSI-stripped severity counts, panic/compiler/private-current markers and recorded finite credential regex counts match the summaries.

| Packet | Stdout lines | INFO | WARN | ERROR | Panic / private-current markers |
| --- | ---: | ---: | ---: | ---: | ---: |
| QA34 | 710 | 634 | 9 | 0 | 0 / 0 |
| QA35 | 441 | 365 | 9 | 0 | 0 / 0 |

Both stderr files are empty. Finite Bearer/JWT/private-key/credential-assignment counts are zero across the referenced logs and original-root files. Additionally scanned every non-PNG payload in both manifests: 34 QA34 text payloads and 40 QA35 text payloads decode strictly and have zero matches under the recorded finite patterns. These checks do not establish exhaustive secret detection. No token/private credential file was read or copied by the review.

## Remaining limits

The evidence does not verify All-Verses cards, all hierarchy levels, large counts, long names, extreme narrow layouts, every scaled Petal card, horizontal-scroll action recovery, actual OS-DPI behavior, adoption, peer/native rendering or restart. Source/integration approvals for later fixes do not retroactively change these images. The packet appropriately keeps full gold/catalog/convergence, final integrated checks, exact-SHA CI and tagged release gates open.

No applications, OS/process inspection, input, API/DB/store access, network requests, Cargo/compiler/tests, production edits or Git mutations were performed by this reviewer. Work consisted of read-only saved-evidence/source inspection, actual PNG viewing, independent hash/JSON/count/geometry checks and this review artifact.
