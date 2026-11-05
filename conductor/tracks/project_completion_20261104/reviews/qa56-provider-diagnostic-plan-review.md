---
type: Review
title: Provider HRESULT diagnostic plan review
date: 2026-11-04
---

# QA56 expanded provider diagnostic plan review

Verdict: **approved for bounded authoring of the described four-file diagnostic change**. No concrete plan blocker remains. This is design/source inspection only, not implementation approval or execution evidence. Write only the proposed worker/controller/scenario/directory documentation slice after root's live-source freeze permits it; no change to QA56's frozen packet or admission policy is authorized by this approval.

Reviewed plan `.omc/research/qa56-provider-property-diagnostic-plan.md`: 8,642 bytes, SHA256 `ec6352d0a161092b5ee41e7abc2f850bde519593d0cce1607cb5daacc24f74bd`. I compared it with actual `worker.cpp`, `worker_protocol.hpp`, `controller.py`, `contract.py` and pinned SDK22621 declarations/constants. Local/root guidance and the shared UI guidance apply. Only this review report was written; no source/packet/Git mutation, implementation parser, formatter, tests, compiler, app/native activity, API, DB or network operation was performed. GUI57 was not touched.

## Complete fixed-stage coverage

The current worker has 32 textual `checked(...)` calls. Thirty use the default `provider_unavailable` code; the two COM initialization/creation calls use explicit `com_unavailable` and correctly remain outside this extension. The plan's named method stages 1–19 cover all thirty default origins, including RawViewWalker/ElementFromHandle, AutomationId/runtime ID and SAFEARRAY helpers, ordinary element getters, walker traversal and CompareElements. Repeated method calls deliberately share one ID: no unsupported element/phase/occurrence localization is promised. Stages20/21 cover the two explicit GetCurrentPropertyValue failure branches. The wrapper is worker-local, evaluates the existing HRESULT expression once and retains the exact FAILED predicate/base code; the generic protocol helper is unchanged.

The observe allowlist is consistent with actual reachability. CurrentName, CurrentBoundingRectangle and GetCurrentPatternAs (13–15) occur only behind Invoke target selection or an actual retained target. In particular, the root bounding-rectangle call at `worker.cpp:583` is guarded by `result.target || (retained && result.original_seen)`; ordinary observe walking does not enter it. Conversely, CompareElements can occur during observe's retained-marked validation and remains allowed. Fixed property IDs match the primary SDK: 30019 IsPassword, 30077 WindowIsModal, 30159 FullDescription; 30031 IsInvokePatternAvailable is Invoke-only and excluded from observe diagnostics. Explicit allowed stages and conditional property membership are preferable to accepting a numeric range with arbitrary new meanings.

## Output, privacy and compatibility boundary

The proposed `provider_failure` has one closed schema: schema/stage_id/hr, with property_id mandatory only for stages20/21 and forbidden elsewhere. Stage/property require exact JSON integers (not bool, float or string); nested hr is exactly eight lowercase hex digits with the failed signed-32-bit high bit. This publishes only a fixed method category, optional fixed SDK property identifier and the already-returned failure code. It does not publish provider values, names, types, handles, runtime IDs, paths, selectors or arbitrary exception text. Method names are documentation, not dynamic wire strings. The example is explicitly not a QA56 measurement.

The typed failure remains derived from `Refusal`. Thus existing generic catches must still receive it: the admission loop rethrows everything except `property_unavailable`, and the post-Invoke catch still converts a successful Invoke followed by refusal into `submitted_state_changed`. Invoke-mode failures must never acquire the diagnostic object, lose their spent state, or be presented as an observe result. Keeping base `receipt.method` untouched preserves Invoke HRESULT semantics; nested hr separately reports the failing operation without translation. At explicit property failures, saving the original HRESULT before the existing VariantClear preserves cleanup and avoids substituting cleanup status.

Worker capture and emitter gates are appropriately redundant: parsed observe, exact `provider_unavailable`, all three raw flags false, base method S_OK, valid fixed stage/property combination, failed nested HRESULT. Existing canonical scalar atoms, total byte bound and one-write seam remain; no generic raw JSON atom or request/ledger parser widening is needed.

Controller validation is specified at the existing correct seam: `worker_output` validates the original raw worker bytes before `execute` replaces `attempt_spent` with independent ledger state (`controller.py:267-274`). It must require actual observe, confirmed worker exit2 with exact int type, raw false flags, zero base hr, exact code and closed nested shape. Existing recursive duplicate-key rejection applies at both levels. Mutually exclusive base/legacy scalar/service signature/provider extensions reject mixed, extra, unknown or duplicate members. Older base-only workers remain accepted; older controllers safely reject unfamiliar extensions. Independently spent/unknown ledger state, lifecycle cleanup and bounded ownership are preserved rather than trusting diagnostic flags to clear them.

## Authoring and verification gates

The plan adds no OS/COM/UIA read, retry, action, discovery pass, capture, fallback or changed success/type predicate. All retained process/HWND/context/handler, proof, watcher, deadline, admission and Invoke floors remain. Source review must confirm each of the thirty default calls is replaced exactly once and only the two explicitly categorized COM calls remain, with no changes to call ordering or single evaluation. It must also inspect the actual C++ catch hierarchy and post-Invoke behavior, not rely solely on Python parser cases.

The proposed extension of the existing exercise is meaningful and proportionate: every allowed stage and stage/property pair; failed-HRESULT endpoints and representative value; exact types and shapes; rejection of Invoke-only/unknown stages, positive HRESULTs, casing/length/type errors, wrong/missing/extra properties, duplicates/mixed extensions, wrong raw mode/code/flags/base hr/exit; and retained legacy plus independently spent-ledger assertions. Those cases remain unexecuted plan requirements. Root owns the subsequent independently reviewed source, matching build and fresh observe evidence.

QA56's failed call, property if any, HRESULT and cause remain unknown permanently within that frozen packet; its external clarification stays mandatory. A future diagnostic can describe only its new execution. This approval authorizes no provider repair, relaxed predicate, native root/witness acceptance, Invoke, capture, peer, retained restart, CI/release or FULL credit.
