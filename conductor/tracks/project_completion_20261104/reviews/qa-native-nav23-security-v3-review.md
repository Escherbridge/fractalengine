---
type: Review
status: source_approved
---

# NAV23 v3: complete independent security/correctness review

**Verdict: APPROVED AT SOURCE for this exact frozen 28-file v3 slice, within its documented local ownership model and remaining execution gates.** NAV23-V2-S1 is repaired: confirmed historical receipts no longer depend on intentionally prunable stage files, while unconfirmed/unknown/reapable histories retain the actual stage and process requirements. No new material blocker was found. This is not integration, parsing/compilation, passing fixtures, native/lifecycle/crash, authority, UI/DB/render, capture, FULL-pass, CI or release acceptance. Both required full reviews must approve before root integrates or executes.

Reviewer `/root/legacy_helper_io_review`, 2026-11-04. The isolated checkout remains HEAD **`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`**. Reviewed the complete v3 slice and focused four-file delta, retaining the complete v1/v2 invariant reviews. Applied the approved helper plan `732571e2a04d3d33bdbcf31eac8e5bd232121c20b5611c99f3c061f8e2256b94`, approved identity/native v3 boundaries and shared UI/UX guidance.

Only this report was authored. No production/source, packet, index/history, compiler/parser/formatter/test, app/helper, process, native/OS API, input/capture, HTTP/database/network operation or Git mutation occurred. Read-only source/Git/document/hash inspection only. Main Rust source is still d8; later main Owner-before-provider and other host changes must survive eventual tooling integration.

## Exact frozen identity and complete scope

| Artifact under the isolated checkout's `.omc/research/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `qa-native-navigation-nav23-v3-source-report.md` | 10118 | `f6672d25dc8a95ae8a33656a7f90880abcea91cbeb4ac313c6c7409c13b2315e` |
| `qa-native-navigation-nav23-v3-hashes.json` | 7616 | `d5dcbd663d02913d01ab5528ffcb1265a6ab2ace0d5a768e6199db6af86420fa` |
| `qa-native-navigation-nav23-v3-source.diff` | 212946 | `45c40eda6e6b2e9ba989121ff7af98d5a9db0e4cfb06c34f2dbca2f8a9156744` |
| `qa-native-navigation-nav23-v3-history.delta.diff` | 22630 | `fb193841726105f64e99d7beaee3e4585cb949ba51e14936c0d233006515e45c` |

All **28** current raw sizes/hashes match the manifest and all v3 raw snapshot copies. Exactly **four** paths differ from preserved v2: `_native_observer.ps1`, `native_observer/lifecycle_scenario.ps1`, and the two QA AGENTS guides. The other **24 files are raw-identical to the complete reviewed v2 slice**. All 28 v2 snapshot hashes and its patch/delta/manifest/report still match their original anchors. Read-only whitespace inspection passes. No application/runtime/database/renderer/pointer/physical source is changed by this tooling slice.

Complete scope: `.gitignore`; `tools/qa/{_lib.ps1,_native_identity.cs,_native_observer.ps1,_native_pair.ps1,AGENTS.md,clean.ps1,fe.py,launch.ps1,pair.ps1,restart.ps1,stage.ps1,stop.ps1}`; `tools/qa/scenarios/evidence-boundaries.py`; and `tools/qa/native_observer/{__init__.py,AGENTS.md,build.ps1,contract.py,controller.py,history.py,ledger.py,lifecycle_scenario.ps1,limits.json,scenario.py,windows.py,worker.cpp,worker.manifest,worker_protocol.hpp}`. This is complete-slice review, not just a helper-function mirror or the earlier narrow draft critique.

## Confirmed history: source repair accepted

`Get-QaNativeWorkerRecords` now performs receipt shape/order/identity and exact saved Run provenance checks before deciding whether a materialized stage is needed.

