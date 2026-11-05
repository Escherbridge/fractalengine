---
type: Review
title: Bounded service signature facts design review
date: 2026-11-04
---

# QA51 service rectangle diagnostic: independent plan review

Verdict: **APPROVED for source authoring of the exact bounded plan.** No functional/design/privacy blocker found. This is not approval of unimplemented source, geometry policy, native service admission or runtime acceptance.

Plan reviewed: `.omc/research/qa51-service-rectangle-diagnostic-plan.md`, SHA256 **`1e47fadbb9a66bcea3c862aba532167bd163669bacdf8dbe1455adff230a0dc7`**, matching the supplied freeze. Reviewed current worker.cpp, controller.py, scenario.py, native AGENTS and the existing contract/protocol boundaries as supporting source; no implementation was edited or executed. Shared UI/UX guidance applied.

## Why the plan is bounded and useful

Actual QA51 returns `window_service_outer_rect_refused`, helper_exit_code2, all raw invocation flags false and HRESULT00000000. Current `worker.cpp:241–265` reads both RECTs and DWORD styles before the ordered pure classifier. Reaching outer_rect establishes the preceding exact style/exstyle comparisons passed, but supplies no coordinates or later predicate acceptance. Reporting the existing eight signed coordinates and two uint32 styles is the smallest useful direct diagnostic for the next source-policy decision. It grants no positive admission and needs no new OS read.

The eleven-member nested object is closed and versioned: one fixed schema, two uint32 values and eight int32 values. Exact type/range checks prohibit Python bool-as-int, floats, overflow, strings/null, nested payloads and extra/missing keys. Existing `contract.py:47–66` rejects duplicate keys recursively through object_pairs_hook and bounds the total bytes before decoding. Local SDK22621 windef.h:154–159 confirms RECT's four LONG fields; WinUser.h:8985/8998 declares the existing GetClientRect/GetWindowRect calls. Signed decimal serialization avoids the current unsigned Atom number helper and avoids width/height subtraction/overflow. No other-window HWND/class/title, provider text, pixels, arbitrary strings, exception data or credential surface is added.

Observe mode, confirmed helper exit2, exact one of the four signature refusal codes, raw false spent/Invoke/return flags and zero HRESULT constrain where the new object may appear. Recomputing the **first** failing strict comparison against0x94000000/0x080800a0 and zero RECTs prevents internally contradictory diagnostics. The schema version pins that policy explicitly. All-passing facts cannot accompany a signature refusal. Samples remain sequential reads from one failed pass, not an atomic snapshot, proof of stable geometry or proof that a later checked predicate passed.

## Compatibility and source implementation conditions

The plan correctly replaces the worker-local ServiceStyleRefusal subtype with a fixed ServiceSignatureRefusal derived publicly from existing Refusal. Current generic catches at worker.cpp:718 and :776 must continue to catch the subtype; especially the post-Invoke S_OK path must retain submitted_state_changed. The dedicated top-level typed catch must precede the generic catch and retain facts only for a successfully parsed observe. No sticky sample or generic exception payload belongs in the protocol.

The small worker-local output serializer may append its one fixed named numeric member after the existing canonical base object is serialized, then perform the existing whole-output byte check and newline/write. It must preserve valid delimiters/quoting for negative coordinates and use the fixed field set, without adding raw-JSON Atom support, generic recursion or a second copy of the canonical writer. This is an output extension only: worker_protocol.hpp's flat request parser and canonical durable ledger remain unchanged. Output member order has no existing controller acceptance meaning; durable ledger canonical order still does.

Controller `worker_output` is currently called at :242 **before** the independent ledger override at :246. New diagnostic validation belongs there, against the raw helper flags plus actual operation/confirmed exit. Existing spent ledger ownership must remain unchanged afterward, including a previously spent ledger on an observe result. Accept exactly base, base+legacy scalar, or base+new object; reject both optional forms together. Preserve legacy scalar validation and absent-field acceptance for older sealed stages; old controllers appropriately refuse the extended object. Do not impose new first-failure semantics retroactively on legacy scalar output.

Four author paths are sufficient: worker.cpp, controller.py, scenario.py's existing exercise, and native AGENTS. No new transport, endpoint, native query, request/proof/lease capability, identity refresh, geometry tolerance or capture authority is warranted. Service admission still requires its unchanged exact predicates and same-set retained identity checks.

## Required subsequent evidence

Existing journey extensions should cover the complete nested key set and duplicate nested keys; schema/types/ranges including uint32/int32 extrema and negative values; all four matching first-failure cases and earlier-failure disagreement; all-passing samples; raw flags/HRESULT/mode/exit gates; and mutually exclusive legacy/new plus legacy absence. These cases exercise actual controller parsing only, not native sampling or full controller lifecycle.

Independent source review must separately inspect the sample's sole origin, exception inheritance/catch ordering, signed serialization/total output bound, raw-before-ledger validation and preserved post-Invoke semantics. Root must build and obtain fresh exact-value evidence before any separate nonzero-rectangle policy proposal. A refusal sample cannot certify positive service/UIA, Invoke, WGC/capture, lifecycle, FULL, tests, CI or release.

This review writes only itself. No implementation/parser/compiler/formatter/test execution, OS/process/app/helper/native action, API/DB/network request or Git mutation occurred. Current source was read; SDK declarations were read locally. No changes to the immutable plan or QA51 packet were made.
