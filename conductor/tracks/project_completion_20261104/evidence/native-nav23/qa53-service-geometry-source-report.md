---
type: ImplementationReport
title: QA53 exact service geometry implementation
date: 2026-11-04
---

# QA53 exact geometry: frozen source handoff

Author `/root/legacy_helper_io_fix`; ready for independent review, not approval
or runtime acceptance. Baseline22039d5add074d4df0e18f5847286606d95407ac.
Approved plan SHA-256db3f899d277f93b164f76da745b1c2eb9482ffca63ae7ecce9d6c2d65b303b86;
review SHA-256010b8e32b72fe99b340fd447f23119f9824d4e8af660efc803a51a6a89965e32.

Exactly four owned paths (68 insertions/16 deletions), all under
tools/qa/native_observer:

| Path | Frozen bytes | Frozen SHA-256 |
| --- | ---: | --- |
| worker.cpp | 46992 | `1a55697ecce51498dcf30161d335f9f331d753d4a780ee380699861b7c9ef1e1` |
| controller.py | 18669 | `7d4376866935f95497865e4ff3185f58bace4fa19b4a2f4027289cd0c51b9726` |
| scenario.py | 18385 | `2d49ff87c1692beb2caf3e709668dc865d0a5d4b32f4f440d8e51fddd91464db` |
| AGENTS.md | 32327 | `ea45a9255e78655ef683b330b753d8f6720ce307e71fd6f4c3ce9255913756b2` |

This packet contains four complete before/ and raw/ snapshots. Manifest2611 bytes,
SHA-256`eaf6470e4ba6d7ab1d0f10688e99de8f0f22b9fb6b9d432b720ebe5e0685714b`;
source.patch11543 bytes,
SHA-256`6ae2c708c72c0fb0d5879c10251f297f83db4fe68dae503dfc68036a719588e8`.
Manifest records every before/after hash and per-path change count. Unified patch
is derived from those text snapshots with standard-library difflib, without Git
or importing/parsing/executing implementation modules. Raw EOL bytes are preserved.

The only two C++ line changes are the exact outer predicate and diagnostic schema:
outer.left/top must equal0 and right/bottom16; worker emits
fe-native-service-signature-v2. Zero outer, shifted/negative origins,15/17 or
asymmetric extents refuse. Client remains exactly all-zero. No range, alternate
value, DPI calculation or ignored coordinate is added. Every other C++ byte is
preserved, including native reads, styles, known identity/root thread/children/
census, retained set, watcher/deadlines, proof/Owner floor, COM/ledger/Invoke and
derived Refusal/generic post-Invoke catch behavior.

Controller first validates one of the two literal schemas and the unchanged closed
shape/types/ranges. Only then v1 dispatches to its original exact outer-zero tuple,
v2 to exact(0,0,16,16). Styles/exstyle/client and first-failure order remain;
unknown schemas refuse before dispatch. Observe/confirmed-exit2/raw-false/HR0 and
raw-before-ledger gates, legacy scalar/absence and mutual exclusion remain.
V1 facts/history are not reinterpreted. Old controllers safely reject v2.

Only existing scenario.exercise is extended. Both versions use the same authored
first-failure/extrema cases. Same coordinate/client facts deliberately produce
different first failures across v1/v2; exact origin and15/17 edge cases are added.
V2 also covers all-pass diagnostic rejection, unknown/null schema, every scalar's
wrong types/ranges, contradictory code, raw flags/HRESULT/mode/exit and combined
legacy/new refusal. Existing v1 cases remain; their formerly unknown-v2 test now
uses v3. No new test registration. These are production diagnostic-validator
assertions, not execution of C++ admission, native sampling or full lifecycle.
No implementation parser/test/formatter/compiler/build was run.

Directory rationale explicitly describes an observed-host profile and the
unproved cached-nonclient explanation; no universal DPI/Win32 normalization or
input-free-service assertion. Root must review/build and obtain fresh exact
profile/children/provider evidence. Later refusal remains a gate, not permission
to remove another constraint. No positive service/UIA/Invoke/capture/lifecycle/
FULL/CI/release credit follows.

Other source, Conductor/evidence and the sealed live QA54 app are untouched.
Only assigned files and this packet/report were written; filesystem/text handling
only, no live OS/app/native/API/DB/network action or Git command/mutation.
