---
type: Plan
title: Bounded provider HRESULT diagnostic plan
date: 2026-11-04
---

# QA56 provider failure diagnostic — expanded source plan

Plan only; no implementation or runtime execution. Production source must stay unchanged during QA57 against sealed `6ca9f49b`. Shared UI/UX guidance applies: one readable diagnostic boundary, fixed public categories, no provider content or authority changes.

## Observed boundary and limits

Frozen `qa/passes/56/packet-manifest.json` SHA256 `7ad0ab744a5f4d0abf193fce617a136798f004cc091e82acfe9a9535f2c50d19` pins `owner-native-observe-exact-geometry.jsonl` (SHA256 `64fb34bac299441e8031d4d4d0dbdf6efabac2bfef926d912a60ae7959a36d2b`). Actual result is `provider_unavailable`, helper exit2, raw no-spent/no-Invoke/no-method-return, base `hr="00000000"`; worker receipt confirms `worker_exited`/exit2. The strict initial window profile was traversed before the UIA failure. No positive UIA tree/root/witness/Invoke result is established.

`worker.cpp:416–424` calls `GetCurrentPropertyValue(property_id,&value)` in `boolean_property`; `:602–603` calls the same SDK method with FullDescription in `root_witness`. Both discard the local failed HRESULT after `VariantClear`. **`worker_protocol.hpp:44` also defaults all unqualified `checked(HRESULT)` calls to `provider_unavailable`.** QA56's actual failing call/HRESULT are unknown, not necessarily property reads. This expanded plan covers every existing default checked origin plus both explicit property failures. The two explicit `com_unavailable` checks at worker.cpp:722,725–726 retain their distinct behavior outside this provider-code extension.

## One closed optional shape

Keep the base result schema/code/`hr` unchanged. Add only optional `provider_failure`:

```json
{"schema":"fe-native-provider-failure-v1","stage_id":20,"property_id":30019,"hr":"80000000"}
```

Example numbers are a schema illustration, **not QA56 measurements**. Exact keys are schema/stage_id/hr for ordinary stages, plus mandatory property_id only at stages20/21. Stage/property are JSON integers (never bool/float/string); nested hr is exactly eight lowercase hex characters for the original failed 32-bit HRESULT. Require high bit set (`80000000..ffffffff`), equivalent to SDK `FAILED`, without interpreting facility/message. No HRESULT translation, exception text or returned value is emitted.

Fixed method-stage enum below covers all current default checked sites. Repeated calls to the same method share one ID: this identifies the failing method, **not** the element, occurrence or initial/final validation phase. No mutable ambient stage or diagnostic context threaded through every helper. Locations are current worker.cpp lines.

| ID | Existing method stage | Call sites |
| --- | --- | --- |
| 1 | CurrentAutomationId | 413 |
| 2 | GetRuntimeId | 428 |
| 3 / 4 / 5 / 6 | SafeArrayGetLBound / UBound / Vartype / Element | 432 / 432 / 433 / 439 |
| 7 | CompareElements | 451 |
| 8 | CurrentProcessId | 523,593 |
| 9 | CurrentControlType | 523,593,619 |
| 10 | CurrentNativeWindowHandle | 524,594 |
| 11 | CurrentIsEnabled | 527,594,619 |
| 12 | CurrentIsOffscreen | 527,595,620 |
| 13 | CurrentName (Invoke only) | 543,617 |
| 14 | CurrentBoundingRectangle (Invoke only) | 547,583,620 |
| 15 | GetCurrentPatternAs (Invoke only) | 559 |
| 16 / 17 | GetFirstChildElement / GetNextSiblingElement | 568 / 574 |
| 18 / 19 | RawViewWalker / ElementFromHandle | 728 / 730 |
| 20 | boolean_property GetCurrentPropertyValue | 418 |
| 21 | root-witness FullDescription GetCurrentPropertyValue | 602 |

Names are documentation only, never dynamic wire strings. Accepted observe stages are the explicit set1–12 and16–21; reject13–15 because those paths require request.invoke or an actual retained/selected target. At20 require property_id30019 or30077; at21 require30159. Boolean property30031 occurs only in Invoke paths (:549,623) and refuses as an observe diagnostic. At every other stage property_id must be absent, not null/zero. Future IDs/stages require explicit review, not arbitrary range acceptance. No element names, values/types, handles, runtime IDs, paths, counters or selectors are added.

