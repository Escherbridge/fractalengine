# Canonical log observation plane v1

**Status:** PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending). Decisions D-CL30..34, RUNBOOK §3 R7. Local implementation in default-off shadow modes is unlocked; network rollout, relay seeding, inbound P2P, the `/ws/canonical` mount, and `canonical_authoritative_local` cutover remain owner-gated.

This document defines the observation plane of the Canonical Fractal Data Log.
It is SPEC-10. It implements D-CL26 (two-plane architecture) and the
observation half of D-CL29. It consumes the D-CL28 gates G1 (unknown-kind
budget), G2 (reference-only reduction), G3 (bulk-artifact lane, SPEC-6 §3.4),
G4 (tiered statistics), and G6 (observation source identity, SPEC-4 §7 rule 5).
It reconciles the 2026-10-13 R5 decision: one generic asset model, a
device-payload record that holds a list of assets alongside scalar
`iot_reading`, and device extensions made of declared format descriptors and
sandboxed decoder plugins.

It is read with [operation-envelope.md](operation-envelope.md) (SPEC-1),
[capabilities-and-revocation.md](capabilities-and-revocation.md) (SPEC-3),
[log-first-materialization.md](log-first-materialization.md) (SPEC-4),
[branches-checkpoints-retention.md](branches-checkpoints-retention.md) (SPEC-5),
[segment-shard-relay.md](segment-shard-relay.md) (SPEC-6),
[commit-preview-wire.md](commit-preview-wire.md) (SPEC-7), and
[migration-plan.md](migration-plan.md) (SPEC-8).

## 1. Conformance vocabulary and boundary

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are
   normative. `Hash32`, `Identifier32`, and byte comparison have their SPEC-1
   §1 meanings.
2. The **operation plane** is the canonical log of SPEC-1 through SPEC-8:
   signed intent, scene structure, and authority. The **observation plane** is
   the part of the same log that commits telemetry and device payloads. It uses
   the same envelope, DAG, capability, admission, quarantine, segment, and
   checkpoint rules. It differs in one respect: each operation commits a
   *batch* of observations by reference, never one operation per observation
   (D-CL26).
3. An **observation** is one immutable fact accepted at an ingest door: a
   scalar **reading**, or a **device payload** (an ordered list of generic
   assets of one declared format).
4. An **ingest door** is a hot-plane write handler that accepts observations
   after its caller has been authenticated and authorized. Today there is one,
   `insert_readings_with_replication`
   (`fe-database/src/handlers/iot_reading.rs:82`); the device-payload
   handler that slice A3 adds is its sibling and follows the same
   durable-first-then-emit rule. REST reaches it directly
   (`POST /api/v1/petals/{p}/iot/readings`, `fe-api/src/server.rs:312`,
   `fe-api/src/iot.rs:104`) or through `DbCommand::InsertIotReadings`
   (`fe-api/src/iot.rs:149`, `fe-database/src/lib.rs:1414`), and the
   simulation lab reaches it through the same command
   (`fe-sim/src/session.rs:454`).
5. The **hot plane** is the set of legacy SurrealDB tables that hold
   observations for query: `iot_reading`, `device_payload`, `asset`, and
   `device_format`. Until an owner-approved cutover (§8) the hot plane is the
   authority for observations. This document does not change that.
6. An **observation batch** is an admitted operation of `operation_kind = 5`
   (§4). Its payload binds one sealed observation hexon by reference.
7. An **observation hexon** is the sealed bulk artifact (SPEC-6 §3.4) that
   holds a batch's observations in columnar form (§3).
8. **Hydration** is the SPEC-3 `materialize/shard` step that expands an
   observation hexon into a derived local catalog (§5). It is not a SPEC-4
   reduction.
9. This document authorizes local implementation only. It authorizes no
   network transport, relay seeding, inbound P2P, canonical replication of
   batches or bulk artifacts, or `/ws/canonical` mount. Those stay under the
   CDL-NET and CANON-WS owner gates, so the preview kind registered in §6 has a
   codec and no live transport. It also authorizes no
   `canonical_authoritative_local` cutover (§8, SPEC-8 §7 rule 4).

## 2. Ingestion model

### 2.1 One ingestion story (informative)

```text
device · connector · simulation lab · (later) decoder plugin
   │  one authenticated ingest batch: readings[] (scalars) + payloads[] (format_id, files[])
   ▼
ingest door (Editor+, petal scope, rate and size caps) ── file bytes ──► iroh blob store (BLAKE3)
   │
   ▼  durable insert first, replication emit second
HOT PLANE (legacy authority; SQL/BI-queryable; union replication)
   asset            generic asset: MIME content_type + content_hash + owner_did + anchor petal_id
   device_format    verse-scoped descriptor: format_id + MIME + optional informative schema
   device_payload   {payload_id, node_id, petal_id, format_id, recorded_at, asset_ids[ ]}
   iot_reading      scalar fact; optional payload_id ("sent with / decoded from this payload")
   │
   ▼  shadow modes only (§8), default off
OBSERVATION PLANE (canonical, local)
   window of this node's own ingests ─► observation hexon (readings + payload_refs, Parquet)
     ─► sealed BulkArtifact/shard ─► kind-5 observation_batch operation ─► verified log
   reduce(): one reference row (G2)      hydration: derived local catalog (§5)
LIVE: preview_kind 1 = telemetry_sample_v1 on the SPEC-7 preview class (lossy, never persisted)
```

Each record has one job. `iot_reading` is the queryable scalar projection.
`device_payload` is the queryable index of opaque bytes. An observation batch
is the durable signed commit of both for one window.

### 2.2 Records (R5)

1. **One door.** Every observation MUST enter through an ingest door,
   whatever produced it: a device, a connector, the simulation lab, a person,
   or a decoder. Every observation the observation plane commits MUST be one an
   ingest door accepted. No component writes an observation to the hot plane
   or to the observation plane by another path.
2. **One asset model.** The generic asset is the existing `asset` record: a
   MIME `content_type`, an iroh blob `content_hash` (BLAKE3 of the bytes), an
   `owner_did`, and an anchor `petal_id` (`fe-database/src/schema.rs:228-241`).
   Device payloads, path assets, and `hexon_p2p_bucket` share this model (R4
   D-17 override). This document adds no second asset record.
3. **Format descriptor.** A device format descriptor declares, for one verse,
   a `format_id`, the MIME that every asset of that format carries, and an
   optional JSON Schema. The schema is informative. V1 stores payload bytes
   opaque (R4 D-5 override, R5) and MUST NOT reject bytes against it. A
   descriptor is immutable per `(verse, format_id)`. A new major version is a
   new `format_id`.
4. **`format_id` grammar.** A `format_id` is lowercase ASCII of 5 to 128 bytes
   of the form `<namespace>/<name>@<major>`. `<namespace>` is two or more
   dot-separated labels, each `[a-z0-9][a-z0-9-]*`. `<name>` is
   `[a-z0-9][a-z0-9._-]*`. `<major>` is `0` or a decimal integer without a
   leading zero, at most 4294967295. Example: `acme.thermo/frame@1`. The
   grammar is fixed here because a `format_id` enters signed bytes (§7.1).
