---
type: ImplementationReport
title: QA51 bounded service signature implementation
date: 2026-11-04
---

# QA51 fixed service signature diagnostic: frozen source

Source author `/root/legacy_helper_io_fix`; ready for independent review, not
self-approved. Baseline `2dac90333307c6307eee9382250c080a7f8124fe`.
Actual directory is tools/qa/native_observer; tools/qa/native does not exist.
Read canonical root/local AGENTS, shared UI guidance and canonical Conductor
plan/review. The manifest records approved original plan SHA1e47fadb..., canonical
OKF plan SHA94579269... and review SHA7e110f8d... without modifying them.

Exactly four source/doc paths, 182 insertions/20 deletions:

| tools/qa/native_observer path | Bytes | Frozen SHA-256 |
| --- | ---: | --- |
| worker.cpp | 46928 | `17bfd627994e541bcae0f13d0f08dec4761698d539d838b13f9576452b1f539f` |
| controller.py | 18508 | `462504abcaafee308ca13802c9fdb454bf0b18586f55d1b0110faaf8fa9ee9c8` |
| scenario.py | 15236 | `4fa400166cf8b305844916e991fecd5bd608f1122171d7afc6de6b07e9de4bac` |
| AGENTS.md | 31536 | `638784bf17b3e48ef03ccf035483ac3557c36488fd942db17ac36435888cf3e0` |

This packet's raw/ contains exact copies. source.patch18602 bytes SHA-256
`3747ae433d80de155643751fa177b03ef50c40f9ffda3bee10d746057f042bf8`;
manifest.json2072 bytes SHA-256
`50d85dfaab7a0061941cf9ae75e0e8636c5026c1f832cecefdc19fa3b3c84b2f`.

Worker-local ServiceSignatureFacts copies only the two existing DWORD styles and
two already-read RECTs after unchanged service identity and successful rectangle
reads. The unchanged pure classifier supplies its first failed signature code;
one derived ServiceSignatureRefusal carries that fixed code/facts. It remains
catch-compatible with Refusal, preserving the generic provider catches and
post-Invoke S_OK -> submitted_state_changed behavior. Only the parsed-observe
top-level catch retains the sample. No generic protocol payload was added.

Emit requires raw unspent/no-Invoke/no-return flags, zero HRESULT and parsed
observe, and recomputes the same first failure before appending service_signature.
The eleven-member object has one schema literal, two uint32 styles and eight
signed int32 coordinates. Signed decimal formatting never casts negative
coordinates to uint64; a LONG/int32 size assertion pins the representation.
Existing scalar canonical formatting serializes the fixed object; only that
fixed named member is appended to the serialized base before the existing newline/
whole-output bound/write. No arbitrary JSON text, new Atom kind, recursive request
parser or durable-ledger serialization change. Existing output member ordering
has no controller acceptance role.

Controller accepts exactly base, base+legacy scalar or base+new object, never both.
Nested schema/keys/type(value)isint/ranges and existing recursive duplicate-key
decoding are strict. Actual observe/confirmed exit2/raw flags/HRESULT and ordered
sample/refusal equality are required before the independent ledger override.
All-passing or contradictory samples refuse. Legacy scalar checks are preserved
without retroactive new-profile comparison. New workers emit only the object;
legacy absence remains accepted and old controllers fail closed.

Only existing scenario.exercise is extended: all four first failures, mixed-failure
priority, every scalar's extrema and invalid types/ranges, negatives, missing/extra/
wrong nested shape/schema, nested/top duplicates, all-passing/earlier-failure
disagreement, mode/code/flags/HRESULT/exit, mutually exclusive extensions and prior
spent-ledger independence. Old legacy cases remain. No new test registration.
These authored cases exercise production output parsing and the existing offline
ledger, not native sampling, full execute lifecycle or post-Invoke runtime behavior.
No cases, parsers, compiler, formatter, build or application were executed.

All exact admission/style/exstyle/zero-geometry/identity/window-set/watcher/proof/
deadline/COM/ledger/Invoke floors and native read counts remain. Sequential facts
are not an atomic snapshot, read-success proof for style, stable timeline or
downstream acceptance. Root must independently review/build and obtain actual
values before a separate geometry-policy decision. No capture/positive UIA/Invoke/
lifecycle/FULL/CI/release credit. Other authors' source, Conductor and evidence
remain untouched. Git use was read-only diff/HEAD; no OS/API/DB/network actions.
