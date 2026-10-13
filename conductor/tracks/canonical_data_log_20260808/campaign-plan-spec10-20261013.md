---
type: plan
track: canonical_data_log_20260808
created: 2026-10-13
title: "Campaign plan: CDL-SPEC10 observation plane + the R5 device-payload ingestion model"
ledger_rows: [CDL-SPEC10, DB-TS-RETENTION (cross-ref), PLUG-HOST (cross-ref), CDL-PROV (cross-ref)]
decisions: [D-CL26, D-CL28 (G3, G6), D-CL29, R4 D-5/D-17 overrides, R5 generic asset + device extensions]
status: proposed
---

> **OWNER DECISIONS 2026-10-13 (R7, RUNBOOK §3) — supersede this plan wherever they differ:**
> 1. **SPEC-10 is PRE-APPROVED with the draft's recommended defaults** — Phase B (B1–B3) builds immediately after A1 drafts the spec; the owner reviews afterwards. Record the approval as 'pre-approved 2026-10-13 (defaults), post-review pending' in the spec and decision log.
> 2. Confirmed: A3–A6 (generic asset + device payloads + format descriptors) are built in THIS campaign.

# Campaign plan: SPEC-10 observation plane, reconciled with R5

This plan covers the fourth large phase in RUNBOOK §7 step 3 (PAS-P3+P4 → PAS-P5 → CDL Wave 4 → **CDL SPEC-10**).
The phase is first a **specification deliverable**. The owner approves specs (track `plan.md`, "Workstream S";
SPEC-8 §7.1). The 2026-10-13 R5 decision already ratified part of what SPEC-10 has to describe, and that part
is implemented without waiting.

---

## 1. Goal and acceptance

### 1.1 Goal

There is one ingestion story for device data. Telemetry arrives once and lands in two **hot projections** that
can be queried immediately:

- `iot_reading`: scalar values that SQL and BI tools can query.
- `device_payload`: a list of generic asset ids that point to opaque bytes in the iroh blob store.

It is also committed **durably** to the canonical log as **observation batches**. Each batch is a
small signed operation that references a sealed columnar observation hexon (D-CL26).

### 1.2 Acceptance (from the ledger, metadata and decisions)

| Source | Acceptance criterion |
|---|---|
| Ledger `CDL-SPEC10` (`conductor/doneness-ledger.md:105`) | A draft spec with conformance-test names, the `iot_reading` graduation path and the LaneClass/ObjectClass mapping is **submitted for owner approval**. |
| metadata `SPEC-10-observation` | Columnar hexon format (segmentby=entity, orderby=hlc, row-group statistics), the observation-batch operation kind and schema, `preview_kind` registration for live telemetry, and the graduation path from `iot_reading`. |
| metadata `G-ERRATA-G3` | A fifth `LaneClass` for bulk artifacts (SPEC-6) that activates SPEC-3 `ObjectClass::{Shard,Tile,Asset}`. |
| metadata `G-ERRATA-G6` | An observation-plane source-identity vocabulary for SPEC-4 §7.3 reproducibility claims. |
| D-CL26 | The batch commit is **by reference**: a canonical-CBOR payload `{hexon_artifact_id, time_range, stats_hash, count}`. Floats never enter signed bytes. Live data goes on the SPEC-7 preview channel and durable data on batch commits. |
| R4 D-5 override | "SurrealDB stores a recordId for a file stored in blob storage on iroh; extensions for IoT devices bring their format …; allow for multiple records (a list of records)." |
| R5 | The `asset` table is the generic asset (MIME + iroh blob hash + owner). A NEW device-payload record (node, petal, format, recorded_at, **list of asset ids**) sits alongside scalar `iot_reading`. Device extensions = a declared format descriptor (format_id + MIME + optional schema) **and** sandboxed decoders. Default order: descriptors first, decoders after the plugin host is wired. |
| R4 D-17 override | Generic assets work alongside `hexon_p2p_bucket`, so there is one asset model and it is not foundry-only. |
| Standing rules | Reviewers fix their own findings. Compile + clippy between slices; tests are written now and run once at campaign end. A security review is mandatory on user-input / ingest surfaces. |

### 1.3 Definition of done for this phase

- **Phase A (does not wait):** the SPEC-10 draft and the G3/G6 errata are written and submitted. The G3/G6
  code has landed. The R5 generic-asset + device-payload + format-descriptor model works end to end (DB,
  REST, MCP, BI query, two-peer replication) with flow tests written. Every slice has a review-and-fix
  verdict recorded.
- **Phase B (waits on owner approval of SPEC-10):** the observation module, the columnar hexon codec, and
  the shadow-mode batch sealer + hydrator are complete. They run in `dual_emit_shadow`, and the default
  stays `legacy_only` (SPEC-8 §3 rule 1).
- If the owner has not approved SPEC-10 when Phase A finishes, Phase B is **parked on that one decision**
  (see §7). It is not dropped.

---

## 2. Current-code reality (verified 2026-10-13)

### 2.1 Canonical log (operation plane): built, local, dormant

- **Lane classes.** `LaneClass` has exactly four lanes (`fe-canonical-log/src/segment/artifact.rs:30-38`):
  HeaderSegment, PayloadShard, HashSeqNode and SegmentManifest. `from_wire` refuses anything other than 1-4
  (`artifact.rs:53-61`). G3 is unbuilt.
- **Object classes.** `ObjectClass::{Shard,Tile,Asset}` exist with bits 0x08/0x10/0x20
  (`capability/verbs.rs:67-80`), but no lane carries them. This is the "dormant classes" that D-CL26 cites.
- **Topic lanes are a separate concept from `LaneClass`.** SPEC-3 §6.1 defines three topic lanes
  (header/payload/availability), and `segment/AGENTS.md` §discovery-lanes maps traffic onto them. G3 adds a
  *sealed-artifact* lane class (wire 5). It does **not** add a fourth topic lane. Bulk artifacts are
  discovered on topic lane `1` (payload), petal-affine.
- **Operation kinds.** `OperationKind` (`kind.rs:9-20`) covers kinds 1-4 plus `Application(u16)` for 5..=65535,
  "whose payload rules live in a schema registry that does not exist yet". No observation kind exists. G1's
  partitioned quarantine is in place and gives an un-upgraded peer a separate budget for a new
  high-volume kind: `compose.rs:51` (`QuarantineReason::UnknownKind`), `compose.rs:154` (class mapping),
  `retention/quarantine.rs:396-399` (`OperationKindAvailability`).
- **G2 bright line is built.** `ProjectionMutation::ReferencedArtifactUnavailable` is at
  `materialize/traits.rs:194`. `reduce()` may only *write* a reference (SPEC-4 §4 rules 8-9).
- **G4 tiered statistics are built.** `SegmentStatistics` (`segment/manifest.rs:77`) is the clear block and
  `SealedStatisticsRef` (`manifest.rs:161`) is the sealed per-lane reference.
- **Preview.** `preview_kind: u32` must be non-zero (`wire/preview.rs:53,87-90`), but there is **no
  registry**: any non-zero value decodes.
- **Checkpoint.** `snapshot_manifest_id: Option<Hash32>` exists (`checkpoint.rs:147`).
- **Crate rules.** fe-canonical-log must not depend on fe-runtime, fe-identity, fe-policy, fe-database or
  surrealdb (`src/AGENTS.md` §Forbidden dependencies), so Parquet/arrow cannot live here. `lib.rs` and
  `Cargo.toml` are "effectively frozen after W1a" (`src/AGENTS.md` §module-ownership); a new module needs a
  recorded exception.