5. **Device payload.** A device payload record holds one ordered, non-empty
   list of asset identifiers of one format, anchored to one node in one petal,
   with sensor time, ingestion HLC, and the ingesting principal. Every listed
   asset MUST be anchored to the payload's petal and MUST carry the
   descriptor's MIME. A client MAY supply the `payload_id` (a ULID) so that a
   retried ingest is idempotent.
6. **Scalar reading.** `iot_reading` stays the SQL- and BI-queryable scalar
   projection. A reading MAY name the `payload_id` it was sent with or decoded
   from. An ingest batch that names an unknown `payload_id` MUST be rejected
   whole.
7. **Device extension.** A device extension is a descriptor (rule 3) and,
   optionally, a sandboxed decoder plugin (Rhai or Wasm, `fe-plugin`) that
   reads a payload's asset bytes and emits readings. A decoder MUST emit
   through an ingest door (rule 1). It MUST set each reading's `payload_id` to
   the payload it decoded. Its readings MUST carry an `ObservationSource` of
   class `decoder` with its `DecoderIdentity` (§7.1). A decoder MUST NOT write a
   fourth record type, MUST NOT mutate a payload or its assets, and MUST NOT
   bypass the door. Decoders come after descriptors and after the plugin host
   is wired (R5; ledger PLUG-HOST). This document fixes only their canonical
   seam.
8. **Hot-plane rules this document relies on.** Hot-plane shapes, routes,
   limits, GC, and replication are specified in the owning crates' `AGENTS.md`
   files as slices A3 to A5 land. Three of those rules are load-bearing here and
   MUST hold:
   1. Asset GC treats an asset that a device payload references as
      referenced. Today GC counts only `node.asset_id`
      (`fe-database/src/handlers/asset_gc.rs:34-41`), so a payload-only asset
      would be deleted after its grace period.
   2. A device asset is never placed as a scene node.
   3. Every blob response carries `Content-Disposition: attachment` (present
      today, `fe-api/src/assets.rs:404,418-423`) and `X-Content-Type-Options:
      nosniff` (absent today), because descriptors admit arbitrary MIME types
      such as `text/html`.

### 2.3 Current code (informative, verified at `0848237`)

| Fact | Where |
| --- | --- |
| `asset` is already MIME-agnostic: `content_type`, `content_hash`, `owner_did`, anchor `petal_id`. | `fe-database/src/schema.rs:228-241` |
| Asset ingest is GLB-only (`validate_glb`), and `place_asset_core` assumes a renderable GLB. | `fe-api/src/upload.rs:160,309,362` |
| `iot_reading` holds scalar `value: f64`, a denormalized `petal_id`, `recorded_at`, `recorded_at_ms`, a packed `hlc_timestamp`, and `source_did`. | `fe-database/src/schema.rs:262-273` |
| The packed HLC holds wall-clock milliseconds in its upper 48 bits and a counter in its lower 16. | `fe-database/src/AGENTS.md` §hlc |
| `reading_id` is minted server-side, so a device retry is not idempotent. | `fe-database/src/handlers/iot_reading.rs:140` |
| Ingest requires Editor+, caps a request at 1000 readings, and is durable-first, then emit. | `fe-api/src/iot.rs:57,82`; `fe-api/src/limits.rs:25`; `fe-database/src/handlers/iot_reading.rs:82` |
| A reading's `source_did` is the caller's token `sub` on the REST path, and the host peer's DID on the simulation path. | `fe-api/src/iot.rs:152`; `fe-sim/src/session.rs:454-460` |
| `iot_reading` replicates as a union (`Timeseries`); every other table is `Row`. | `fe-database/src/replication_mode.rs:34-39` |
| The inbound row path applies verse, fractal, petal, and node only; asset rows do not replicate. | `fe-database/src/handlers/replicated_row.rs:184-207` |
| Blobs move between peers by `SyncCommand::FetchBlob`. | `fe-sync/src/messages.rs:18`; `fe-sync/src/sync_thread.rs:766,1203` |
| `iot_reading` is petal-scoped and BI-queryable; `asset` is not queryable. | `fe-api/src/query_guard.rs:54-61`; `fe-api/AGENTS.md` §assets |
| The timeseries fabric shards by `(petal, anchor, time bucket)`. | `fe-sync/src/AGENTS.md` §sharding |
| `fe-plugin` is dormant: no shipped binary depends on it, and its WIT is aspirational. | `fe-plugin/src/AGENTS.md:131-134` |
| `InsertIotReadings` is dispatched but missing from the mapping inventory. | `fe-runtime/src/messages.rs:618`; `fe-database/src/lib.rs:1414`; `fe-database/src/migration/mapping_inventory.rs:49-87` |
| `LaneClass` has four lanes, and `from_wire` refuses anything but 1 to 4. | `fe-canonical-log/src/segment/artifact.rs:30-61` |
| `ObjectClass::{Shard,Tile,Asset}` exist with bits `0x08`, `0x10`, `0x20`; no lane carries them. | `fe-canonical-log/src/capability/verbs.rs:67-113` |
| `OperationKind` covers kinds 1 to 4 plus `Application(u16)` for 5 and above. | `fe-canonical-log/src/kind.rs:9-20` |
| G1 is built: `QuarantineReason::UnknownKind`, a wildcard-free class map, `OperationKindAvailability`. | `fe-canonical-log/src/compose.rs:51,150-167`; `fe-canonical-log/src/retention/quarantine.rs:394-399` |
| G2 is built: `ReferencedArtifactUnavailable`; `reduce` takes `meta` and `envelope_bytes` only. | `fe-canonical-log/src/materialize/traits.rs:164-200,271-275` |
| G4 is built: `SegmentStatistics` and `SealedStatisticsRef`. | `fe-canonical-log/src/segment/manifest.rs:77,161` |
| `preview_kind` is a `u32` that must be non-zero; no registry exists. | `fe-canonical-log/src/wire/preview.rs:53,87-90,143-146` |
| The SPEC-8 modes exist; `legacy_only` is the default. | `fe-database/src/migration/mode.rs:14-38,76-109` |
| Arrow/Parquet 54 is available behind the `fe-query` `parquet` feature, built with `default-features = false`, so no compression codec is compiled in (the locked `parquet 54.3.1` resolves no `zstd`, `snap`, `lz4`, or `flate2` crate). | `fe-query/Cargo.toml:15,29-30`; `fe-query/src/columnar/geoparquet/mod.rs:110,144` |
| `HexonType` has no observation type. | `fe-format/src/manifest.rs:6-23` |
| The `.hexon` importer requires `manifest.json` and `entries.json` and caps decompression. | `fe-format/src/archive.rs:256-279`; `fe-format/src/limits.rs:8-14` |

## 3. Observation hexon

