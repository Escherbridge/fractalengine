---
type: Review
status: changes_requested
---

# NAV23 v2 independent functional and design re-review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**REQUEST CHANGES; the initial complete-slice integration approval is withdrawn.** All three functional blockers in my v1 review are repaired in the production paths, and the paired restart is a real coordinated membership transition. An independently identified, source-confirmed historical-stage dependency still lets supported cleanup make subsequent lifecycle operations refuse. The v1-fix assessments below remain valid, but they do not approve the complete v2 slice while F4 is unresolved.

No compiler, parser, formatter, test, application, OS/native/process/input/capture, API/DB/network action or Git mutation was performed. Only this review report was written and subsequently amended. Independent corrected-source approval, exact integration review, compilation and actual native/lifecycle acceptance remain separate gates.

## Reassessment history and F4 — blocking historical-stage dependency

The initial 12,414-byte version of this report had SHA-256 `c7f35817302fa1c791a0eaccb8869a51a990a730189e8f192306f5e28868d455` and approved v2 after checking the three v1 fixes. Before publication, the independent security review identified a further lifecycle counterexample. I independently traced it through the immutable v2 snapshot and concur. That dependency survived my initial review; this amendment withdraws the broad approval rather than treating the three repaired findings as sufficient completion evidence. The frozen v2 source, manifest and patches are unchanged.

In v2 `_native_observer.ps1:201`, `Get-QaNativeWorkerRecords` unconditionally calls `Get-QaStageManifest` for each historical receipt. That requires the old physical stage/manifest/executable before `confirmed` is computed at lines 247–250. Yet `Get-QaNativeRetainedStages:282–286` yields only unconfirmed stages. `clean.ps1:26–28` retains those references; lines 36–41 retain a live run and its current stage; lines 49–59 may then remove a separate eligible historical stage.

The concrete supported sequence is:

1. An unpaired native run uses Stage A and completes an observation with an exact `exit_confirmed` receipt.
2. A supported unpaired ResumeRun changes to valid Stage B, preserving the old start/bootstrap/receipts. The current FE process is live on B.
3. Clean inspects the old receipt successfully while A still exists. It retains the live run and B, but no unconfirmed receipt requires A. With the ordinary age/Keep policy, clean may remove A.
4. The next stop, restart or clean parses that same retained confirmed receipt and tries to load Stage A before classifying it confirmed. The missing old stage now poisons ownership inspection and prevents the live B run from reaching its normal lifecycle path.

No forged record, corruption, cap overflow or unknown worker is required. This is a source-supported ordering, not an executed reproduction. It is a functional lifecycle blocker even though the refusal is conservative.

Required correction: retain every physical artifact still needed to inspect retained history, or validate confirmed history from exact archived start/stage/bootstrap metadata without requiring intentionally prunable binaries. Any record that can authorize reaping or remains unconfirmed/unknown must retain its strict actual stage, hash, file and process checks. Missing files must not become proof of confirmed absence. Preserve copied-foreign-subtree refusal, spent ledgers, unknown outcomes and old receipts.

If retaining all referenced stages is chosen, separate the stage dependency from run liveness: passing every historical stage through the current combined `liveRuns`/`liveStages` retention loop would keep every run that ever observed indefinitely. Extend a meaningful existing lifecycle journey through confirmed receipt, supported stage change, the actual cleanup/reference decision and subsequent complete inspection. Verify unknown/reapable history still cannot use missing physical dependencies.

No v2 integration should occur until this repair and its exact replacement source are independently re-reviewed. My findings that EOF, aggregate admission and ordinary/native paired reseal are repaired remain useful bounded source conclusions; they are not a substitute for this additional dependency repair.

## Exact source identity

Checkout: `.omc/worktrees/qa-native-navigation-helper`, based on `023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`. Independently checked all 28 working files against the v2 byte lengths/hashes and the complete v2 raw snapshot. Compared every path with the preserved v1 manifest: 15 paths remain byte-identical; nine original paths change; four paths are new. Those thirteen delta paths exactly match `changedFromV1`.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| v2 hash manifest | 8144 | `05f266c26c3f816a2ee670650e5a7da63a28b4d00ad328643b7ca39192ab60b1` |
| v2 full source patch | 201242 | `b1372272984e68df812e3fa07399a878ee982f2974df72ae47b2d45f7942689f` |
| v1-to-v2 review delta | 61054 | `42377e8fe7a0a4082f180d0f517e65133093b3770caeb58ebfcd87ef995deee5` |
| v2 author report | 14411 | `a576f0d4dcc49285c09fc0ce3d90210e0b1ec3775a79d89f603c8dde73388a38` |