- **fe-database `canon_log` is parallel and dormant.** Nothing in the running engine calls it
  (`fe-database/src/canon_log/AGENTS.md` §parallel-and-dormant). SPEC-8 modes exist as
  `migration/mode.rs:14` `MigrationMode` and `MigrationFlags::from_env` (`mode.rs:76`).
- **Pre-existing drift.** `InsertIotReadings` is **missing** from `OTHER_DISPATCHED_MUTATION_KINDS`
  (`fe-database/src/migration/mapping_inventory.rs:49-89`), even though it is a dispatched mutation
  (`fe-runtime/src/messages.rs:618`, `fe-database/src/lib.rs:1404`). `migration/AGENTS.md` §inventory-scope
  says to add new mutating variants "in the same change". Slice A3 fixes this.

### 2.2 IoT, assets and blobs (the legacy hot plane): live

- **`iot_reading`** (`fe-database/src/schema.rs:262-273`) stores a scalar `value: f64`, a denormalized
  `petal_id`, three timestamps (`recorded_at`, `recorded_at_ms`, `hlc_timestamp`) and `source_did`. The
  `reading_id` is minted server-side as a ULID (`handlers/iot_reading.rs:140`), so device retries are not
  idempotent.
- **Ingest path.** `POST /api/v1/petals/{p}/iot/readings` (`fe-api/src/server.rs:312`, `fe-api/src/iot.rs:50`)
  requires Editor+, checks ULID + petal scope + rate limit, and caps a batch at `IOT_INGEST_MAX_READINGS`
  = 1000 (`fe-api/src/limits.rs:25`). It then calls `insert_readings_with_replication`
  (`handlers/iot_reading.rs:82`). The rule is durable-first, then emit (`fe-database/src/AGENTS.md`
  §iot-readings).
- **Replication.** `ReplicationMode::for_table("iot_reading") == Timeseries`, a union CRDT
  (`fe-database/src/replication_mode.rs:34-39`). The inbound Row-mode handler applies only verse, fractal,
  petal and node; every other table is `NotApplicable` (`handlers/replicated_row.rs:184-207`). **Asset rows
  do not replicate today.** Blobs move between peers with `SyncCommand::FetchBlob { hash, verse_id }`
  (`fe-sync/src/messages.rs:18`, `sync_thread.rs:766`, `handle_fetch_blob` at `sync_thread.rs:1203`), proven
  by `fe-test-harness/src/scenarios/two_peer_blob_exchange.rs`.
- **The timeseries fabric already shards by (petal, anchor, time bucket)** (`fe-sync/src/AGENTS.md`
  §sharding). It is the natural row-group/segment boundary for segmentby=entity.
- **Generic asset table.** `asset` (`schema.rs:228-241`) is already MIME-agnostic: `content_type`,
  `content_hash`, `owner_did`, anchor `petal_id`. Ingest, however, is **GLB-only**: `ingest_glb_with_limit`
  calls `validate_glb` (`fe-api/AGENTS.md` §asset-ingest). `place_asset_core` (`fe-api/src/upload.rs:362`)
  assumes a renderable GLB.
- **Asset serving.** Assets are served with the DB's `content_type` (`fe-api/src/assets.rs:398-400`) and
  `Content-Disposition: attachment` (`assets.rs:420-424`). There is **no
  `X-Content-Type-Options: nosniff`**. Once arbitrary device MIME types such as `text/html` are stored,
  that header matters.
- **Blob GC (data-loss trap).** `gc_unreferenced_assets` treats an asset as referenced **only** through
  `node.asset_id` (`fe-database/src/handlers/asset_gc.rs:35-40`). An asset referenced only by a
  device-payload row would be **deleted after the grace period**. Slice A3 must extend the reference set.
- **BI query.** `PETAL_SCOPED_TABLES` (`fe-api/src/query_guard.rs:54-61`) includes `IOT_READING`. The
  `asset` table is deliberately not queryable (`fe-api/AGENTS.md` §assets).
- **GeoParquet.** A GeoParquet writer/reader exists at `fe-query/src/columnar/geoparquet/`, behind the
  `parquet` feature (arrow/parquet 54, `fe-query/Cargo.toml:15,29-30`). fe-database depends on fe-query
  without that feature (`fe-database/Cargo.toml:13`). arrow is already in the workspace through fe-api's
  `datafusion` feature, so enabling it adds no new compile unit.
- **Plugin host.** `fe-plugin` is dormant: no binary depends on it, and the WIT is aspirational
  (`fe-plugin/src/AGENTS.md:131-134`, ledger `PLUG-HOST`).
- **Hexon archive.** `HexonType` (`fe-format/src/manifest.rs:6-23`) has no observation type. The archive
  importer has per-entry and total decompression caps (fe-format AGENTS.md §decompression-limits).

### 2.3 How the three records relate: one ingestion story

```
device / connector / fe-sim / (later) decoder plugin
        │  one authenticated batch:  readings[] (scalars)  +  payloads[] (format_id, files[])
        ▼
fe-api ingest door (Editor+, petal scope, rate+size caps)  ── bytes ──► iroh blob store (BLAKE3)
        │                                                               │
        ▼  DbCommand::InsertIotReadings / InsertDevicePayloads          ▼
HOT PLANE (legacy authority; SQL/BI now; replicates as union rows):
   asset            ◄── generic asset: MIME (from format descriptor) + content_hash + owner + anchor petal
   device_payload   ── {payload_id, node_id, petal_id, format_id, recorded_at, asset_ids[ ]}  (R5)
   iot_reading      ── scalar facts; optional payload_id = "decoded from / sent with this payload"
        │
        ▼  (Phase B, after SPEC-10 approval; MigrationMode::DualEmitShadow, default off)
OBSERVATION PLANE (canonical, local only):
   window (petal, time bucket) ─► columnar observation hexon (readings.parquet + payload_refs.parquet,
   segmentby=anchor node, orderby=hlc) ─► sealed as LaneClass::BulkArtifact / ObjectClass::Shard
   device payload blobs ─► referenced by content hash as ObjectClass::Asset artifacts (never inlined)
   ─► observation_batch operation (kind 5): {hexon_artifact_id, len, hlc range, sensor-time range,
       stats_hash, count, sorted asset refs, source identities (G6)} ─► verified_op_log
   reduce() writes ONLY an observation_batch reference row (G2); a separate HYDRATOR
   (materialize/shard verb, SPEC-3 §3.3) expands the hexon into a rebuildable local catalog.
LIVE: preview_kind 1 = telemetry_sample_v1 on the SPEC-7 preview channel (lossy, never persisted).
```

Each record has its own job:

- `iot_reading` is the **queryable scalar projection**.
- `device_payload` is the **queryable index of opaque bytes**.
- An **observation batch** is the **durable signed commit of both** for one window. Its hexon carries the
  readings as columns, and the payloads as `(payload_id, anchor, hlc, format_id, asset content hashes)`
  rows.

The graduation path (Phase B, SPEC-10 §8) makes the hot tables **rebuildable from batches**. It does not
replace them, because BI still queries `iot_reading`. Decoders, once PLUG-HOST is wired, write
`iot_reading` rows with `payload_id` set and a decoder identity in the batch's G6 source set. They do not
need a fourth record type.

---

## 3. Slices (strictly sequential; each executor runs cargo check + clippy on its own crates only)

Model tiers follow the standing rules. Every slice ends with a **review-and-fix** pass (a reviewer that fixes
its own findings), and its verdict is recorded in the track's review ledger. Slices A3-A5 are ingest of
untrusted user input, so their review is a **security** review-and-fix.

