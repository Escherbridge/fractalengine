---
type: Review
status: source_approved
---

# NAV23 v3 independent functional and design re-review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**APPROVE the exact complete 28-file v3 tooling source slice, subject to independent integration verification and the outstanding execution gates below.** The historical-stage dependency that blocked v2 is repaired in the production ownership reader. Confirmed history can survive ordinary pruning of its obsolete stage; unknown or reapable history still requires its actual stage and strict process/image ownership. I found no additional concrete functional or maintainability blocker in the complete slice.

This is source approval only. No compiler, production parser, formatter, test/journey, application/helper, OS/native/process/input/capture, API/DB/network action or Git mutation was performed. Only this report was written. Hash/JSON processing inspected packet identity; it did not execute the reviewed implementation or fixture. The unchanged 24 paths carry forward my full prior source assessments through independently verified byte identity; all four v3 changes were read completely and their producer/consumer/lifecycle interactions re-traced.

## Exact source identity and review coverage

Checkout: `.omc/worktrees/qa-native-navigation-helper`, base `023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`. Independently verified every one of the 28 working paths against its v3 manifest length/SHA and the complete v3 raw snapshot: zero mismatches. Comparing all paths against the immutable v2 manifest gives exactly the four declared changes: `_native_observer.ps1`, `native_observer/lifecycle_scenario.ps1` and the two QA directory guides. No path is added or removed in v3.

| Frozen v3 artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Source report | 10118 | `f6672d25dc8a95ae8a33656a7f90880abcea91cbeb4ac313c6c7409c13b2315e` |
| Hash manifest | 7616 | `d5dcbd663d02913d01ab5528ffcb1265a6ab2ace0d5a768e6199db6af86420fa` |
| Complete source patch | 212946 | `45c40eda6e6b2e9ba989121ff7af98d5a9db0e4cfb06c34f2dbca2f8a9156744` |
| V2-to-v3 history delta | 22630 | `fb193841726105f64e99d7beaee3e4585cb949ba51e14936c0d233006515e45c` |

The complete review scope is:

| Responsibility | Reviewed paths |
| --- | --- |
| Tooling setup, policy, stage and launch lifecycle | `.gitignore`; `tools/qa/AGENTS.md`; `_lib.ps1`; `_native_identity.cs`; `_native_observer.ps1`; `_native_pair.ps1`; `stage.ps1`; `launch.ps1`; `pair.ps1`; `restart.ps1`; `stop.ps1`; `clean.ps1` |
| Native CLI and independent evidence journey | `tools/qa/fe.py`; `tools/qa/scenarios/evidence-boundaries.py` |
| Native package, contract and durable ownership | `tools/qa/native_observer/__init__.py`; `AGENTS.md`; `build.ps1`; `contract.py`; `controller.py`; `history.py`; `ledger.py`; `limits.json` |
| Windows worker, wire, retained provider and lifecycle fixtures | `tools/qa/native_observer/windows.py`; `worker.cpp`; `worker.manifest`; `worker_protocol.hpp`; `scenario.py`; `lifecycle_scenario.ps1` |

The approved helper plan, retained v1/v2 reports, repository/directory instructions and shared `C:/Users/atooz/.gemini/UI-UX-guidance.md` apply. This packet changes tooling only. Integration must preserve main's later `d8da3a46` Owner-before-provider host ordering correction and other unrelated reviewed source; this tooling review grants no permission to overwrite them with the older checkout.

## F4 repaired: ownership history survives supported stage cleanup

The v2 refusal was concrete: observe on Stage A with a confirmed receipt, perform a supported unpaired restart on Stage B, retain the live run/B, prune eligible A, then find that the next lifecycle operation required A before it could classify the retained receipt. My amended v2 report remains a request-changes record, including its initial approval withdrawal; this new packet supersedes that source verdict only for the exact v3 bytes above.

`_native_observer.ps1:160` gathers bounded current/archived start metadata for this exact run, with synthetic input disabled. The receipt reader still requires its bounded fixed layout, 1–3 canonical JSON lines, exact supported keys/schema/operation, app/controller process identities, helper path inside the stage root and the correct incarnation directory. The recorded app executable must be that stage's `fractalengine.exe`. At lines 207–218, the original app PID/birth/executable and stage path must match an owned start whose recorded application hash has the supported shape and whose native helper schema/image/hash match the receipt. Foreign copied history does not become owned merely because physical Stage A is absent.

