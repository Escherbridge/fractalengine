# Independent QA33 full packet review

Reviewer: `/root/modifier_chord_review`, 2026-11-03.

**Approved QA33's bounded executed findings**, against packet-manifest SHA256 **`26f0f1ba5068ea04e1b10996b8160bfa87027f0d0c8cb14345cd596a1973d095`**. The packet supports the focused Owner pointer-selection repair and the observed raise-history diagnosis. It does not verify a volume fix, A1/A2/native UIA, fixture/adoption, peer GPU rendering/convergence, the full application campaign, final tests, CI, or releases.

This review is independent saved-evidence verification. The builder and packet library were read, not executed or imported. Packet files, canonical findings, source and other agents' work were not edited. No application/API/DB, input, process/OS inventory/control, Cargo/tests or network operations were performed. Raw local logs were inspected through hashes, strict decoding, bounded counts and finite scans; no token file or raw credential value was read or published.

## Packet integrity and provenance

All **83 payloads** match their recorded byte lengths and SHA256 hashes, with unique names, no missing/extra files, and **84 total files including the manifest**. All **80 JSON/JSONL files, containing 87 top-level records**, parse. The declared 53 original root captures are accompanied by exact prior-response/source-report copies and bounded derived reports; they remain covered by this frozen manifest. The two copied PNGs are byte-identical to their recorded original files and both have 1280×720 dimensions. Both were visually inspected.

Context/verdict and the saved start receipt agree on Owner PID **91876**, start ticks **639293055444599545**, and the executable in stage **24b5660c-20261103052222933**. The sealed executable's actual SHA256 matches `C418DC191184D9CD85C5DDBABAB0E7C679A38983A21CF062DD48061B840691A7`. The previous-start receipt matches QA32's exact Owner identity. Retained root, no reseed, unchanged port, no stale-stage override, and stage/source agreement are recorded consistently. **source_dirty=true** is retained; this is not a clean-checkout claim or a reconstruction of the later checkout's fingerprint.

The build records/logs retain **189.2 seconds** overall staging and **20.2 seconds** launch. These are recorded operation timings, not a new benchmark; the Cargo completion lines have their own narrower timings. The retry log contains successful dev-profile completions with zero compiler warning/error diagnostics. The earlier shell-capture attempt is explicitly recorded as failed with no stage/app and compiler outcome not established. Its partial UTF-16 log contains no compiler error marker, which alone proves neither compiler success nor a successful stage. This review does not turn that partial file into a Rust success claim.

Later A1/fixture integrations and their compiler correction were not in this executed 24b stage.

## Hierarchy scroll and selection

I independently compared the actual UI records rather than accepting comparison-oracle booleans. The six pre-drag cuts—initial scrolled layout, plain Portal selection, plain Creation selection, Ctrl addition, Shift range, and immediately before drag—have **identical complete Hierarchy scroll metadata** and all **28 row IDs, names, order and bounds**. Offset y remains exactly **189.8050994873047**, content ID `1649309098613198024`, parent ID `15076918100674134496`, content size 220×1619. The four affected row y coordinates remain **511, 542, 573, 604**. No intermediate re-scroll is requested by these selection command records.

The recorded plain clicks are at (144,523) and (144,554). The first selects QA Portal 18; the second replaces it with QA Scoped Creation 21. Ctrl+click at (144,523) produces exactly those two selected accessible rows, a right-panel count of **2**, and **QA Portal 18** as the primary. Shift+click at (144,616) produces the contiguous four affected selected rows, a right-panel count of **4**, and **QA Asset Placement TRS 21** as primary. Corresponding native primary ECS state IDs agree. This establishes the sampled presentation/primary workflow, not an independent ECS census of every selected member or coverage of every alias/range case.

The single background drag is explicitly (16,580) -> (16,570), with 35 wait frames. Afterward the offset is **221.81521606445312**, a settled delta of **32.01011657714844**; the same four rows remain selected and the same primary is retained. Identity/order remain stable. This supports working declared background drag without the previous reset, but **does not prove an exact ten-pixel scroll displacement or zero press-frame delta**. Settled inertial behavior and the source journey's separately authored motion-frame assertion must not be conflated.

