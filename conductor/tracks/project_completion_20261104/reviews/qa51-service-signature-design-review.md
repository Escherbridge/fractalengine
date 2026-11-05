---
type: Review
title: QA51 service signature independent design review
date: 2026-11-04
---

# QA51 service signature v1: independent design/readability review

Verdict: **APPROVED for the exact four-file source freeze below. No functional/design blocker found.** The implementation follows the approved diagnostic plan without introducing a generic payload/parser framework or changing service admission. This is source-only approval, not executed scenario, native sampling, positive service/UIA/Invoke/capture, lifecycle, FULL, CI or release acceptance. Security review is a separate lane.

Baseline `2dac90333307c6307eee9382250c080a7f8124fe`. Approved plan SHA256 `1e47fadbb9a66bcea3c862aba532167bd163669bacdf8dbe1455adff230a0dc7` was rechecked. Manifest `.omc/research/qa51-service-signature-v1/manifest.json`: **2072 bytes**, SHA256 **`50d85dfaab7a0061941cf9ae75e0e8636c5026c1f832cecefdc19fa3b3c84b2f`**. Complete patch: **18602 bytes**, SHA256 **`3747ae433d80de155643751fa177b03ef50c40f9ffda3bee10d746057f042bf8`**. All raw snapshots match both manifest and working source exactly.

| Frozen path | Bytes | Approved SHA256 |
| --- | ---: | --- |
| `tools/qa/native_observer/worker.cpp` | 46928 | `17bfd627994e541bcae0f13d0f08dec4761698d539d838b13f9576452b1f539f` |
| `tools/qa/native_observer/controller.py` | 18508 | `462504abcaafee308ca13802c9fdb454bf0b18586f55d1b0110faaf8fa9ee9c8` |
| `tools/qa/native_observer/scenario.py` | 15236 | `4fa400166cf8b305844916e991fecd5bd608f1122171d7afc6de6b07e9de4bac` |
| `tools/qa/native_observer/AGENTS.md` | 31536 | `638784bf17b3e48ef03ccf035483ac3557c36488fd942db17ac36435888cf3e0` |

Read root/native AGENTS and shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`; reviewed the frozen implementation/patch against the plan and supporting protocol/contract flow. The simple typed boundary suits the requested readability and extension requirements; no additional abstraction is warranted.

## Fact ownership and native behavior

Worker :9–17 gives ServiceSignatureFacts exactly two DWORD and two RECT copies, owned by one Refusal-derived subtype. The sole construction at :288 occurs after the same exact service identity checks, successful existing rectangle reads and the existing two style reads. The unchanged classifier at :246–254 preserves style→exstyle→outer→client first-failure order and exact policy. Child checks still occur only after the signature passes. No extra native getter, rediscovery, state refresh, window exemption or tolerance is introduced.

Only the terminal top-level typed catch at :849–851 retains those facts, and only after a successfully parsed observe request. Facts are not cached from a previous validation. Public inheritance preserves generic Refusal catches, including :742's discovery flow and :800's post-Invoke handling. The latter still converts successful method return plus later refusal into submitted_state_changed; it does not attach this observation-only diagnostic. Invoke/unknown-outcome ownership remains intact.

This is a sequential failed-pass sample. Existing reads can race, a zero getter result is not an acquisition/ownership proof, and later predicates are not certified merely because their numeric inputs were read. The directory guide explicitly retains these limitations. The diagnostic provides useful exact numbers for a later policy review without claiming admission.

## Serializer and protocol boundary

Worker :257–275 uses one small signed_coordinate helper and the existing canonical/quote machinery to serialize a closed eleven-member object. LONG/int32 size is asserted; the signed cast and std::to_string preserve negative values and int32 extrema without unsigned wrap or coordinate subtraction. DWORD styles serialize through the existing unsigned number constructor. There are no raw strings, handles, titles, provider/exception text, free-form nested objects or arbitrary caller-supplied JSON.

Emit :818–830 first serializes the known nonempty base Object. Only a retained parsed-observe sample with raw false spent/Invoke/return and zero HRESULT can append the fixed member; the same classifier must return a refusal matching the terminal code. Removing the final base-object brace and appending one fixed member plus closing brace preserves the object shape. The newline is added before the unchanged 256KiB whole-output check and single write. Output member order is not a worker-result acceptance predicate; the canonical ledger is untouched.

Supporting `worker_protocol.hpp`, `contract.py`, `ledger.py`, `windows.py`, `limits.json` and `history.py` were independently compared with the baseline and are unchanged after normal text/EOL decoding. The flat request parser is not extended; no generic raw-JSON Atom or duplicate canonical writer was added. Existing EOF handling, process/lease/receipt ownership and budgets remain outside this patch.

## Controller validation and compatibility

Controller :21–24 names the closed field groups once; :169–183 validates exact nested dictionary keys/schema, true int types, uint32/int32 bounds and first-failure consistency. Existing shared decode rejects duplicate keys recursively before this helper, including duplicates inside the new object. All-passing samples return None and cannot equal an allowed string refusal code.

`worker_output` :186–208 accepts exactly base, base+legacy scalar, or base+new object. Both optional fields together fail the outer shape. The new object requires actual observe=True, exact integer helper exit2, zero HRESULT and raw false flags before classification. Legacy scalar validation remains unchanged, including its older acceptance semantics; absent extensions still work for older sealed workers. An older strict controller refuses the new shape safely.

The actual execute flow :268 calls worker_output with the confirmed worker exit before :272 replaces the returned attempt_spent with the independently pinned ledger result. Reading that ledger beforehand is not a raw-flag override. The new parser cannot clear a previous spend or establish a pristine slot. No Owner token/HTTP/auth path, outcome acknowledgement or retry path was added. Matching policy constants in Python and the native pure classifier are deliberately bound by diagnostic schema v1, with future policy changes requiring the documented version decision.

## Authored journey coverage and practical limits

Only the existing scenario.exercise is extended. New cases call the production worker_output validator. Valid cases cover all four failure stages, both uint32 extrema, each of the eight signed coordinate fields at both int32 extrema, and a multi-failure sample preserving first-stage priority. Invalid cases cover every missing member, wrong numeric types/ranges including bool/float/null/arrays/objects, invalid whole-object types, wrong schema/extra key, earlier-stage disagreement, all-passing facts, wrong code/flags/HRESULT/mode/exit, both legacy/new forms together and nested/top-level duplicates. Original legacy absence/scalar cases remain.

The assertions also retain the already-spent independent fixture ledger while accepting an observe diagnostic whose raw attempt_spent is false. This is a useful parser/state-separation contract, not execution of the complete controller or OS sampling. No new registered test or implementation test run is implied. Generic catch compatibility and signed native serialization were assessed from source separately because Python cases cannot prove them.

Required next evidence remains parent-owned compilation, the existing integrated journey at the authorized sweep, and fresh confirmed observe refusal with actual exact values. No nonzero rectangle allowance or geometry-policy change is approved here. No actual sample is fabricated from test inputs.

This review writes only this file. No source/index/packet edits, implementation parser/test/compiler/formatter execution, app/helper/process/OS/native/API/DB/network action or Git mutation occurred. Hashing, evidence JSON inspection and read-only source/Git comparisons were used. Approval is limited to the exact hashes above; later changes require review.