Native history additionally matches the original run/ledger/file IDs, recorded ledger path and helper hash to the owned start's bootstrap at lines 219–229. Observe-only history instead requires empty ledger identifiers, an ordinary start, the app-identity-derived run ID and observe mode at lines 230–237. Those branches remain distinct; cleanup does not create a new bootstrap, reset an attempt or adopt a receipt.

The shared identity validator and lines 239–269 check the receipt before assigning confirmation. Worker identities must have the exact PID/string birth/rooted image shape, refer to the recorded helper image and be born no earlier than the controller and app. Terminal code shape and integral signed/unsigned Windows exit-code range are checked. Phase ordering admits the actual producer sequences, while only the complete `reserved -> worker_started -> exit_confirmed` sequence with identical valid original and terminal worker identities qualifies as confirmed. A lone terminal, null identity, repeated terminal, changed identity or lifetime-unknown sequence cannot establish confirmed absence.

Crucially, actual stage materialization is now inside `if (-not $confirmed)` at lines 270–275. A confirmed receipt has already established its retained historical provenance and does not call `Get-QaStageManifest` or require A's prunable application/helper binaries. `Resolve-QaChildPath` still constrains the recorded helper path and refuses existing reparse-point ancestry, but it permits a missing child path; it does not accidentally reintroduce a physical-stage dependency for confirmed records.

For unconfirmed records, the real stage reader still checks the actual application image/hash and supported stage, and the complete materialized manifest must canonically equal a matching owned start's recorded manifest. The reaper remains unchanged: it refuses a live/unverifiable controller, pins the real helper image, hashes it and checks the exact worker PID/birth/image before termination. Missing physical dependencies never prove a worker exited.

`Get-QaNativeRetainedStages` still emits only unconfirmed stage references. `clean.ps1:26–28` retains those runs/stages; live current roots retain their current stage at lines 36–41. Fully confirmed old history therefore no longer requires A while the current B process remains live, and otherwise dead confirmed-only runs can still age out. No retain-everything policy, dropped receipt or ledger deletion masks the problem. The repair removes the undeclared dependency rather than turning conservative refusal into normal restart behavior.

## Producer compatibility and meaningful regression coverage

The unchanged controller's `started` callback at `controller.py:200–204` records the actual worker identity durably. Its ordinary success path at lines 224–226 appends the matching identity, integral process return code and fixed terminal code after the worker has exited. `windows.py::run_worker` returns those facts only after waiting for exit, or records/refuses an unknown lifetime; output refusal is distinct from exit confirmation. V3's strict three-phase classification is compatible with that actual producer.

An early failure can return a terminal without a durable `worker_started` line, or with no identity. V3 accepts the supported two-line terminal as unconfirmed and keeps the actual stage dependency. It does not claim that an absent started line proves no process existed. A complete started-plus-lifetime-unknown record remains unconfirmed even when its identity is consistent.

The existing `lifecycle_scenario.ps1` now creates real scratch stage manifests and dummy image bytes with matching hashes instead of overriding `Get-QaStageManifest`. Its F4 sequence creates a properly bound confirmed A receipt, archives the exact original start, changes the current incarnation to B with fresh recorded identifiers, checks that an unconfirmed A reference is retained, restores confirmation and checks that A is prunable. It resolves and bounds the scratch target inside its scratch stage root before the authored deletion, retains B, then calls the production ownership reader and helper stop with an erroring process seam that must remain untouched for confirmed history.

The same fixture authors missing-A refusals for unstarted, reapable, lifetime-unknown, lone-terminal, duplicate-terminal and mismatched-identity records, and still refuses a foreign archived run. The capacity fixtures also use the real stage reader. Existing ordinary/native pair transition and issuer fixtures remain intact. This is useful regression source against the actual failed dependency and retention boundary; it is not a second implementation of cleanup and does not obtain credit for real app restart, process absence, bootstrap file IDs, native behavior or durable media semantics.