### 3.1 Container

1. The plaintext of an observation hexon is an fe-format `.hexon` ZIP archive
   whose `manifest.json` names `hexon_type = "observation_batch"`. This is a new
   `HexonType` value. Older parsers treat it as opaque
   (`docs/hexon-format-spec.md` §Forward Compatibility).
2. The archive contains exactly these members, in this order:
   `manifest.json`, `entries.json`, `observations/readings.parquet`,
   `observations/payload_refs.parquet`. Both Parquet members are always present.
   Either MAY hold zero rows, but not both (§4.3 rule 3).
3. Every member is stored uncompressed (ZIP method `Stored`) with the MS-DOS
   epoch timestamp (1980-01-01 00:00:00), no extra fields, no file or archive
   comment, and fixed external attributes. The archive bytes are therefore a
   pure function of the member bytes.
4. `manifest.json` is the canonical JSON (keys sorted recursively, no
   whitespace; fe-format `src/AGENTS.md` §signature) of a manifest in which
   every field is a function of the batch:

   | Field | Value |
   | --- | --- |
   | `schema_version` | `"1.0.0"` |
   | `hexon_id` | lowercase hex of `BLAKE3(BLAKE3(readings member) ‖ BLAKE3(payload_refs member))` |
   | `hexon_type` | `"observation_batch"` |
   | `publisher_did` | the batch author's DID |
   | `name` | `"observation-batch"` |
   | `version` | `"1"` |
   | `created_at`, `updated_at` | RFC 3339 UTC, millisecond precision, of the batch's maximum ingestion HLC wall time |
   | every optional field | absent, or empty for list fields |

   The manifest carries no `signature`. The kind-5 operation's signature
   already binds the sealed hexon, and a second signature would be a second
   use of the author key with nothing further to bind.
5. `entries.json` is the empty catalog `{"entries":[]}`. The fe-format
   importer requires the member (`fe-format/src/archive.rs:272-279`).
   Observation tables are not scene assets and are not catalogued.
6. A reader MUST apply the fe-format entry-count, per-entry, JSON-entry, and
   whole-archive decompression limits (`fe-format/src/limits.rs:8-14`) before
   it allocates. It MUST reject an archive whose member set, member order, or
   storage method differs from rules 2 and 3.

### 3.2 Parquet profile

1. Each Parquet member is one Parquet file written with the pinned writer
   profile of `hexon_format_version = 1`:
   - pages are uncompressed;
   - column-chunk statistics (`min`, `max`, `null_count`) are enabled for
     every column;
   - `created_by` is the constant `fe-observation-hexon-v1`;
   - the file carries exactly one key-value metadata entry,
     `fe.observation.hexon_format_version = "1"`;
   - the writer version, data-page size limit, dictionary policy, and
     maximum row-group row count are fixed constants, recorded with the
     implementation in `fe-query/src/AGENTS.md` §observation-hexon.

   A change to any pinned value is a new `hexon_format_version`.
2. Floating-point values are permitted inside the hexon. Its interior is an
   opaque sealed artifact, not signed CBOR. The SPEC-1 §2 float ban applies
   only to signed bytes (D-CL26). Every `DOUBLE` value MUST be finite.
3. `observations/readings.parquet` has exactly these columns, in this order:

   | # | Column | Parquet type | Nullable | Source |
   | ---: | --- | --- | --- | --- |
   | 0 | `reading_id` | `BYTE_ARRAY` (UTF8) | no | `iot_reading.reading_id` |
   | 1 | `anchor` | `FIXED_LEN_BYTE_ARRAY(32)` | no | §4.5 legacy identifier of `node_id` |
   | 2 | `anchor_node_id` | `BYTE_ARRAY` (UTF8) | no | `node_id` |
   | 3 | `metric` | `BYTE_ARRAY` (UTF8) | no | `metric` |
   | 4 | `value` | `DOUBLE` | no | `value` |
   | 5 | `units` | `BYTE_ARRAY` (UTF8) | no | `units` |
   | 6 | `recorded_at` | `BYTE_ARRAY` (UTF8) | no | `recorded_at` |
   | 7 | `recorded_at_ms` | `INT64` | no | `recorded_at_ms` |
   | 8 | `hlc_wall_ms` | `INT64` | no | packed `hlc_timestamp >> 16` |
   | 9 | `hlc_counter` | `INT32` | no | packed `hlc_timestamp & 0xFFFF` |
   | 10 | `payload_id` | `BYTE_ARRAY` (UTF8) | yes | `payload_id` |
   | 11 | `source_index` | `INT32` | no | index into the batch's `sources` (§4.3) |

4. `observations/payload_refs.parquet` has exactly these columns, in this
   order:

   | # | Column | Parquet type | Nullable |
   | ---: | --- | --- | --- |
   | 0 | `payload_id` | `BYTE_ARRAY` (UTF8) | no |
   | 1 | `anchor` | `FIXED_LEN_BYTE_ARRAY(32)` | no |
   | 2 | `anchor_node_id` | `BYTE_ARRAY` (UTF8) | no |
   | 3 | `format_id` | `BYTE_ARRAY` (UTF8) | no |
   | 4 | `recorded_at` | `BYTE_ARRAY` (UTF8) | no |
   | 5 | `recorded_at_ms` | `INT64` | no |
   | 6 | `hlc_wall_ms` | `INT64` | no |
   | 7 | `hlc_counter` | `INT32` | no |
   | 8 | `asset_ids` | `LIST<BYTE_ARRAY (UTF8)>` | no |
   | 9 | `asset_content_hashes` | `LIST<FIXED_LEN_BYTE_ARRAY(32)>` | no |
   | 10 | `source_index` | `INT32` | no |

5. **segmentby = entity.** The entity is the anchor node (`anchor`). A row
   group MUST hold rows of exactly one anchor. An anchor MAY span several
   consecutive row groups. Row groups are ordered by `anchor` ascending (byte
   comparison).
6. **orderby = hlc.** Within one anchor, rows are strictly ascending by
   `(hlc_wall_ms, hlc_counter, id)`, where `id` is `reading_id` or
   `payload_id`. An anchor's rows split into row groups only at the pinned
   maximum row count.
7. **Row-group statistics are required.** Every column chunk of every row
   group MUST carry `min`, `max`, and `null_count`. They live inside the
   sealed hexon, so they are sealed under the scope key, which is the
   SPEC-6 §3.3 rule 6 tier (G4).
8. Given the same set of rows, a conforming writer MUST produce byte-identical
   Parquet members, and so a byte-identical archive, whatever the input order.
9. Every `source_index` MUST be less than the length of the batch's `sources`
   list, and every source in that list MUST be referenced by at least one row.
10. In `payload_refs`, `asset_ids` and `asset_content_hashes` have the same
    non-zero length and keep the ingest order of the payload's assets.
    `asset_content_hashes[i]` is the BLAKE3 content hash of the asset that
    `asset_ids[i]` names.

### 3.3 Statistics artifact

