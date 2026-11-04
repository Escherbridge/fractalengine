---
type: SourceTrace
title: Asset publication equality guard source trace
date: 2026-11-04
---

# QA45 asset metadata guard: source trace

Author: `/root/native_nav23_functional_review`, 2026-11-04. Source-only trace; no implementation, database, API or native execution and no production edit in this trace.

**The actual failing statement is now identified, and its JSON round trip is provably unsafe. The exact runtime field that differed has not been observed.** QA45's unfrozen `initial-publication-diagnostics.json` is 2,125 bytes, SHA-256 `b822b15c5b68cb461182ed917ddd74f138f8793864a8906343601ec1c8c9d4d3` at inspection. All twelve retained seed messages identify `statement 12 stage asset_metadata kind Thrown reason asset_metadata_changed errors 12` across the three fresh cubes. The Publication classifier maps the fixed `scene asset changed` throw to this reason; that throw is the nine-field equality guard at `scene_publication/publisher.rs:212`.

The capture at publisher.rs:188 selects exactly `asset_id,name,content_type,size_bytes,content_hash,created_at,owner_did,petal_id,publication_provenance`, decodes to `Vec<serde_json::Value>` at 190 and later binds that decoded vector as `scene_asset_before` at 215. The transaction recomputes the typed database projection and compares it to that rebound JSON. Serialization of the dependency bytes uses a separate clone at 199–205, stripping private owner/provenance and nulls; that does not repair the equality witness.

Pinned SDK proof:

- `surrealdb-core-3.0.5/src/expr/field.rs:284–295` evaluates each explicitly projected field and sets it in the output. `src/val/value/set.rs:75–80,324–360` retains an assigned NONE value rather than inferring its original type from JSON.
- `surrealdb-types-3.0.5/src/value/into_json.rs:22` maps both database NONE and NULL to JSON null.
- `src/traits/surreal_value.rs:1367–1396` rebinds JSON null as database NULL and recursively does so for object/array members.
- `surrealdb-core-3.0.5/src/val/mod.rs:276–311` distinguishes NONE from NULL and compares objects/arrays structurally; `src/fnc/operate.rs:47–52` implements SQL equality/inequality through that equality.

Therefore a stable explicitly projected NONE field can fail this guard with no mutation. The fresh-upload path in `handlers/crud.rs:390–400` omits publication_provenance, while `schema.rs:305` declares it optional. That supplies a concrete normal source path to this failure. It is a sufficient mechanism, not proof that this particular runtime field was the sole difference. Node/asset created_at are declared strings; no datetime claim is needed. A real metadata change must continue to fail.

Existing adjacent patterns clarify the correction scope. `scene_publication/current.rs:208–239` already captures interpreted six-field metadata and a same-server CBOR/base64 witness in one projection, fencing the latter. `scene_asset_transfer.rs:51,135–142` and adoption use typed server witnesses too. Their projections differ in purpose. Reusing the current reader's six columns for publication would silently drop name, created_at and publication_provenance from the original fence and is unacceptable. Editor batch capture guards node/history/Petal state, not this nine-field asset projection; QA45 does not establish that this guard caused its selection-read outcome.

No existing public read supplies the needed native type for this uploaded asset. Ordinary and elevated query allowlists both exclude ASSET (`fe-query/src/sql_guard.rs:22–25,312–314`). Scoped asset routes deliver bytes/basic metadata. The node-bound publication audit has an explicit pre/post-adoption contract: without operation_id it requires absent owner, anchor and provenance (`scene_asset_audit.rs:258–265`), so it legitimately refuses an API upload with owner/anchor. QA45's one preserved 409 establishes refusal only. Do not retry with an invented operation ID, widen the audit, use raw/elevated asset SQL or change stored provenance to make a probe pass. My earlier candidate read suggestions were superseded after checking those full guards.

The minimal justified correction is the existing typed-witness pattern applied to the **same nine-field publication projection**, captured together with interpreted metadata in one server SELECT, then compared as ordered opaque witness strings at the final existing transaction. Keep unique-row/count checks, actual Editor/ancestry, upload/adoption proof, row/point/journal/clock/asset guards, private-field stripping and enqueue order. Tests should extend the existing real-store journey for unchanged absent fields, NONE-to-NULL/type changes, every projected field and row-count changes, with no newly registered test or claimed execution. Root has now authorized that narrow source fix without claiming the precise runtime field. A fresh reviewed build must still establish actual publication/recovery and preserve real-change rollback.