### Phase A: does not wait for approval

#### A1. SPEC-10 draft + G3/G6 errata text + decision entries (docs, opus, M)

**Owned files**
- `docs/spec/canonical-log/observation-plane.md` (new)
- `docs/spec/canonical-log/segment-shard-relay.md` (G3: new §3.4 + Errata entry)
- `docs/spec/canonical-log/log-first-materialization.md` (G6: §7 rule 5 + Errata entry)
- `docs/spec/canonical-log/AGENTS.md`
- `conductor/decisions/canonical-data-log-20260808.md` (new **PENDING-RATIFICATION** rows only)
- `conductor/tracks/canonical_data_log_20260808/metadata.json` (task status strings only)

**Authority split (precedent: commit `c375beb`)**
- **G3 and G6 are D-CL28-ratified gates.** G1/G2/G4/G5 had their erratum text and code land without a
  further approval round. G3/G6 follow the same path, under an "Owner-ratified under D-CL28" Errata
  subsection dated 2026-10-13.
- **SPEC-10 itself** is a NEW normative spec. Its Status line reads `**Status:** Draft — owner approval
  required before implementation.`, which is the exact wording SPEC-1..8 used at draft (`git show 70c5509`).

**SPEC-10 sections.** Follow the house layout: numbered terse normative rules, Design notes, Errata,
required conformance tests.

1. **Conformance vocabulary and boundary.** Defines the observation plane vs the operation plane (D-CL26).
   States that no network, relay, inbound P2P or `/ws/canonical` mount is authorized (CDL-NET / CANON-WS
   NO-GO).
2. **Ingestion model.** Covers the §2.3 story, normatively for the canonical part and informatively for the
   hot tables. Includes the R5 records (`asset`, `device_payload`, `device_format` descriptor) and the
   `iot_reading.payload_id` link.
3. **Observation hexon (columnar).**
   - Container: an fe-format `.hexon` archive with a new `HexonType::ObservationBatch`. Entries are
     *stored* (uncompressed zip, fixed timestamps), so the archive bytes are a pure function of the
     entries.
   - Contents: `observations/readings.parquet` and an optional `observations/payload_refs.parquet`.
   - Parquet profile: required columns and types, f64 allowed (the interior is opaque, SPEC-1 §2), and
     pinned writer properties (zstd pages, `created_by` constant, statistics enabled).
   - **segmentby=entity**: entity = anchor `node_id`. A row group MUST NOT mix entities; an entity MAY
     span several row groups.
   - **orderby=hlc**: ascending `(hlc_wall_ms, hlc_counter, reading_id)` within an entity.
   - **Row-group statistics**: Parquet-native per-column min/max are REQUIRED. They are inside the sealed
     artifact and therefore sealed, which is consistent with G4.
   - Batch identity: one petal and one time bucket per batch (SPEC-6 §3.2.2 single-scope rule).
   - Size is bounded by the `max_segment_bytes` caveat.
4. **Observation-batch operation.**
   - **Dedicated `operation_kind = 5` (`observation_batch`).** It is not a kind-1 schema, so an
     un-upgraded peer quarantines it under the G1 `unknown_kind` budget rather than spending
     `unknown_schema`.
   - Requires ≥1 parent and an encrypted non-empty payload.
   - Payload CBOR (integers only):
     `{0 hexon_artifact_id: Hash32, 1 hexon_stored_length: u64, 2 hlc_range: {min,max}, 3 sensor_time_range_ms: {min,max} i64, 4 stats_hash: Hash32, 5 count: u64, 6 anchors: sorted Identifier list, 7 asset_refs: sorted Hash32 list, 8 sources: sorted ObservationSource list (G6), 9 hexon_format_version: u16}`.
   - Structural rules: no inverted range; `count == 0` iff the hexon has no rows; anchors and asset refs
     are strictly ascending.
   - Capabilities: `append/op` with `allowed_operation_kinds ∋ 5`, plus `append/shard` for the hexon and
     `append/asset` for each referenced payload blob.
   - Reduction: writes only an `observation_batch` reference row (G2).
5. **Hydration.** `materialize/shard` produces a derived, rebuildable local catalog. It is never an input to
   `reduce`. A missing hexon is reported as `referenced_artifact_unavailable` at hydration.
6. **`preview_kind` registry.**
   - `1 = telemetry_sample_v1`: canonical CBOR `{anchor, metric, value_nano: i64, units, recorded_at_ms}`.
   - `0` stays invalid. 2..=1023 are reserved for spec registration; ≥1024 are application/experimental.
   - The decoder of a registry-aware receiver drops unregistered kinds.
   - Restates the SPEC-7 §7.1.4 dual path: live = preview, durable = batch.
7. **Source identity (G6).**
   - `ObservationSource` = `{source_class (device|connector|decoder|manual|simulation), source_id: Identifier32, ingest_principal: 32-byte key, format_id | null, decoder {plugin_id, plugin_version, module_hash} | null}`.
   - Reproducibility rule (cross-references SPEC-4 §7 rule 5): a blended result is reproducible only if
     it records the canonical tuple plus the sorted observation-batch `op_id` set it read.
8. **`iot_reading` graduation path.** The SPEC-8 stages applied to the observation family:
   - `legacy_only`, the default.
   - `dual_emit_shadow`: seal windows of hot rows into batches; legacy stays authoritative.
   - `shadow_rebuild`: hydrate into a separate catalog, then run the SPEC-8 §5.3 comparison.
   - `canonical_authoritative_local`: owner gate SPEC-8 §7.4.
   - After cutover only: retention GC of hot rows covered by a checkpointed batch (DB-TS-RETENTION). The
     window is caller-parameterized per D-CL24(4).
9. **LaneClass / ObjectClass / topic-lane mapping table.**
   - Observation hexon → `LaneClass::BulkArtifact` (wire 5) / `ObjectClass::Shard` / topic lane 1,
     petal-affine.
   - Payload blob → BulkArtifact / `Asset` / lane 1.
   - Sealed statistics artifact → BulkArtifact / `Shard`.
   - Terrain tile → BulkArtifact / `Tile`. Listed for completeness; nothing produces it in v1.
10. **Required conformance tests.** Use the names listed under B1-B3 below.
11. **Design notes.** The D-CL26 cost math (≈43 GB/day of headers at 1000 sensors × 1 Hz vs ~10⁴
    readings/batch), why the kind is dedicated, why hexon-in-archive, and why the hot tables stay.
12. **Owner approval gates.** Lists what this approval unlocks (Phase B) and what it does not (network,
    cutover).

**G3 erratum text (SPEC-6 new §3.4 "Bulk-artifact lane")**
- A fifth lane class whose inner body is `{scope, object_class ∈ {shard, tile, asset}, content_length, content_bytes}`.
- Sealed through the same §2 rules.
- Addressed only by `artifact_id` from an operation payload. The manifest does **not** index bulk
  artifacts: reachability comes from the referencing op, which keeps the G4 manifest wire format unchanged.
- One petal-affine scope per artifact.

**G6 erratum text (SPEC-4 §7 rule 5)**
- Adds the observation input set to the canonical input identity.

**Decision rows (marked PENDING-RATIFICATION)**
- D-CL30 SPEC-10 as a whole.
- D-CL31 kind 5.
- D-CL32 hexon-archive container + Parquet profile.
- D-CL33 the preview_kind registry and `telemetry_sample_v1`.
- D-CL34 provisional wire numbers (lane 5, payload keys 0-9, ObservationSource keys) join the CDL-PROV
  ratification index.