1. Every batch has one statistics artifact: a second bulk artifact, object
   class `shard`, whose content is one Parquet file written with the §3.2
   rule 1 profile.
2. It has one row per `(anchor, table, key)`, strictly ascending by that
   triple. `table` is `1` for readings, where `key` is the `metric`, and `2`
   for payloads, where `key` is the `format_id`. Its columns, in order, are
   `anchor` (`FIXED_LEN_BYTE_ARRAY(32)`), `table` (`INT32`), `key` (UTF8),
   `row_count` (`INT64`), `min_value` and `max_value` (`DOUBLE`, null when
   `table = 2`), `min_recorded_at_ms` and `max_recorded_at_ms` (`INT64`),
   `min_hlc_wall_ms` (`INT64`), `min_hlc_counter` (`INT32`), `max_hlc_wall_ms`
   (`INT64`), and `max_hlc_counter` (`INT32`).
3. Every value MUST equal the matching aggregate over the hexon's rows.
   Hydration checks this (§5.2 rule 5).
4. The statistics artifact is sealed under the same scope as its hexon.

### 3.4 Batch scope and bounds

1. A batch covers exactly one petal (the SPEC-6 §3.2 rule 2 single-scope
   rule), one author, and one `hexon_format_version`.
2. A batch covers one **window**: the observations that this author's ingest
   door accepted for that petal within one half-open interval of ingestion HLC
   wall time. The window width is sealer policy, not canonical semantics. The
   default is the verse's timeseries bucket width (`fe-sync/src/AGENTS.md`
   §sharding). A window MAY be sealed as several batches, to respect rule 4 or
   after a scope-epoch bump. A batch MUST NOT span two petals.
3. An observation, identified by `reading_id` or `payload_id`, MUST appear in
   at most one batch by the same author.
4. The sealed hexon and the sealed statistics artifact MUST each fit the
   sealing principal's effective `max_segment_bytes` caveat (SPEC-3 §2.3 key 1;
   SPEC-6 §3.2 rule 5). A sealer splits an oversized window by anchor. It MUST
   NOT create an oversized artifact or silently omit rows.
5. A batch includes only observations that this node's own ingest door
   accepted. Window membership MUST be recorded at the ingest door, after the
   durable insert, from the accepted batch itself. A sealer MUST NOT select
   members by scanning the hot tables or by a row column such as `source_did`:
   replicated rows carry author-controlled column values, so a peer could stamp
   this node's DID on a row and have this node sign it. Observations that
   arrived by legacy replication are sealed, if at all, by the node that
   ingested them.
6. Hot observation rows are immutable facts: they are append-only, and union
   replication never overwrites them (`fe-database/src/AGENTS.md`
   §replication-mode). Reading a member row back by the identifier the ingest
   door recorded is therefore not a reverse-build in the SPEC-8 §2 rule 3
   sense.

## 4. Observation-batch operation

### 4.1 Operation kind

1. `operation_kind = 5` is `observation_batch`. This document registers it
   under SPEC-1 §6 rule 7.
2. Structural rules, checked on the header alone (`invalid_envelope` on
   failure):
   1. at least one parent;
   2. an encrypted, non-empty payload reference, never the SPEC-1 §3.5
      no-payload form;
   3. `scope.petal_id` is non-null and `scope.resource_id` is null.
3. `schema_hash` MUST equal
   `BLAKE3(ASCII("fe-schema-observation-batch-v1") || 00)`. A kind-5 candidate
   with any other schema hash is `unknown_schema` (SPEC-4 §5): the kind is
   known and the schema version is not.
4. A build that does not implement this document has no structural rule for
   kind 5. It MUST quarantine a kind-5 candidate as `unknown_kind`, charged to
   that reason's own budget (SPEC-5 §4 rule 4). It MUST NOT charge it to
   `unknown_schema` or reinterpret it as kind 1.

### 4.2 Authority

1. Appending a batch requires `append/op` at the operation's scope with
   `allowed_operation_kinds` either null or containing `5` (SPEC-3 §2.3 key 4,
   §3.3).
2. Sealing the hexon and the statistics artifact requires `append/shard` at
   that scope. The "validated parent Hexon manifest" that SPEC-3 §3.3 requires
   for a `shard` is the hexon's own `manifest.json`.
3. Sealing a device payload blob as a bulk `asset` artifact requires
   `append/asset`. V1 seals none (§9 rule 2).
4. The operation payload, the hexon, and the statistics artifact MUST be
   sealed under the same payload-topic scope, scope epoch, and current scope
   key. A sealer that observes a scope-epoch bump before the operation is
   appended MUST discard the partly sealed batch and seal it again.

### 4.3 Payload

The decrypted payload is a canonical CBOR map (SPEC-1 §2; no floats) with
exactly these keys:

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `hexon_artifact_id` | `Hash32` | SPEC-6 `artifact_id` of the sealed observation hexon. |
| 1 | `hexon_stored_length` | unsigned `u64` | Exact stored byte length of that artifact. |
| 2 | `hlc_range` | map: `0` min, `1` max, each a SPEC-1 §3.4 HLC map | Inclusive ingestion-HLC range over every row in the hexon. |
| 3 | `sensor_time_range_ms` | map: `0` min, `1` max, each a signed `i64` | Inclusive `recorded_at_ms` range over every row. |
| 4 | `stats_hash` | `Hash32` | `artifact_id` of the sealed statistics artifact (§3.3). |
| 5 | `count` | unsigned `u64` | Total rows across both Parquet members. |
| 6 | `anchors` | array of `Identifier32` | The set of anchors, strictly ascending. |
| 7 | `asset_refs` | array of `Hash32` | The set of asset content hashes, strictly ascending. |
| 8 | `sources` | array of `ObservationSource` maps (§7.1) | Strictly ascending by complete canonical encoding. |
| 9 | `hexon_format_version` | unsigned `u16` | MUST be `1`. |
| 10 | `stats_stored_length` | unsigned `u64` | Exact stored byte length of the statistics artifact. |

A payload that breaks any rule below is `invalid_payload` (SPEC-4 §5):

1. It has exactly keys 0 to 10, each once. An unknown or duplicate key is
   rejected.
2. Each range has its minimum not after its maximum. HLC order is
   `(wall_ms, counter)`.
3. `count` is at least 1. A batch with nothing in it is not sealed.
4. `anchors` is non-empty, strictly ascending, and no longer than `count`.
5. `asset_refs` is strictly ascending. It MAY be empty: a batch of scalar
   readings references no asset.
6. `sources` is non-empty, strictly ascending, and every entry is valid under
   §7.1.
7. `hexon_artifact_id` differs from `stats_hash`, and both stored lengths are
   non-zero.

These are the only checks a receiver can make before hydration. Whether the
hexon's content agrees with this payload is checked at hydration (§5.2), not
at admission, because SPEC-4 §4 rule 8 forbids admission and reduction from
reading the hexon.

### 4.4 Reduction

