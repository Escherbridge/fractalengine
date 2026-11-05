---
type: Review
title: Provider diagnostic original source review
date: 2026-11-04
---

# QA56 isolated provider diagnostic candidate review

Verdict: **approved as an isolated source candidate for later exact integration and verification**. The four-file candidate implements the approved diagnostic plan without a concrete correctness/security blocker found in this review. This does not authorize changing the live GUI57 source, supply compiler/parser/test results, localize QA56's old failure, or grant native acceptance. Root owns the stop/integration/build/fresh execution sequence.

Only this report was written. No production/candidate/packet source, Git state, process, app, API or DB was changed; no implementation parser, formatter, compiler, test, native/runtime or network operation was run. Root/local native-observer guidance and shared UI guidance were applied. The candidate, rather than the root production files, was reviewed.

## Exact source and production freeze

Packet `.omc/research/qa56-provider-diagnostic-v1` records baseline `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| manifest.json | 3127 | `a9e802958b033c53e5b9a8248a715699559ba08d552e9be5cf3f4d807b4b5b38` |
| source.patch | 29646 | `7fddaf18865e0f05f8269335afa596ac10f5e4a2abddb864bdb40b86201fe344` |
| candidate worker.cpp | 51181 | `f2ee668d464f349fb7fdff996578ca3a9cab4fc62ad8dfec4391abf86f4b6b42` |
| candidate controller.py | 20121 | `9b3eb778c98bf1f4f9f3464f248ad0162dd4b72d1113fcf9dcb2a8089ead2c87` |
| candidate scenario.py | 23314 | `7a0bc60dc2e783733a4c2cb3c8f0080cf709ae8aa4a691d5dd0f0a7a80d0e4b3` |
| candidate AGENTS.md | 35755 | `f330612d8754930329c5dbd7bb781b4b4d25075d82ac22dda0d612f64e68d490` |

All raw and before files match manifest sizes/hashes. Both plan/review anchors match: approved plan `ec6352d0a161092b5ee41e7abc2f850bde519593d0cce1607cb5daacc24f74bd`, plan review `063a4d85701d8f46484548a4ab3aa8f341b16ae7b448d5887ee7a4413a9b5f3f`. All four root production files still match their frozen before bytes exactly; they have not adopted the candidate. The complete patch and affected worker/controller/scenario context were read.

## Worker coverage and unchanged behavior

All thirty original default `checked(...)` expressions are replaced once with the worker-local `checked_provider(expression, ProviderStage::...)`; the candidate has thirty call sites plus its wrapper definition. Only the two original explicitly categorized COM calls remain as `checked`, unchanged. Manual comparison of the full replacement hunks confirms the method-stage mapping for AutomationId, runtime ID, four SAFEARRAY operations, CompareElements, ProcessId/ControlType/NativeWindow/Enabled/Offscreen, Name/Rectangle/Pattern, child/sibling traversal and RawWalker/ElementFromHandle. No original call was duplicated, removed or reordered. The two property-read FAILED branches keep their original status and VariantClear and now throw the same derived diagnostic family with their actual fixed SDK property IDs.

`checked_provider` retains the exact FAILED condition and `provider_unavailable` base code. `ProviderFailure : Refusal` carries only fixed enum stage, original HRESULT and optional PROPERTYID; it has no ambient mutable stage, arbitrary location string or provider content. No COM/native read, discovery, retry, action, fallback, success/type condition or admission predicate is added. The protocol header, request/ledger parsers, retained process/COM/target ownership, watchdog/deadlines, watcher, window policy, original proof and Invoke path remain unchanged apart from substituting typed failure at existing calls.

The observe emitter's allowlist (`worker.cpp:38-59` in candidate) rejects stages13–15 and admits only1–12/16–19 without property, stage20 with IsPassword/WindowIsModal, or stage21 with FullDescription. Invoke-only IsInvokePatternAvailable is not accepted. This matches actual reachability: even the root rectangle read is conditional on an actual target/retained target. The local SDK constants used are the reviewed30019/30077/30159; no new arbitrary numeric property becomes observable.

The outer typed catch precedes generic Refusal and retains diagnostic facts only after a parsed observe request. The actual admission-loop catch and entire Invoke/result/post-Invoke catch block compare exact to the before snapshot. Consequently the derived failure still rethrows through the admission loop unless it is the pre-existing `property_unavailable` case, and a refusal after S_OK Invoke still becomes `submitted_state_changed`. Invoke-mode failures keep generic base behavior and never emit the new object. No successful method HRESULT is replaced with a provider HRESULT and no spent/unknown state is cleared.

`emit` at candidate lines868–898 repeats the parsed-observe/raw-not-spent/not-entered/not-returned/base-method-S_OK/code/failed-HRESULT/allowlist conditions. It appends only the closed scalar object through the existing canonical-output seam, preserves the262144-byte bound and one WriteFile rule, and remains exclusive with the service signature extension. Base hr remains the Invoke result; nested hr encodes the already-read signed-32-bit failure as eight lowercase hexadecimal digits. There are no provider names, returned values/types, paths, IDs for elements, runtime IDs, handles or exception messages in this new object.

## Controller boundary and compatibility

`validate_provider_failure` at candidate controller lines190–200 requires an exact dict/schema, exact int stage (excluding bool/float/string), failed eight-character lowercase hex hr, and exact conditional key/property set. Ordinary stages require property absence; property stages require the corresponding fixed property and reject null/zero/unknown/mismatched IDs. Unknown or Invoke-only stages fail closed. Existing recursive duplicate-key rejection applies to nested and top-level objects.

`worker_output` accepts exactly base, base+legacy scalar, base+service signature or base+provider failure. Mixed/unknown extensions fail. The provider branch requires actual observe, terminal exit2 with exact int type, all three raw false flags, exact refusal code and base hr zero before nested validation. Existing service/legacy branches are preserved. The entire `execute` function and later source compare exact to before: actual worker terminal receipt still precedes decoding, raw validation at lines291–292 still precedes the independent ledger override at line296, and live-file checks, durable admission, outcome classification and cleanup/unknown ownership remain. Thus diagnostic raw flags cannot discharge an independently spent attempt. Base-only older workers still work; strict older controllers reject unfamiliar extensions safely.

## Authored cases and remaining gates

The existing exercise imports and extends the actual controller validator without adding another registration. It authors all sixteen ordinary allowed stages plus the three allowed stage/property combinations with failure endpoints and a representative HRESULT; it also retains a base-only legacy provider refusal. Negative cases cover missing/extra/wrong schema, stage/property exact types and forbidden combinations, Invoke-only/unknown stages, positive/malformed/uppercase HRESULTs, non-object samples, raw code/base-hr/flag mismatch, mixed extensions, observe-mode mismatch, wrong/bool/non-int exit codes and recursive duplicate keys. The assertions retain a separately spent ledger while requiring the parsed raw `attempt_spent` to remain false. These are meaningful authored parser-boundary cases, not evidence that C++ or Python has executed. The unchanged existing post-Invoke behavior was assessed in actual C++ source, not inferred from those cases.

No concrete C++ type/constructor/optional/comparison or Python validation-flow blocker was found by inspection. Compilation and exercise execution remain unproved. After the live-source freeze ends, root should verify exact candidate integration, build with the pinned recipe, run the planned final checks and obtain a fresh observe result. Any new diagnostic describes only that execution; QA56's failed call/property/HRESULT/cause remain unknown and its external clarification remains mandatory. No provider repair, relaxed gate, positive native root/witness/Invoke/capture, peer, retained restart, CI/release or FULL credit follows from this source approval.
