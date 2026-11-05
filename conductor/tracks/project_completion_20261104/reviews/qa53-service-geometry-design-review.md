---
type: Review
title: QA53 service geometry source design review
date: 2026-11-04
---

# QA53 exact service geometry v1: independent design/source review

Verdict: **APPROVED for the exact four-file source freeze below. No functional/design blocker found.** The patch implements the approved restricted observed-host profile, with explicit diagnostic-version semantics. It is not positive native service/UIA/provider/Invoke/capture acceptance. Fresh parent-owned build and runtime observations remain required; the extended journey has not been executed by this reviewer.

Baseline `22039d5add074d4df0e18f5847286606d95407ac`. Manifest `.omc/research/qa53-service-geometry-v1/manifest.json`: **2611 bytes**, SHA256 **`eaf6470e4ba6d7ab1d0f10688e99de8f0f22b9fb6b9d432b720ebe5e0685714b`**. Patch: **11543 bytes**, SHA256 **`6ae2c708c72c0fb0d5879c10251f297f83db4fe68dae503dfc68036a719588e8`**. All four raw and before snapshots match manifest hashes; all working sources match raw snapshots, and baseline Git text matches before copies. Read root/native AGENTS and shared UI guidance; no source or evidence packet was changed.

| Frozen source | Bytes | Approved SHA256 |
| --- | ---: | --- |
| tools/qa/native_observer/worker.cpp | 46992 | `1a55697ecce51498dcf30161d335f9f331d753d4a780ee380699861b7c9ef1e1` |
| tools/qa/native_observer/controller.py | 18669 | `7d4376866935f95497865e4ff3185f58bace4fa19b4a2f4027289cd0c51b9726` |
| tools/qa/native_observer/scenario.py | 18385 | `2d49ff87c1692beb2caf3e709668dc865d0a5d4b32f4f440d8e51fddd91464db` |
| tools/qa/native_observer/AGENTS.md | 32327 | `ea45a9255e78655ef683b330b753d8f6720ce307e71fd6f4c3ce9255913756b2` |

## Actual evidence and approved scope

The actual QA53 894-byte result `qa/passes/53/owner-native-observe-signature-facts.jsonl` independently matches SHA256 `7487e2a56ce79db0976bf681dee84203785a83d1376f74cb89d9a98ea681624a`. Its v1 signature sample is style0x94000000/exstyle0x080800a0, outer(0,0,16,16), client(0,0,0,0), strict outer refusal/exit2 and no Invoke/return/capture. This supports the exact observed profile, not an atomic geometry snapshot, later child/provider acceptance or a universal Win32/DPI rule.

The source plan SHA256 `db3f899d277f93b164f76da745b1c2eb9482ffca63ae7ecce9d6c2d65b303b86` and independent policy review SHA256 `010b8e32b72fe99b340fd447f23119f9824d4e8af660efc803a51a6a89965e32` match their supplied identities. They explicitly authorize replacing the outer-zero assumption with one exact(0,0,16,16) profile and diagnostic v2. This review does not reopen the approved policy or add an unrequested causal-proof gate. Cached nonclient geometry remains a plausible explanation, not established cause; zero client area is not proof of an inability to receive messages/input.

## Classifier, version dispatch and retained boundaries

Worker production changes are exactly two lines: :252 requires all four outer coordinates equal0,0,16,16; :264 emits fe-native-service-signature-v2. There is no zero-or16 alternative, maximum size, width/height arithmetic, ignored origin, tolerance or DPI-derived normalization. Shifted/negative origins, zero outer and15/17/asymmetric extents fail. The same pure classifier is used by actual service validation (:287) and diagnostic emission (:822), so current worker policy and sample version are synchronized.

Exact style/exstyle, all-zero client and first-failure order are unchanged. All process/class/root-thread/owner/parent/visibility/children checks, complete bounded window census and original optional service HWND/thread retention remain outside the two-line worker delta. No new native call, action, target discovery, proof/deadline refresh, capture permission or positive result is added. The service does not become an actionable UIA target.

Controller :169–183 first permits **only literal v1 or v2**, then selects exact outer zero for v1 or exact outer16 for v2. Its else branch is reached only after that closed literal check, so unknown versions have no fallback. Style/exstyle/client ordering is common and unchanged. Historical QA53/v1 outer16 remains an outer refusal under the new controller; its history is not reinterpreted. V1 zero geometry is accepted only for interpreting v1 refusal diagnostics, not as an alternate current-worker admission profile.

The exact eleven-member nested schema, true-int type/range checks and recursive duplicate-key decoder remain. Legacy scalar/absence and mutually exclusive extensions remain unchanged. Raw observe/confirmed exit2/false spent-Invoke-return/zero HRESULT gates still run before independent ledger override (:269→273). All-passing signatures return None and cannot validate a refusal object; that is a diagnostic consistency rule, not a new success path. The fixed signed serializer, total output bound, sole-origin facts and Refusal-compatible post-Invoke behavior are unchanged from the prior reviewed source.

Supporting worker_protocol.hpp, contract.py, ledger.py, windows.py, history.py and limits.json were compared with the explicit baseline and remain text-identical. The request parser, EOF handling, durable attempts, lifetime/cleanup budgets and invocation ownership have not changed in this slice.

## Readability and authored journey

The implementation remains minimal: one explicit native rectangle comparison and one small closed-version selection in the existing Python classifier. The field-name grouping and fixed serializers are reused; no policy registry, generic geometry matcher, parser fork or additional framework is warranted. Directory guidance clearly distinguishes old/current rules, the restricted environment and unproved causes.

Existing scenario.exercise now runs the original uint32/int32 boundary cases for both versions. Same-facts cases distinguish outer versus client first failure depending on version; v2 near-miss cases exercise shifted origins and15/17/asymmetric extents. New v2 negatives cover all-pass refusal rejection, unknown/null schema, extra keys, numeric type/range errors, wrong result code/HRESULT/raw flags, both optional fields, wrong mode and wrong exit. Existing v1 missing-key/duplicate/earlier-failure and legacy absence/scalar cases remain; their shared shape/decoder gates are unchanged.

These cases invoke the production controller validator, so their assertions are meaningful contract coverage rather than a separate validator imitation. They are authored source only: they do not execute the C++ predicate, sample native windows or prove full controller/native acceptance. Parser acceptance of a correctly labeled failure means the diagnostic is consistent, not that its geometry was admitted. Source inspection separately establishes the native exact predicate; the parent must obtain fresh compiled exact-profile/children/provider evidence. Any next refusal remains a real unresolved gate.

This review writes only this report. No implementation parser/test/compiler/formatter/build, app/helper/process/OS/native/input/capture, API/DB/network action or Git mutation occurred. Evidence JSON/hash inspection and read-only source/Git comparison were used. No FULL, metric, peer/restart, integrated tests, CI or release credit is granted. Approval is limited to the exact hashes above.