1. A kind-5 reduction writes exactly one `observation_batch` reference row,
   keyed by `op_id`. The row holds the `op_id`, the scope, the author public
   key, the envelope HLC, and every payload field in §4.3.
2. Its admissible inputs are the envelope facts and the admitted operation's
   own decrypted, schema-validated payload: "the admitted operation's own
   verified bytes and extracted facts" (SPEC-4 §4 rule 8). It MUST NOT open,
   fetch, test the presence of, or read the length or statistics of the hexon,
   the statistics artifact, or any asset blob.
3. The payload binds each artifact's identity and stored length in signed
   bytes, so a kind-5 reduction can always construct its references. It MUST
   NOT return `referenced_artifact_unavailable`. An absent hexon is visible
   only at hydration.
4. A kind-5 reduction MUST NOT write `iot_reading`, `device_payload`, `asset`,
   or catalog rows.

### 4.5 Legacy identifiers in canonical bytes

1. Hot-plane identifiers for verse, petal, and node are text ULIDs. Canonical
   bytes need an `Identifier32`. They enter canonical bytes through one
   mapping:

   ```text
   legacy_identifier(table, id) =
       BLAKE3(ASCII("fe-legacy-identifier-v1") || 00 || canonical_cbor([table, id]))
   ```

   `table` is the text string `"verse"`, `"petal"`, or `"node"`, and `id` is
   the exact stored identifier string.
2. This is the observation family's provisional mapping (D-CL34). When the
   SPEC-8 candidate scope derivation (SPEC-8 §4.2 rule 2) ratifies a general
   mapping, the two MUST be the same function. This document MUST NOT keep a
   second one.
3. The mapping is deterministic and enumerable: anyone who knows a node id can
   compute its identifier. Identifiers therefore appear only inside encrypted
   payloads and sealed hexons, in blinded topics, and in the scope of a
   header. They never appear in clear manifest statistics.

## 5. Hydration

### 5.1 Contract

1. Hydration expands an admitted batch's hexon into a derived local catalog.
   It requires `fetch/shard`, `decrypt/shard`, and `materialize/shard` for the
   batch's scope (SPEC-3 §3.3).
2. The catalog is never an input to `reduce`, to a checkpoint projection root,
   or to a SPEC-4 projection. It is derived and rebuildable: dropping it and
   hydrating again from the verified log and the available artifacts MUST
   reproduce it.
3. Before cutover (§8) the catalog is stored apart from `iot_reading` and
   `device_payload`. It MUST NOT be exposed through the public REST API, MCP,
   BI query, or WebSocket surfaces (SPEC-8 §2 rule 5).

### 5.2 Validation order

A hydrator MUST perform these steps in order:

1. Re-hash each stored artifact and compare its `artifact_id` and stored
   length with the payload (SPEC-6 §5 rule 1).
2. Validate the sealed outer form, resolve the current authorized scope key,
   and authenticate and decrypt. Require lane class `BulkArtifact`, object
   class `shard`, and an inner scope equal to the batch's scope (SPEC-6 §3.4,
   §5 rule 3).
3. Validate the archive (§3.1) and the Parquet profile (§3.2), including
   segmentby, orderby, and the presence of statistics.
4. Check the hexon against its signed payload: the row total equals `count`;
   the distinct `anchor` set equals `anchors`; the HLC and sensor-time extrema
   equal the two ranges exactly; the distinct set of `asset_content_hashes`
   equals `asset_refs`; every `source_index` is in range and every source is
   used; and the Parquet metadata's format version equals
   `hexon_format_version`.
5. Validate the statistics artifact the same way, including §3.3 rule 3.

### 5.3 Outcomes and catalog semantics

1. Each batch has exactly one hydration outcome:

   | Outcome | Condition | Required result |
   | --- | --- | --- |
   | `hydrated` | Every step of §5.2 passes. | The batch's rows enter the catalog. |
   | `referenced_artifact_unavailable` | The hexon or statistics artifact is not held, its scope key is unavailable, or held bytes fail §5.2 step 1 (they are then not the artifact and are discarded, SPEC-6 §5 rule 5). | Retryable. Record the batch as pending. Never substitute an empty or partial catalog entry (SPEC-4 §5). |
   | `observation_batch_invalid` | Authentic artifact bytes fail §5.2 steps 2 to 5. | Permanent for those bytes. Exclude the batch's rows and retain the operation and artifacts as evidence (SPEC-4 §4 rule 6). Never repaired by editing the catalog. |

2. Catalog rows are keyed by `reading_id` and `payload_id`. The catalog is the
   union of every hydrated batch.
3. Duplicate rows with the same identifier and byte-identical content collapse
   into one row.
4. Conflicting duplicates, with the same identifier and different content, are
   excluded from the catalog and retained as conflict evidence that names both
   batches. Neither wins. This is the D-CL25 equivocation rule applied to
   observations.
5. Given the same admitted batch set and the same hydration outcomes, every
   conforming hydrator produces the same catalog.

## 6. Preview kind registry

1. SPEC-7 §7.1 rule 1 requires a "registered non-zero `preview_kind`" and no
   registry existed. `preview_kind` is a `u32` on the wire
   (`fe-canonical-log/src/wire/preview.rs:53`). Values are allocated as
   follows:

   | Value | Meaning |
   | --- | --- |
   | `0` | Invalid. Rejected by the SPEC-7 decoder (`preview.rs:87-90`). |
   | `1` | `telemetry_sample_v1` (rule 3). |
   | `2` to `1023` | Reserved for specification registration. MUST NOT be sent until a specification registers the value. |
   | `1024` to `4294967295` | Application and experimental kinds. No interop is claimed. |

2. Structural decoding is unchanged: a SPEC-7 decoder accepts any non-zero
   kind. A registry-aware receiver MUST drop a preview of a reserved kind that
   no specification has registered, and MUST drop an application kind it has
   no local handler for. A drop is silent display loss. It is never an error to
   the sender and never durable.
3. `telemetry_sample_v1` preview data is a canonical CBOR map (SPEC-1 §2; no
   floats) with exactly these keys:

   | Key | Name | Representation | Rule |
   | ---: | --- | --- | --- |
   | 0 | `anchor` | `Identifier32` | §4.5 legacy identifier of the anchor node. |
   | 1 | `metric` | text | Non-empty. |
   | 2 | `value_nano` | signed `i64` | The value in nano-units of `units`, rounded half to even (D-CL5). |
   | 3 | `units` | text | MAY be empty. |
   | 4 | `recorded_at_ms` | signed `i64` | Sensor time. |

   An unknown, duplicate, or missing key is rejected. A value whose nano-unit
   quantization overflows `i64` MUST NOT be sent. None of the SPEC-7 §7.2
   rule 5 operation fields may appear.
4. **Dual path.** A telemetry preview is lossy live display (SPEC-7 §7.1
   rule 4). It MUST NOT be converted into a reading, a batch, or hot-plane
   state. The durable record of the same observation follows the ingest door,
   then the hot plane, then a batch.
