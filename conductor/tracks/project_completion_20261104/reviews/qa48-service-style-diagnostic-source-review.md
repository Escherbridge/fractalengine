---
type: Review
title: Bounded service style diagnostic source review
date: 2026-11-04
---

# QA48 optional style diagnostic: independent source review

**APPROVED AT SOURCE** for all four frozen paths against baseline `a16eab87032a41d012e6936776d8dcd9228b7a54`. No concrete correctness/security blocker was found. Root may commit/build and collect matching fresh evidence; no implementation was executed and no actual style value or admission-policy change is approved.

Manifest SHA-256 **`b3dbc57a872c51bd1aa12027d906a785c60ecd48c7f91779009270e483fd7c50`** (1949 bytes); full diff SHA-256 **`e8c0ce8f28924f0aaa9c53fe5ee86e49e258ac4e5ba077b412282dd16c3b89c3`** (11040 bytes). All four frozen raw/current working files match manifest lengths/hashes; baseline and current HEAD source blobs are equivalent after newline normalization. Author report SHA-256: `6fe452d8f0c0a6b6938bc8acad6857387ea877ad3b11dbe94ebc18a0faa2aee3`.

| Path under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 45633 | `8c7f7bffbcc011aa15c7fc2bfb77090b4067d6af32e1222aa4b5c999d31c27bc` |
| `controller.py` | 16856 | `5c7b1842df9745805981bb1d9f91befde01f9434a2db3566ae253030cf068f56` |
| `scenario.py` | 10143 | `0b6f2482fdbde1ed2a73f13f34b00156760672f4cbc3719484eaa6e95b591252` |
| `AGENTS.md` | 29298 | `bbfab95de071abfd811a7875faa9d9573b073688b621a920e17ae14fe77dcd44` |

The worker's local `ServiceStyleRefusal` publicly derives from `Refusal`, fixes its code in the constructor and carries only a DWORD. Its sole throw site follows the same exact recognized-service identity and successful rectangle reads and uses the normalized style value passed to the unchanged strict classifier. Naming the existing two style reads introduces no extra read or tolerance. Their formerly unspecified function-argument evaluation is now explicit local sampling; neither implementation claims an atomic native fact pair or timeline. All strict comparisons/order, census, retained-set/watcher boundaries, proof/deadline and COM/intent/Invoke paths remain.

The existing post-Invoke `catch (const Refusal&)` catches this subtype, preserving S_OK -> `submitted_state_changed` and existing failure outcome semantics. It does not reach the top-level diagnostic catch from that route. The top-level catch precedes the generic catch and only stores the value for `parsed_observe`, which defaults false and is assigned only after successful request parsing. Emit additionally requires exact style-refusal code, raw unspent/no-Invoke/no-return flags and zero HRESULT. `std::optional` presence correctly includes DWORD zero; numeric emission uses existing unsigned `Atom::number` rather than strings. Malformed/invoke/success/other-refusal routes cannot carry the field, and worker exit remains 2 for this observe refusal.

Controller validation accepts exactly the original keys or those keys plus `service_style_u32`. The existing bounded unique-key decoder remains; the added value requires exact Python int, inclusive uint32 range, actual observe mode, exact integer helper exit 2, matching code/HRESULT and all three raw flags false. The production call supplies explicit actual mode and exit only after confirmed `worker_exited`; checks happen **before** final independent ledger-spent replacement. No ledger reset, new capability, positive observation or relaxed invocation outcome follows. Legacy sealed workers without the field remain accepted, while old strict controllers fail closed on the extension. Search found all production/scenario callers updated; no implicit mode defaults or stale call remains.

The existing scenario exercises the actual parser with legacy absence, zero/max/controlled representative values, invalid types/ranges, wrong code/HRESULT/flags/key/mode/exit and duplicate keys. Its independent spent-ledger assertion is honestly offline; it does not execute the controller's full process route or native sampling. No new journey registration was added. These cases were not parsed/run here. The representative `0x94000000` is controlled test data, not evidence of QA48's value.

Only this review was authored. Read-only source/Git/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, app/helper, OS/process/native/input/capture, API/DB/network action. Parallel root/numeric/evidence work and session log were preserved. Approval covers only these four paths and an optional exact-body publication review copy.

Root must build exact reviewed source, execute appropriate meaningful checks and retain a fresh owned failed-code/exit/integer result. Zero remains a sample without proof of successful native style acquisition. The diagnostic does not prove later exstyle/rectangle/child predicates or service admission, identify any prior run's bitset, or authorize masking a flag. SDK interpretation and any eventual strict-policy change require their own review. Positive UIA/Invoke, native lifecycle, capture, FULL campaign, final sweep, CI and release gates remain separate.
