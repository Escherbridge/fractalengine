---
type: Review
status: changes_requested
---

# NAV-2/NAV-3 v1 independent functional and design review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**REQUEST CHANGES.** The frozen slice has a coherent ownership design and preserves the Owner HTTP boundary, but three concrete functional/lifecycle defects prevent source approval. These are source conclusions; no compiler, parser, formatter, test, helper, application, UIA/OS/process, HTTP/DB/network or Git mutation was executed.

## Reviewed identity and scope

Reviewed the complete 24-file changed/new source slice in `.omc/worktrees/qa-native-navigation-helper`, whose actual HEAD remains `023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`. Read the approved main helper plan, author report, directory guides, shared UI/UX guidance and relevant unchanged ownership support. Independently checked all 24 working files against both the raw snapshot and the frozen manifest, including exact byte lengths and SHA-256 values.

- Manifest: `qa-native-navigation-nav23-v1-hashes.json`, SHA-256 `6e3b29d54137fe2df6facae39d91af20ec2db643c213bf25d2e3f4a007710bb1`.
- Full source patch: `qa-native-navigation-nav23-v1-source.diff`, SHA-256 `85a3f1e82fd3e466f2bf7cec6c0393aba77b60a9dbd5a70a4770d1f1c809aea8`.
- Worker: `cb11508933e81da8a90fb49ba69a91152b7bbd94bc855909d2cbb34b59a39c72`.
- Worker header: `9b9da68575db4ae6db2a72fd4eb1128a8cc304b743b7134c4a833b285d137393`.

The 24 reviewed paths are `.gitignore`; `tools/qa/{_lib.ps1,_native_identity.cs,_native_observer.ps1,AGENTS.md,clean.ps1,fe.py,launch.ps1,pair.ps1,restart.ps1,stage.ps1,stop.ps1}`; `tools/qa/scenarios/evidence-boundaries.py`; and `tools/qa/native_observer/{__init__.py,AGENTS.md,build.ps1,contract.py,controller.py,ledger.py,scenario.py,windows.py,worker.cpp,worker.manifest,worker_protocol.hpp}`. This is a full functional review of this slice, independent of the author and the narrow worker critique. It is not runtime acceptance or approval of unchanged application source.

## F1 — high: normal anonymous-pipe completion is refused before request parsing

`tools/qa/native_observer/worker_protocol.hpp:273–282`, especially line 278, requires every `ReadFile` to return nonzero before checking the byte count. The controller's actual `windows.py::send` writes the complete request, flushes it, and closes `process.stdin`. The worker continues its loop after consuming the payload and reaches the closed write end. For an anonymous pipe this normal completion returns `FALSE` with `ERROR_BROKEN_PIPE`, so `read_pipe` throws `request_io` instead of returning the complete request to `Request::parse`.

Both normal observation and invocation are affected. In invocation mode the controller has already durably reserved the process slot; failure to parse does not make it available again. The worker's output cannot establish normal UIA readiness or navigation because discovery is never reached on this path.

