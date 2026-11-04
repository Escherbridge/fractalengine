---
type: Review
status: changes_requested
---

# NAV23 v2: complete independent security/correctness re-review

**Verdict: REQUEST CHANGES for NAV23-V2-S1, historical confirmed-stage cleanup dependency.** V2 repairs the three earlier reviewed EOF/capacity/paired-restart issues at source level. The complete slice still has a reachable lifecycle failure: clean can remove a confirmed historical worker's stage while retaining its run, but later native ownership parsing requires that deleted stage. No source/integration/build/native approval is granted until that producer/consumer dependency is corrected and re-reviewed.

Reviewer `/root/legacy_helper_io_review`, 2026-11-04. Source-only review of all **28** frozen paths in `.omc/worktrees/qa-native-navigation-helper`, unchanged HEAD **`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`**. Applied the approved helper plan `732571e2a04d3d33bdbcf31eac8e5bd232121c20b5611c99f3c061f8e2256b94`, approved identity/native v3 boundaries and shared UI/UX guidance. Root's seven-file pair/cap draft review was read as context, not substituted for this full review.

Only this report was authored. No source, index/history, compiler/parser/formatter/test, app/helper, process, native/OS API, input/capture, HTTP/DB/network operation or Git mutation occurred. Read-only source/SDK/document/Git and byte processing only. Main Rust source remains d8 and its later Owner-before-provider ordering must survive eventual integration.

## Exact frozen scope

| Artifact in the isolated checkout's `.omc/research/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `qa-native-navigation-nav23-v2-source-report.md` | 14411 | `a576f0d4dcc49285c09fc0ce3d90210e0b1ec3775a79d89f603c8dde73388a38` |
| `qa-native-navigation-nav23-v2-hashes.json` | 8144 | `05f266c26c3f816a2ee670650e5a7da63a28b4d00ad328643b7ca39192ab60b1` |
| `qa-native-navigation-nav23-v2-source.diff` | 201242 | `b1372272984e68df812e3fa07399a878ee982f2974df72ae47b2d45f7942689f` |
| `qa-native-navigation-nav23-v2-review.delta.diff` | 61054 | `42377e8fe7a0a4082f180d0f517e65133093b3770caeb58ebfcd87ef995deee5` |

Every current raw file matches its manifest length/hash and corresponding v2 snapshot bytes. The declared delta is exactly **nine modified existing files plus four new files**; the other **15 files remain raw-identical to their fully reviewed v1 content**. All 24 original v1 source snapshot anchors and original v1 artifacts remain preserved: patch `85a3f1e8...`, manifest `6e3b29d5...`, report `4e130195...`. Read-only whitespace inspection passes. No application/runtime/DB/renderer/pointer source is in this tooling slice.

Full scope evaluated: the original `.gitignore`; `tools/qa/{_lib.ps1,_native_identity.cs,_native_observer.ps1,AGENTS.md,clean.ps1,fe.py,launch.ps1,pair.ps1,restart.ps1,stage.ps1,stop.ps1}`; `tools/qa/scenarios/evidence-boundaries.py`; all original eleven `native_observer` files; plus new `_native_pair.ps1` and `native_observer/{history.py,limits.json,lifecycle_scenario.ps1}`. The unchanged v1 worker/controller/ownership invariants were retained by exact-content comparison and called-boundary review; the full changed files and new producer/consumer paths were inspected, including complete pair preparation/archive/revision/publication and the authored lifecycle journey.

## NAV23-V2-S1 — blocking correctness/lifecycle: clean prunes a stage still required by retained confirmed history

Actual source gates:

- `_native_observer.ps1:201` calls `Get-QaStageManifest` for **every** historical worker receipt, before it computes `confirmed` at lines 247–250. That helper requires the stage's real `stage.json` and hashes its real `fractalengine.exe`. The historical start must match those current stage facts.
- `Get-QaNativeRetainedStages`, lines 282–286, yields a stage only for **unconfirmed** worker records.
- `clean.ps1:26–28` retains runs/stages yielded by that function; lines 36–41 retain a live run and only its **current** stage. Lines 49–59 can therefore remove an eligible old stage while preserving that same run on a new stage.
- An **unpaired** ResumeRun retains its supported explicit stage option; only a paired restart is restricted to the same stage. Historical worker records and archived starts remain in the preserved run as intended.

Minimal ordinary ordering, requiring no corrupt record or forged identity:

1. Native-enabled unpaired Run R uses Stage A. A normal observation worker exits and records `exit_confirmed` with exact worker identity.
2. Explicitly restart R on a valid Stage B. The old current/start/bootstrap/outcomes remain archived. Run R's FE root is now live on B.
3. Run ordinary clean with A eligible by age/Keep policy. Its initial ownership scan succeeds while A exists. All A receipts are confirmed, so `Get-QaNativeRetainedStages` does not retain A. The live current root retains R and B. Clean is consequently allowed to remove A.
4. The next stop/restart/clean for R parses its retained A receipt and calls `Get-QaStageManifest(A)` before recognizing confirmed exit. A's missing files now cause refusal. The live B root cannot reach its ordinary cleanup path despite the historical helper already having confirmed exit.