## Prior repairs and full-slice invariants retained

The other 24 v2 paths are exact byte matches. The three earlier blockers remain repaired:

- **F1 request EOF:** `worker_protocol.hpp::read_pipe` accepts Windows anonymous-pipe writer closure only for `ERROR_BROKEN_PIPE`, alongside successful zero-byte EOF. Other errors and the bounded strict request parser still refuse. The controller continues its actual write/flush/close path; no reservation or output rule is weakened.
- **F3 admission/capacity:** shared `limits.json` governs both roots, ledgers and receipts, incarnation directories and archived starts. Python admission precedes receipt creation/reservation; PowerShell admission precedes restart archival/old-app stop and repeats before bootstrap allocation. Cleanup uses that same bounded inventory. Existing failed/empty directories and spent/unknown history keep consuming capacity; nothing is silently discarded to recover room.
- **F2 actual paired restart:** the explicit lifecycle-owned transition validates the old pair/member and live unchanged counterpart, archives the exact parent pair, starts a new incarnation and fresh native bootstrap when enabled, checks fresh accepted bootstrap issuer continuity and distinctness, then publishes a replacement membership revision. Ordinary pairs remain null-bootstrap. Native pair binding checks the new complete bootstrap and process/stage facts, so a successful supported restart is usable rather than permanently refused by the old membership.

The pair transition still fixes stage/port/input/native mode and requires the counterpart live. New current plus authoritative pair replacement are not a multi-file crash transaction; interruption can leave conservative unavailability for diagnosis, and failure cleanup targets the exact newly created process. No automatic adoption/replay, both-already-stopped recovery, stage-changing pair upgrade or retained-store restart acceptance is claimed. These limits remain explicit and do not replace the supported live-counterpart restart with refusal-only behavior.

The worker retains its original same-worker provider, target COM element and InvokePattern through the sole durably fenced invocation. Watcher-before-discovery, retained process/handle ownership, absolute worker timing, pre-worker reservation/pre-Invoke intent, bounded output and unknown outcomes remain intact. The host's immutable original target proof deadline is outside this tools-only delta and must survive composition. Native CLI dispatch remains before token/HTTP setup, with no direct UiAction, synthetic input/focus, broadened Owner HTTP authority or new selection/permission owner. Capture remains unavailable.

The package boundaries remain readable: history owns inventory/admission, pair helpers own an explicit prepared/archived/revision/commit transition, the controller owns request/receipt orchestration and the worker owns retained native proof/invocation. The v3 correction extends the existing ownership reader through one shared process identity validator and one post-classification physical-stage branch. Directory guides explain the rationale without introducing an alternate process manager or speculative abstraction.

## Integration and outstanding execution gates

Preserve rejected v1/v2 packets and reviews. Integrate only the 28 reviewed tooling paths from the exact v3 raw snapshot; independently check the composed source scope and bytes while retaining later host fixes and unrelated main work. My approval is bounded to source functionality/design and does not replace the independent security verdict or integration review.

Parent-owned compilation/parsing and the integrated existing journey remain required. Actual Windows acceptance must cover the real controller's anonymous-pipe close path, request bounds/error refusals, inherited handle restrictions, absolute deadlines/late startup, durable reservation/intent, crash/unknown cleanup, the final admitted inventory and first refused allocation. Exercise actual ordinary and native paired restart with unchanged issuer, fresh bootstrap/membership and live counterpart, followed by usable new-incarnation native observation/action and independent navigation/data/role checks. Preserve Owner HTTP refusal and the distinction between submitted/unknown outcome and observed app consumption.

The F4 real execution gate remains observe A -> exact confirmed worker exit -> supported unpaired restart B -> actual eligible cleanup of A -> subsequent stop/restart/clean. Also verify unknown/reapable A retains its real dependencies and refuses a genuinely missing stage. The authored scratch fixture provides no executed checkpoint credit.

No GUI capture/rendering, all-Verse or selected Inspector/node/group profile, retained-store restart, FULL campaign pass, final integrated sweep, exact-SHA green CI, tagged downloadable release or goal-completion claim follows from this review.