**metadata.json task status strings**
- `SPEC-10-observation: draft_submitted_2026-10-13`
- `G-ERRATA-G3` / `G-ERRATA-G6: spec_text_landed_2026-10-13` (the code lands in A2)

**Invariants**
- Approved spec text is changed only through a dated Errata subsection (`docs/spec/canonical-log/AGENTS.md`).
- Normative rules stay numbered and terse; rationale goes to Design notes.
- Never touch the `.json` / `.mjs` fixtures (they are frozen).

**Flow tests:** none. This is a spec slice. Conformance-test *names* are written into the spec and
implemented in A2/B1-B3.

#### A2. G3 bulk-artifact lane + G6 source-identity vocabulary (fe-canonical-log, opus, M)

**Owned files**
- `fe-canonical-log/src/segment/artifact.rs`
- `fe-canonical-log/src/segment/relay_policy.rs`
- `fe-canonical-log/src/segment/discovery_labels.rs`
- `fe-canonical-log/src/segment/mod.rs` (re-exports only)
- `fe-canonical-log/src/materialize/identity.rs`
- `fe-canonical-log/src/materialize/mod.rs` (re-exports only)
- `fe-canonical-log/src/segment/AGENTS.md`
- `fe-canonical-log/src/materialize/AGENTS.md`
- `fe-canonical-log/src/AGENTS.md` (§Provisional wire numbering, new "Owner-ratified errata, 2026-10-13" section)
- **(Critic, cross-plan)** `fe-canonical-log/src/compose.rs`: the `BulkArtifact` arm of CDL-W4-4's
  `SealedProofEnvironment` `LaneClass` match only. W4-4 made that match wildcard-free, so
  without this file the slice cannot compile.

**Review:** **security** review-and-fix (critic upgrade). The slice adds a seal lane, an AAD domain
and a crypto-gated body codec.

**Steps**
1. Add `LaneClass::BulkArtifact`: `to_wire` 5, `from_wire(5)`, and a provisional AAD domain
   `b"fe-segment-bulk-artifact-v1\0"`.
2. Add a `BulkArtifactBody` inner-body codec (deterministic CBOR; `object_class` restricted to
   Shard/Tile/Asset; refuses Operation/Segment/Checkpoint).
3. Route sealing through `seal_artifact` only.
4. Map BulkArtifact to topic lane 1 (payload), petal-affine, in `discovery_labels`.
5. Make `relay_policy` handle the lane. Every `match` on `LaneClass` is total, so the compiler finds all
   sites.
6. In `materialize/identity.rs`, add `ObservationSource`, `SourceClass` and `DecoderIdentity` with a
   canonical CBOR codec (provisional keys, no floats, unknown keys rejected). Add an
   `AnalyticsSourceIdentity` that extends the §7 tuple with a sorted, deduplicated observation `op_id` set.

**Invariants**
- §single-seal-entry-point: `SealedArtifact::seal` stays `pub(crate)`. The epoch/key/nonce gates apply to
  lane 5 with no opt-in.
- §G4: tiering by derivation. Nothing payload-derived goes in clear.
- §Forbidden dependencies: no new crate dependency.
- `lib.rs` and `Cargo.toml` are untouched (no new module is needed).

**Flow tests** (in-file `#[cfg(test)]` and/or `fe-canonical-log/tests/`)
- `bulk_artifact_seals_through_single_door_and_round_trips`: seal → `artifact_id == BLAKE3(outer bytes)` →
  open → identical body.
- `bulk_artifact_refused_under_retired_epoch_key` and `bulk_artifact_nonce_reuse_refused`. Inverting either
  gate turns them red.
- `bulk_lane_body_mismatch_rejected`: a lane-5 outer map with a payload-shard body, and the reverse.
- `bulk_artifact_object_class_table` (table-driven): Shard/Tile/Asset accepted; Operation/Segment/Checkpoint
  refused.
- `bulk_artifact_discovery_is_payload_lane_petal_affine`: a verse-wide scope is refused.
- `observation_source_cbor_canonical_round_trip_and_refusals` (table-driven): float, unknown key,
  duplicate key and non-canonical length are all refused.
