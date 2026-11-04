---
type: Review
title: qa43-full-evidence-review
date: 2026-11-04
status: approved
---

# QA43 full frozen evidence review

**APPROVED as an accurately bounded failed-native packet: `NATIVE_OBSERVATION_REFUSED_BEFORE_UIA_ROOT`.** Manifest SHA-256 `c2dee8722f1f5cf9f19d961f230d05a225ca551c8fd2282fd9aca5a587994639` identifies 12 payloads totaling 21,688 bytes, plus the manifest. Every file size/hash, all three original-artifact anchors and all six source-copy mappings independently match. No packet correction or external erratum is required by this review. Approval establishes evidence integrity and the recorded refusal, not positive native/UIA behavior or campaign completion.

Only this review was authored. All packet/evidence/source inspection was read-only; no reviewed implementation/parser execution, compiler/test/build, app/helper, native/OS/process/input/capture, API/DB/network or Git mutation occurred. JSON/log/byte/hash/finite-regex processing was inspection of retained evidence, not a rerun of the QA builder or implementation. No packet byte or concurrent QA44 process/source work was changed.

## Provenance and packet integrity

The packet binds run `.qa/run/20261104-151750-618` to stage `.qa/stage/925d7cba-20261104150814356`. The copied stage manifest records source commit `925d7cbaddcdbd9996fc32d10078326cae0aaaa3`, source fingerprint `D10FCBDC9D20E4B54ECB849F0CBBDA44BCB5BD8C9B08C380782708B00C8B1325`, and **`source_dirty: true`**. This is a fingerprinted staged working source, not proof of a clean commit-only build. The owned-context stage object agrees with the copied manifest. Native helper SHA-256 `633dd7e67f2dc49bbf91a76aac4d8454f6d9ec7cf9ad61edd357173af95ca04f` agrees between stage/context and worker reservation.

The manifest file set exactly matches the actual 12 payload files; there are no duplicate paths, undeclared packet files or images. Independently checked raw-copy mappings are the owned-context record, cleanup record, UTF-16 launch/stop logs, stage manifest and original run-relative worker receipt. All six are byte-identical to their retained source files. The builder `.omc/research/build_qa43_evidence.py` was read, not executed; inspected bytes hash to `bf0b5343bacdf5cac4a6cd61d5dba89eb03fd1c3b6f7ff027452598d08bb868a`. Its copy/anchor/summary/scan/manifest ordering agrees with the packet. No private token/current file is copied into it.

| Original retained artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `owner-native-observe-first.jsonl` | 581 | `4e395ba798f638f8bd56e8231f05d4a8e5faed75c47944c9b6d4aa01b84753cd` |
| `owner-separate-owner-state.jsonl` | 124 | `960f87469eb9d455217c7aedb7b5cb05e82acc716584660fde42317c37b994bf` |
| `fixture-public-detail.json` | 2020 | `e3a30e3bcaa7defddc2dec7d86a38243a9cadefc08978612de5ccbd37f6d8352` |

The fixture public detail and launch log identify the deterministic basic terrain fixture and three values summing to 60. They establish launch/fixture context only; no terrain operation, numeric result, post-navigation state or data transformation was exercised in this packet.

## Actual native refusal and independent Owner refusal

The original native output is exactly `window_ambiguous`, helper exit code 2, `attempt_spent: false`, `invoke_entered: false`, `method_returned: false`, `capture_available: false`, and `outcome: observation`. Root and target hashes are all-zero placeholders and both watcher sequence values are zero. `hr: 00000000` is the unchanged receipt field before a method result; it is not evidence of a successful Invoke or provider call.

The worker receipt contains exactly three records: `reserved`, `worker_started`, `exit_confirmed`, with one operation ID. The started and terminal worker identities are identical: PID 119164, start ticks `639294275278548362`, staged `native_observer.exe` path. Reservation binds the same staged FE app identity as owned-context. The terminal lifecycle code is **`worker_exited`**, with **exit code 2**; this confirms recorded worker termination, not successful observation. Worker receipt SHA-256 is `a37413906ecf4097a3cca85d141cd5316c887d5c4ff4caa64e3f342b5fa080ed` (1502 bytes).

The private retained attempt ledger independently matches summary SHA-256 `4c3f3897bd00f19108b3a3efcd4cdf4cabae17d746933a772e6ee60ec849ab1d` and contains exactly one bootstrap record. No invocation reservation or intent record is present. Its raw contents remain unpublished. A worker lifecycle reservation for an observe operation must not be confused with a spent invocation slot.

Read-only source at the recorded source commit puts the generic `window_ambiguous` predicate in `discover_window` before `window_current` and before COM/UIA creation or `ElementFromHandle` (`worker.cpp:230-248`, `597-614`). Therefore this output supports refusal before a UIA root is acquired. That generic predicate covers incomplete enumeration, callback refusal and zero/multiple candidates; the exact cause remains unknown in QA43. Later diagnostic source changes earn no retrospective cause or fix credit here.

The separate Owner response remains **HTTP 403 `qa_gate_refused`**, with `refused: true` and the explicit diagnostics-disabled message. It is not successful state access and must not be recast as native authority. The packet records that the sequence stopped before its planned Owner screenshot. There is no image or capture artifact to inspect or credit. Nothing in this review claims visual, UIA, navigation, role or rendered-state acceptance from those refusals.

## Cleanup and log summaries

`cleanup-identities.json` records the check at **2026-11-04T22:23:04.3185465Z**. Its eight exact identities match the owned-context root plus seven recorded descendants, preserving PID, start ticks and executable path. Each has both `pid_present: false` and `recorded_identity_present: false`; the aggregate absent flag is true. This is historical evidence of those recorded identities being absent at that check, not a fresh process query or a claim about every machine process. The native worker's separately retained terminal receipt supplies its exit confirmation.

Launch and stop logs decode strictly from UTF-16 BOM and match their raw originals. Launch reports 12.6 seconds, root PID 97060 and the stated run; stop retains that same run. Both local application logs independently match the published sizes/hashes and summaries:

| Local log | Bytes | Lines | INFO | WARN | ERROR | Panic markers |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| stdout | 57,753 | 453 | 366 | 20 | 0 | 0 |
| stderr | 0 | 0 | 0 | 0 | 0 | 0 |

stdout SHA-256 is `5338ce4df16c7b4326a8e21027ce0cccbe5a8c2002f4f9da52025f14a251cee2`; stderr is the standard empty-file hash. Recorded compiler warning/error and private-current-error marker counts are zero under the summary's stated regexes. This is not a warning-free or final compiler-sweep claim: 20 application WARN records remain, and application logs are local with summaries published.

## Finite credential inspection and limits

Every one of the 13 actual packet files strictly decodes with no NUL characters: nine UTF-8, two UTF-8 BOM and two UTF-16 BOM. The frozen scan records 11 pre-scan files, correctly excluding its own later output and the later manifest. Independent inspection also scanned those two final files. All four documented finite patterns return zero matches across all 13 files and both retained application logs; the per-file encodings/counts agree with the frozen scan. `universal_absence_claim: false` correctly limits this result. No universal secret-absence or credential-security acceptance is implied.

QA43 supplies failed discovery evidence, actual worker launch/terminal receipt, a preserved independent Owner refusal and recorded cleanup. It supplies **no** Invoke, positive native root/provider/navigation observation, selected-Inspector capture, WGC freshness, role/peer/restart/pruning acceptance, source-fix closure, numeric measurement, FULL campaign, integrated sweep, CI or release credit. No existing finding is closed by this review. Future matching-source evidence must establish those outcomes separately.
