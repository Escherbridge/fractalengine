---
type: SourceReport
title: Typed publication asset witness source author report
date: 2026-11-04
---

# QA45 exact publication asset witness: source report

Author: `/root/native_nav23_functional_review`, 2026-11-04.

**Narrow source fix authored and frozen for independent review. No source approval, compilation or runtime acceptance is claimed.** The outbound asset metadata fence now compares a same-server typed witness over its exact original nine fields instead of rebinding decoded JSON as the expected database object. The actual differing QA45 runtime field remains unidentified; no data, schema or authority is altered to make the guard pass.

## Exact four-file freeze

Baseline: `be37b2dc260b06b42307a9cab3a49f08202c108c`. Packet `.omc/research/qa45-asset-metadata-witness-v1/` contains complete raw files, manifest and normalized review patch.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `manifest.json` | 1304 | `787589d07c4bb0e40a1fe91fac19b715667ee564318e37a2622c5da7dde760ce` |
| `source.patch` | 11041 | `dced6f83187358b2b257005bef01ad0f57b81a27df5d83cf21cdec31fadee626` |

| Frozen path | Bytes | SHA-256 |
| --- | ---: | --- |
| `fe-database/src/handlers/scene_publication/AGENTS.md` | 20211 | `f1cd86b12ea1875ce998aee7e5bb1b93ad593057a79236aab2f1ec7cd398b1ce` |
| `fe-database/src/handlers/scene_publication/mod.rs` | 3176 | `f0a462e146a5bb2337bd939fa5da4145fd5bc90506d515707e777d93f864f37f` |
| `fe-database/src/handlers/scene_publication/publisher.rs` | 19561 | `465314106bf819b98dc9f53bada0c03b05b30aba037088a4ee21e92f9d1bc4e7` |
| `fe-database/src/op_log.rs` | 338768 | `154aa7f30e39401d7dc850916aca5af0a7aaac5b326d420ab32832d032aeede1` |

The manifest records baseline blob hashes too. Raw bytes govern identity; the review patch normalizes CRLF to LF only. `op_log.rs` contains additions inside its existing test journey. No new module/test registration, public API, request shape, table/schema, query permission, runtime setting or transport route is added. Native-author files, root live fixtures/evidence and unrelated changes remain outside this slice.

## Evidence and established mechanism

QA45's unfrozen `initial-publication-diagnostics.json` contained twelve seed failures identifying statement 12, asset_metadata, Thrown, asset_metadata_changed. Its inspected 2,125-byte SHA was `b822b15c5b68cb461182ed917ddd74f138f8793864a8906343601ec1c8c9d4d3`. Those fixed messages identify the exact outbound `scene asset changed` guard. They do not expose the differing native field or prove that every failure is a concurrent change.

The pinned SDK's projection/JSON/rebinding/equality source establishes the unsafe round trip: an explicitly projected native NONE can become JSON null, then be rebound as native NULL, which is unequal in the typed object/array comparison. Fresh upload omits the optional publication_provenance field, providing a normal source-reachable case. The source trace is `.omc/research/qa45-asset-metadata-guard-source-trace.md`; it distinguishes that sufficient mechanism from an unobserved precise runtime-field claim.

The single existing audit attempt returned 409 and remains preserved by root. Its pre/post-adoption contract intentionally cannot audit an owned anchored upload without the corresponding adoption state. No invented operation ID, retry, raw/elevated asset query, audit widening or data mutation is part of this correction.

## Production correction and boundaries

`PUBLICATION_ASSET_COLUMNS` is the exact original projection:

`asset_id, name, content_type, size_bytes, content_hash, created_at, owner_did, petal_id, publication_provenance`

Capture and final fence now generate their SQL from that same constant. `capture_publication_asset_metadata` wraps the original bounded projected SELECT in a single server SELECT returning both interpreted `metadata` and `encoding::base64::encode(encoding::cbor::encode($this))`. It keeps the asset lookup ID, two-row probe, five-second statement timeouts and the existing batched model lookup. Returned metadata must be an object and every corresponding witness a nonempty string before either is used.

The decoded rows still drive the original unique-row/anchor check, actual local upload/adoption proof, content-hash validation and dependency serialization. Private owner/provenance stripping, null omission, 32-KiB dependency ceiling and scene marker are unchanged. Interpreted JSON no longer claims to preserve native database types for equality.

`publication_asset_metadata_fence` recomputes the same nine-field projection's ordered witness list inside the existing final transaction and compares it to `scene_asset_fingerprints`. The row count remains part of the list comparison; creation/deletion/duplicates and any typed projected-field change invalidate the captured state. The final projection retains the previous complete matching-row comparison rather than truncating it to the initial probe. The original fixed throw text and safe indexed diagnostic stage remain.

Actual Editor/ancestry, node/point/journal/local-proof/clock guards, shared asset upload/adoption checks, original HLC/provenance and transaction ordering remain unchanged. Dependencies and node bytes still enqueue only after the complete final transaction succeeds. A refused publication does not rewrite its metadata, manufacture local history, broaden role authority or relabel an earlier successful local edit as failed. Model/schema and unproven-dependency deferral stay explicit.

The separate current-reader six-field typed witness and asset transfer/adoption guards are unchanged. Reusing that narrower six-field projection here would omit name/created_at/publication_provenance and weaken the prior publication fence; this slice deliberately retains all nine. No new generic SQL/projection interface is introduced.

## Existing meaningful journey extensions, unexecuted

The existing real-store operation journey now calls the production publication capture and fence helpers through test-only module re-exports. Its schema-less metadata fixtures allow deliberate native type changes without changing production schema behavior. Authored cases cover:

- Empty capture invalidated by asset creation.
- A fresh uploaded-style row with present owner/anchor and absent publication provenance: the original decoded-JSON equality refuses without mutation, while the production typed witness accepts unchanged state.
- Each of the nine projected fields, including descriptive name/created_at, changes independently and must refuse. Integer-to-float size and absent-to-NULL provenance exercise native-type preservation; provenance object replacement remains fenced.
- Deletion and an additional same-asset-ID row invalidate the captured list.
- Each changed-state fence runs after a probe write inside one transaction; the probe must be absent afterward, establishing complete transaction rollback rather than merely returning a guard error.

Existing current-reader cases remain, including its intentionally different treatment of an unrelated descriptive rename. No newly registered test declaration or passing result is claimed. These authored checks are not actual fixture publication, peer delivery, live rollback or durable-media evidence.

## Inspection and next gates

Only read-only source/Git/hash inspection and source/evidence authoring were performed. Scoped `git diff --check` returned zero. No compiler, implementation parser/formatter, tests/journeys, application/helper, OS/process/native/API/DB/network action or Git mutation occurred. The author does not self-approve this slice.

Independent full review should verify exact nine-field identity, same-server capture/witness coupling, ordered row/count comparison, retained metadata/provenance/authority/transaction/enqueue behavior and the test-only boundary. Root owns final staging/commit/build. After review and build, use a fresh disposable normal fixture to establish actual publication/recovery behavior with retained safe diagnostics, then actual peer/data/render delivery independently. Preserve genuine concurrent metadata-change refusals and unknown outcomes. No native, FULL campaign, final integrated sweep, CI or tagged-release credit follows from this source fix.
