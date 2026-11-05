---
type: Plan
title: Bounded service signature facts diagnostic plan
date: 2026-11-04
---

# QA51: fixed service signature snapshot diagnostic plan

Source plan only, 2026-11-04; independent review before implementation.
QA51 result `qa/passes/51/owner-native-observe-exact-style.jsonl` is598 bytes,
SHA-256 `50113619ae694b6f1790a265dc3705e03c0419e25a3124a59b22eb8e036851d0`.
It reports window_service_outer_rect_refused/exit2 with no Invoke. Compiled ordering
establishes both rectangle reads succeeded and exact style0x94000000/exstyle
0x080800a0 passed, then the outer-zero comparison failed. Actual coordinates,
client-zero/children/service admission/UIA remain unproved. No geometry relaxation.

## Closed, versioned optional result

Keep existing result schema and base keys. Add one optional `service_signature`
JSON object with **exactly eleven** members:

- schema: literal `fe-native-service-signature-v1`;
- style_u32 and exstyle_u32: JSON integers0..4294967295;
- outer_left_i32, outer_top_i32, outer_right_i32, outer_bottom_i32;
- client_left_i32, client_top_i32, client_right_i32, client_bottom_i32.

Each coordinate is a JSON integer -2147483648..2147483647. Reject bool, float,
null/string, missing/unknown/duplicate keys, overflow, arrays and nested payloads.
No width/height subtraction or derived coordinate interpretation. The fixed
object contains no HWND, class/name/title, pixel, error text or other-window fact.

Allow it only for actual parsed observe, confirmed helper exit2, exact one of
style/exstyle/outer_rect/client_rect signature-refusal codes, raw attempt_spent/
invoke_entered/method_returned=false and HRESULT00000000. Check before independent
ledger-spent replacement; never clear earlier spent ownership. No identity/query/
rectangle-read/child/watcher failure, success, malformed request or invoke output
may carry it. Reject code/sample mismatch by recomputing the first failed strict
comparison in the existing order against0x94000000/0x080800a0 and zero RECTs.
All-passing samples cannot accompany a signature refusal. This validates only the
diagnostic, never substitutes a successful profile/admission path.

## Same reads and catch-compatible propagation

Use one small local ServiceSignatureFacts containing the existing two DWORD and
two RECT copies after current known service identity checks and successful reads.
On a non-null pure classifier result, throw local ServiceSignatureRefusal deriving
from Refusal, retaining that fixed code and the facts. Replace the narrower style
subtype; do not add generic payloads to protocol Refusal. Preserve all generic
catches, particularly S_OK post-Invoke -> submitted_state_changed. A dedicated
top-level typed catch retains facts only when parsed_observe was successfully set;
emit repeats the strict mode/code/raw-flag/HRESULT gates. No sticky state, later
read, target discovery, exception text, new native call or proof renewal.

These are sequential already-read facts from one failed validation pass, **not
an atomic OS snapshot or unchanged-state timeline**. Earlier reads could race;
do not infer client/children or any predicate after the primary failure passed.

## Minimal authoring and compatibility

Four paths: worker.cpp, controller.py, existing scenario.py::exercise and native
AGENTS.md. Keep worker_protocol.hpp/request parser flat and unchanged. For output,
a fixed worker-local serializer builds this one typed numeric object using existing
quote/canonical helpers and decimal formatting of int32 coordinates. Append its
fixed named member to the already serialized base result object, without a generic
raw-JSON Atom/recursive parser or copying the generic writer. Preserve valid JSON
and existing total output bound; member order is not an acceptance predicate for
worker results, unlike the separately canonical durable ledger. No free-form JSON
input reaches this output-only serializer. No ledger serialization change.

Controller accepts exactly base, base+legacy service_style_u32, or base+new object;
never both. Preserve legacy field validation and absent-field acceptance for older
sealed workers. New worker emits only the new object on the four failures. Old
controllers fail closed. New-object schema v1 pins these exact comparison values;
future profile-policy changes require an explicit diagnostic version decision.
Stage/helper hashes and all leases/caps/ownership/request/proof/Invoke rules stay.

## Independent review and acceptance

Extend only the existing offline journey: exact nested shape; uint32/int32 extrema
including negatives; wrong types/ranges/schema/keys/duplicates; all four matching
first-failure cases; inconsistent/all-passing cases; wrong mode/code/flags/HRESULT/
exit; mutually exclusive legacy/new and legacy absence. No new test registration.
Review actual sole-origin and generic post-Invoke catch compatibility separately;
parser cases cannot prove native sampling or full controller execution.

Installed SDK22621 shared/windef.h154-159 defines RECT's four LONG members;
um/WinUser.h8985,8998 declares the existing client/window reads. Root alone builds
and obtains fresh failed-code/exact-values evidence. Any nonzero rectangle policy
proposal must follow those measurements and separate review. No positive service,
UIA/Invoke/capture/lifecycle/FULL/CI/release credit. Only this plan was written;
no implementation, compiler/parser/test, OS/app/helper/API/DB/network or Git action.