This is also possible when a kept inactive run outlives a separately pruned old stage; the live unpaired restart ordering above is the clearer witness. It is a source-supported ordering, not an executed reproduction. It was present in v1 and survived its review; this complete v2 review now identifies it. The new capacity alignment does not fix dependency retention.

Required contract: **every artifact needed to validate a retained history must remain available while that history can block lifecycle operations**, or the confirmed-history validator must no longer depend on intentionally prunable binaries. Two bounded repair directions are possible:

- Retain every stage required to inspect any retained run history, while separating stage-reference retention from the decision to retain the run forever. Merely yielding all stages through the current combined `liveRuns/liveStages` loop would permanently retain every run that ever observed; design the lifetime dependency deliberately.
- Validate confirmed historical receipts against their exact archived start/stage/bootstrap metadata without requiring the old executable to still exist, while retaining actual stage-file pins/hash/process checks before **any** possible worker reap or unconfirmed/unknown handling. A missing stage must never convert an unknown receipt into confirmed absence, and a copied foreign subtree must still fail current/archived Run ownership.

Do not delete historical receipts, reset spent/unknown state, weaken exact process identity, or infer exit from a missing stage. Extend a meaningful existing lifecycle journey through actual confirmed historical receipt -> supported stage change -> production cleanup/reference decision -> subsequent complete ownership inspection/stop. The present support script replaces `Get-QaStageManifest` with a constant seam, so it cannot detect this failure if that seam remains in the new stage-pruning case. Use actual scratch stage records/files for the dependency under test, and keep only unrelated process/native authority seams controlled. Root owns all execution after source approval.

## Earlier blocker corrections: source-supported

### Normal anonymous-pipe EOF

The actual `worker_protocol.hpp::read_pipe` now treats precisely `ReadFile == FALSE` with `ERROR_BROKEN_PIPE` as EOF and refuses other failed reads. It preserves accumulated-byte bounds and sends content through the same strict parser. The installed watchdog still precedes stdin. The controller's actual write/flush/close path is unchanged. Read the root-captured Microsoft ReadFile primary document at `.omc/research/native-readfile-official-20261104.md`, source line 142, which supports that anonymous-pipe EOF distinction. No network request or pipe execution was performed.

This closes the v1 source defect; it is not a compiled wire-roundtrip result. Empty/truncated/oversized/invalid UTF-8/JSON and unrelated I/O failures remain required execution checks. A spent invocation reservation is never made available by parse failure.

### Shared aggregate history admission

Python and PowerShell read the same exact `limits.json` schema and positive bounded integer policy: 256 aggregate JSONL files across both native roots, including attempt ledgers and receipts; 256 aggregate incarnation directories; 256 archived start directories. The actual fixed-layout inventories reject unknown/reparse entries and excess remainder. Failed/empty directories consume capacity. No old outcome or receipt is pruned to regain room.

`history.py::admit_operation` runs before observe-only directory/lock allocation and again in execute before reservation/receipt/worker creation. Context does not spend an invoke slot before that second check. ResumeRun checks room for archive plus new native ledger/incarnation before archiving or stopping the old FE root; bootstrap repeats admission before creating its files. The same consumer inventory accepts the last admitted count. This closes v1's ordinary producer/cleanup count mismatch and the 257th-start boundary at source. Full capacity refuses new activity without reducing existing ownership evidence. The historical stage dependency above remains distinct.

### Coordinated ordinary/native pair restart and fresh issuer

The actual launch ResumeRun path now performs its paired transition under its existing lifecycle lease; `restart.ps1` forwards to that path. Native helper admission uses the same lease. No nested lifecycle acquisition or opportunistic pair-adoption command is added.

`Open-QaPairRestart` validates the exact old member/current/link, pair owner path/schema/revision, stage/port/input/native settings, current counterpart/link/process/bootstrap and common stage provenance before archive/old stop. It retains the unchanged counterpart process handle and full current/pair byte hashes. Ordinary/null-bootstrap pairs remain ordinary; native mode requires the original bootstrap/input-off state. Paired migration of binary/port/mode is refused; unpaired options remain separate. This is the explicit provisional implementation boundary, not an upgrade feature.

Archive creation preserves exact prior pair bytes/hash and prepared lineage. New current gets only a freshly created actual FE birth/bootstrap. Revision creation requires strictly newer birth, same executable/stage, changed run/ledger/file IDs/paths and same helper. `Assert-QaFreshPairBootstrap` reads actual new ledger/observer file identities, pristine canonical bootstrap content and empty observer state. Current process identity remains matched to the new bootstrap. Old outcomes, target/proof and attempt slot remain untouched.

The narrow draft's issuer gap is corrected in the actual final code: launch passes the subject of its already accepted fresh protected `$bootstrap.Token`/VerseId through the shared bounded `Get-QaPairIssuerSubject`; scope/type/max-role/DID syntax must match. The fresh subject must equal the archived member public DID and differ from the unchanged counterpart DID before replacement. It is not merely copied from the old member. This parser is used only by authorized launch/initial pair setup; native CLI still invokes no token or HTTP helper.