- `analytics_source_identity_dedupes_and_sorts_batch_ids`: real branching, so it is kept.
- **(critic)** `sealed_proof_environment_opens_a_bulk_artifact_lane_and_refuses_a_retired_key`
  (through W4-4's composition, so the new arm is exercised, not just compiled).

#### A3. R5 data layer: descriptors, device payloads, GC, replication (fe-sdk + fe-runtime + fe-database, sonnet, L)

**Owned files**
- `fe-sdk/src/device_format.rs` (new) and `fe-sdk/src/lib.rs` (one `pub mod`)
- `fe-runtime/src/messages.rs` (new variants: `RegisterDeviceFormat`, `ListDeviceFormats`,
  `InsertDevicePayloads`, `ListDevicePayloads`, plus replies; `IotReadingInput.payload_id: Option<String>`
  with `#[serde(default)]`)
- `fe-database/src/schema.rs` (`device_format` and `device_payload` tables, `iot_reading.payload_id`
  optional field, indexes, `ALL_TABLE_NAMES`)
- `fe-database/src/admin.rs` (`KnownTable`)
- `fe-database/src/handlers/device_payload.rs` (new) and `fe-database/src/handlers/mod.rs`
- `fe-database/src/handlers/iot_reading.rs` (validate and write `payload_id`)
- `fe-database/src/handlers/asset_gc.rs`
- `fe-database/src/replication_mode.rs`
- `fe-database/src/handlers/replicated_row.rs`
- `fe-database/src/lib.rs` (dispatch arms)
- `fe-database/src/migration/mapping_inventory.rs`
- `fe-database/src/AGENTS.md`, `fe-database/src/migration/AGENTS.md`, `fe-sdk/src/AGENTS.md`
- `fe-database/tests/device_payload_test.rs` (new)
- **(Critic — files the slice needs but did not own; verified at `877e801`):**
  `fe-test-harness/src/peer.rs` (its `DbCommand` match is exhaustive, with no wildcard: the only
  catch-all is `Shutdown | Err(_)`, `:1086`), and `fe-runtime/src/app.rs`
  (`correlation_of_command`/`correlation_of_result`, `:229-260`) for every new command that
  carries a `correlation_id`. A4/A5 answer REST/MCP through that routing; the `_ => None` arm
  would compile and then silently never deliver the reply.

**Shapes** (defaults recorded in §7 as assumptions)
- **`DeviceFormatDescriptor`** (fe-sdk, serde-only, so a future plugin manifest can embed it):
  `{format_id: "vendor.device/name@major", mime, schema: Option<serde_json::Value> (JSON Schema, informative only), description}`.
- **`device_format` row:** `{format_id, verse_id, mime, schema, declared_by, created_at}`, keyed
  `(verse_id, format_id)`. It is immutable per key; re-declaring with a different MIME is a conflict.
- **`device_payload` row:** `{payload_id (ULID; client MAY supply it for idempotent retries), node_id, petal_id, verse_id, format_id, recorded_at, recorded_at_ms, hlc_timestamp, asset_ids: array<string> (ordered, 1..=MAX), source_did}`.

**Steps**
1. Write the handlers.
   - **register_format:** verse scope; a conflicting MIME is refused.
   - **insert_device_payloads:** mirrors `insert_readings_with_replication`. It validates everything
     first:
     - the anchor is in the petal;
     - the format is declared for the verse;
     - each asset id exists, is anchored to this petal, and has a `content_type` equal to the
       descriptor MIME;
     - the timestamp parses.
     It then inserts durably, then emits replication: the referenced `asset` rows first, then the payload
     row. A known client `payload_id` is an idempotent no-op that returns the existing row.
   - **list:** by petal / node / format / time window.
2. GC: add `SELECT VALUE asset_ids FROM device_payload` (flattened) to the `referenced` set in
   `asset_gc.rs`.
3. Replication:
   - `ReplicationMode::for_table("device_payload") = Timeseries`.
   - Inbound `device_payload` follows the iot_reading union path, after the A3 admission gate.
   - Inbound `asset` rows: insert-if-absent, never overwrite, **only for rows with an anchor `petal_id`**,
     with the anchor petal used for admission.
   - `device_format`: Row mode, insert-if-absent.
   - **(Critic, security: strictest reader.)** Inbound `device_format` requires the author to be
     **Manager+** at the verse, resolved by the same A3 role resolver from local tables. This
     matches the local/REST gate (A4, §7.2 A2): "shared cores + separate guards = drift"
     (RUNBOOK §5). The old text left the inbound gate at Editor+ until role propagation.
   - **(Critic, security: referential confinement, pre-empting P2P-REF-CONFINE.)** Every new
     inbound arm checks that its references are in the **replica's verse** before applying:
     `asset.petal_id`; `device_payload.petal_id`, `node_id ∈ that petal`, and every `asset_ids[i]`
     anchored to that petal; `device_format.verse_id == replica verse`. A reference to another
     verse → `Denied`. A reference unknown locally → `Denied` for now. Role propagation RP-4 turns
     it into `Deferred Parent` and owns that change.
4. Inventory: add `InsertIotReadings` (pre-existing drift), `RegisterDeviceFormat` and
   `InsertDevicePayloads` to `OTHER_DISPATCHED_MUTATION_KINDS`.

**Invariants**
- §iot-readings durable-first-then-emit ordering.
- §replication-mode: union semantics never widen authority, because the A3 gate runs before mode dispatch.
- The `asset_gc.rs` doc rule: the blob directory is never scanned.
- §schema-macro conventions.
- §log-first-commit is unaffected: these are hot-plane tables, like iot_reading.
- AGENTS anti-pattern 6: never swallow a channel send error.

**Flow tests** (`fe-database/tests/device_payload_test.rs`, through the public handlers and DbCommand dispatch, like `tests/iot_reading_test.rs`)
- `device_payload_ingest_records_asset_list_and_lists_it_back`: declare format → two blobs + asset rows →
  ingest → list by node/window returns one row whose `asset_ids` keep their order.
- `invalid_payload_batch_writes_nothing` (table-driven):
  - unknown format;
  - an anchor in another petal;
  - an asset anchored to a foreign petal;
  - a MIME/descriptor mismatch;
  - an empty asset list;
  - a bad timestamp.
  Each case leaves zero `device_payload` rows and a typed rejection.
- `retried_payload_with_same_client_id_is_idempotent`: the same row comes back and the row count is 1.
- `payload_referenced_asset_survives_gc_and_orphan_is_collected`: with grace 0, the orphan's row and blob
  are gone and the payload's assets remain. Deleting the new GC clause turns this red.
- `readings_link_to_their_payload`: readings with `payload_id` are written; an unknown `payload_id`
  rejects the batch.
- `inbound_device_payload_is_union_append_only` and `inbound_anchored_asset_row_insert_if_absent`:
  re-delivery is idempotent, a "newer" variant does not overwrite, an empty `del` is NotApplicable, and a
  Viewer-authored row is denied.
- `payload_emits_asset_rows_before_payload_row_after_durable_insert`: real `repl_tx` capture, order
  asserted.
- **(critic)** `inbound_device_rows_referencing_another_verse_are_denied` (table:
  `asset.petal_id`, `device_payload.petal_id`, a `device_payload.asset_ids` entry anchored
  elsewhere, `device_format.verse_id`).
- **(critic)** `an_editor_authored_inbound_device_format_is_denied_and_a_manager_one_applies`.
- **Note for role propagation:** RP-3 flips the "Viewer-authored row is denied" expectations in
  this file to `Deferred`, and RP-4 flips the unknown-parent rows to `Deferred Parent`. Both edits
  are owned there.

#### A4. R5 REST surface: generic asset ingest + device routes (fe-api, sonnet, L; security review-and-fix)

**Owned files**
- `fe-api/src/device.rs` (new)
- `fe-api/src/upload.rs` (generic `ingest_blob_with_limit` beside `ingest_glb_with_limit`;
  `place_asset_core` refuses non-GLB)
- `fe-api/src/assets.rs` (`X-Content-Type-Options: nosniff` on every blob response)
- `fe-api/src/iot.rs` (accept `payload_id` on readings)
- `fe-api/src/server.rs` (routes + per-route body limits)
- `fe-api/src/limits.rs`
- `fe-api/src/lib.rs`
- `fe-api/AGENTS.md` (new §device-ingest; §assets generic MIME + nosniff)
- `fe-api/tests/device_ingest_test.rs` (new)

**Routes**
- `POST /api/v1/verses/{v}/device-formats`: **Manager+** at verse scope (assumption, §7).
- `GET /api/v1/verses/{v}/device-formats`: Viewer+.
- `POST /api/v1/petals/{p}/iot/payloads`: Editor+, multipart. Fields: `format_id`, `node_id`,
  `recorded_at?`, `payload_id?`, and `file` parts in order.
  1. Stores each file through `ingest_blob_with_limit`. That function applies the owner quota, size caps
     and BLAKE3, and **never** runs `validate_glb` unless the MIME is `model/gltf-binary`.
  2. Creates asset rows with the descriptor MIME, the owner and the anchor petal.
  3. Sends `InsertDevicePayloads`.
  4. Returns 201 `{payload_id, asset_ids}`.
- `GET /api/v1/petals/{p}/iot/payloads?node_id&format_id&from&to`: Viewer+.

**Limits**
- `DEVICE_PAYLOAD_MAX_ASSETS = 32`.
- Per-file cap = `MAX_ASSET_BYTES`.
- Route body limit = 32 × file cap is too large, so cap the whole request at 256 MiB + 1 MiB, which is
  the same as the asset route.

**Invariants**
- §existence-oracle: a missing resource and a foreign one return byte-identical 404s.
- The §assets reach rule: anchor first; `owner_did` alone grants nothing.
- §asset-ingest: bytes never cross the DB channel (the `StoredGlb` pattern is generalized to `StoredBlob`).
- Real HTTP statuses, as in iot.rs.
- The 2026-10-12 `client` token claim (if it has landed by then) keys `owner_did`/quota. Otherwise `sub`,
  as today.

**Flow tests** (`fe-api/tests/device_ingest_test.rs`, ApiHarness)
- `register_format_ingest_multipart_payload_then_fetch_each_blob`: Manager declares the format; Editor
  POSTs 2 files and gets 201; `GET /assets/by-id/{id}` returns identical bytes with the descriptor MIME,
  `attachment` and `nosniff`; the list shows the payload.
- `device_routes_authz_and_limits` (table-driven, one test):
  - Viewer ingest → 403;
  - Editor register-format → 403;
  - foreign petal → 404 with oracle-identical text;
  - 33 files → 413;
  - over quota → 413;
  - unknown format → 422;
  - malformed ULID → 400.
- `device_asset_cannot_be_placed_as_a_scene_node`: REST `create_node` with a device (non-GLB) `asset_id`
  is refused. This protects the renderer.
- `html_payload_is_served_as_attachment_with_nosniff`. This is a security regression test.
- `readings_posted_with_payload_id_join_to_the_payload`.

#### A5. R5 MCP parity + BI query (fe-api, sonnet, M)

**Owned files**
- `fe-api/src/mcp/tools.rs`
- `fe-api/src/mcp/mod.rs`
- `fe-api/src/query_guard.rs` (`DEVICE_PAYLOAD` and `DEVICE_FORMAT`→verse-scoped handling, or keep
  `device_format` non-queryable; default: `device_payload` petal-scoped queryable, `device_format` via the
  REST list only)
- `fe-api/tests/mcp_integration.rs` (new test fn only)
- `fe-api/tests/analytics_scope_test.rs` (new test fn only)
- `fe-api/AGENTS.md` §mcp-dispatch / §query-guard

**Tools**
- `register_device_format`
- `list_device_formats`
- `ingest_device_payload` (base64 files; total decoded ≤ `MCP_MAX_ASSET_BYTES` = 11 MiB, matching the
  §asset-ingest envelope arithmetic)
- `list_device_payloads`

All four use the same core functions as A4, so there is no second validation path.

**Flow tests**
- `mcp_device_payload_full_loop`: register → ingest (2 files) → list → REST by-id returns the bytes.
- `query_device_payload_is_scoped_to_token_petals`: `/query SELECT * FROM device_payload` returns the
  in-scope row and the foreign petal's row is absent. Removing `DEVICE_PAYLOAD` from
  `PETAL_SCOPED_TABLES` turns it red.

#### A6. Two-peer flow + Phase A docs close (fe-test-harness + docs, sonnet, M)

**Owned files**
- `fe-test-harness/src/scenarios/two_peer_device_payload_sync.rs` (new)
- `fe-test-harness/src/scenarios/mod.rs`
- the harness scenario registry/list it already uses
- `fe-test-harness/src/AGENTS.md` §scenarios
- `README.md` (Status line: device payloads)
- `conductor/tracks/iot_spatial_reporting_20260714/` (a dated note only)

**Flow test**
- `two_peer_device_payload_sync`:
  1. Alice declares a format and ingests a payload with 2 blobs.
  2. Bob receives the `device_format`, the 2 anchored `asset` rows and the `device_payload` row (union).
  3. Bob's blob miss triggers `SyncCommand::FetchBlob`, and Bob then serves byte-identical content by
     `content_hash`.
  4. A re-delivered payload row does not duplicate.

  This is a real transport over the existing harness loopback, as in `two_peer_blob_exchange.rs` /
  `two_peer_timeseries_sync.rs`.

**Gate:** the Phase A review checkpoint. A critic/monitor pass checks that no unneeded deferral crept in;
see §6.

### Phase B: waits on owner approval of SPEC-10 (D-CL30..34)

#### B1. `observation` module: kind 5, payload codec, preview registry (fe-canonical-log, opus, M)

**Owned files**
- `fe-canonical-log/src/observation.rs` or `src/observation/` (new; `lib.rs` gains one `pub mod`, recorded
  as a §module-ownership exception in `src/AGENTS.md`)
- `fe-canonical-log/src/kind.rs` (`OperationKind::ObservationBatch` = 5, structural rule)
- `fe-canonical-log/src/wire/preview.rs` (registry-aware decode helper; `TELEMETRY_SAMPLE_PREVIEW_KIND = 1`
  + `TelemetrySamplePreview` codec)
- `fe-canonical-log/src/wire/AGENTS.md`
- `fe-canonical-log/src/AGENTS.md`

**Review:** **security** review-and-fix (critic upgrade). Kind 5 adds a new structural rule on the
peer-bytes ingress door (`decode_and_admit`) and a preview decoder.

**Steps**
1. Write the observation payload codec, using the A2 `ObservationSource`.
2. Add the kind-5 structural rule (`StructuralRuleError` variants).
3. Add an `OperationKindAvailability` impl that knows kind 5.
4. Write a reduction that emits only the reference mutation.

**Invariants**
- §ingress: `decode_and_admit` is the only door for peer bytes.
- §unified-vocabularies: no new quarantine enum, and `class()` stays wildcard-free.
- §Envelope invariants.
- G2: `reduce` reads only `meta` + `envelope_bytes`.

**Flow tests**
- `observation_batch_op_signs_admits_and_reduces_to_reference_only`: build → sign → `decode_and_admit` →
  reduce yields exactly one reference mutation with the payload fields.
- `observation_batch_structural_refusals` (table-driven):
  - 0 parents;
  - no-payload form;
  - inverted HLC or sensor range;
  - `count` / empty mismatch;
  - unsorted anchors or asset refs;
  - a float anywhere.
- `unupgraded_peer_quarantines_kind5_without_evicting_missing_parent_backlog`: G1 exercised with real
  kind-5 traffic at volume.
- `reduce_is_identical_whether_or_not_the_hexon_is_present`: the bright-line determinism test.
- `telemetry_preview_registry_accepts_kind1_drops_unregistered_and_rejects_op_fields`.

#### B2. Columnar observation hexon codec (fe-query `parquet` feature + fe-format, sonnet, M)

**Owned files**
- `fe-query/src/columnar/observation/` (new: `mod.rs`, `writer.rs`, `reader.rs`, `profile.rs`)
- `fe-query/src/columnar/mod.rs`
- `fe-query/src/AGENTS.md` (new §observation-hexon)
- `fe-format/src/manifest.rs` (`HexonType::ObservationBatch`)
- `fe-format/src/AGENTS.md` §manifest

**Steps**
1. Write the writer: segmentby anchor, orderby `(hlc, reading_id)`, pinned writer properties, row-group
   statistics on, plus `payload_refs.parquet`.
2. Write the stored-zip `.hexon` packer using the fe-format archive, with fixed timestamps.
3. Write the reader/validator. It enforces the profile and returns typed errors. **(Critic, one
   zip reader.)** Open the `.hexon` through `fe_format::archive_index::HexonArchiveIndex` (built
   in PAS-P4 S5, extended with verified reads in PAS-P5 S2) under `fe_format::limits`. Do not add
   a third zip reader. The doctrine is "shared cores + separate guards = drift" (RUNBOOK §5).
4. Compute a stats summary (per-anchor min/max/time range) that becomes the sealed statistics artifact
   (`stats_hash`).

**Invariants**
- fe-format §decompression-limits on import.
- `TilesetMeta`-style additive serde for any new manifest field (`fe-format/src/AGENTS.md` §scale).

**Flow tests**
- `observation_hexon_round_trip_honours_segment_and_order_profile`: readings for 3 anchors written in
  shuffled order; the reader sees per-anchor row groups, ascending HLC and correct row-group statistics.
- `same_rows_any_input_order_produce_identical_hexon_bytes`.
- `profile_violations_rejected` (table-driven over crafted files):
  - a mixed-entity row group;
  - unsorted HLC;
  - a missing required column;
  - a wrong `HexonType`.
- `zip_bomb_observation_hexon_refused_without_allocation`. This reuses the REG-DECOMP fixture style.

#### B3. Shadow sealer, reference materializer, hydrator (fe-database canon_log + migration, opus, L)

**Owned files**
- `fe-database/src/canon_log/observation.rs` (new)
- `fe-database/src/canon_log/mod.rs`
- `fe-database/src/canon_log/AGENTS.md` (new §observation)
- `fe-database/src/migration/mapping_inventory.rs` (register the observation family as `Mapped` under the
  approved SPEC-10 schema; D-CL20 kinds remain refused)
- `fe-database/src/migration/comparator.rs` (observation-family comparison)
- `fe-database/src/schema.rs` (`observation_batch` reference table, appended like the canon tables and
  **not** in `ALL_TABLE_NAMES`)
- `fe-database/Cargo.toml` (enable `fe-query/parquet`)
- `fe-database/tests/observation_plane_test.rs` (new)
- **(Critic, ordering)** `fe-database/src/canon_log/verifier.rs`: register the kind-5 observation
  schema through CDL-W4-7's `RegisteredSchemaSet::register`. Without this, the durable verifier
  answers `UnknownKind` and quarantines every batch the sealer appends, so the parity test cannot
  pass.

**Review:** **security** review-and-fix (critic upgrade). The sealer signs operations with the
node key and appends to the verified log.

**Steps**
1. Write the sealer. For a closed `(petal, bucket)` window of hot rows:
   1. build the hexon (B2);
   2. seal it as BulkArtifact/Shard (A2);
   3. seal the stats artifact;
   4. assemble and sign a kind-5 op with the node key;
   5. append through the `canon_log` append store.
   It runs **only** when `MigrationFlags::mode()` is `DualEmitShadow`/`ShadowRebuild`. The default is
   `legacy_only` (SPEC-8 §3 rule 1).
2. Materialize the reference rows.
3. Write the hydrator: `materialize/shard` into a separate catalog. A missing artifact is reported as
   unavailable and is never guessed.
4. Rebuild from the log.
5. Compare the hydrated catalog with legacy `iot_reading`/`device_payload` and produce the SPEC-8 §6
   report.

**Invariants**
- `canon_log/AGENTS.md` §verified-log (re-verify on read).
- §parallel-and-dormant: the live ingest path is unchanged when the mode is `legacy_only`.
- Wiring the sealer to live ingest happens only behind the mode flag; the flag is never flipped by
  parity (SPEC-8 §3 rule 4).
- No network.

**Flow tests**
- `legacy_only_ingest_creates_no_observation_candidate`: the default path stays byte-identical.
- `shadow_mode_seals_window_commits_batch_and_hydrates_parity_with_legacy`: real DB with readings + a
  payload → sealer → a kind-5 op in `verified_op_log` → reference row → the hydrated catalog equals the
  legacy rows (comparator report: zero divergences).
- `rebuild_from_log_reproduces_observation_reference_rows`.
- `missing_hexon_surfaces_unavailable_at_hydration_not_reduce`.
- `analytics_identity_records_the_batches_it_read`: the G6 end-to-end check.

#### B4. Phase B docs + approval close (docs, haiku, S)

**Owned files**
- `docs/spec/canonical-log/observation-plane.md` (Status → "Owner-approved <date>" only when the owner
  says so)
- `commit-preview-wire.md` (Errata cross-ref to the registry, under D-CL33)
- `fe-canonical-log/src/AGENTS.md` (module table row `observation | SPEC-10 | implemented`)
- `conductor/tracks/canonical_data_log_20260808/metadata.json`
- `retro` section in the track

---

## 4. Docs to update (owner per slice above)

- **New spec:** `docs/spec/canonical-log/observation-plane.md`. Errata in `segment-shard-relay.md` (G3),
  `log-first-materialization.md` (G6), and `commit-preview-wire.md` (registry cross-ref, after approval).
- **Code AGENTS:**
  - `fe-canonical-log/src/AGENTS.md` (errata 2026-10-13, provisional wire numbers for lane 5, the
    observation payload, ObservationSource keys, and the module exception)
  - `segment/AGENTS.md` §discovery-lanes (a BulkArtifact row)
  - `materialize/AGENTS.md`
  - `wire/AGENTS.md`
  - `fe-database/src/AGENTS.md` (§device-payloads new; §iot-readings `payload_id`; §replication-mode for
    device_payload / anchored asset / device_format)
  - `asset_gc` notes in §handlers
  - `migration/AGENTS.md` (inventory additions, including the fixed `InsertIotReadings` drift)
  - `canon_log/AGENTS.md` §observation
  - `fe-api/AGENTS.md` (§device-ingest, §assets nosniff + generic MIME, §mcp-dispatch, §query-guard)
  - `fe-query/src/AGENTS.md` §observation-hexon
  - `fe-format/src/AGENTS.md`
  - `fe-sdk/src/AGENTS.md`
  - `fe-test-harness/src/AGENTS.md`
- **Decision register** rows D-CL30..34 (PENDING-RATIFICATION). Track `metadata.json` task strings.
- **Handed to the bookkeeping owner** (not edited here): ledger rows `CDL-SPEC10` (→ draft submitted /
  Phase A done), `DB-TS-RETENTION` (now blocked only on cutover), and a new row for the R5 device-payload
  model; RUNBOOK §2/§8.

---

## 5. Risks

1. **GC data loss.** The current GC treats assets referenced only by a payload as unreferenced
   (`asset_gc.rs:35`). Mitigation: the A3 clause plus the mutation-checked flow test. A3 must land before
   any device-payload route is reachable, so A3 precedes A4.
2. **Stored XSS / MIME sniffing.** Arbitrary descriptor MIME types are served from the API origin.
   Mitigation: `attachment` (already present) + `nosniff` (new) + a regression test, plus the A4 security
   review.
3. **Asset-row replication resurrection.** A peer that GC'd an orphan can receive the same row again
   through the union path. Mitigations: inbound applies only anchored rows; payload-referenced assets are
   never GC-eligible; the asset-before-payload emit order is protected by the GC grace window (default
   from `FEAPI-BLOB-GC`). The resurrected-orphan case is harmless (a row without a blob returns 404); this
   is documented.
4. **Schema skew across peers.** `iot_reading.payload_id` is an additive field on a SCHEMAFULL table. An
   old peer would refuse it. The iroh flag day (2026-10-12: "no 0.35 window, peers upgrade together")
   already requires a lockstep upgrade.
5. **Provisional wire numbers** (lane 5, kind 5, payload keys) ship before CDL-PROV ratification. They are
   allowed by D-CL24(3) with no interop claim, and recorded in the ratification index.
6. **Parquet determinism.** The arrow/parquet 54 line is pinned through datafusion 46 (`fe-query/Cargo.toml:28`).
   A future bump can change bytes. Mitigations: `created_by` is pinned and there is a determinism test.
   Batch identity is the author's bytes, so non-determinism only costs dedupe, not correctness.
7. **Concurrent edits from earlier phases.** CDL-W4 edits `compose.rs`, the fe-canonical-log `lib.rs`
   surface and `migration/boundary.rs`. Role propagation (after this phase) edits `replicated_row.rs`, the
   same file as A3. Slices are sequential, so the risk is a merge-intent loss. Role propagation's brief
   must preserve the `device_payload` / `asset` / `device_format` arms.
8. **Build weight.** B3 makes fe-database compile arrow/parquet. That is not a new compile unit for the
   workspace, but `cargo check -p fe-database` becomes slower. Accepted.
9. **Approval latency.** If the owner has not reviewed SPEC-10, Phase B cannot start. Phase A is
   deliberately self-sufficient value, so nothing is wasted.

---

## 6. Deferral audit (the user forbids unneeded deferrals)

| Item | Left out of this phase? | Justification class |
|---|---|---|
| G3 / G6 code + errata text | **No.** Done in A1/A2. | D-CL28 already ratified them; precedent `c375beb`. |
| R5 descriptors, device payloads, generic asset ingest, MCP/BI parity, two-peer sync | **No.** Done in A3-A6. | Ratified by R5. *Note:* this pulls the R5 slice forward from the "second campaign" assumption in RUNBOOK §4; see §7 assumption 1. |
| Phase B (observation module, hexon codec, shadow sealer/hydrator) | **Waits, not deferred.** | Missing **user decision**: SPEC-10 approval. Policy: SPEC-8 §3 rule 3 and §7.1, track plan Workstream S. |
| Decoder plugins (Rhai/Wasm → readings) | Yes. The seam lands now (`payload_id` link + `DecoderIdentity` in G6). | **Ratified sequencing** (R5: "decoders after the host is wired") + **hard dependency** on PLUG-HOST (L, dormant host, mandatory sandbox security review). Trigger: PLUG-HOST scheduled. |
| Preview *delivery* of `telemetry_sample_v1` over `/ws/canonical` | Yes. The codec + registry land in B1. | **NO_GO class:** CANON-WS mount (network exposure, owner-gated). |
| Canonical network replication of batches/segments, relay seeding | Yes. | **NO_GO class:** CDL-NET (`network_enablement: prohibited`). |
| `canonical_authoritative_local` cutover for the observation family | Yes. | **User decision:** SPEC-8 §7.4 local cutover gate (owner reviews the §6 report). |
| `iot_reading` / `device_payload` retention GC behind batches (DB-TS-RETENTION) | Yes. Mechanism specified in SPEC-10 §8. | **Hard dependency:** legacy rows are authoritative until cutover, so deleting them earlier loses data. Also D-CL24(4): retention numbers are caller policy. Trigger: cutover approved. |
| Directory assets (`application/x-fe-directory` 501, FEAPI-DIRASSET) | Out of scope (not a deferral of this phase's scope) | R5 chose a *list of single assets*, so this phase does not need them. They belong to the D-20 W5 op-store, which is another track. **(Critic)** The "second campaign" placement comes from RUNBOOK §4, an *unasked assumption*, not a user decision; it is raised to the user with §7.2 A1. |
| Placeholder rendering of non-GLB assets on nodes (bucket FR-3) | Out of scope; A4 *refuses* placement | It belongs to the hexon_p2p_bucket FR-3 track. Refusing is the safe v1 behaviour and is pinned by a test (`device_asset_cannot_be_placed_as_a_scene_node`). The same §4 caveat applies. |
| Manifest statistics validation in selective fetch (CDL-STATS) | Yes. | Trigger-gated: no selective-fetch consumer exists (ledger `CDL-STATS`). |
| Poll-based connectors (IOT-4.2) | Yes. | D-6 ratified push-style ingest (R3); poll connectors stay future work. |

No other omissions. The critic/monitor the user asked for should challenge any row above whose class is not
one of: missing user decision, NO_GO, hard external dependency, or explicit ratified sequencing.

---

## 7. User decisions and assumptions

### 7.1 Genuine user decisions

1. **Approve SPEC-10 (D-CL30..34).** This is the only gate on Phase B. The draft presents each choice with
   its default and the alternatives:

   | Choice | Default | Alternative |
   |---|---|---|
   | Operation kind | dedicated kind 5 | kind 1 + registered schema |
   | Hexon container | `.hexon` stored-zip containing Parquet | a bare Parquet blob, or a CBOR-wrapped Parquet |
   | Entity (segmentby) | anchor `node_id` | `(node_id, metric)` |
   | Preview registry | `1 = telemetry_sample_v1`, reserved ranges | — |

   The user's standing rule is that the user approves specs, so this cannot become an assumption.

### 7.2 Assumptions (sensible defaults, with reversal cost)

1. **R5 slices run in this phase** (A3-A6), not in the "second campaign" of RUNBOOK §4. SPEC-10 has to
   describe them and they are ratified. Reversal: move A3-A6 to campaign 2. Cheap before A3 starts.
2. **Format registration is Manager+ at verse scope.** It is schema-defining, like branch controls; payload
   ingest is Editor+. Reversal: one `require_role` string plus a test row (cheap).
3. **Anchored `asset` rows replicate insert-if-absent; legacy un-anchored rows do not.** Reversal: moderate
   (inbound arm + harness scenario).
4. **Client-supplied `payload_id` gives idempotency.** Server mints a ULID when absent. Reversal: cheap.
5. **Limits:** 32 assets per payload; per-file `MAX_ASSET_BYTES`; MCP ≤ 11 MiB decoded. Reversal: constants.
6. **Descriptor `schema` is informative JSON Schema; bytes are never validated against it in v1** (R5:
   "bytes stored opaque"). Reversal: add validation later (additive).
7. **`device_format` is immutable per (verse, format_id).** A new version means a new `@major` id.
   Reversal: moderate.

---

## 8. Critic amendments (2026-10-13)

Build order: PAS-P3P4 → PAS-P5 → CDL-W4 → **CDL-SPEC10** → role propagation. An earlier phase
owns a shared piece; this plan notes the dependency.

**RUNBOOK conflict (assumption, not a §3 decision, so it is flagged rather than reversed).**
- RUNBOOK §4 (2026-10-13, *"unasked in the grill"*): *"The 2026-10-13 scope additions (generic
  asset + device-payload records + format descriptors, decoder plugins/PLUG-HOST, …) form a SECOND
  campaign after that sweep, so the first sweep is not deferred behind months of code · to
  reverse: merge the campaigns (cheap now)."*
- This plan, §7.2 A1: A3-A6 build those records in **this** campaign.
- PAS-P5's deferral table had cited the same §4 line the other way. That row is now re-classified,
  and **this plan owns** the `asset` / `device_payload` / `device_format` tables. PAS-P5 changes
  no table, so there is no schema collision.
- No §3 decision forbids the merge, and R5 ratified the shapes. **Kept as planned; raised to the
  user** as a remaining decision (default: keep A3-A6 here).

**Edits made.**

| # | Change | Class |
|---|---|---|
| C1 | A2 owns the `BulkArtifact` arm in `compose.rs` (W4-4's wildcard-free `LaneClass` match), and has a test exercising it. | cross-plan collision / ordering |
| C2 | A3 owns `fe-test-harness/src/peer.rs` (exhaustive `DbCommand` match) and `fe-runtime/src/app.rs` (correlated reply routing). | missing files (compile + silent non-delivery) |
| C3 | A3 inbound `device_format` raised to Manager+, matching A4's local gate (strictest reader / no guard drift). | security |
| C4 | A3 inbound arms check referential confinement to the replica's verse; two tests added. Role propagation RP-4 inherits them and only adds `Deferred Parent`. | security + cross-plan (P2P-REF-CONFINE) |
| C5 | B2 reads `.hexon` through `HexonArchiveIndex` (PAS-P4/P5); no third zip reader. | cross-plan collision |
| C6 | B3 registers kind 5 via CDL-W4-7's `RegisteredSchemaSet::register` and owns `canon_log/verifier.rs`. Without it every sealed batch quarantines as `UnknownKind`. | ordering bug |
| C7 | A2, B1 and B3 reviews upgraded to **security** review-and-fix (crypto lane, ingress rule, node-key signing). | missing security review |
| C8 | The directory-asset / placeholder rows are re-classed as out-of-scope (other tracks). The §4 "second campaign" cite is flagged as an assumption. | deferral justification |

**Cross-plan note.** A6's device-payload blob fetch uses `SyncCommand::FetchBlob` over the blob
ALPN. PAS-P3P4 S8b may add an iroh-blobs request filter, and its brief requires that filter to
key on verse membership so A6 and `two_peer_blob_exchange` keep working.
