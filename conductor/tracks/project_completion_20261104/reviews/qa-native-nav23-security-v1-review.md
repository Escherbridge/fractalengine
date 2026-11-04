---
type: Review
status: changes_requested
---

# NAV-2/NAV-3 independent security and correctness review — frozen v1

**Verdict: REQUEST CHANGES; do not integrate/build this v1 as source-approved.** Two concrete implementation blockers are independently identified below: normal anonymous-pipe request EOF is refused, and legitimate observation history can exceed cleanup's acceptance cap. The paired-restart limitation is also a real functional boundary requiring an explicit supported contract, as independently raised by root's functional review. No authority bypass, proof renewal, replay permission or positive capture claim is established by this review.

Reviewer `/root/legacy_helper_io_review`, 2026-11-04. Reviewed all 24 frozen source/document files in `.omc/worktrees/qa-native-navigation-helper` against baseline **`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`**. Applied the approved helper plan **`732571e2a04d3d33bdbcf31eac8e5bd232121c20b5611c99f3c061f8e2256b94`**, its approved identity v2 boundary, and `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Root's separate functional lens and the earlier child's narrow integration critique are not substitutes for this full review.

No source/index/history change, compiler/parser/formatter/test, app, native/OS API, process operation, input, capture, HTTP, database or network action was performed. Read-only source/SDK/Git and byte/hash processing only; this report is the sole authored file. Main's later d8 host-ordering correction is not present in the tooling baseline and must survive eventual integration.

## Exact frozen anchors

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `qa-native-navigation-nav23-v1-source.diff` | 152905 | `85a3f1e82fd3e466f2bf7cec6c0393aba77b60a9dbd5a70a4770d1f1c809aea8` |
| `qa-native-navigation-nav23-v1-hashes.json` | 6171 | `6e3b29d54137fe2df6facae39d91af20ec2db643c213bf25d2e3f4a007710bb1` |
| `qa-native-navigation-nav23-v1-source-report.md` | 15058 | `4e13019585b2a9b58f14dbb07cb832103c9c25235bd95656ebf578debf2bf036` |

All **24 current raw file sizes/hashes match the manifest**. Read-only Git reports the expected 11 tracked modifications plus 13 new files. `git diff --check` passes. No application/DB/runtime/renderer/pointer source is part of this tooling slice.

The full review covered `.gitignore`; `tools/qa/{_lib.ps1,_native_identity.cs,_native_observer.ps1,AGENTS.md,clean.ps1,fe.py,launch.ps1,pair.ps1,restart.ps1,stage.ps1,stop.ps1}`; `tools/qa/scenarios/evidence-boundaries.py`; and all eleven `native_observer/{__init__.py,AGENTS.md,build.ps1,contract.py,controller.py,ledger.py,scenario.py,windows.py,worker.cpp,worker.manifest,worker_protocol.hpp}` files. Existing unchanged launcher/owned-file/host proof context was inspected where needed; unchanged baseline code is not newly authored approval.

Critical implementation anchors:

| Source | SHA-256 |
| --- | --- |
| `native_observer/worker.cpp` | `cb11508933e81da8a90fb49ba69a91152b7bbd94bc855909d2cbb34b59a39c72` |
| `native_observer/worker_protocol.hpp` | `9b9da68575db4ae6db2a72fd4eb1128a8cc304b743b7134c4a833b285d137393` |
| `native_observer/controller.py` | `5911abf92ada74cac4b0fa8e62f93fce01b6a3c5a7aeac2f83768424f726db93` |
| `native_observer/windows.py` | `81ed839f7ad4088e1bdbe4a1ac42926967edb2e4bc39e4774c07710a39ecee61` |
| `native_observer/ledger.py` | `8134a423451865d698742739a3a5704de2f7a3207862ba80ec28427d1d4d55c2` |
| `_native_observer.ps1` | `d2c9b9b55f52406fc8232371596ab06de95b6efcee35ea86f9d6b33f79270d18` |

## Blocker NAV23-S1: normal request EOF becomes `request_io`

`native_observer/windows.py:210–214` sends the complete bounded payload, flushes and closes the anonymous-pipe writer. `worker_protocol.hpp:273–282` loops `ReadFile` until EOF, but line 278 requires every call to return nonzero before it checks `count == 0`. When the anonymous-pipe writer closes after the payload, Windows signals broken-pipe EOF with `ReadFile == FALSE` / `ERROR_BROKEN_PIPE`. That normal terminating condition is currently thrown as `request_io`.

Minimal production ordering: controller reserves an invocation slot if invoking, starts the actual helper, writes its request and closes stdin; helper consumes the payload, attempts the next read, then refuses before `Request::parse` and all UIA discovery. An invocation reservation remains spent, so retrying the same action is not a workaround. Observation also cannot complete the normal request protocol. This needs no hypothetical duplicate event, corrupt metadata or malicious client.

Fix the actual worker pipe reader to recognize this precise EOF condition while preserving the 64-KiB bound and refusing unrelated read failures. Do not catch every read failure as successful EOF. Preserve empty/partial/oversized/invalid UTF-8/JSON refusal and the watchdog before stdin. A wire check must use the actual compiled worker and normal controller pipe close; the current Python in-memory ledger tests do not exercise this native boundary. No such execution was performed here.

## Blocker NAV23-S2: observation producer can make legitimate cleanup inadmissible

`_native_observer.ps1:101–105` first recursively collects every `*.jsonl` under each native root and refuses when the count exceeds 256. Only **after** that check does it skip `attempt.jsonl`. `controller.py:185` creates one new durable worker receipt for **every** observation, with no matching history-cap admission check in Context/execute. The exclusive lifecycle/observer handles serialize attempts but do not bound their total retained file count.

Minimal production ordering with a normal bootstrapped run: one `attempt.jsonl` exists; run 256 sequential `native observe` commands, each of which creates a worker receipt even if the provider refuses; the root now contains 257 JSONL files. Subsequent ordinary `stop.ps1`, ResumeRun's pre-cleanup, pair Stop's pre-cleanup, and `clean.ps1` all enter this inventory and refuse `native_worker_record_cap` before normal owned FE cleanup. The same denial is reachable without one successful Invoke, without changing the ledger, and even with v1's earlier EOF failure. The directory/file budget and stored ownership receipts, rather than event duplication, cause it.

Align producer admission with the retained inventory budget **before creating a receipt/worker or growing a bootstrap/history beyond what cleanup can validate**. Count actual bootstrap ledger files and both native roots consistently. Alternatively provide a bounded incremental complete cleanup inventory that supports all legitimately produced histories. Never delete unknown/spent receipts or replace a ledger to regain capacity. Keep file/line bounds, exact start provenance and conservative unknown retention. The analogous 256-start-directory consumer cap at `_native_observer.ps1:82` also needs a matching pre-allocation check: ResumeRun currently can append a 257th archived start after accepting the previous 256, leaving the next native cleanup unable to inspect it.

Extend the existing actual producer/cleanup ownership journey with the last-admissible/first-refused boundary. Merely testing `count > 256` in isolation would mirror the implementation and miss this production ordering.

## Paired restart boundary: fail closed, but no supported native recovery

Root independently raised this functional issue and the source confirms it. `launch.ps1:284` preserves `native_pair` while ResumeRun creates a new FE birth/bootstrap. `controller.py::pair_binding` requires the pair member's exact process identity and full stage provenance to equal the new current record. Only pair Start writes `pair.json` membership and member links; restart only forwards ResumeRun. The old pair identity therefore fails `pair_member_changed`, and retained `native_pair` also prevents treating the restarted run as standalone. Existing pair Info/Stop already conservatively recognizes changed membership; the new native path has no supported reseal operation to make that restarted member usable.

This is a safe refusal, not a discovered authority bypass. However the slice must not claim ordinary paired stop/restart/new native attempt usability while merely mentioning a future separately valid handoff. Either implement an explicitly coordinated pair-restart/reseal under owned lifecycle/identity checks, preserving old pair/start/outcome provenance and creating only a new-process ledger, **or reject unsupported paired restart before stopping/replacing the current member**, with a clear supported operator path. Do not drop the pair link, accept old member identity by name/path, reuse old proofs or clear old attempt state. Treat this as a functional source hold until the paired restart contract is made concrete; it is not independently assigned a new security exploit severity here.

## Positive design findings to preserve during correction

### Native entry, provenance and file/process ownership

`fe.py` dispatches the native branch immediately after its existing parser and before normal `load_run`, token selection, logs or HTTP. The supported modes are observe and one declared controlled-fixture Petal invocation. Explicit run and pair member are mandatory as applicable; no `.qa/current` fallback, arbitrary HWND/helper, coordinate/index/regex action, generic UiAction, hidden IPC, focus/activation or synthetic input is added. Owner HTTP authorization remains unchanged. Existing launch/pair Owner setup remains a separately explicit bootstrap, not a recipient-side prerequisite probe.

Controller canonical pins inherit the reviewed owned-file implementation: local drive/no ADS or aliases, retained ancestors, no reparse/hardlinks, no-write/no-delete metadata and executable pins, exact PID/birth/exe and a query/synchronize FE handle. Parent/app user, nonzero session and medium integrity/UIAccess checks precede the helper. Worker independently checks process birth/path/session/user/token/desktop. Opt-in staging seals the x64 SDK22621 helper binary and source recipe; launch requires source-current helper stage and input off. No arbitrary unstaged helper is executed by native CLI. Build/link/manifest behavior remains unexecuted.

`windows.py` uses an explicit inherited handle list, hidden owned child, bounded stdin/stdout and zero stderr. It excludes the receipt writer but retains lifecycle, metadata/executable/process/observer/ledger pins in the child through OS exit. The retained created worker object, rather than a PID/name scan or FE handle, owns termination. Unknown start/exit retains parent owners in `UNKNOWN_LIFETIMES`; a late start gets no request and is reaped through that owned object. Output-reader uncertainty after confirmed process exit stays separate. Those are appropriate conservative ownership choices; no successful kill/cancellation is inferred by source.

### Durable process-incarnation slot and output semantics

Bootstrap uses CreateNew, durable flush/readback and native file identity before current handoff. Existing input-off runs without bootstrap can only observe; they cannot create a substitute invocation ledger. `AttemptLedger.reserve` checks exact canonical bootstrap, appends a process/run/name-hash reservation, flushes and rereads the same pinned handle **before** worker creation. Worker requires exactly that two-line canonical prefix and its SHA/file ID; before Invoke it appends `submission_possible=true` intent with actual source/target facts, FlushFileBuffers and exact same-handle readback. Missing, partial, corrupt, replaced and oversized state never becomes pristine.

No retry/reset operation clears a reservation for the same FE birth. Reservation interruption, worker uncertainty, post-intent refusal, expiry, HRESULT and S_OK remain spent. Controller rereads the actual ledger and checks worker exit/app liveness before accepting sanitized output. S_OK remains submitted/outcome unknown; failed HRESULT remains invocation outcome unknown. No helper result claims GUI consumption, navigation, DB persistence or rendering. Refusal at an unknown boundary reports unknown attempt state instead of inventing an unused slot. These invariants must survive all fixes.

### Actual retained COM element and immutable host proof

The watcher is installed at `worker.cpp:602` before HWND discovery and the sole `ElementFromHandle` at line 614. It has process-scoped create/destroy handling, 256-event refusal, heartbeat and generation-matched posted-loop barriers. It remains a sampled progress witness, not lossless HWND-birth proof. The retained process, HWND/property, original root/target and host source epochs remain necessary. The worker bounds roots/children/depth/nodes/candidates and refuses unknown modal/password/foreign/unsupported surfaces or incomplete traversal.

The actual original outer/marked elements, target and InvokePattern remain on the same MTA worker thread. Later walks compare original COM identity/runtime/ancestry/tag/bounds/availability; they never replace the target, reacquire its pattern or repair it by name/runtime lookup. The same original pattern reaches the only `Invoke()` at line 680. Preflight is repeated after durable intent, before that call. Observe mode reads no tree names or InvokePattern; FullDescription is read only after the exact fixed root marker/type checks. Only typed candidates' names are read for the bounded declared fixture, and sanitized output contains hashes/static codes rather than names/values/provider exception strings.

**Rolling FEW1 pass/tick is not itself a proof-renewal defect.** `fresh_root` updates its latest observed FEW1 record only after same process/window/context/binding/surface/profile and nonregressing pass. It does not rewrite the original retained target tag/COM pattern. The approved host `navigation.rs::proof_is_current` retains the original demand's one-second deadline; `navigation_ui` remap, translated binding and final callback checks use that same proof, with permanent provider-alias retirement on replacement. A current root tick is not authority and cannot extend the private host deadline. Keep these actual host gates and the final refusal path; do not add an untyped fallback to improve helper success rates.

### Cleanup provenance and archived incarnations

The corrected ownership inventory compares each receipt to the actual Run's current or archived start, exact app PID/birth/exe/stage/helper and, for bootstrapped records, run/ledger/file ID/path. Observe-only histories derive their run hash from the same exact identity. An unmodified native subtree copied from another run cannot authorize killing its helper. Cleanup refuses a live/unverifiable controller, pins/checks helper hash, and targets only the recorded helper PID/birth/exe. Unknown identity remains retained rather than inferred unarmed; clean keeps referenced runs/stages. Current/archived provenance must remain despite fixing admission caps and pair restart.

The directory documentation honestly excludes equivalent-privilege same-user metadata tampering and does not turn sampled enumeration into exhaustive proof. The resource-cap blocker above is ordinary legitimate production, not that excluded tampering model.

## Required next gates

Freeze a revised full manifest/diff/report and obtain both full re-reviews of actual fixes before integration. Preserve exact v1 artifacts. Eventual integration must retain main d8's `ProcessOutput -> Owner QA serve -> ProviderHandoff` ordering and current host/terrain/physical/pointer behavior; these 24 tooling files give no reason to overwrite later host files.

After approved source, root owns compilation/parse checks and the meaningful existing integrated journey, actual worker pipe-close handling, actual inherited-handle/deadline/late-start behavior, durability/crash/corruption and cleanup-budget boundaries, paired restart/reseal or pre-mutation refusal, process/HWND/context/provider replacement, watcher gaps/overflow, and real Viewer/Editor one-visible-Petal invocation. Verify independent state/data observations and unchanged Owner HTTP refusal. The authored in-memory ledger cases and mocked native dispatch are useful offline seams but do not cover native request EOF, COM, process death, filesystem flush or real lifecycle cleanup.

All-Verse bootstrap, selected Inspector/node/group surfaces, WGC/capture, peer/restart final acceptance, integrated final sweep, exact-SHA CI and tagged release remain separate campaign work. No source or capture approval is granted by this blocked v1 review.