- The prefix must be the reserved record with exact known keys/schema/operation and the expected receipt filename. Controller and app identities use the shared exact-shape validator.
- Current/archived start must identify this actual Run, input-off mode, matching app PID/birth/exe, recorded stage path, supported stage kind and well-formed app SHA. Recorded native helper schema/file/hash must match the receipt. Bootstrap records additionally match run/ledger/file ID and canonical ledger path; observe-only history retains its app-derived run ID/no-bootstrap constraints. A foreign Run or unmodified copied foreign subtree cannot supply this ownership.
- Every worker identity present must have exact identity fields and valid types, use the staged helper path, and be born no earlier than app/controller. Terminal code shape and signed/unsigned Windows exit-code range are checked.
- Only the **three-phase reserved -> worker_started -> exit_confirmed** record sequence, with validated matching original and terminal worker identity, becomes confirmed. A repeated/reordered terminal, changed identity, lone terminal or null identity cannot establish this confirmation.
- A valid two-line early failure remains unconfirmed even if its terminal says exit_confirmed: without the durable worker_started record it does not grant confirmed absence. Lifetime_unknown remains unconfirmed. There is no fallback based on missing files.

Only after that classification does an unconfirmed record call real `Get-QaStageManifest` and require full actual manifest equality with the recorded start provenance. Confirmed historical records need the retained metadata, not the deleted stage.json/app/helper bytes. `Resolve-QaChildPath` still confines the helper path and rejects redirected surviving ancestors, but does not require an intentionally missing stage descendant to exist. Therefore it does not silently reintroduce the fixed existence dependency.

`Stop-QaNativeObserverWorkers` remains unchanged: confirmed records are skipped; unknown identity remains unknown; a live/unverifiable controller refuses; reapable records require real helper pin/hash and exact worker PID/birth/exe followed by owned exit. `Get-QaNativeRetainedStages` and clean remain unchanged, retaining the real dependencies of unconfirmed histories while allowing obsolete confirmed-only stages and otherwise dead runs to be pruned. No receipt is deleted, no missing image becomes exit evidence, no spent attempt is reset.

The v2 counterexample is consequently closed at source: observe/confirm A -> supported unpaired restart B -> production reference decision allows old A pruning -> retained confirmed A receipt can still be parsed and skipped while B remains live. An unknown A receipt cannot take that path; absent A refuses before reaping. Actual execution of that complete lifecycle sequence remains mandatory.

## Authored regression uses the real dependency

The existing lifecycle support no longer overrides `Get-QaStageManifest`. `New-ScenarioStage` creates real scratch stage.json records and dummy image bytes with matching hashes for the actual production reader; nothing is launched.

The added sequence builds a properly bound confirmed A receipt, archives A's start, models a new current B incarnation/stage, and uses production `Get-QaNativeRetainedStages` first with unknown then confirmed history. It confines the resolved absolute scratch deletion target before removing A, then calls complete production record parsing and native-helper stop. Its process seam must remain untouched for the already-confirmed helper.

Absent-stage unstarted/reapable/lifetime-unknown/lone-terminal histories refuse; duplicated terminal, mismatched identity and foreign archived Run also refuse. Existing history-cap and ordinary/native paired transition/issuer cases remain. Only unrelated process/native-bootstrap authority seams are controlled. The scratch fixture is meaningful dependency coverage, not actual process, native identity, filesystem crash-durability or application upgrade proof. It remains authored and unexecuted; no new registered test declaration is added.

## Complete v2/v1 guarantees retained

The other 24 files are exact v2 bytes, and the cleanup delta introduces no alternate action/authority path:

1. **Native recipient entry:** native observe/invoke dispatch remains before ordinary run loader, token/log selection and HTTP. Explicit run/member and declared controlled Petal target remain required. No Owner floor is weakened, no implicit Owner request or broad IPC/raw UiAction is added, and no focus/activation/scroll/synthetic prerequisite appears.
2. **Owned files/process/worker lifetime:** canonical no-alias/no-reparse/no-hardlink file and ancestor pins, sealed app/helper hashes, exact PID/birth/exe and user/session/integrity remain. Worker start stays hidden with explicit related inherited handles and bounded pipes. Unknown start/exit retains owners. Only the retained created worker object may be terminated; FE is never its termination target.
3. **Durable one-attempt slot:** actual CreateNew bootstrap precedes handoff. Reservation is flushed and read back before invoke-worker creation; same-handle intent precedes its sole Invoke. Missing/partial/corrupt/replaced/oversized ledger is not pristine. Timeout, crash, pipe loss, post-arm refusal, HRESULT/S_OK and host expiry stay spent. No retry/reset/implicit restart clears the process-incarnation slot.
4. **Native producer identity:** watcher remains installed before the sole provider lookup. Original outer/marked COM roots, actual target and InvokePattern remain on the same MTA thread. Bounded later walks compare rather than repair/reselect/reacquire. Sampled watcher progress is not lossless HWND-birth proof.
5. **Immutable host permission:** rolling FEW1 pass/tick cannot renew the retained target's private original one-second host proof. Exact source/tag/provider alias and current host checks remain required through delivery/callback. No untyped fallback on expiry is introduced.
6. **EOF and capacity:** ERROR_BROKEN_PIPE is the specific anonymous EOF case; other read failures and invalid/bounded request content still refuse. Shared aggregate history counts both roots/ledgers/receipts/incarnations/starts. Admission precedes allocation/reservation/stop and leaves the last admitted history inspectable; no old outcome is discarded to gain capacity.
7. **Real pair transition:** linked ResumeRun validates old membership/settings, retains unchanged counterpart and full byte hashes, archives exact pair lineage, creates only a fresh FE birth/bootstrap, checks its actual fresh accepted issuer subject and distinctness, then publishes new current and atomically replaces pair membership under the existing lifecycle lease. Old outcomes/links survive. Ordinary null-bootstrap mode remains ordinary; native mode remains input-off. Unpaired upgrade options are preserved. No automatic adoption/replay/crash recovery or multi-file transaction guarantee is claimed.
8. **Privacy and outcomes:** no token/name/value/provider exception dump or capture mode is added. S_OK remains submitted/outcome unknown; failed HRESULT remains invocation outcome unknown. Helper exit, root tick/pass and COM return never acknowledge GUI/DB/rendering. All-Verse and selected Inspector/node/group profiles remain separate unsupported work.

## Nonblocking metadata-correlation hardening observation

For a bootstrapped **unconfirmed** receipt, `$owned` identifies saved starts with the required bootstrap, while final full actual stage-manifest equality uses the wider `$matchingStarts` collection. A conjunction using the already bootstrap-owned subset would be clearer and stricter if differing saved manifests existed for the same Run/app birth/stage. No supported producer ordering was found that creates such differing immutable stage manifests for an identical exact process incarnation; real stage/helper/process pins still precede any reap. Within the documented exclusion of equivalent-privilege same-user metadata tampering, this is **not a discovered authority bypass or blocking reachable lifecycle failure**. Tightening it later must preserve both bootstrapped and observe-only exact-start correlations and receive its own source review; this approval applies to the frozen v3 bytes, not an unreviewed later change.

## Execution and integration gates remain

Root alone integrates after both full source approvals and verifies exact raw contributions. Preserve later main d8 `ProcessOutput -> Owner QA serve -> ProviderHandoff` and all current host/UI/terrain/physical/pointer work; this tooling baseline gives no authority to overwrite host files.

Run the integrated parser/compiler/existing-journey sweep only in the authorized execution lane. Meaningful required checks include real anonymous-pipe close/error cases; inherited handle/deadline/late-start and unknown lifetime behavior; durable reservation/intent/crash/corruption; last-admitted history; the complete real A->B confirmed-prune-followup lifecycle and unknown-stage refusal; actual ordinary/native paired restart with fresh issuer/bootstrap and unchanged live counterpart; real Viewer/Editor retained-element invocation and independent navigation/data/role observations; unchanged Owner HTTP refusal and original host proof expiry/alias retirement.

No such execution occurred here. Capture/WGC freshness, selected-object/all-Verse surfaces, retained/collaboration final matrices, full campaign/catalog/UX passes, final integrated sweep, exact-SHA CI and tagged downloadable release remain unfinished requirements.

## Changed v3 raw anchors

| Path | Bytes | SHA-256 |
| --- | ---: | --- |
| `tools/qa/_native_observer.ps1` | 21164 | `878bf3ba80ab03faf218db33e31eb70909fb18fc4576658b4f15c2b6dfdae9a0` |
| `tools/qa/native_observer/lifecycle_scenario.ps1` | 19301 | `ae322a252a363a1a71e3c899f064f6a8d7c6a9e9212ebca901a1c057141340ab` |
| `tools/qa/native_observer/AGENTS.md` | 21259 | `22ff52e6010ab62b541aace83bb894ae478e813af3e6e784c1bc8539eafde85b` |
| `tools/qa/AGENTS.md` | 87980 | `3b2efe0a03eec9fb8f0dbb4b053885617ddc60198922a842a3089acd1d6821fc` |

The full 28-file manifest remains authoritative for all other exact source bytes. Source approval closes the identified source blockers only; execution results and the broader goal are not complete.