This re-review covers the complete prior 24-file slice and its retained invariants, plus `_native_pair.ps1`, `native_observer/history.py`, `native_observer/limits.json` and `native_observer/lifecycle_scenario.ps1`. I read every changed/new v2 path and used the independently verified unchanged v1 anchors for the remaining complete bodies already reviewed. Applied the approved helper plan, repository/directory instructions and shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`.

## F1 repaired: actual request-pipe EOF

`worker_protocol.hpp::read_pipe` now accepts precisely the documented `FALSE`/`ERROR_BROKEN_PIPE` result as closed-writer EOF, alongside zero-byte successful EOF. Other read failures still throw `request_io`. Accumulated content is still bounded at 64 KiB and enters the same strict UTF-8/known-shape parser; EOF does not make an empty or malformed request valid. The watchdog remains installed before the first stdin read.

The unchanged controller still writes, flushes and closes stdin. The source repair therefore addresses the actual incompatible producer/consumer boundary identified in v1. It does not alter reservation or outcome handling, and an interrupted reserved invocation remains spent. The official ReadFile evidence captured by root supports this correction; no memory-only parser assertion is credited as executed anonymous-pipe acceptance.

Required execution remains the compiled worker under the actual controller close path, with valid bounded requests reaching parsing and malformed/empty/truncated/over-bound/other-error refusals retained.

## F3 repaired: producers and ownership inspection share admission limits

`limits.json` supplies one aggregate policy for both native roots: 256 JSONL files including attempt ledgers and receipts, 256 incarnation directories, and 256 archived start directories. Python and PowerShell validate the same known keys and positive bounded values. The inventories accept only the producer's fixed layout, reject aliases/unknown layout and check bounds as they account for the complete inventory.

The actual controller invokes `admit_operation` before observe-only directory creation and again before reservation/receipt/worker creation. At capacity an invocation therefore refuses before spending its slot. The initial context/bootstrap checks, original file pins and global lifecycle lease remain; no concurrent authorized producer can allocate behind that admission.

PowerShell cleanup now consumes `Get-QaNativeHistoryInventory`, the same inventory used by `Assert-QaNativeAllocation`; ledgers count before receipt filtering, and the two native roots share the aggregate count. ResumeRun checks additions for the archive and any fresh native ledger/incarnation before archive creation or old-FE stop. Bootstrap repeats its own admission before CreateNew allocation. Empty/failed incarnation directories consume capacity, so failures do not create free unbounded growth.

No unknown receipt, spent ledger, old attempt or incarnation is pruned to regain room. A full admitted inventory is still supported by ownership inspection/stop/clean, while subsequent production refuses. This resolves the previous off-by-one/per-root and uncontrolled restart-history growth mechanisms. The existing journey extensions exercise real inventory/admission and ownership-record parsing at the last accepted/first refused boundaries, including accumulated historical ledgers and both roots. These are authored offline checks, not executed native cleanup or media-durability evidence.

## F2 repaired: supported paired restart and membership reseal

The new `_native_pair.ps1` keeps the transition at the existing lifecycle owner. Both `restart.ps1` and direct `launch.ps1 -ResumeRun` enter the same production path; no post-hoc arbitrary adoption command or nested lifecycle lease is introduced.

Before archiving or stopping the old FE process, `Open-QaPairRestart` checks the recorded member/link, authoritative owner pair path/schema/revision, exact old process/stage/bootstrap and settings, and the unchanged counterpart's current record/link/provenance. It requires distinct roots/ports and common stage provenance and retains the counterpart's exact live Process object. Native mode requires the existing native bootstrap and input off; ordinary mode legitimately carries no bootstrap and gains no invocation ledger. Paired stage, port and input/native mode remain fixed during this transition, while unpaired restart keeps its existing options.

`Save-QaPairRestartArchive` copies the previous pair byte-for-byte, flushes and hash-checks it, and records prepared lineage before the old process is stopped. Old current/start metadata, ledger contents and outcomes remain retained. The actual new FE birth receives a fresh bootstrap through the existing launcher path.

At publication, `New-QaPairRestartRevision` requires a strictly newer birth with the same run/API/executable/stage provenance and persistent pair link. Native mode additionally requires the actual new process-bound bootstrap and distinct run/ledger/file identities and paths; `Assert-QaFreshPairBootstrap` checks its canonical pristine ledger bytes, native file identities and empty observer file. Ordinary/null-bootstrap mode remains ordinary. The new record derives its member from the actual new current metadata and preserves the counterpart member.

Issuer continuity is now checked using the launcher's already accepted fresh bootstrap token, not a copied DID assertion. The shared subject parser retains the original type/max-role/Verse-scope/DID constraints; the fresh subject must equal the archived member and differ from the counterpart. That setup-only parser is not called by native observe/invoke and does not change their no-token/no-HTTP boundary. Generated unsigned claim fixtures in the offline scenario are explicitly parser fixtures, not authentication proof.

`Complete-QaPairRestart` rechecks the old pair hash, counterpart's complete current-file hash, retained counterpart liveness, links/member facts and actual new process liveness. It writes the new current through the existing launcher publication, then uses same-volume `File.Replace` as the authoritative membership commit point, incrementing revision and linking the exact archived parent hash/path. Native pair binding now checks complete bootstrap equality as well as process and stage. Thus a successful normal restart has a supported new membership that the real native controller can accept; the v1 permanent `pair_member_changed` path is repaired.

### Failure and interruption limits

These remain intentional conservative limits, not full recovery acceptance:

- The global lifecycle lease excludes supported native operations during replacement. A failed startup uses the launcher's exact newly created-process cleanup; no old action is replayed or old ledger reset.
- New current and pair replacement are not a multi-file crash transaction. Failure or interruption can leave mismatched/unavailable metadata for diagnosis; the source does not silently adopt it or claim that a submitted action was cancelled.
- A failure after the pair replacement but before the committed receipt/handoff can still lead to exact new-process cleanup. The resulting pair may identify an exited incarnation; a later explicit supported transition must again prove its own lineage and liveness. A receipt write is not GUI/data acknowledgement.
- This single-member restart requires the unchanged counterpart to remain live. Restarting both already-stopped peers, changing both stages/modes, or automatic crash-metadata repair is not implemented by this transition and receives no acceptance credit here.

Those restrictions are documented and do not replace normal paired restart with refusal. Actual interruption/recovery usability remains part of the broader campaign and must be assessed with real owned fixtures. The implementation should not be used as proof that retained campaign stores have a reviewed require-existing startup path; that separate hold remains.

## Retained design and authority boundaries

The v1 production worker body, Windows lifetime layer, ledger implementation, strict contract and early native CLI dispatch remain unchanged. The same-worker original provider/target/InvokePattern, watcher-before-discovery, host original proof deadlines, one durable pre-worker reservation/pre-Invoke intent, explicit unknown outcomes, exact owned-worker termination, bounded output and capture refusal survive the v2 changes. Nothing adds focus/input, direct UiAction, Owner HTTP relaxation, token access during recipient operations or a new selection/permission owner.

The new modules have narrow responsibilities: a shared retained-history policy/inventory, and an explicit pair transition with preparation, archive, fresh revision validation and commit. They use the existing launcher/ownership functions rather than a second process manager. The directory documentation accurately separates authored source from executed/native acceptance. I found no reason to demand a broader abstraction or unrelated refactor as part of these fixes.

The existing evidence-boundaries journey remains the registration point. Its Python/PowerShell extensions call actual admission, ownership-record parsing and pair revision/publication paths while explicitly controlling process/native-file proofs. They grant zero application/native checkpoint coverage; the non-Windows unavailable result is marked skipped, not real Windows coverage. No new registered test declaration or passing execution is claimed.

## Integration and acceptance gates

Preserve v1's rejected packet/reviews, all v2 raw anchors and the initial report hash above. After F4 is repaired and the complete replacement slice approved, integrate only the reviewed tooling paths, retaining main's later `d8da3a46` Owner observation ordering correction and unrelated reviewed source. Verify exact composed bytes and scope independently. This reassessment grants no source or integration approval for unresolved v2.

Parent-owned compilation/parsing, the integrated existing journey, real controller/anonymous-pipe execution, handle inheritance/deadlines, durable reservation/intent/crash/unknown cleanup, last admitted inventory, and actual paired native and ordinary restart with stable issuer and live counterpart remain required. Then verify new-incarnation native observation/action with independent navigation/data checks and unchanged Owner HTTP refusal. S_OK is still submitted/outcome unknown.

No native rendering/capture, retained-store restart, all-Verse/selected Inspector/node/group profile, FULL pass, final sweep, exact-SHA CI, release artifact or goal-completion claim follows from this approval.