5. No preview transport is authorized (§1 rule 9). Implementation is the codec
   and the registry only.

## 7. Source identity (G6)

### 7.1 `ObservationSource`

An `ObservationSource` is a canonical CBOR map (SPEC-1 §2; no floats) with
exactly these keys:

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `source_class` | unsigned integer | `1` device, `2` connector, `3` decoder, `4` manual, `5` simulation. |
| 1 | `source_id` | `Identifier32` | §7.2. |
| 2 | `ingest_principal` | byte string, 32 bytes | Ed25519 public key of the principal the ingest door authenticated. |
| 3 | `format_id` | text or `null` | The descriptor format of the observation's payload, or `null`. |
| 4 | `decoder` | `DecoderIdentity` map or `null` | Non-null exactly when `source_class = 3`. |

A `DecoderIdentity` is a canonical CBOR map with exactly these keys:

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `plugin_id` | text | 1 to 128 bytes. |
| 1 | `plugin_version` | text | 1 to 64 bytes. |
| 2 | `module_hash` | `Hash32` | BLAKE3 of the exact decoder module bytes: the Wasm module, or the Rhai source. |

1. Unknown and duplicate keys are rejected. Text is NFC (SPEC-1 §2 rule 6).
2. `source_class = 0` and values above 5 are rejected. Zero is never a valid
   class, so a zero-initialized map cannot read as a well-formed source.
3. `decoder` is non-null if and only if `source_class = 3`. Class 3 also
   requires a non-null `format_id`.
4. A non-null `format_id` satisfies the §2.2 rule 4 grammar.
5. `ingest_principal` is the key of the `did:key` DID that the ingest door
   recorded as the observation's `source_did` (today the token `sub`,
   `fe-api/src/iot.rs:152`). An observation whose `source_did` is empty or is
   not an Ed25519 `did:key` has no valid source and is uncovered (§8 rule 3).
6. The ingest door MUST assign class `simulation` to every observation the
   simulation lab produces. It MUST NOT assign such an observation class
   `device` or `connector`. Simulated data never presents as field data in
   signed bytes. The class MUST travel explicitly from the caller to the door.
   It cannot be inferred from `source_did`, which the simulation lab sets to
   its host peer's DID (`fe-sim/src/session.rs:454-460`).

### 7.2 `source_id`

```text
source_id = BLAKE3(ASCII("fe-observation-source-id-v1") || 00 ||
                   canonical_cbor([source_class, source_label]))
```

`source_label` is a text string chosen by class:

| Class | `source_label` |
| --- | --- |
| device | The authenticating token's `client` claim when present (RUNBOOK 2026-10-12 token decision), else its `sub`. |
| connector | The connector's configured identifier. |
| decoder | `plugin_id`. |
| manual | The acting principal's DID. |
| simulation | The simulation fleet identifier. |

Binding the class into the hash keeps a device and a decoder with the same
label distinct.

### 7.3 Reproducibility

1. SPEC-4 §7 rule 5 (erratum G6) adds the **observation input set** to the
   canonical input identity of any analytics computation that reads
   observation data. The observation input set is the canonically sorted,
   deduplicated set of kind-5 `op_id`s whose data the computation read.
2. Each batch's signed payload binds its hexon and its `sources`. Recording the
   input set therefore fixes both the data that was read and its provenance,
   including the decoder identity of every decoded reading.
3. The implementation vocabulary is `ObservationSource`, `SourceClass`,
   `DecoderIdentity`, and an `AnalyticsSourceIdentity` that extends the SPEC-4
   §7 rule 1 tuple with the observation input set. Today the SPEC-4 §7 analytics
   identity exists only as a test fixture
   (`fe-database/tests/canon_log_materialization_test.rs:1357-1430`).

## 8. `iot_reading` graduation path

The SPEC-8 stages apply to the observation family (`iot_reading` and
`device_payload`) as follows:

| Mode | Ingest door | Observation plane | Authority |
| --- | --- | --- | --- |
| `legacy_only` (default) | Current path. | Nothing recorded, sealed, or appended. | Hot plane. |
| `seam_ready` | Current path. | Nothing. The ingest door is already the single seam for this family. | Hot plane. |
| `dual_emit_shadow` | Current path, and window membership recorded after the durable insert (§3.4 rule 5). | The sealer seals closed windows. Batches are appended to the local verified log, and reference rows materialize. | Hot plane. |
| `shadow_rebuild` | As above. | As above, plus hydration into a separate catalog and the SPEC-8 §5 comparison. | Hot plane. |
| `canonical_authoritative_local` | Only after the SPEC-8 §7 rule 4 owner gate. | Authoritative. | Hydrated catalog; hot tables become rebuildable projections. |

1. The mode is the process-wide SPEC-8 flag
   (`fe-database/src/migration/mode.rs:14,76`). The default is `legacy_only`,
   and parity never flips the flag (SPEC-8 §3 rule 4).
2. In `legacy_only` and `seam_ready` the ingest path MUST be byte-identical to
   the path without this document: no window record, no sealing, and no
   observation candidate.
3. **Coverage.** An accepted observation that cannot be sealed is
   `legacy_succeeded_without_candidate` and blocks promotion (SPEC-8 §6.1,
   capture completeness). Causes include no valid source under §7.1 rule 5, a
   non-finite value, and a missing ingestion HLC.
4. **Comparison** (`shadow_rebuild`). The catalog is compared with the hot rows
   of the same membership, which is this node's ingests only. Compared fields
   are the identifiers, anchor, metric, `value` by exact IEEE-754 bit pattern
   (no tolerance, SPEC-8 §5.3 rule 3), units, `recorded_at`,
   `recorded_at_ms`, ingestion HLC, `payload_id`, and source. For payloads they
   are also `format_id`, the ordered `asset_ids`, and the content hashes. Rows
   the hot plane holds from replication are outside the comparison. Three
   empty-state hydrations MUST be byte-identical (SPEC-8 §5.2 rule 3).
5. **Cutover** (`canonical_authoritative_local`) is the SPEC-8 §7 rule 4 owner
   gate. After it, `iot_reading` and `device_payload` stay the BI-queryable
   surface (`fe-api/src/query_guard.rs:54-61`) and become rebuildable from
   batches. They are not removed.
6. **Retention.** Only after cutover, and only for hot rows covered by a
   hydrated batch inside a replay-verified checkpoint, MAY a node release hot
   rows (ledger DB-TS-RETENTION). The retention window is caller policy
   (D-CL24(4)); this document fixes none. Before cutover hot rows are the
   authority and MUST NOT be deleted on the strength of a batch.
7. This document does not graduate `asset` or `device_format`. They remain
   hot-plane authority. Their canonical form belongs to the generic asset
   model's own specification, together with the D-20 path-asset op-store.
8. The six D-CL20 legacy kinds stay refused. Registering this family's
   mapping in the SPEC-8 inventory does not change `blocks_promotion` for them.