## Worker and controller handling

Introduce one typed `ProviderFailure : Refusal` carrying method stage, optional property ID and already-returned failed HRESULT. A **local worker.cpp** `checked_provider(HRESULT, ProviderStage)` wrapper replaces every existing default `checked(...)` call, preserving the same FAILED condition/base code and single evaluation. Callers pass a named enum literal; no dynamic text or last-stage tracker. Keep worker_protocol.hpp and its explicit-code helper unchanged. At the two property FAILED branches retain HRESULT, perform the existing `VariantClear`, then throw that same typed failure at20/21. No extra reads, retries, capture, discovery, fallback or success/type predicates. Source review must enumerate remaining checked calls: only the two explicit com_unavailable sites may remain outside this wrapper.

Dedicated top-level catch before generic `Refusal` copies this scalar diagnostic only for a successfully parsed observe request. Preserve inheritance so **all generic catches still catch it**, especially the post-Invoke S_OK → `submitted_state_changed` handler (`:800–802`) and admission-loop rethrow of failures other than `property_unavailable` (`:742–744`). Invoke-mode failures retain generic behavior and never emit this object. Do not alter `receipt.method`: base hr describes Invoke; nested hr describes the failed HRESULT-returning provider operation.

Emitter repeats parsed-observe, code=`provider_unavailable`, raw spent/entered/returned=false, base method=S_OK and exact stage/conditional-property/failed-HRESULT checks. Serialize the closed object using existing canonical scalar atoms/output-only append seam and existing total byte bound/one-write rule. Never add a generic raw JSON Atom or change the request/ledger parser.

Controller accepts **exactly one** optional extension: base only, legacy service scalar, service signature, or this property diagnostic. Mixed/duplicate/unknown keys or schemas refuse. Before independent ledger-spent override, require actual observe mode, confirmed worker exit2 (`type(exit) is int`, not bool), raw three false flags, base hr zero, exact refusal code and closed nested mapping/failed HRESULT. Existing recursive duplicate rejection remains. Preserve prior spent/unknown records, cleanup budgets, lifecycle/leases/owned identities, reservation/intent, all admission/proof/deadline floors and outcome classification. Base-only older stages remain valid; older strict controllers reject the new extension safely. No version change to service diagnostics or worker request/bootstrap/ledger contracts.

## Review and execution boundary

Proposed minimum implementation files: `tools/qa/native_observer/{worker.cpp,controller.py,scenario.py,AGENTS.md}` only. Extend the existing offline exercise, no new registration: every observe-stage ID, all three stage/property pairs, failed-HRESULT endpoints/representative value; reject13–15/unknown stages, missing or forbidden property_id, mismatches/property30031, S_OK/S_FALSE/positive hr, uppercase/length/type errors, bool/float IDs, absent/extra/duplicate keys, mixed extensions, wrong raw flags/code/base hr, Invoke, exits0/3/bool. Preserve legacy acceptance and independently spent ledger assertions. Source review verifies complete call-site coverage, same single evaluation/cleanup and generic/post-Invoke propagation; parser examples alone do not execute C++.

Independent functional/security review precedes implementation; source review precedes root-only build/fresh observe. Older stages and any unclassified base-only failure remain unknown, not evidence of a successful property call. No guessed HRESULT, provider fix, relaxed predicate or positive UIA claim is authorized. The exact QA56 call/HRESULT remain unknown even after implementing this plan; new evidence concerns only the new execution.

Primary local SDK22621 references: `um/UIAutomationClient.h:1597,1644,1668–1737` declare element HRESULT methods; `:1626–1628` GetCurrentPropertyValue takes PROPERTYID/VARIANT output; constants `:898`=30019, `:922`=30031, `:1014`=30077, `:1178`=30159; tree-walker declarations `:3457,3465`. `um/OleAuto.h:143–144,149,159` declares the four HRESULT-returning SAFEARRAY operations. `shared/wtypesbase.h:299–302` defines HRESULT as LONG; `shared/winerror.h:29887` defines FAILED as signed HRESULT<0. Repo anchors are unchanged current source; no network documentation, implementation parser, tests, build, OS/native/app/API/DB or Git mutations were executed.