The parent independently verified the Win32 behavior against [Microsoft's ReadFile documentation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile), captured at root `.omc/research/native-readfile-official-20261104.md`. This review itself did not use network access.

Required correction: recognize only the documented anonymous-pipe closed-writer condition as EOF, while retaining the byte bound, strict UTF-8/JSON validation, watchdog and refusal of other I/O errors. An empty or truncated request must still fail parsing. Extend an existing meaningful I/O/contract journey or the eventual staged-helper acceptance so the actual controller's write/flush/close path demonstrably reaches request parsing; do not substitute a memory-only JSON parser assertion for this boundary.

## F2 — medium: paired restart has no supported valid native handoff

`controller.py:70–89` deliberately requires the member process identity and stage in `pair.json` to equal the current run's record. This is a sound stale-membership refusal. However, `launch.ps1:100–103` archives the prior start, line 206 creates a new process-bound bootstrap, and line 284 copies `native_pair` unchanged into the newly published current record. `restart.ps1` only forwards its arguments to `launch.ps1`. The only membership writer is the initial pair Start path at `pair.ps1:110–123`.

Consequently a successful restart creates a new PID/birth/ledger but leaves the recorded pair member describing the old incarnation. Native commands then refuse `pair_member_changed`. They cannot legitimately fall back to standalone mode because the preserved link correctly requires explicit paired membership. The documentation's reference to a future separately valid pair handoff does not provide a command or implementation that performs one.

This is fail-closed rather than an authorization bypass, but it does not complete the stated restart/recovery workflow. The existing pair record itself tells the operator to stop/restart the explicit Viewer run, so this is a reachable supported operation, not an invented edge case.

Required correction: provide an explicit owned pair restart/reseal path that validates original pair lineage, the actual restarted incarnation and fresh bootstrap, the unchanged peer and stage/provenance, and atomically publishes a new membership revision under lifecycle ownership. Preserve the old start, receipts, reservations and unknown outcomes. Do not delete `native_pair`, adopt an arbitrary current process into the pair, renew an old proof, reuse an old attempt ledger, or replay the old target. A refusal before stopping the current process can be a temporary guard, but refusal alone is not final acceptance of the full goal's paired restart requirement.

Extend an existing journey for the real membership transition and refusal cases. Actual paired restart followed by read-only native observation and separately authorized new-incarnation invocation remains a live gate.

## F3 — medium: producer budgets can make normal cleanup permanently refuse

`controller.py:185` creates a new operation receipt for every observation, with no admission/history count bound. `_native_observer.ps1:101–105` recursively counts every `*.jsonl` under each native root, refuses above 256, and only then skips `attempt.jsonl`. A bootstrapped incarnation therefore has one bootstrap ledger plus its receipts. After 256 ordinary sequential observations the root has 257 JSONL files, and `Get-QaNativeWorkerRecords` refuses before it can establish any otherwise valid ownership record. Stop, restart and clean all call this reader, so ordinary permitted activity reaches a state those lifecycle commands cannot process.

The same mismatch applies across process incarnations: each bootstrap adds another ledger below the shared native root. `_native_observer.ps1:82` also refuses more than 256 archived start directories, while the actual ResumeRun writer has no matching pre-admission history budget. At the boundary it can create the 257th directory after the earlier cleanup check passed at 256. These lifecycle hooks are called for ordinary runs as well, so their admission policy must not create an unmanageable state and must preserve existing non-native behavior.

Required correction: share explicit bounded admission/accounting rules between receipt/bootstrap/start producers and cleanup consumers, and refuse a new operation or restart before creating the file or replacing the old process when it would exceed a supported budget. Alternatively use a bounded, independently verifiable archival design for confirmed records. Unknown outcomes and ownership evidence must remain retained; do not fix this by truncating unknown receipts, deleting ledgers, silently ignoring an unvisited remainder or increasing a single hard-coded consumer cap without controlling producer growth.

Extend an existing journey for the last accepted and first refused observation, accumulated bootstrap ledgers, restart-history boundary, and successful stop/clean ownership inspection of all admitted states.

## Design findings without additional blocking changes

The following choices are readable, appropriately scoped and should be retained during correction:

- `fe.main` dispatches native mode before ordinary run loading, token resolution, log probing and HTTP. The authored existing-journey extension exercises that exact dispatch seam and explicitly grants zero native/application checkpoint credit.
- The controller uses the existing canonical no-reparse/no-ADS ownership layer rather than adding an independent path model. It pins metadata, binaries, related ancestors and the actual app handle; the receipt writer is excluded from inheritance.
- `AttemptLedger` is a small append-only state machine. Reservation is flushed and reread before worker launch; the worker independently verifies the canonical prefix/file identity and durably writes intent before its single possible Invoke. Unknown state is not converted into a pristine slot.
- The worker retains one outer provider, marked root, actual target and InvokePattern on the same MTA thread. Subsequent walks compare original identities rather than replacing the target. The watcher starts before discovery, bounds traversal, and is a sampled progress witness rather than a claim of lossless window history.
- Current FEW1 pass/tick advance is allowed only with stable lifetime/source/target identity. This alone is not a refreshed target-permission defect: the unchanged host still enforces its original private one-second proof deadline and alias retirement at actual use boundaries.
- Controller/worker results distinguish submission or unknown outcomes from application consumption. Capture remains explicitly unavailable. No Owner HTTP-floor change, focus/activation input, generic UI action or rendering acknowledgement is introduced by this slice.
- Directory-level rationale contains the cross-cutting contract. No speculative abstraction or broad manager refactor is required to fix the concrete defects above.

I found no additional concrete functional blocker after reading the complete slice. This does not prove SDK defaults, real pipe/process timing, durable media behavior, actual provider activation, watcher delivery, native-child enumeration, Viewer/Editor navigation, cleanup under crashes or rendering. In particular, the watcher retrieval loop's actual OUTOFCONTEXT delivery and heartbeat behavior still require the planned native acceptance; this source review does not claim a defect solely because it does not call `DispatchMessageW`.

## Re-review and integration boundary

Preserve v1's patch, manifest, raw snapshot and this rejection. Repair the three findings as one coherent slice and publish exact replacement hashes plus a delta report for independent re-review. Do not claim compilation or authored fixtures as executed acceptance. Main's later `d8da3a46` Owner observation ordering correction and unrelated reviewed source must survive eventual integration; these 24 paths contain tooling and documentation only.

Full native/paired/lifecycle execution, the final integrated sweep, FULL passes, exact-SHA CI and tagged-release delivery remain separate unfinished gates.