## 9. Lane, object-class, and topic-lane mapping

| Artifact | SPEC-6 lane class (provisional wire) | SPEC-3 object class | SPEC-3 §6.1 topic lane | Scope | V1 producer | Sealing authority |
| --- | --- | --- | --- | --- | --- | --- |
| Observation hexon | `BulkArtifact` (5) | `shard` (`0x08`) | `1` payload | petal-affine payload-topic scope | shadow-mode sealer | `append/shard` |
| Statistics artifact | `BulkArtifact` (5) | `shard` (`0x08`) | `1` payload | same scope as its hexon | shadow-mode sealer | `append/shard` |
| Device payload blob, sealed form | `BulkArtifact` (5) | `asset` (`0x20`) | `1` payload | petal-affine | none in v1 (rule 2) | `append/asset` |
| Terrain tile | `BulkArtifact` (5) | `tile` (`0x10`) | `1` payload | petal-affine | none in v1 | `append/tile` |
| `observation_batch` header | header segment (1) | `op` (`0x01`) | `0` header | verse-wide | shadow-mode sealer | `append/op`, kinds ∋ 5 |
| `observation_batch` payload ciphertext | payload shard (2) | `op` (`0x01`) | `1` payload | petal-affine | shadow-mode sealer | — (derived packing) |

1. A segment manifest never indexes a bulk artifact. A bulk artifact is
   reachable only through the payload of the operation that commits it
   (SPEC-6 §3.4 rule 5).
2. V1 seals no device payload blob. Blobs stay in the local iroh blob store,
   referenced by plaintext content hash in `asset_refs` and in
   `payload_refs.asset_content_hashes`, and are kept alive by the hot plane and
   asset GC (§2.2 rule 8). A blob is sealed as a `BulkArtifact` of class `asset`
   only when the canonical plane must deliver it to another peer, which
   requires CDL-NET.
3. Terrain tiles are listed for completeness. Nothing produces one in v1.
4. The provisional AAD domain of the bulk-artifact lane is
   `fe-segment-bulk-artifact-v1` (NUL-terminated, SPEC-6 §9 rule 1). It is
   recorded in the provisional register (D-CL34).

## 10. Required conformance tests

A conforming implementation MUST provide deterministic fixtures and at least
these named tests:

1. **`observation_batch_op_signs_admits_and_reduces_to_reference_only`** — a
   kind-5 operation is built, signed, admitted through the single ingress, and
   reduced to exactly one reference mutation that carries the §4.3 fields.
2. **`observation_batch_structural_refusals`** — zero parents, the no-payload
   form, a verse-wide or resource scope, an inverted HLC or sensor range, a
   zero `count`, more anchors than `count`, unsorted anchors, asset refs or
   sources, an unknown key, and a float anywhere are each refused with the
   right SPEC-4 category.
3. **`unupgraded_peer_quarantines_kind5_without_evicting_missing_parent_backlog`**
   — a build without kind 5 quarantines kind-5 traffic at volume under
   `unknown_kind`, and its missing-parent backlog survives.
4. **`reduce_is_identical_whether_or_not_the_hexon_is_present`** — the
   reduction of a batch is equal on a peer holding its hexon and on a peer
   holding nothing.
5. **`telemetry_preview_registry_accepts_kind1_drops_unregistered_and_rejects_op_fields`**
   — kind 1 decodes, a reserved unregistered kind and an unhandled application
   kind are dropped, and a telemetry preview carrying a reserved operation
   field is rejected.
6. **`observation_hexon_round_trip_honours_segment_and_order_profile`** —
   readings for three anchors written in shuffled order read back as
   per-anchor row groups in ascending HLC order with correct row-group
   statistics.
7. **`same_rows_any_input_order_produce_identical_hexon_bytes`** — writer
   determinism (§3.2 rule 8).
8. **`profile_violations_rejected`** — a mixed-anchor row group, unsorted HLC,
   a missing or extra column, a compressed member, a wrong member order, and a
   wrong `hexon_type` are each rejected.
9. **`zip_bomb_observation_hexon_refused_without_allocation`** — the fe-format
   decompression limits apply before allocation.
10. **`hydration_rejects_a_hexon_that_disagrees_with_its_signed_batch`** — a
    wrong row count, anchor set, range, asset set, unused or out-of-range
    source index, or format version yields `observation_batch_invalid`, and no
    row of that batch enters the catalog.
11. **`missing_hexon_surfaces_unavailable_at_hydration_not_reduce`** — reduction
    succeeds and hydration reports `referenced_artifact_unavailable`, then
    hydrates once the artifact arrives.
12. **`conflicting_duplicate_observation_is_retained_as_evidence_not_resolved`**
    — two hydrated batches holding the same `reading_id` with different values
    contribute neither row, and the conflict names both batches.
13. **`legacy_only_ingest_creates_no_observation_candidate`** — in the default
    mode the ingest path records, seals, and appends nothing.
14. **`shadow_mode_seals_window_commits_batch_and_hydrates_parity_with_legacy`**
    — in a real database with readings and a device payload, the sealer
    appends a kind-5 operation, its reference row materializes, and the
    hydrated catalog equals the legacy rows with zero differences.
15. **`sealer_never_signs_replicated_observations`** — a replicated row whose
    `source_did` names this node is not sealed by this node.
16. **`rebuild_from_log_reproduces_observation_reference_rows`** — an empty
    rebuild from the verified log reproduces every reference row.
17. **`observation_source_cbor_canonical_round_trip_and_refusals`** — a float,
    an unknown key, a duplicate key, a non-canonical length, class 0, a decoder
    on a non-decoder class, and a malformed `format_id` are each refused.
18. **`analytics_source_identity_dedupes_and_sorts_batch_ids`** — the
    observation input set is canonical whatever the read order.
19. **`analytics_identity_records_the_batches_it_read`** — an end-to-end
    analytics read records its observation input set, and an unavailable
    listed batch fails the computation rather than shrinking the set.

## 11. Owner approval gates and pre-approved choices

### 11.1 Status of this document

1. The owner pre-approved this specification on 2026-10-13 with the
   recommended defaults in §11.2 (RUNBOOK §3 R7). Owner post-review is pending.
   A change from post-review lands as a dated entry under Errata, and the
   implementation follows it.
2. The pre-approval unlocks local implementation of §§3 to 9 in the
   default-off shadow modes of §8: the observation module (kind 5, payload
   codec, preview registry), the columnar hexon codec, and the shadow sealer,
   reference materializer, and hydrator. Approval removes a gate; it does not
   reorder work. That implementation still follows its code dependencies,
   which are the bulk-artifact lane and source vocabulary (G3, G6), the
   device-payload records (§2.2), the single `.hexon` archive reader, and the
   canonical schema registry.