The `scroll-retained.png` screenshot visibly retains the selected Creation row in the scrolled hierarchy, with its normal Inspector, matching node ID and local-byte availability presentation. The `four-after-drag.png` screenshot visibly shows four highlighted hierarchy rows, the stronger primary outline, **4 selected objects**, the full primary name and mixed transform values in the right panel. It also shows rendered Owner geometry. Neither image proves peer GLB provenance/delivery/rendering or an external WGC/native-provider path.

## Stored data and raise history

All **20 complete responses for 10 nodes** match QA32 final responses byte-for-byte. Each also matches its exact copied restart-origin baseline and the recorded SHA256. The Group registry matches QA32 and its copied baseline exactly. There are **21 baseline copies**. These are actual whole-response comparisons, not selected-field equality.

The initial/final raise tail responses are byte-identical. The full history contains **106 sequential entry versions**, has current version **106**, no further page, and coverage start **34**. Versions **1–33 are explicitly `unavailable_interval`**, not reconstructible history. Version 34 is the baseline. Tail versions 101–106 exactly match the corresponding full-response entries, and both responses report current-state verification. QA32's copied final history already reports current version 106 while its first page ends at version 100. Thus the tail exposes previously uninspected facts; it does not demonstrate that QA33 startup created them.

The durable entries are:

| Version | UTC record time | Ordinary property fact | Resulting cut/fill pair |
| --- | --- | --- | --- |
| 103 | 2026-11-03 11:39:47.1050875 | cut_volume_m3 = 0 | 0 / 38.83056640625 |
| 104 | 2026-11-03 11:39:47.1808408 | fill_volume_m3 = 0 | 0 / 0 |
| 105 | 2026-11-03 11:39:48.0816131 | cut_volume_m3 = 0 | 0 / 0 |
| 106 | 2026-11-03 11:39:48.1487991 | fill_volume_m3 = 38.83056640625 | 0 / 38.83056640625 |

The saved payloads support these facts and resulting pairs; the times precede this QA33 process. They fit the previous navigation interval, but no height-sample coverage census at those historical frames is present. I checked the named 24b production seams: the integrator skips cells without both samples and returns numeric zero; reports can publish without complete region coverage; the UI maps by region and sends both properties independently when the pair changes, then advances its cache on channel acceptance. These source facts support the proposed mechanism and the investigation's limitations. They do **not** prove exact missing coverage at those past frames, corruption, a foreign write, or a verified repair. The source investigation is an exact copy of its retained original; `volume_fix_verified` remains false.

## Settled states, logs and cleanup

All **seven** saved states report `ui_known=0` and `settled=true` under the explicitly limited definition. DB/sync completion remains unknown. The terrain diagnostic's request-time `scene_petal_mismatch` is not evidence reconstructing historical sample coverage or stale rendering.

I independently read and strictly decoded all six referenced local logs, matched each byte length/SHA256 and encoding, and reproduced the reported counts. The shell-failure log uses UTF-16 BOM; no lossy decoding or NUL-tolerant fallback was used. Final stdout has **598 lines, 519 INFO, 12 WARN, 0 ERROR**, zero panic markers and zero named private-current transaction-error markers. This is not a warning-free runtime claim. Stage-retry compiler warning/error diagnostics are zero.

Finite scans over packet text and referenced logs find zero bearer-value, JWT-value, private-key-block, credential-assignment and invitation-URI matches. These are bounded pattern results, not exhaustive secret detection. Raw logs remain local; no token files were read.

The saved pre-stop census and end checks have the same exact root plus seven descendant identities as the context. All **eight** end entries report `captured_identity_alive=false`. This verifies the recorded cleanup evidence, not a fresh or machine-wide process census.

The focused pointer finding can be reconciled from these reviewed observations within this scenario. The volume churn finding remains open pending separately reviewed source and runtime proof. Nothing in this packet closes native observer, adoption, peer selected-Inspector/GPU, broader campaign, final-check, CI or tagged-release gates.
