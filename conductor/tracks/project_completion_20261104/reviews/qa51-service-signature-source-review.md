---
type: Review
title: QA51 service signature independent source review
date: 2026-11-04
---

# QA51 signature-facts diagnostic: independent security/correctness review

**APPROVED AT SOURCE** for all four frozen paths against `2dac90333307c6307eee9382250c080a7f8124fe`. No concrete correctness/security blocker was found. This implements the approved bounded diagnostic plan, not a geometry-policy or admission change. Root may commit/build after exact staging; native sampling, controlled cases and positive acceptance remain unexecuted here.

Manifest **2072 bytes**, SHA-256 **`50d85dfaab7a0061941cf9ae75e0e8636c5026c1f832cecefdc19fa3b3c84b2f`**; complete patch **18602 bytes**, SHA-256 **`3747ae433d80de155643751fa177b03ef50c40f9ffda3bee10d746057f042bf8`**. All four frozen raw lengths/hashes match actual working bytes and manifest, and source bases remain equivalent to HEAD after newline normalization. Frozen source report SHA-256: `c2530ff621731f9290003e8351c4b9edc1eb82dea802aada2a5da8e605f28317`. Approved original plan is `1e47fadbb9a66bcea3c862aba532167bd163669bacdf8dbe1455adff230a0dc7`; canonical OKF copy/review identities agree with the freeze.

| Path under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 46928 | `17bfd627994e541bcae0f13d0f08dec4761698d539d838b13f9576452b1f539f` |
| `controller.py` | 18508 | `462504abcaafee308ca13802c9fdb454bf0b18586f55d1b0110faaf8fa9ee9c8` |
| `scenario.py` | 15236 | `4fa400166cf8b305844916e991fecd5bd608f1122171d7afc6de6b07e9de4bac` |
| `AGENTS.md` | 31536 | `638784bf17b3e48ef03ccf035483ac3557c36488fd942db17ac36435888cf3e0` |

## Sole origin, catches and serialization

`ServiceSignatureFacts` copies only the existing normalized two DWORD reads and two RECT results, after unchanged known-service identity and successful rectangle reads. The unchanged pure classifier supplies the first of four fixed failure codes. Its sole typed throw stores those exact sequential facts; there is no additional native call, later rediscovery, arbitrary payload, title/class/handle or other-window data.

`ServiceSignatureRefusal` derives publicly from `Refusal`. The unchanged post-Invoke generic catch therefore preserves S_OK -> submitted-state-changed and existing failed-Invoke handling rather than promoting this exception to a new outcome. The dedicated top-level catch stores facts only after parsed-observe was established. Emit repeats parsed mode, raw no-spent/no-Invoke/no-return and zero-HRESULT checks, then recomputes the same first failure and requires the terminal code to match. Identity/read/child/watcher failures, malformed requests, invoke, successful results and contradictory/all-passing facts cannot emit the object.

The fixed eleven-member output serializer uses two existing unsigned numeric Atoms and eight local signed-coordinate Atoms. SDK RECT has four LONG members; the width assertion and int32 cast preserve signed extrema, and trusted decimal `to_string` avoids unsigned wrapping or raw input injection. It reuses canonical scalar-object quoting, then appends one fixed named object to the serialized known nonempty base object. The base's final brace is guaranteed by the existing canonical writer. Output member order is not a result acceptance predicate; durable ledger canonicalization is unchanged. Existing final newline, 262144-byte output limit and checked write remain. No generic raw-JSON Atom, recursive request parser, generic writer copy or protocol-header change is introduced.

## Closed controller validation and compatibility

The controller accepts exactly base, base plus legacy scalar, or base plus new object; both extensions together refuse. The existing bounded JSON decoder rejects duplicate keys recursively. Nested object requires the exact schema and eleven keys; exact int types exclude bool/float/string/null/array/object, with inclusive uint32 styles and int32 coordinates. It recomputes style `0x94000000`, exstyle `0x080800a0`, outer-zero then client-zero first failure, requiring equality with the already allowed fixed terminal code. All-passing samples return no failure and cannot match a valid result code.

Actual observe mode, confirmed integer helper exit 2, raw false flags and zero HRESULT are checked before the existing independent ledger-spent replacement. The production caller still follows confirmed worker termination and live file/pin checks; the sample cannot reset a prior spent slot or confer permission. Legacy absent/scalar results remain accepted under their original checks, without retroactively imposing the new profile on sealed older workers. Old strict controllers reject the new shape. Diagnostic schema v1 explicitly pins current constants/order; future policy changes require an explicit version decision.

All admission/style/exstyle/geometry, original root/service presence/identity, aggregate bounds, watcher, private proof/deadline, same COM/target, durable intent/ownership, Invoke and success/exit behavior remain unchanged. Independently checked unchanged protocol header, contract, ledger, Windows process/lease adapter and build script against the baseline. Sequential sampled facts are not an atomic snapshot, stable timeline, successful style-read proof or later-predicate acceptance. No capture or Owner HTTP floor is weakened.

## Meaningful unexecuted coverage and boundary

Only the existing scenario is extended. Production parser cases cover all four first failures, mixed-failure priority, scalar extrema/negative coordinates, wrong types/ranges/schema/shape/missing/extra/duplicates, mismatching/all-passing samples, mode/code/flags/HRESULT/exit, mutually exclusive extensions and legacy cases. Offline spent-ledger independence remains explicitly separate from full controller/process or post-Invoke runtime execution. No new registered journey/harness is added and none of these cases was run here.

Only this review was authored. Applied root/local guidance and shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Read-only source/Git/SDK/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, apps/helpers, OS/process/native/input/capture, API/DB/network action. Other authors, packet publication and session log were preserved. Root owns exact commit/build, meaningful checks and fresh failed-code/signature evidence. Any nonzero-geometry policy requires separate evidence/review; UIA/Invoke/capture/lifecycle/FULL/CI/release gates remain open.