3. It does not unlock any of the following. Each keeps its own gate:
   - network transport, relay seeding, inbound P2P, and canonical replication
     of batches or bulk artifacts (CDL-NET);
   - the `/ws/canonical` mount and therefore live delivery of
     `telemetry_sample_v1` (CANON-WS);
   - `canonical_authoritative_local` for this family (SPEC-8 §7 rule 4);
   - retention of hot rows behind batches (DB-TS-RETENTION, after cutover);
   - decoder plugins (PLUG-HOST and its mandatory sandbox security review);
   - sealing device payload blobs as canonical `asset` artifacts (§9 rule 2,
     CDL-NET).

### 11.2 Decisions D-CL30 to D-CL34

**D-CL30 — SPEC-10 as a whole.**

- *Default:* adopt the two-plane observation contract as written: batches by
  reference, hot tables kept as the queryable projection, graduation through
  the SPEC-8 stages, and one ingestion story for scalars and device payloads.
- *Alternatives:* (a) one signed operation per reading; (b) replace the hot
  tables with canonical-only storage; (c) keep observations outside the
  canonical log.
- *Reasoning:* (a) is the cost D-CL26 already rejected (§12). (b) breaks the
  BI egress that queries `iot_reading` directly. (c) leaves telemetry with no
  durable signed history, no provenance for decoded values, and no answer to
  SPEC-4 §7 reproducibility.

**D-CL31 — dedicated operation kind 5.**

- *Default:* `operation_kind = 5` (`observation_batch`) with its own structural
  rule and schema hash.
- *Alternatives:* (a) kind 1 with a registered observation schema; (b) a kind
  from the application range.
- *Reasoning:* an un-upgraded peer receives every header verse-wide. A
  dedicated kind lands in the `unknown_kind` quarantine budget, which G1
  created for exactly this traffic. A kind-1 schema would spend the
  `unknown_schema` budget, which also holds the peer's ordinary schema-skew
  backlog. A dedicated kind also lets the header-level structural rule
  require a petal scope and a payload, which a kind-1 schema can only check
  after decryption. Kind 1 means intent with materialized state; a batch is a
  fact commit.

**D-CL32 — hexon container and Parquet profile.**

- *Default:* a stored-zip `.hexon` archive holding two Parquet members, with
  segmentby = anchor node, orderby = ingestion HLC, required row-group
  statistics, uncompressed pages, a pinned writer, and a separate sealed
  statistics artifact.
- *Alternatives:* (a) a bare Parquet blob; (b) Parquet wrapped in a new CBOR
  container; (c) zstd-compressed pages; (d) entity = `(node_id, metric)`.
- *Reasoning:* (a) has no manifest, so SPEC-3's `append/shard` "validated
  parent Hexon manifest" has nothing to validate, and it cannot carry the
  second table. (b) invents a container with no tooling, while fe-format
  already bounds decompression. (c) needs a Cargo feature that the tree does
  not build today (§2.3), and every codec version change becomes a
  byte-determinism hazard; a batch of about 10⁴ rows is around a megabyte
  uncompressed. (d) multiplies row groups, and the fabric already shards by
  anchor, not metric (`fe-sync/src/AGENTS.md` §sharding: "Anchor, not metric").

**D-CL33 — preview kind registry and `telemetry_sample_v1`.**

- *Default:* `0` invalid, `1` = `telemetry_sample_v1`, `2` to `1023` reserved
  for specifications, `1024` and above for applications.
- *Alternatives:* (a) no registry, the status quo in which any non-zero value
  decodes; (b) string-named kinds; (c) per-verse negotiated kinds.
- *Reasoning:* (a) lets two applications collide on one number with no
  owner. (b) changes the SPEC-7 wire type. (c) needs a network exchange that
  is not authorized. Integer nano-units keep telemetry previews in the SPEC-1
  §2 profile, so one decoder serves frames and preview data.

**D-CL34 — provisional wire numbers.**

- *Default:* the following ship as provisional and join the CDL-PROV
  ratification index (`fe-canonical-log/src/AGENTS.md` §Provisional wire
  numbering), with no cross-implementation interop claim, under D-CL24(3):
  - lane class 5 and the AAD domain `fe-segment-bulk-artifact-v1`;
  - the bulk-artifact inner-body keys;
  - the kind-5 payload keys 0 to 10;
  - the `ObservationSource` and `DecoderIdentity` keys and the
    `source_class` values;
  - the `telemetry_sample_v1` keys;
  - the schema-hash domain `fe-schema-observation-batch-v1`, the source-id
    domain `fe-observation-source-id-v1`, and the legacy-identifier domain
    `fe-legacy-identifier-v1`.
- *Alternative:* hold implementation until the numbers are ratified.
- *Reasoning:* nothing outside this repository reads these bytes, and the
  network gate is closed. Holding Phase B on numbering would block the
  local value of the plane for no interop benefit. Post-review either
  confirms the tables, which then move to the register's normative section,
  or replaces them before any second implementation exists.

## 12. Design notes

- **Two planes, one door.** The operation plane authors state; the
  observation plane commits facts. Both sit behind one ingest door, so a
  reading, a payload, and a decoded reading share one validation path, one
  authority check, and one provenance record. Three doors would mean three
  sets of guards that drift apart.
- **Cost math (D-CL26).** 1 000 sensors at 1 Hz is 86.4 M readings a day. As
  one operation each, that is about 47 GB (44 GiB) a day of roughly 546-byte
  headers on every peer, plus about an hour a day of Ed25519 verification.
  Batches of about 10⁴ readings cut both by four orders of magnitude, to about
  8 640 operations a day.
- **Why the statistics artifact is separate.** A sealed artifact can only be
  authenticated whole. Parquet's footer-first range reads do not work on AEAD
  ciphertext, so the hexon's own row-group statistics cannot be consulted
  without fetching and decrypting the entire hexon. The small statistics
  artifact is what a selective reader fetches first. It is sealed because its
  values are payload-derived (G4).
- **Why membership is recorded at the door.** The hot tables mix this node's
  ingests with rows replicated from peers, and every column on a replicated
  row is author-controlled. Selecting window members by a table scan, or by
  `source_did`, would let a peer get its rows signed by this node.
- **Why decoders write readings, not a fourth record.** Decoded values have
  to be queryable exactly like device-reported ones. The `payload_id` link plus
  the decoder identity in the batch's `sources` keeps the derivation auditable
  without a second scalar store.
- **Why the hot tables stay.** BI egress, the REST query guard, and the
  distributed query fan-out all read `iot_reading`. Graduation makes those
  tables rebuildable from the log. It does not move the query surface.
- **A seam this document exposes.** `CausalMaterializer::reduce` receives
  `meta` and `envelope_bytes` only
  (`fe-canonical-log/src/materialize/traits.rs:271-275`), and the envelope
  carries no payload (SPEC-1 §1 rule 2). A kind-5 reduction needs its admitted
  payload's extracted facts (§4.4 rule 2). Every payload-bearing kind shares
  this need, including the `node-transform-v1` schema of the Wave 4 harness.
  How the facts reach `reduce` is an implementation decision that rule 2
  constrains; it is not a protocol change.

### Errata

None.
