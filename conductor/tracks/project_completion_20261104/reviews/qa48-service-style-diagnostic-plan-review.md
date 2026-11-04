---
type: Review
title: Bounded service style diagnostic plan review
date: 2026-11-04
---

# QA48 style diagnostic plan: independent security/design review

**APPROVED AT DESIGN SCOPE** for `.omc/research/qa48-service-style-diagnostic-plan.md`, SHA-256 **`fe9ca08e23a9c7743a02ff13592fce3575a5bd7d0f4ef685129b264ef7a36565`**. No blocking plan defect was found. This permits the specified four-path authoring for independent source review; it approves no implementation, style-policy change, build or runtime result.

Only this review was authored. Read-only plan/source inspection; no production/Git mutation, implementation/parser/compiler/tests/build, app/helper, OS/process/native/input/capture, API/DB/network action. Root's QA47/48 evidence and parallel numeric UI investigation were preserved.

The extension is bounded and justified by the failed-comparison diagnostic need: one optional JSON uint32 from the normalized style value already read for the exact known service. It must not perform another native read, expose other windows or introduce raw names/classes/handles/title/error text, a selector, generic payload or new action channel. Exact base-or-base-plus-one key sets, duplicate-key rejection and `type(value) is int` with inclusive uint32 bounds are appropriate. Explicit bool/float/string/null/overflow/unknown-key rejection is necessary; generic Python integer-subclass acceptance would violate the plan.

Worker emission and controller acceptance are restricted to **parsed observe mode**, exact style-refusal code, raw worker no-spent/no-Invoke/no-return flags and default-zero HRESULT, with actual confirmed helper exit 2 on the controller route. No success/other refusal/invoke/malformed-request case may emit or accept it. The parsed-observe flag must default false and be set only after successful request parsing. Controller checks apply before its independent ledger-derived spent override; a previously spent slot may remain spent during observe and must never be reset by this diagnostic.

One implementation requirement deserves explicit source scrutiny: the local typed style exception must remain **catch-compatible with generic `Refusal`**, for example as a subtype, so the existing post-Invoke catch still handles it and preserves `submitted_state_changed` after successful Invoke. It must not bypass that catch, replace an invocation outcome, or leak a diagnostic into invoke mode. The dedicated top-level typed catch goes before the generic catch; generic behavior elsewhere remains. This follows the plan's stated requirement to preserve generic/post-Invoke catches, rather than authorizing a new outcome route.

Named read locals must carry the exact sampled DWORD used by the unchanged strict predicate, not a later re-read or global/sticky value. The result is one terminal mismatch sample, not an atomic style timeline or evidence that the later extended-style/rectangle/child comparisons passed. Initial and repeated service checks may originate it under the same restrictions. No scope/proof/deadline/COM/retained-window/ledger/Invoke/capture change is justified. A zero sampled value also must not be reinterpreted as proof of successful native style acquisition or a safe admission policy.

Compatibility is fail-closed: absence remains valid for sealed older workers; old strict controllers reject an extended result. Keep exact stage/helper pinning, existing byte caps, request/ledger/bootstrap schemas and success/exit semantics. Field presence is no capability or positive service/UIA/action acknowledgement. Keeping result schema v1 with precisely this optional extension does not permit arbitrary future keys.

The existing offline scenario can meaningfully exercise the production parser with min/max/representative mismatch, all invalid types/ranges/keys/duplicates, wrong operation/code/flags/HRESULT/exit, legacy absence and spent-override independence. Source review must inspect actual worker sole-origin and post-Invoke exception routing as well; parser cases alone cannot establish those native/control-flow properties. No cases were executed here. Root owns exact reviewed build and fresh failed-code/exit/value evidence, followed by SDK interpretation and a separately reviewed policy decision if warranted. QA48 currently identifies only the failed comparison; no particular extra bit or style correction is inferred.
