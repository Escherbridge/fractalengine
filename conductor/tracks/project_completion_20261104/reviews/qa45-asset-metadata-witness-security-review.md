---
type: Review
title: Typed publication asset witness correctness review
date: 2026-11-04
---

# QA45 publication asset witness: independent security/correctness review

**APPROVED AT SOURCE** for the complete four-file frozen slice `.omc/research/qa45-asset-metadata-witness-v1/`, baseline `be37b2dc260b06b42307a9cab3a49f08202c108c`. No concrete source blocker was found. The comparison preserves the original nine-field fence while avoiding a lossy JSON expected-value round trip. Actual QA45's differing field remains unknown; successful build/publication/recovery/peer acceptance is not established here.

Manifest **1304 bytes**, SHA-256 **`787589d07c4bb0e40a1fe91fac19b715667ee564318e37a2622c5da7dde760ce`**; complete patch **11041 bytes**, SHA-256 **`dced6f83187358b2b257005bef01ad0f57b81a27df5d83cf21cdec31fadee626`**. All four raw snapshot files match their declared sizes/hashes, current working bytes and manifest baseline blob hashes. Author report SHA-256 is `9caf7595663d215aeb8a4029808d3fbeb3fe76d06b08023d4d96c333c27b4eab`.

| Frozen path | Bytes | SHA-256 |
| --- | ---: | --- |
| `fe-database/src/handlers/scene_publication/AGENTS.md` | 20211 | `f1cd86b12ea1875ce998aee7e5bb1b93ad593057a79236aab2f1ec7cd398b1ce` |
| `fe-database/src/handlers/scene_publication/mod.rs` | 3176 | `f0a462e146a5bb2337bd939fa5da4145fd5bc90506d515707e777d93f864f37f` |
| `fe-database/src/handlers/scene_publication/publisher.rs` | 19561 | `465314106bf819b98dc9f53bada0c03b05b30aba037088a4ee21e92f9d1bc4e7` |
| `fe-database/src/op_log.rs` | 338768 | `154aa7f30e39401d7dc850916aca5af0a7aaac5b326d420ab32832d032aeede1` |

Only this review was authored. Read-only frozen/repository/Git/installed primary SDK/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, app/helper, OS/process/native/input/capture, API/DB/network action. Parallel native/evidence authors and the session log were preserved. No raw/elevated asset query or repeated audit was performed.

## Typed capture and use-time equality

`publisher.rs:12-13,180-205` defines one fixed original projection: asset ID, name, content type, size, content hash, creation time, owner DID, Petal ID and publication provenance. Capture returns interpreted metadata and its server CBOR/base64 witness together from the same projected `$this`. It retains the two-row probe and five-second statement limits, with the original batched one-row model lookup unchanged. Metadata must be an object and its corresponding witness a nonempty string; witness/metadata vectors are built in lockstep. IDs remain bound values; there is no caller-supplied projection or generic SQL surface.

The final guard recomputes that same nine-field projection and server encoding inside the existing containing transaction, then compares the complete witness list. It does not truncate the final comparison to the initial two-row probe. A prepared single asset subsequently deleted, duplicated or changed in any projected type/value changes the expected list and refuses. This preserves the prior field/count fence instead of substituting the current reader's distinct six fields or decoding the expected object back through JSON.

Pinned `surrealdb-core-3.0.5/src/fnc/encoding.rs:51-59` encodes the server public value directly through its CBOR implementation. `rpc/format/cbor/convert.rs:369-374` emits distinct NONE-tag versus NULL and integer versus float representations. It supports the stated type-preservation mechanism; this is not evidence identifying QA45's precise native field. No data/schema coercion or mutation is used to make equality pass.

## Authority, bounds and preserved publication behavior

The same decoded metadata still drives the original unique-row/anchor check, unchanged local upload/adoption proof and hash validation. Private owner/provenance removal, null omission, scene marker and 32-KiB serialized outbound dependency ceiling remain exactly as before. The witness stays local to the prepared transaction; no CBOR/private metadata is added to outbound bytes or API output. The model/schema and unproven-dependency deferral paths remain.

Scope/Editor/full ancestry, original node/point/journal/tombstone provenance, HLC/clock, asset adoption proof and dependency/node enqueue ordering are untouched. Only the final metadata expected-value representation and comparison expression change. Any final guard error still prevents that row's enqueue and rolls back its transaction; it does not undo a successful earlier local edit or manufacture local history.

Bounds are precise: capture has at most two projected rows, model probe one, five-second statements, object/nonempty-string witness checks and the unchanged 32-KiB outbound metadata bound. **There is no independent hard byte ceiling for the full private CBOR witness.** The old implementation also materialized/bound the full private nine-field JSON metadata; this slice does not establish a new global memory bound. In particular, the outbound stripped-payload ceiling is not a claimed bound on all local private provenance/witness allocations. No concrete new authority or admission weakness follows from that inherited limit in this delta.

Initial absent/ambiguous/unproven metadata retains the original dependency-deferral behavior; production does not install an empty-witness creation guard in that branch. Thus the test helper's empty-capture/creation rejection is not a claim that all initially absent or duplicate assets abort the entire publisher. For an admitted prepared single asset, later creation of a duplicate, deletion or field/type mutation is fenced as required. The guide's preserved duplicate/count language must be read with that unchanged deferral distinction.

## Existing meaningful journey and remaining gates

`op_log.rs` production prefix is unchanged after newline normalization; all additions are in the existing operation journey and registered test declarations are unchanged. Module exports of capture/fence/column helpers are test-only. The journey uses actual production capture/fence SQL against controlled metadata and authors the relevant counterexample: absent native provenance becomes decoded null, the old JSON rebind equality fails without mutation, and the new typed witness accepts unchanged state. It then changes each of nine fields, including integer-to-float and absent-to-NULL, plus deletion/duplicate cases. Each failure follows a probe write in the same transaction and checks probe absence after refusal, exercising rollback rather than merely error text. These checks were not parsed, compiled or executed here.

The prior 409 audit is an existing restricted-contract refusal, not proof of a particular runtime field or permission bypass. No audit/API widening, invented adoption operation, retry or forensic raw data access belongs to this fix. Safe fixed diagnostics identify the failed guard only. Root must build exact approved source, execute the appropriate meaningful journeys/integrated sweep and collect a fresh disposable normal fixture's actual publication/recovery result. Preserve real concurrent metadata refusals and independently verify peer/data/render delivery. Retained restart, native acceptance, FULL campaign, final checks, exact-SHA CI and tagged-release verification retain their separate gates.
