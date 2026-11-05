---
type: Review
title: QA53 service geometry source correctness review
date: 2026-11-04
---

# QA53 exact service geometry v1: independent source review

**APPROVED AT SOURCE** for the complete four-path freeze against `22039d5add074d4df0e18f5847286606d95407ac`, implementing approved plan `db3f899d277f93b164f76da745b1c2eb9482ffca63ae7ecce9d6c2d65b303b86`. No correctness/security blocker was found. This approves the restricted observed-host geometry and explicit diagnostic-version change; it grants no compiled/live service, UIA, Invoke or capture acceptance.

Manifest **2611 bytes**, SHA-256 **`eaf6470e4ba6d7ab1d0f10688e99de8f0f22b9fb6b9d432b720ebe5e0685714b`**; complete patch **11543 bytes**, SHA-256 **`6ae2c708c72c0fb0d5879c10251f297f83db4fe68dae503dfc68036a719588e8`**. All four before/raw snapshots match manifest lengths/hashes and current working raw files. The preserved before identities agree with the prior approved source freeze. Exact snapshot identities govern this review without running implementation code.

| Path under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 46992 | `1a55697ecce51498dcf30161d335f9f331d753d4a780ee380699861b7c9ef1e1` |
| `controller.py` | 18669 | `7d4376866935f95497865e4ff3185f58bace4fa19b4a2f4027289cd0c51b9726` |
| `scenario.py` | 18385 | `2d49ff87c1692beb2caf3e709668dc865d0a5d4b32f4f440d8e51fddd91464db` |
| `AGENTS.md` | 32327 | `ea45a9255e78655ef683b330b753d8f6720ce307e71fd6f4c3ce9255913756b2` |

The worker changes exactly two lines: the full outer predicate now requires left/top 0 and right/bottom 16, and emitted diagnostic schema becomes literal v2. Reversing those two lines independently reproduces the entire before worker byte for byte. Zero outer, 15/17/asymmetric extents and shifted/negative origins fail; no alternative, tolerance, DPI calculation or ignored coordinate exists. Client remains all-zero and exact styles remain unchanged.

Consequently all other native reads, identity/class/PID/root-thread/owner/parent/visibility/child checks, census limits, retained optional original set and repeated use boundaries, watcher/deadline/private proof, COM/target, ledger/intent/spent, derived-refusal post-Invoke handling, serializer/bounds and outcome rules are preserved. Only the intended outer geometry admission predicate changes. No live app or sealed stage was touched here.

Controller accepts only the two literal signature schemas after unchanged closed shape and exact numeric type/range checks. V1 uses `(0,0,0,0)` and v2 `(0,0,16,16)`; style/exstyle and client-zero checks and first-failure priority remain the same. Reversing only schema acceptance and this outer-dispatch block reproduces the entire before controller byte for byte. Raw observe/confirmed-exit2/false flags/zero-HRESULT gates precede the independent ledger override; all-pass diagnostic samples and contradictory codes still refuse. Legacy scalar/absence, mutually exclusive extensions, recursive duplicate rejection and immutable helper/stage binding are unchanged. V1 historical facts are not reinterpreted, and old strict controllers reject v2.

Existing scenario extensions sensibly cover both versions' four failure/extrema routes, same coordinate/client facts producing version-specific first failures, exact origin/15/17 edge negatives, v2 all-pass rejection, malformed types/ranges/schema/context and scalar/new mutual exclusion. Changing the old unknown-v2 case to unknown-v3 is necessary compatibility coverage. No new registered journey is introduced. These use the production diagnostic validator but do not execute C++ admission, native reads, process ownership or the full lifecycle; all remain unexecuted source assertions here.

The guide correctly names a restricted observed-host profile, preserves the unknown 16-pixel causal mechanism/non-universal DPI behavior, and avoids an input-free-service claim. It withholds later children/provider/Invoke/capture acceptance and does not treat a later refusal as permission to remove another constraint. Root must build exact reviewed source, perform appropriate meaningful checks and collect fresh matched process/controller evidence before live acceptance.

Only this review was authored. Applied root/local/shared UI guidance. Read-only snapshot/source/hash inspection; no source/packet/Git mutation or Git command, implementation/parser/compiler/formatter/tests/build, apps/helpers, store/DB open, OS/process/native/input/capture, API/network action. Live QA54/55 was not inspected or interacted with. Other authors and publication work were preserved. No finding closure, positive native/UIA/capture, lifecycle/FULL/CI/release acceptance is granted.