Before membership commit, Complete rechecks old pair hash, counterpart full current hash/link/member and retained live counterpart, validates the fresh member/bootstrap, and retains the new process. Current is published first, then same-volume `File.Replace` atomically replaces the authoritative pair revision. Both links remain. An interruption between files leaves mismatched/unavailable diagnosable state; ordinary failed-launch cleanup owns the exact new process. There is no claimed multi-file crash transaction, automatic adoption, rollback, replay or recovery command. Both already-stopped members are outside this single-member transition; a live unchanged counterpart is required. These explicit limits do not weaken ordinary native authority.

## Positive v1 invariants retained in complete v2

- Native dispatch still occurs before ordinary run loading, Owner token selection, log probes and HTTP. Explicit run/pair member and controlled exact Petal name remain required. No hidden broad IPC, raw UiAction, focus/activation/scroll prerequisite, synthetic input, arbitrary HWND/helper or Owner-floor weakening is added.
- Exact canonical no-reparse/no-alias/no-hardlink files and ancestors, sealed helper/app hashes, retained app handle/PID/birth/exe and user/session/integrity checks remain. Hidden worker start uses explicit related handle inheritance, no inherited receipt writer, bounded request/stdout and zero stderr. Unknown start/exit retains ownership; termination targets only the retained created worker object, never FE.
- The one durable process-incarnation slot remains reserved and reread before worker creation. The worker's exact same-handle intent flush/readback precedes its one possible Invoke. Missing/corrupt/partial/replaced/oversized state never becomes pristine. HRESULT/S_OK, timeout, crash, post-arm refusal and host expiry remain spent and are never replayed.
- Worker body is raw-identical to v1. Watcher precedes the sole HWND/provider lookup; original root/target/InvokePattern remain on the same MTA worker thread. Bounded safety walks compare rather than repair/reselect the target. Watcher progress remains sampled, not lossless window-birth proof.
- Current FEW1 pass/tick does not renew host proof: same source and retained tag/alias remain necessary, and the host uses the original private one-second demand deadline through mapping/delivery/callback. No fallback on expiry or fresh untyped action is introduced.
- Output remains static codes/hashes with submitted/outcome-unknown semantics. No COM return, helper exit, tick or root pass becomes GUI/DB/render acknowledgement. Capture remains unavailable; direct native observation is not a WGC freshness claim.
- Current/archived start and bootstrap binding still prevents an unmodified foreign subtree from authorizing cleanup. The actual helper file/process checks for unknown/reapable records must remain through the historical-stage repair.

## Existing journey and required next gates

The existing evidence-boundary journey still calls scenario.exercise; no new registered test declaration is introduced. New Python admission cases call the actual inventory/admit functions. The PowerShell support script calls actual admission/full receipt parsing and pair preparation/revision/File.Replace with explicitly controlled process and native-file seams. Generated issuer fixtures exercise parsing/distinctness, not signature authentication. These source-authored cases grant zero actual native/application/crash/durability coverage and were not executed here.

Preserve v2 snapshot/patch/report, repair NAV23-V2-S1 coherently, freeze revised exact anchors and obtain both full re-reviews. After source approval, root owns integrated parse/build/journey checks, actual native request close/EOF, inherited handle/deadline/late-start behavior, durable reservation/intent/crash/unknown cleanup, real history limits, stage-retention dependency, paired restart/reseal with fresh identity/issuer and a live unchanged peer, new native observation/invocation and independent state/data/role observations. Owner HTTP refusals and host one-second/alias gates must remain intact.

Future integration must preserve main d8's `ProcessOutput -> Owner QA serve -> ProviderHandoff` and current host/UI/terrain/physical/pointer changes. All-Verse/bootstrap, selected Inspector/node/group, capture/WGC, full campaign, final integrated sweep, exact-SHA green CI and tagged downloadable release remain separate pending work.

## Key v2 raw anchors

| Source | Bytes | SHA-256 |
| --- | ---: | --- |
| `_native_observer.ps1` | 19267 | `b8a48ac190a1b594cda11c719d2e061229eb505e6eac121a147d93b350347b40` |
| `_native_pair.ps1` | 13422 | `9e1a0c0a22e2ac11516b6b805095f07f907d83a26b975d262ea9a22301df9ac2` |
| `clean.ps1` | 2952 | `d84d7e7b3944290fcc708d87828edb46da6f00fb90d876a96a5411fc02010de9` |
| `launch.ps1` | 20477 | `f2027f0c06db4190fda72084e8c0686455f9c330f857a3a66528b0168dc4c6b5` |
| `native_observer/controller.py` | 15767 | `b9b0a1a85274870362fc4792f8b80718e3236196ed6271f349725d216e6ccd4d` |
| `native_observer/history.py` | 3277 | `b09acba1141503e4b5167dde20f6aed5fa704a32b90bd77c51b858b55e38bf09` |
| `native_observer/limits.json` | 113 | `c85634387fecdddafb2a3a172a703e99133486a9f700d6ea5b5170c6a6662a49` |

The complete 28-path raw manifest is authoritative beyond these selected anchors. No execution success or complete source approval is asserted for blocked v2.
